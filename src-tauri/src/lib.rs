//! Tauri 壳：装配核心 → 迁移 → 内嵌数据面 → 命令注册（阶段 3a）。
//!
//! 启动顺序（对应旧 electron/main.js 的 whenReady 链，架构文档 §1.1）：
//! 1. 解析数据目录（env > 开发态仓库 data/ > 应用数据目录）
//! 2. 一次性迁移（幂等）
//! 3. CoreState 初始化 + axum 数据面（127.0.0.1 随机端口）
//! 4. 单实例锁（二次启动唤起已有窗口）

// 注意：Tauri generate_handler 需要命令模块公开可见（__cmd__ 宏重导出）
pub mod clipboard;
pub mod clipboard_panel;
pub mod commands;
pub mod desktop;
pub mod floating;
pub mod permissions;
pub mod recording;
pub mod tray;
pub mod typing;

use att_core::{migration, state::CoreState, Platform};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

/// 命令层共享状态。
pub struct AppState {
    pub core: Arc<CoreState>,
    pub backend_port: u16,
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
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 二次启动：唤起主窗口（与旧版 second-instance 行为一致）
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            // 关闭主窗口（与旧 electron/main.js 的 close / window-all-closed 行为对齐）：
            // 悬浮窗是预创建的隐藏窗口，任由主窗口销毁会让进程带着全局热键残留在后台，
            // 二次启动也找不到 "main" 可唤起。
            // - macOS：只隐藏，点 Dock 图标（RunEvent::Reopen）再显示；Cmd+Q 照常退出
            // - 其他平台：关闭即退出应用
            if window.label() == clipboard_panel::LABEL {
                // 剪贴板悬浮框点到别处即收起（Spotlight 式）
                if let tauri::WindowEvent::Focused(false) = event {
                    clipboard_panel::on_blur(window.app_handle());
                }
                return;
            }
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if cfg!(target_os = "macos") {
                    let _ = window.hide();
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .setup(|app| {
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

            let mut core = CoreState::init(&data_dir, Platform::current())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // 提示词预设：开发态用仓库 prompts/，打包版用随包资源（bundle.resources）
            core.prompts_dir = att_core::prompts::resolve_dir(&data_dir, app.path().resource_dir().ok().as_deref());
            tracing::info!("提示词目录: {}", core.prompts_dir.display());
            let core = Arc::new(core);

            // 数据面：先用同步 bind 占住端口（优先固定端口，占用则回退随机），
            // 再把 accept 循环显式 spawn 到 Tauri 的运行时里。
            let server_state = Arc::new(att_server::ServerState { core: core.clone() });
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

            // 诊断信息：端口落盘，排障时可直接打开 http://127.0.0.1:<port>/api/health
            let _ = std::fs::write(data_dir.join(".backend-port"), port.to_string());

            // 事件桥：核心事件 → 前端（Tauri event，3b 起随录音接入逐步生效）
            spawn_event_bridge(app.handle().clone(), core.clone());

            // 录音 Actor + 全局热键（热键直达 Actor，不经过前端：窗口隐藏/后台时同样可用）
            let recorder = recording::RecordingController::spawn(app.handle().clone(), core.clone());
            let hotkey_error = start_hotkey_listener(app.handle().clone(), core.clone(), recorder.clone());

            // 状态先注册：保证窗口首屏的 invoke 一定能取到 State
            // （shutdown_tx 放进 AppState 随应用存活，防止数据面被误关）
            app.manage(AppState {
                core,
                backend_port: port,
                shutdown_tx,
                recorder,
                hotkey_error: std::sync::Mutex::new(hotkey_error),
            });

            // 主窗口在 setup 里手工创建（而非 tauri.conf 声明）：
            // 这样能把后端端口通过 initialization_script 注入到页面脚本执行之前，
            // 前端 getBackendPort() 首选的 window.__ATT_BACKEND_PORT__ 因此永远可用，
            // 不再依赖 invoke 是否成功（3a 实机验收时 invoke 路径曾失败导致回退 8000）。
            let init_script = format!(
                "window.__ATT_BACKEND_PORT__ = {port}; window.__ATT_IS_DESKTOP__ = true;"
            );
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
                .title("Audio to Text")
                .inner_size(1200.0, 800.0)
                .min_inner_size(800.0, 600.0)
                .decorations(false)
                .center()
                .background_color(tauri::window::Color(10, 10, 11, 255))
                .initialization_script(&init_script)
                .build()?;

            // 录音悬浮窗：预创建隐藏，由录音 Actor 直接驱动
            if let Err(error) = floating::create(app.handle()) {
                tracing::warn!("创建悬浮窗失败（不影响录音）: {error}");
            }
            // 剪贴板悬浮框：预创建隐藏，由全局快捷键唤起
            if let Err(error) = clipboard_panel::create(app.handle(), port) {
                tracing::warn!("创建剪贴板悬浮框失败: {error}");
            }
            // 托盘（四态图标 + 最近记录菜单）与剪贴板后台监控
            if let Err(error) = tray::create(app.handle()) {
                tracing::warn!("创建托盘失败: {error}");
            }
            if let Some(state) = app.try_state::<AppState>() {
                clipboard::start(state.core.clone());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_backend_port,
            commands::get_platform,
            commands::window_minimize,
            commands::window_toggle_maximize,
            commands::window_close,
            commands::get_desktop_settings,
            commands::save_desktop_settings,
            commands::sync_input_settings,
            commands::sync_desktop_settings,
            commands::sync_recording_state,
            commands::read_clipboard_text,
            commands::write_clipboard_text,
            commands::write_clipboard_image,
            commands::paste_active,
            commands::get_clipboard_panel_context,
            commands::hide_clipboard_panel,
            commands::clipboard_panel_finish,
            commands::clipboard_panel_resize,
            commands::clipboard_panel_move,
            commands::clipboard_panel_reset_size,
            commands::clipboard_panel_reset_position,
            commands::set_tray_icon,
            commands::update_tray_menu,
            commands::show_floating_window,
            commands::hide_floating_window,
            commands::toggle_floating_window,
            commands::update_floating_window_state,
            commands::debug_log,
            commands::recording_start,
            commands::recording_stop,
            commands::recording_cancel,
            commands::recording_get_state,
            commands::list_audio_devices,
            commands::get_hotkey_status,
            commands::get_floating_state,
            commands::open_main_settings,
            commands::open_media_file,
            commands::get_permission_status,
            commands::open_permission_settings,
            commands::get_app_info,
            commands::reveal_app_path,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 应用启动失败")
        .run(|_app, _event| {
            // macOS：主窗口被隐藏后点 Dock 图标重新显示（旧版 app.on('activate')）
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_main_window(_app);
            }
        });
}

/// 显示并聚焦主窗口（二次启动 / Dock 唤起 / 悬浮窗「设置」按钮共用）。
pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 日志同时输出到终端与 `data/logs/att-app.log`（每次启动覆盖；打包版没有终端可看）。
fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,att_core=debug,att_server=debug,att_app=debug,att_hotkey=debug".into());
    let log_dir = data_dir.join("logs");
    let file = std::fs::create_dir_all(&log_dir)
        .and_then(|_| std::fs::File::create(log_dir.join("att-app.log")));
    match file {
        Ok(file) => {
            let file = std::sync::Arc::new(file);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stdout.and(file))
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
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
    app.path()
        .app_data_dir()
        .expect("无法解析应用数据目录")
        .join("data")
}

/// 启动全局键盘监听：按键匹配当前启用的绑定 → 录音交给录音 Actor，剪贴板悬浮框直接切换显示。
/// 返回不可用原因（如 macOS 未授予「输入监控」权限），由设置页/首页提示用户。
fn start_hotkey_listener(
    app: tauri::AppHandle,
    core: Arc<CoreState>,
    recorder: recording::RecordingController,
) -> Option<String> {
    use att_core::settings::hotkey::HotkeyAction;
    let binding_core = core.clone();
    let bindings: att_hotkey::listener::BindingSource = Arc::new(move || {
        binding_core
            .settings()
            .hotkeys
            .iter()
            .filter(|b| b.enabled && matches!(b.action, HotkeyAction::Record | HotkeyAction::ClipboardPanel))
            .map(att_hotkey::HotkeySpec::from_binding)
            .collect()
    });
    let sink: att_hotkey::listener::EventSink = Arc::new(move |action, phase| match action {
        HotkeyAction::ClipboardPanel => {
            // 不在键盘钩子线程里同步操作窗口：可拦截的监听回调超时会被系统停用
            if phase == att_hotkey::KeyPhase::Pressed {
                let app = app.clone();
                std::thread::spawn(move || clipboard_panel::toggle(&app));
            }
        }
        _ => recorder.hotkey(action, phase),
    });
    match att_hotkey::listener::start(bindings, sink) {
        Ok(()) => {
            let settings = core.settings();
            let combo = |action| settings.binding_of(action).map(|b| b.display(core.platform)).unwrap_or_default();
            tracing::info!(
                "全局快捷键已启用: 录音 {} / 剪贴板悬浮框 {}",
                combo(HotkeyAction::Record),
                combo(HotkeyAction::ClipboardPanel)
            );
            None
        }
        Err(message) => {
            tracing::warn!("全局快捷键不可用: {message}");
            Some(message)
        }
    }
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
