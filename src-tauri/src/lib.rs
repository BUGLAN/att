//! Tauri 壳：装配核心 → 迁移 → 内嵌数据面 → 命令注册（阶段 3a）。
//!
//! 启动顺序（对应旧 electron/main.js 的 whenReady 链，架构文档 §1.1）：
//! 1. 解析数据目录（env > 开发态仓库 data/ > 应用数据目录）
//! 2. 一次性迁移（幂等）
//! 3. CoreState 初始化 + axum 数据面（127.0.0.1 随机端口）
//! 4. 单实例锁（二次启动唤起已有窗口）

// 注意：Tauri generate_handler 需要命令模块公开可见（__cmd__ 宏重导出）
pub mod commands;

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
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,att_core=debug,att_server=debug,att_app=debug".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 二次启动：唤起主窗口（与旧版 second-instance 行为一致）
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let data_dir = resolve_data_dir(app);
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

            let core = CoreState::init(&data_dir, Platform::current())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
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

            // 状态先注册：保证窗口首屏的 invoke 一定能取到 State
            // （shutdown_tx 放进 AppState 随应用存活，防止数据面被误关）
            app.manage(AppState {
                core,
                backend_port: port,
                shutdown_tx,
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
            commands::set_tray_icon,
            commands::update_tray_menu,
            commands::show_floating_window,
            commands::hide_floating_window,
            commands::toggle_floating_window,
            commands::update_floating_window_state,
            commands::debug_log,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
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
