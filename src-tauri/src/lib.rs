//! Tauri 壳：装配核心 → 迁移 → 内嵌数据面 → 命令注册。
//!
//! 启动顺序（与旧 Electron 版 whenReady 链一致）：
//! 1. 解析数据目录（env > 开发态仓库 data/ > 应用数据目录）
//! 2. 一次性迁移（幂等）
//! 3. CoreState 初始化 + axum 数据面（127.0.0.1，优先固定端口，占用时依次 +1）
//! 4. 单实例锁（二次启动唤起已有窗口）；插件之外再加一把进程级文件锁兜底（见 instance_lock.rs）

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("ATT 桌面应用只支持 macOS 与 Windows");

// 注意：Tauri generate_handler 需要命令模块公开可见（__cmd__ 宏重导出）
pub mod app_update;
pub mod clipboard;
pub mod clipboard_image;
pub mod clipboard_panel;
pub mod commands;
pub mod desktop;
pub mod floating;
pub mod instance_lock;
pub mod launch;
#[cfg(target_os = "macos")]
mod menu;
pub mod permissions;
pub mod recording;
pub mod tray;
pub mod typing;
pub mod window_icon;
pub mod window_shape;

use att_core::{migration, state::CoreState, Platform};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

const LOCAL_MODEL_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// 命令层共享状态。
pub struct AppState {
    pub core: Arc<CoreState>,
    pub backend_port: u16,
    /// 本次启动生成的数据面令牌（前端经注入或命令获取，敏感 HTTP 端点要求带上）
    pub backend_token: String,
    /// 数据面停机信号发送端。
    ///
    /// **必须随应用存活**（由 `app.manage` 持有）：watch 通道的全部发送端被 drop
    /// 会让接收端 `wait_for` 立刻返回，等于触发 axum 的优雅关闭——曾导致数据面
    /// 在 setup 结束后静默死亡（端口消失、无任何错误日志，界面全部请求失败）。
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// 录音会话 Actor（UI 按钮与全局热键共用同一入口）
    pub recorder: recording::RecordingController,
    /// 全局热键不可用的原因（权限缺失等），前端挂载后可查询
    pub hotkey_error: std::sync::Mutex<Option<String>>,
    /// 更新后首次启动时记下的更新前版本（没记过版本号的旧版本为空串）；不是更新后首次启动或引导关掉后为 None
    pub update_notice: std::sync::Mutex<Option<String>>,
}

pub fn run() {
    let builder = tauri::Builder::default();
    // macOS：中文菜单栏代替 Tauri 的英文默认菜单（其他平台本就没有菜单栏，不动）
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build);
    builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 二次启动：唤起主窗口（与旧版 second-instance 行为一致）
            // 系统登录项触发的二次启动仍保持后台，不把已打开的主窗口带到前台。
            if !args.iter().any(|arg| arg == launch::AUTOSTART_ARG) {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![launch::AUTOSTART_ARG]),
        ))
        // 应用内更新：下载经签名校验的更新包，安装后重启（见 app_update.rs）
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            // 关闭主窗口（与旧 Electron 版的 close / window-all-closed 行为对齐）：
            // 悬浮窗是预创建的隐藏窗口，任由主窗口销毁会让进程带着全局热键残留在后台，
            // 二次启动也找不到 "main" 可唤起。
            // - macOS：只隐藏，点 Dock 图标（RunEvent::Reopen）再显示；Cmd+Q 照常退出
            // - 其他平台：关闭即退出应用
            if window.label() == clipboard_panel::LABEL {
                // 悬浮框可能成为键盘焦点窗口。Alt+F4 / Cmd+W 应只收起，销毁后快捷键将无法再唤起。
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    clipboard_panel::hide(window.app_handle(), false);
                    return;
                }
                // 剪贴板悬浮框点到别处即收起（Spotlight 式）
                if let tauri::WindowEvent::Focused(false) = event {
                    clipboard_panel::on_blur(window.app_handle());
                }
                return;
            }
            if window.label() == floating::LABEL {
                // 录音悬浮窗同样要跨会话复用；系统关闭键只走已有的淡出隐藏流程。
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    floating::hide(window.app_handle());
                }
                return;
            }
            if window.label() != "main" {
                return;
            }
            // 进出全屏：全屏贴满屏幕不留圆角，退出后恢复
            if let tauri::WindowEvent::Resized(_) = event {
                window_shape::apply(window);
            }
            // 跨屏换缩放：窗口图标换成新 DPI 对应的那一档
            if let tauri::WindowEvent::ScaleFactorChanged { .. } = event {
                window_icon::apply(window);
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let close_action = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|state| state.core.settings().main_window_close_action)
                    .unwrap_or_else(|| "tray".into());
                if launch::should_hide_main_window(Platform::current(), &close_action) {
                    let _ = window.hide();
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .setup(|app| {
            // 必须最先做：二次启动的进程不能动正在运行的实例的日志、数据与热键
            claim_single_instance(app);
            let data_dir = resolve_data_dir(app);
            init_logging(&data_dir);
            tracing::info!("数据目录: {}", data_dir.display());

            // 一次性迁移：尽力而为，失败不阻断启动
            let report = migration::run(&data_dir, Platform::current());
            if !report.renamed_audio.is_empty()
                || !report.matched_audio.is_empty()
                || !report.orphan_audio.is_empty()
                || !report.missing_audio.is_empty()
            {
                tracing::info!(
                    "迁移完成：改名 {} / 时长匹配 {} / 孤儿 {} / 缺失 {}（详见 migration-report.json）",
                    report.renamed_audio.len(),
                    report.matched_audio.len(),
                    report.orphan_audio.len(),
                    report.missing_audio.len()
                );
            }

            // 启动前就有 settings.json = 不是全新安装（判断「从没记过版本号的旧版本升级上来」用）
            let had_settings = data_dir.join("settings.json").is_file();
            let mut core = CoreState::init(&data_dir, Platform::current())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // 随包资源：开发态提示词用仓库 prompts/，打包版用安装资源；Qwen 推理库同理优先用安装资源
            let resource_dir = app.path().resource_dir().ok();
            core.prompts_dir = att_core::prompts::resolve_dir(&data_dir, resource_dir.as_deref());
            core.resource_dir = resource_dir;
            tracing::info!("提示词目录: {}", core.prompts_dir.display());
            // 检查更新用打包版本号（与「关于」页显示的版本一致）
            core.app_version = app.package_info().version.to_string();
            match launch::reconcile_launch_at_login(app.handle(), &core) {
                Ok(true) => tracing::info!("开机自启：系统登录项已启用"),
                Ok(false) => tracing::debug!("开机自启：系统登录项未启用"),
                Err(error) => tracing::warn!("{error}"),
            }
            // 更新后首次启动：记下更新前的版本，主窗口据此检查权限并引导重新授权（见 permissions::update_notice）
            let updated_from = permissions::previous_version_if_updated(
                &core.settings().last_run_version,
                &core.app_version,
                had_settings,
            );
            if core.settings().last_run_version != core.app_version {
                let version = core.app_version.clone();
                if let Err(error) = core.update_settings(|settings| settings.last_run_version = version) {
                    tracing::warn!("记录本次运行版本失败: {error}");
                }
            }
            if let Some(previous) = &updated_from {
                tracing::info!("更新后首次启动：{} → {}", if previous.is_empty() { "旧版本" } else { previous }, core.app_version);
            }
            let core = Arc::new(core);
            // 上次退出时还在「重新识别」/「优化」的记录：后台任务随进程没了，复位状态（文字不动），免得一直显示「转写中」「优化中」。
            // 放后台线程：要扫一遍记录目录，不挡启动
            let recover_core = core.clone();
            std::thread::spawn(move || {
                let recovered = att_core::records::retranscribe::recover_interrupted(&recover_core);
                if recovered > 0 {
                    tracing::info!("{recovered} 条记录的重新识别被上次退出打断，已标为失败");
                }
                // 还停在「优化中」的记录同理：否则界面上没有「优化」按钮，也一直显示优化中
                let recovered = att_core::llm::optimizer::recover_interrupted(&recover_core);
                if recovered > 0 {
                    tracing::info!("{recovered} 条记录的优化被上次退出打断，已复位");
                }
            });

            // 数据面：先用同步 bind 占住端口（优先固定端口，占用时依次 +1，仍失败才回退随机），
            // 再把 accept 循环显式 spawn 到 Tauri 的运行时里。
            // 每次启动生成新的数据面令牌：外部网页 / 进程拿不到，转写等敏感端点默认拒绝无 token 请求
            let backend_token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
            let server_state = Arc::new(att_server::ServerState { core: core.clone(), token: backend_token.clone() });
            // 注意：发送端必须存活到应用退出（否则 watch 通道关闭 → 优雅关闭数据面，
            // 见 AppState::shutdown_tx 注释），这里先创建、随 AppState 交给 Tauri 托管。
            let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
            let preferred_ports: Vec<u16> = std::env::var("ATT_BACKEND_PORT")
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .into_iter()
                .chain(std::iter::once(17890u16))
                .collect();
            let (listener, port) = att_server::bind_local(&preferred_ports).map_err(|e| {
                tracing::error!("数据面绑定失败: {e}");
                e
            })?;
            tauri::async_runtime::spawn(att_server::serve_on(listener, server_state, shutdown_rx));
            tracing::info!("数据面 API 就绪: http://127.0.0.1:{port}/api");

            // 自检：裸 TCP 真打一次 /api/health（重试几次，避开 spawn 与首连的竞态）
            let mut self_check = Err("未执行".to_string());
            for _ in 0..20 {
                match att_server::self_check_health(port) {
                    Ok(status_line) => {
                        self_check = Ok(status_line);
                        break;
                    }
                    Err(error) => {
                        self_check = Err(error);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            match self_check {
                Ok(status_line) => tracing::info!("数据面自检通过: {status_line}"),
                Err(error) => tracing::error!("数据面自检失败（前端将连不上）: {error}"),
            }

            // 诊断信息：端口落盘；不带令牌只能看到探活字段，带 ?token=<持久令牌> 看完整运行态
            let _ = std::fs::write(data_dir.join(".backend-port"), port.to_string());

            // 事件桥：核心事件 → 前端（Tauri event）
            spawn_event_bridge(app.handle().clone(), core.clone());
            app_update::install_driver(app.handle().clone(), &core);
            spawn_startup_update_check(core.clone());

            // 录音 Actor + 全局热键（热键直达 Actor，不经过前端：窗口隐藏/后台时同样可用）
            let recorder = recording::RecordingController::spawn(app.handle().clone(), core.clone());
            let hotkey_error = start_hotkey_listener(app.handle().clone(), core.clone(), recorder.clone());
            spawn_local_model_idle_monitor(core.clone(), recorder.clone());

            // 状态先注册：保证窗口首屏的 invoke 一定能取到 State
            // （shutdown_tx 放进 AppState 随应用存活，防止数据面被误关）
            app.manage(AppState {
                core,
                backend_port: port,
                backend_token: backend_token.clone(),
                shutdown_tx,
                recorder,
                hotkey_error: std::sync::Mutex::new(hotkey_error),
                update_notice: std::sync::Mutex::new(updated_from),
            });

            // 主窗口在 setup 里手工创建（而非 tauri.conf 声明）：
            // 这样能把后端端口通过 initialization_script 注入到页面脚本执行之前，
            // 前端 getBackendPort() 首选的 window.__ATT_BACKEND_PORT__ 因此永远可用，
            // 不再依赖 invoke 是否成功（早期实机验收时 invoke 路径曾失败导致回退 8000）。
            let init_script = format!(
                "window.__ATT_BACKEND_PORT__ = {port}; window.__ATT_BACKEND_TOKEN__ = \"{backend_token}\"; window.__ATT_IS_DESKTOP__ = true;"
            );
            let main = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
                .title("落墨")
                .inner_size(1200.0, 800.0)
                .min_inner_size(800.0, 600.0)
                .visible(!launch::started_from_autostart())
                .decorations(false)
                .center()
                .background_color(tauri::window::Color(10, 10, 11, 255))
                .initialization_script(&init_script)
                .build()?;
            window_shape::apply(&main.as_ref().window());
            window_icon::apply(&main.as_ref().window());

            // 录音悬浮窗：预创建隐藏，由录音 Actor 直接驱动
            if let Err(error) = floating::create(app.handle()) {
                tracing::warn!("创建悬浮窗失败（不影响录音）: {error}");
            }
            // 剪贴板悬浮框：预创建隐藏，由全局快捷键唤起
            if let Err(error) = clipboard_panel::create(app.handle(), port, &backend_token) {
                tracing::warn!("创建剪贴板悬浮框失败: {error}");
            }
            // 托盘（四态图标 + 最近记录菜单）与剪贴板后台监控
            if let Err(error) = tray::create(app.handle()) {
                tracing::warn!("创建托盘失败: {error}");
                // 自启本应完全后台驻留；托盘不可用时至少显示主窗口，避免进程不可访问。
                if launch::started_from_autostart() {
                    show_main_window(app.handle());
                }
            }
            if let Some(state) = app.try_state::<AppState>() {
                clipboard::start(state.core.clone());
                // 定期清理（启动时先跑一次，应用关着的那些天也算进保留期）：
                // 剪贴板历史上限每小时一次，录音音频保留期每天一次；不限 / 永久时什么都不做
                let housekeeping_core = state.core.clone();
                let _ = std::thread::Builder::new().name("att-housekeeping".into()).spawn(move || {
                    const HOUR: std::time::Duration = std::time::Duration::from_secs(3600);
                    let mut last_audio_prune: Option<std::time::Instant> = None;
                    loop {
                        housekeeping_core.prune_clipboard();
                        if last_audio_prune.is_none_or(|at| at.elapsed() >= HOUR * 24) {
                            housekeeping_core.prune_record_audio();
                            last_audio_prune = Some(std::time::Instant::now());
                        }
                        std::thread::sleep(HOUR);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_backend_port,
            commands::get_backend_token,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::get_desktop_settings,
            commands::save_desktop_settings,
            commands::sync_input_settings,
            commands::sync_desktop_settings,
            commands::write_clipboard_text,
            commands::write_clipboard_image,
            commands::get_clipboard_panel_context,
            commands::hide_clipboard_panel,
            commands::clipboard_panel_finish,
            commands::clipboard_panel_resize,
            commands::clipboard_panel_move,
            commands::clipboard_panel_reset_size,
            commands::clipboard_panel_reset_position,
            commands::set_tray_icon,
            commands::update_tray_menu,
            commands::hide_floating_window,
            commands::open_main_window,
            commands::floating_hit_rect,
            commands::debug_log,
            commands::recording_start,
            commands::recording_stop,
            commands::recording_cancel,
            commands::recording_get_state,
            commands::list_audio_devices,
            commands::get_hotkey_status,
            commands::get_floating_state,
            commands::open_main_settings,
            commands::open_media_files,
            commands::choose_backup_export_path,
            commands::choose_backup_import_path,
            commands::open_models_directory,
            commands::restart_app,
            commands::allow_transcribe_path,
            commands::get_permission_status,
            commands::open_permission_settings,
            commands::get_update_permission_notice,
            commands::dismiss_update_permission_notice,
            commands::get_app_info,
            commands::reveal_app_path,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 应用启动失败")
        .run(|_app, event| {
            // 托盘「退出」、关闭主窗口时退出都走到这里：录音中被「暂停外放」静音的系统输出要先恢复
            if let tauri::RunEvent::Exit = event {
                recording::restore_output_before_exit();
            }
            // macOS：主窗口被隐藏后点 Dock 图标重新显示（旧版 app.on('activate')）
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(_app);
            }
        });
}

/// 显示并聚焦主窗口（二次启动 / Dock 唤起 / 悬浮窗「设置」按钮共用）。
/// 默认优化预设变了（快捷键切换 / 托盘选择）：托盘重建勾选、悬浮窗提示预设名、主窗口同步设置页。
pub fn announce_default_preset(app: &tauri::AppHandle, preset: &att_core::prompts::Preset) {
    use tauri::Emitter;
    tracing::info!("默认优化预设切换为「{}」", preset.name);
    tray::refresh(app);
    floating::preset_notice(app, &preset.name);
    let _ = app.emit_to(
        "main",
        "settings:default-preset",
        serde_json::json!({ "default_preset_id": preset.id, "name": preset.name }),
    );
}

/// 「切换优化预设」快捷键：按预设列表顺序切到下一个。
fn cycle_default_preset(app: &tauri::AppHandle) {
    let Some(core) = app.try_state::<AppState>().map(|state| state.core.clone()) else { return };
    match core.cycle_default_preset() {
        Ok(Some(preset)) => announce_default_preset(app, &preset),
        Ok(None) => floating::preset_notice(app, "还没有优化预设"),
        Err(error) => tracing::warn!("切换优化预设失败: {error}"),
    }
}

pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 显示 / 隐藏主窗口（快捷键「显示主窗口」用）：已经在前台时隐藏，否则显示并聚焦。
pub fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let focused = window.is_focused().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(false);
    if visible && focused {
        let _ = window.hide();
    } else {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 日志同时输出到终端与 `data/logs/att-app.log`（打包版没有终端可看）。
///
/// 每次启动先把上一次的日志改名为 `att-app.prev.log` 再新建：崩溃后用户重启应用，
/// 上一次的现场还在。二次启动的进程在 single-instance 插件里就退出了，走不到这里，
/// 不会把正在运行的实例的日志挪走。
fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,att_core=debug,att_server=debug,att_app=debug,att_hotkey=debug".into());
    let log_dir = data_dir.join("logs");
    let log_path = log_dir.join("att-app.log");
    let file = std::fs::create_dir_all(&log_dir).and_then(|_| {
        if log_path.exists() {
            // 改名失败（如文件被别的程序占着）不阻断启动：照旧覆盖
            let _ = std::fs::rename(&log_path, log_dir.join("att-app.prev.log"));
        }
        std::fs::File::create(&log_path)
    });
    install_panic_hook();
    match file {
        Ok(file) => {
            let file = std::sync::Arc::new(file);
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stdout.and(file)).try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}

/// panic 写进日志：默认的 panic 输出只到 stderr，打包版看不到。任何线程（含 tokio 任务）的 panic
/// 都会经过这里；记完再交给默认处理（终端照常打印）。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("线程 {} panic：{info}\n{backtrace}", thread.name().unwrap_or("<未命名>"));
        default_hook(info);
    }));
}

/// 单实例兜底：single-instance 插件没拦住（macOS 上它的 /tmp 套接字文件可能已被删）时，
/// 用应用数据目录里的进程级文件锁再判一次；另一个实例还活着就直接退出，免得两个进程同时响应热键、
/// 同一段录音输出两遍。加锁本身出错（目录不可写等）不阻断启动。
fn claim_single_instance(app: &tauri::App) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    match instance_lock::acquire(&dir, instance_lock::DEFAULT_WAIT) {
        Ok(Some(lock)) => {
            app.manage(lock);
        }
        Ok(None) => {
            // 日志还没初始化（也不能初始化：会把运行中实例的日志改名），只能写 stderr
            eprintln!("已有另一个落墨实例在运行（开发态与已安装版也算），本进程退出；请先退出那个实例再启动");
            std::process::exit(0);
        }
        Err(error) => eprintln!("单实例锁不可用，照常启动: {error}"),
    }
}

/// 数据目录解析：ATT_DATA_DIR > 开发态仓库 data/（编译期锚点）> 应用数据目录。
fn resolve_data_dir(app: &tauri::App) -> PathBuf {
    if let Ok(dir) = std::env::var("ATT_DATA_DIR") {
        return PathBuf::from(dir);
    }
    // 开发态：使用仓库内 data/（与旧版 npm run electron:dev 行为一致）
    #[cfg(debug_assertions)]
    {
        let repo_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("data");
        if repo_data.is_dir() {
            return repo_data;
        }
    }
    app.path().app_data_dir().expect("无法解析应用数据目录").join("data")
}

/// 启动全局键盘监听：按键匹配当前启用的绑定 → 录音与优化选中文本交给录音 Actor，剪贴板悬浮框直接切换显示。
/// 返回不可用原因（如 macOS 未授予「输入监控」权限），由设置页/首页提示用户。
fn start_hotkey_listener(
    app: tauri::AppHandle,
    core: Arc<CoreState>,
    recorder: recording::RecordingController,
) -> Option<String> {
    use att_core::settings::hotkey::HotkeyAction;
    let cancel_binding = recorder.cancel_binding();
    let binding_core = core.clone();
    let binding_cancel = cancel_binding.clone();
    let bindings: att_hotkey::listener::BindingSource = Arc::new(move || {
        let specs = binding_core.hotkey_specs();
        // 录音会话期间的内部绑定，不来自设置项：非 Idle 时 Esc 取消并吞键
        if let Some(spec) = binding_cancel.spec() {
            let mut with_cancel = Vec::with_capacity(specs.len() + 1);
            with_cancel.extend(specs.iter().cloned());
            with_cancel.push(spec);
            return Arc::new(with_cancel);
        }
        specs
    });
    let sink: att_hotkey::listener::EventSink = Arc::new(move |action, phase| match action {
        HotkeyAction::CancelRecording => {
            if phase == att_hotkey::KeyPhase::Pressed {
                recorder.cancel();
            }
        }
        HotkeyAction::ClipboardPanel => {
            // 不在键盘钩子线程里同步操作窗口：可拦截的监听回调超时会被系统停用
            if phase == att_hotkey::KeyPhase::Pressed {
                let app = app.clone();
                std::thread::spawn(move || clipboard_panel::toggle(&app));
            }
        }
        HotkeyAction::ShowMainWindow => {
            // 同上：窗口的显示 / 隐藏不在键盘钩子线程里做
            if phase == att_hotkey::KeyPhase::Pressed {
                let app = app.clone();
                std::thread::spawn(move || toggle_main_window(&app));
            }
        }
        HotkeyAction::CyclePreset => {
            // 写设置、重建托盘、推悬浮窗都不在键盘钩子线程里做；录音中切换同样生效（优化时才读默认预设）
            if phase == att_hotkey::KeyPhase::Pressed {
                let app = app.clone();
                std::thread::spawn(move || cycle_default_preset(&app));
            }
        }
        _ => recorder.hotkey(action, phase),
    });
    match att_hotkey::listener::start(bindings, sink) {
        Ok(()) => {
            let settings = core.settings();
            let combo = |action| {
                settings
                    .binding_of(action)
                    .filter(|b| b.enabled)
                    .map(|b| b.display(core.platform))
                    .unwrap_or_else(|| "未设置".into())
            };
            tracing::info!(
                "全局快捷键已启用: 按住录音 {} / 点按录音 {} / 剪贴板悬浮框 {} / 优化选中文本 {} / 显示主窗口 {} / 语音改写选中文本 {} / 切换优化预设 {} / 录音中 Esc 取消",
                combo(HotkeyAction::Record),
                combo(HotkeyAction::RecordToggle),
                combo(HotkeyAction::ClipboardPanel),
                combo(HotkeyAction::RewriteSelection),
                combo(HotkeyAction::ShowMainWindow),
                combo(HotkeyAction::VoiceRewriteSelection),
                combo(HotkeyAction::CyclePreset)
            );
            None
        }
        Err(message) => {
            tracing::warn!("全局快捷键不可用: {message}");
            Some(message)
        }
    }
}

/// 开启「自动下载更新」时，启动后稍等片刻在后台检查一次，有新版本就下载安装包
/// （关闭时启动不发任何更新请求，只在用户点「检查更新」时才联网）。
fn spawn_startup_update_check(core: Arc<CoreState>) {
    if !core.settings().auto_download_updates {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // 避开启动高峰（模型预热、窗口创建）
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let result = att_core::update::check_and_maybe_download(core).await;
        tracing::info!("启动时自动检查更新：{}", result.status);
    });
}

/// 每分钟检查一次本地模型空闲时间；录音、文件转写或加载期间跳过。
fn spawn_local_model_idle_monitor(core: Arc<CoreState>, recorder: recording::RecordingController) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(LOCAL_MODEL_IDLE_CHECK_INTERVAL).await;
            let minutes = core.settings().local_model_idle_unload_minutes;
            if minutes <= 0 {
                continue;
            }
            if recorder.is_using_asr() || core.transcriber.is_busy() {
                continue;
            }
            let threshold = Duration::from_secs((minutes as u64).saturating_mul(60));
            let unload_core = core.clone();
            let unloaded = tokio::task::spawn_blocking(move || unload_core.local_models.unload_idle(threshold, false))
                .await
                .unwrap_or(0);
            if unloaded > 0 {
                tracing::debug!("本地模型空闲回收完成：{unloaded} 个引擎");
            }
        }
    });
}

/// 核心事件 → 前端 Tauri events（事件名与旧 WS/IPC 对齐，见 events::AppEvent）。
fn spawn_event_bridge(app: tauri::AppHandle, core: Arc<CoreState>) {
    tauri::async_runtime::spawn(async move {
        let mut rx = core.events.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(payload) = serde_json::to_value(&event) {
                        use tauri::Emitter;
                        let _ = app.emit(event.frontend_event_name(), payload);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!("事件桥丢帧 {skipped}（UI 消费慢，可接受）");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
