//! Tauri 命令层：对旧 electronAPI 31 个方法的 1:1 替换（前端 useElectron.js 换内核不换形状）。
//!
//! 端口/平台/窗口/桌面设置/剪贴板/粘贴模拟/录音会话/麦克风枚举/热键状态/托盘/悬浮窗。

use crate::AppState;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

pub fn err(message: impl Into<String>) -> String {
    message.into()
}

// ---------------- 基础 ----------------

#[tauri::command]
pub fn get_backend_port(state: State<'_, AppState>) -> u16 {
    state.backend_port
}

/// 数据面令牌：与端口一样优先走 initialization_script 注入，这里是注入失败时的兜底。
#[tauri::command]
pub fn get_backend_token(state: State<'_, AppState>) -> String {
    state.backend_token.clone()
}

/// 平台字符串沿用 Electron 命名（win32/darwin/linux），前端零改动。
#[tauri::command]
pub fn get_platform() -> String {
    att_core::Platform::current().as_str().to_string()
}

// ---------------- 窗口 ----------------

#[tauri::command]
pub fn window_minimize(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.minimize();
    }
}

#[tauri::command]
pub fn window_toggle_maximize(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let is_max = window.is_maximized().unwrap_or(false);
        let _ = if is_max { window.unmaximize() } else { window.maximize() };
    }
}

#[tauri::command]
pub fn window_close(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }
}

// ---------------- 桌面设置（单一真相源 = data/settings.json）----------------

/// 旧版 userData/desktop-settings.json 的兼容形状，并补充启动与窗口桌面设置。
pub fn desktop_settings_view(state: &AppState) -> Value {
    let settings = state.core.settings();
    json!({
        "hotkey": settings.legacy_hotkey_view(state.core.platform),
        "pause_audio_on_recording": settings.pause_audio_on_recording,
        "launch_at_login": settings.launch_at_login,
        "main_window_close_action": settings.main_window_close_action,
    })
}

#[tauri::command]
pub fn get_desktop_settings(app: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    crate::launch::reconcile_launch_at_login(&app, &state.core)?;
    Ok(desktop_settings_view(&state))
}

/// 保存桌面设置（hotkey 旧形状 + 静音策略 + 开机自启），写回唯一真相源。
#[tauri::command]
pub fn save_desktop_settings(app: AppHandle, state: State<'_, AppState>, payload: Value) -> Result<Value, String> {
    if let Some(value) = payload.get("launch_at_login").filter(|value| !value.is_null()) {
        let enabled = value.as_bool().ok_or("字段 launch_at_login 类型非法，期望布尔值")?;
        crate::launch::set_launch_at_login(&app, &state.core, enabled)?;
    }
    let result = state.core.update_settings(|settings| {
        if let Some(hotkey) = payload.get("hotkey").filter(|h| h.is_object()) {
            settings.update_record_binding_from_legacy(state.core.platform, hotkey);
        }
        if let Some(pause) = payload.get("pause_audio_on_recording").and_then(|v| v.as_bool()) {
            settings.pause_audio_on_recording = pause;
        }
    });
    match result {
        Ok(()) => Ok(desktop_settings_view(&state)),
        Err(e) => Err(err(e.to_string())),
    }
}

/// 旧 desktop:sync-input-settings 通道：更新按住录音的组合键。
/// 热键监听每次按键都读取最新设置，不需要重新注册。
#[tauri::command]
pub fn sync_input_settings(state: State<'_, AppState>, payload: Value) -> Result<Value, String> {
    let mut restart_hint = false;
    let result = state.core.update_settings(|settings| {
        if let Some(hotkey) = payload.get("hotkey").filter(|h| h.is_object()) {
            settings.update_record_binding_from_legacy(state.core.platform, hotkey);
            restart_hint = true;
        }
    });
    match result {
        Ok(()) => {
            if restart_hint {
                tracing::debug!("录音快捷键已更新");
            }
            Ok(json!({
                "success": true,
                "managedBy": "rust-core",
                "settings": { "hotkey": state.core.settings().legacy_hotkey_view(state.core.platform) },
                "desktopSettings": desktop_settings_view(&state),
            }))
        }
        Err(e) => Err(err(e.to_string())),
    }
}

/// 旧 desktop:sync-settings：静音策略等。
#[tauri::command]
pub fn sync_desktop_settings(state: State<'_, AppState>, payload: Value) -> Result<Value, String> {
    let result = state.core.update_settings(|settings| {
        if let Some(pause) = payload.get("pause_audio_on_recording").and_then(|v| v.as_bool()) {
            settings.pause_audio_on_recording = pause;
        }
    });
    match result {
        Ok(()) => Ok(json!({ "success": true, "settings": desktop_settings_view(&state) })),
        Err(e) => Err(err(e.to_string())),
    }
}

// ---------------- 剪贴板 ----------------

#[tauri::command]
pub fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| err(e.to_string()))?;
    clipboard.get_text().map_err(|e| err(e.to_string()))
}

#[tauri::command]
pub async fn write_clipboard_text(text: String) -> Result<bool, String> {
    // 不能在主线程上写：剪贴板被占用时要等几轮重试，期间所有窗口都卡住
    tauri::async_runtime::spawn_blocking(move || crate::desktop::write_clipboard(&text))
        .await
        .map_err(|e| err(e.to_string()))??;
    Ok(true)
}

/// 写图片剪贴板：imagePath 解析（相对路径锚定 data/clipboard/），PNG 解码为 RGBA。
///
/// 落剪贴板的格式由 `clipboard_image` 决定：Windows 上必须同时给 CF_DIB / CF_DIBV5（BI_RGB）+ PNG，
/// 否则 Chromium/Skia 系应用（含网页输入框）粘不出图。
///
/// 放到阻塞线程：读文件 + 整图解码 + 转位图，大截图要几百毫秒，不能卡住主线程（所有窗口都会冻住）。
#[tauri::command]
pub async fn write_clipboard_image(app: AppHandle, state: State<'_, AppState>, payload: Value) -> Result<bool, String> {
    let raw_path = payload.get("imagePath").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let data_dir = state.core.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || write_clipboard_image_blocking(&app, &data_dir, &raw_path))
        .await
        .map_err(|e| err(e.to_string()))??;
    Ok(true)
}

fn write_clipboard_image_blocking(app: &AppHandle, data_dir: &std::path::Path, raw_path: &str) -> Result<(), String> {
    let resolved = resolve_clipboard_image_path(data_dir, raw_path)?;
    let bytes = std::fs::read(&resolved).map_err(|e| err(format!("读取图片失败: {e}")))?;
    let img = image::load_from_memory(&bytes).map_err(|e| err(format!("图片解码失败: {e}")))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    // 拿不到换代计数的平台才需要按内容标记（要对整图算哈希）；其余写完记换代计数即可
    if crate::desktop::clipboard_change_count().is_none() {
        crate::clipboard::mark_internal_image(rgba.as_raw());
    }
    // 原文件本来就是 PNG：直接作为剪贴板的 PNG 格式，免得重新编码
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n").then_some(bytes.as_slice());
    crate::clipboard_image::write_rgba(app, rgba.as_raw(), width, height, png)?;
    crate::clipboard::mark_internal_change();
    Ok(())
}

/// 解析剪贴板图片路径（相对路径锚定 `data/clipboard/`），并确认真实路径落在数据目录内。
///
/// 必须先 canonicalize 再比较：`Path::starts_with` 只按路径分段比较、不解析 `..`，
/// `data/clipboard/../../x` 这类路径会原样通过；符号链接同理。
pub fn resolve_clipboard_image_path(data_dir: &std::path::Path, raw_path: &str) -> Result<std::path::PathBuf, String> {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return Err(err("imagePath 不能为空"));
    }
    let path = std::path::Path::new(raw_path);
    let joined = if path.is_absolute() { path.to_path_buf() } else { data_dir.join("clipboard").join(path) };
    let resolved = std::fs::canonicalize(&joined).map_err(|e| err(format!("读取图片失败: {e}")))?;
    let root = std::fs::canonicalize(data_dir).map_err(|e| err(format!("数据目录不可用: {e}")))?;
    if !resolved.starts_with(&root) {
        return Err(err("图片路径越界"));
    }
    Ok(resolved)
}

/// 模拟「粘贴」到焦点窗口（Cmd/Ctrl+V）。
#[tauri::command]
pub async fn paste_active() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(crate::desktop::paste_active).await.map_err(|e| err(e.to_string()))??;
    Ok(json!({ "success": true }))
}

// ---------------- 剪贴板悬浮框 ----------------

/// 悬浮框页面的上下文（回车行为、唤起快捷键），每次唤起时读取。
#[tauri::command]
pub fn get_clipboard_panel_context(state: State<'_, AppState>) -> Value {
    crate::clipboard_panel::context(&state.core)
}

/// Esc 收起：焦点还给唤起前的应用。
#[tauri::command]
pub fn hide_clipboard_panel(app: AppHandle) {
    crate::clipboard_panel::hide(&app, true);
}

/// 拖动调整悬浮框尺寸（面板宽高，逻辑像素）；`save` 时记进设置。返回夹取后的实际尺寸。
#[tauri::command]
pub fn clipboard_panel_resize(app: AppHandle, width: f64, height: f64, save: bool) -> Value {
    crate::clipboard_panel::resize(&app, width, height, save)
}

/// 上下拖动悬浮框（离屏幕底边的距离，逻辑像素）；`save` 时记进设置。返回夹取后的实际距离。
#[tauri::command]
pub fn clipboard_panel_move(app: AppHandle, bottom_offset: f64, save: bool) -> Value {
    crate::clipboard_panel::move_to(&app, bottom_offset, save)
}

/// 设置页「恢复默认尺寸」。
#[tauri::command]
pub fn clipboard_panel_reset_size(app: AppHandle) {
    crate::clipboard_panel::reset_size(&app);
}

/// 设置页「恢复默认位置」/ 悬浮框头部双击：回到贴底。
#[tauri::command]
pub fn clipboard_panel_reset_position(app: AppHandle) {
    crate::clipboard_panel::reset_position(&app);
}

/// 选中条目后收尾（条目已由页面写进系统剪贴板）：收起并还焦点，`paste` 时再模拟一次粘贴。
#[tauri::command]
pub async fn clipboard_panel_finish(app: AppHandle, paste: bool) -> Result<Value, String> {
    crate::clipboard_panel::hide(&app, true);
    if paste {
        tauri::async_runtime::spawn_blocking(crate::desktop::paste_active).await.map_err(|e| err(e.to_string()))??;
    }
    Ok(json!({ "success": true }))
}

// ---------------- 录音（Rust 侧采集 + 识别，前端只做展示）----------------

#[tauri::command]
pub fn recording_start(state: State<'_, AppState>) {
    state.recorder.start();
}

#[tauri::command]
pub fn recording_stop(state: State<'_, AppState>) {
    state.recorder.stop();
}

#[tauri::command]
pub fn recording_cancel(state: State<'_, AppState>) {
    state.recorder.cancel();
}

#[tauri::command]
pub fn recording_get_state(state: State<'_, AppState>) -> Value {
    state.recorder.state()
}

/// 麦克风列表（设置页下拉框；Tauri 下不再走 getUserMedia 枚举）。
#[tauri::command]
pub async fn list_audio_devices() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(att_audio::list_input_devices).await.unwrap_or_default()
}

/// 全局快捷键状态：{ enabled, error }（error 为权限缺失等引导文案）。
#[tauri::command]
pub fn get_hotkey_status(state: State<'_, AppState>) -> Value {
    let error = state.hotkey_error.lock().ok().and_then(|e| e.clone());
    json!({ "enabled": error.is_none(), "error": error })
}

// ---------------- 托盘 / 悬浮窗 ----------------

#[tauri::command]
pub fn set_tray_icon(app: AppHandle, icon_type: String) {
    crate::tray::set_icon(&app, &icon_type);
}

/// 前端记录列表变化时触发；菜单内容以 Rust 记录库为准（载荷忽略）。
#[tauri::command]
pub fn update_tray_menu(app: AppHandle, records: Value) {
    let _ = records;
    crate::tray::refresh(&app);
}

#[tauri::command]
pub fn show_floating_window(app: AppHandle) {
    crate::floating::update(&app, crate::floating::last_state(), true);
}

/// 悬浮窗结果态停留到点后自行收起：只收已结束的会话（与新一次录音并发时不误藏）。
#[tauri::command]
pub fn hide_floating_window(app: AppHandle) {
    crate::floating::dismiss(&app);
}

#[tauri::command]
pub fn toggle_floating_window(app: AppHandle) {
    let visible = app.get_webview_window(crate::floating::LABEL).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if visible {
        crate::floating::hide(&app);
    } else {
        crate::floating::update(&app, crate::floating::last_state(), true);
    }
}

/// 悬浮窗页面上报胶囊区域 [x, y, 宽, 高]（CSS px）；null = 胶囊不可见。用于透明部分点击穿透。
#[tauri::command]
pub fn floating_hit_rect(rect: Option<[f64; 4]>) {
    crate::floating::set_hit_rect(rect);
}

/// 悬浮窗失败态点胶囊：唤起主窗口（看完整原因与失败记录）。
#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    crate::show_main_window(&app);
}

/// 悬浮窗「设置」按钮：唤起主窗口并跳到设置页（旧 main:open-settings 通道）。
#[tauri::command]
pub fn open_main_settings(app: AppHandle) {
    use tauri::Emitter;
    crate::show_main_window(&app);
    let _ = app.emit_to("main", "navigate:settings", ());
}

/// 悬浮窗页面挂载时拉取最近状态（页面加载晚于首次推送的情形）。
#[tauri::command]
pub fn get_floating_state() -> Value {
    crate::floating::last_state()
}

// ---------------- 转文字页 ----------------

/// 系统「选择文件」对话框（可多选，按顺序排队转写）：只列音视频容器；用户取消返回空列表。
#[tauri::command]
pub async fn open_media_files(app: AppHandle) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    let extensions: Vec<&str> = att_core::transcribe::media::VIDEO_EXTENSIONS
        .iter()
        .chain(att_core::transcribe::media::AUDIO_EXTENSIONS)
        .copied()
        .collect();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择要转写的音视频文件")
            .add_filter("音视频文件", &extensions)
            .blocking_pick_files()
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    picked
        .into_iter()
        .filter_map(|file| file.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// 设置 → 诊断「导出备份」：系统保存对话框，默认文件名带日期；取消返回 None。
#[tauri::command]
pub async fn choose_backup_export_path(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let suggested = format!("att-backup-{}.zip", chrono::Local::now().format("%Y-%m-%d"));
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("保存数据备份")
            .set_file_name(suggested)
            .add_filter("ATT 备份", &["zip"])
            .blocking_save_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked.and_then(|picked| picked.into_path().ok()) else {
        return Ok(None);
    };
    let selected = path;
    let path = ensure_zip_extension(selected.clone());
    if path != selected && path.exists() {
        return Err(format!("已存在同名备份 {}，请重新选择并在文件名中保留 .zip 后缀", path.display()));
    }
    if !state.core.backup_paths.authorize_export(&path) {
        return Err("备份保存路径无效，请重新选择".into());
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// 设置 → 诊断「从备份恢复」：系统文件选择对话框，只列 zip；取消返回 None。
#[tauri::command]
pub async fn choose_backup_import_path(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("选择要恢复的 ATT 备份").add_filter("ATT 备份", &["zip"]).blocking_pick_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked.and_then(|picked| picked.into_path().ok()) else {
        return Ok(None);
    };
    if !state.core.backup_paths.authorize_import(&path) {
        return Err("备份文件路径无效，请重新选择".into());
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

fn ensure_zip_extension(mut path: std::path::PathBuf) -> std::path::PathBuf {
    let has_zip = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if !has_zip {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".zip");
        path.set_file_name(name);
    }
    path
}

/// 恢复备份后旧进程仍持有旧密钥与目录索引：由用户确认后重启整个应用。
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        app.restart();
    });
}

/// 允许转写一个本机文件：用户在界面里选择 / 拖入 / 手动确认路径后由前端调用。
///
/// 这是 HTTP 转写接口的精确白名单入口；外部网页与进程无法调用 Tauri 命令，
/// 即使拿到数据面地址也不能自行扩大可读文件范围。
#[tauri::command]
pub fn allow_transcribe_path(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let resolved = state.core.allow_transcribe_file(std::path::Path::new(&path))?;
    Ok(resolved.to_string_lossy().into_owned())
}

// ---------------- 设置页：权限 / 诊断 ----------------

/// 系统权限状态（只查询，不触发授权弹窗）。
#[tauri::command]
pub fn get_permission_status() -> Value {
    crate::permissions::status()
}

/// 打开对应权限的系统设置面板。
#[tauri::command]
pub fn open_permission_settings(id: String) -> Result<(), String> {
    crate::permissions::open_settings(&id)
}

/// 更新后首次启动的权限引导：`{ show, previous_version, current_version, items }`。
/// 不是更新后首次启动、引导已关掉、或权限都正常时 `show` 为 false。
#[tauri::command]
pub fn get_update_permission_notice(app: AppHandle, state: State<'_, AppState>) -> Value {
    let previous = state.update_notice.lock().unwrap_or_else(|e| e.into_inner()).clone();
    crate::permissions::update_notice(previous.as_deref(), &app.package_info().version.to_string())
}

/// 关掉更新后的权限引导（本次启动内不再提示；下次启动版本号已一致，也不会再出现）。
#[tauri::command]
pub fn dismiss_update_permission_notice(state: State<'_, AppState>) {
    *state.update_notice.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 诊断信息：版本、数据目录、日志文件、提示词目录、数据面端口。
#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> Value {
    json!({
        "version": app.package_info().version.to_string(),
        "data_dir": state.core.data_dir.display().to_string(),
        "log_file": state.core.data_dir.join("logs").join("att-app.log").display().to_string(),
        "prompts_dir": state.core.prompts_dir.display().to_string(),
        "backend_port": state.backend_port,
    })
}

/// 在文件管理器中显示数据目录（kind = "data"）、日志文件（kind = "log"）、模型目录
/// （kind = "models"）或已下载的更新安装包（kind = "update"）。
#[tauri::command]
pub fn reveal_app_path(state: State<'_, AppState>, kind: String) -> Result<(), String> {
    let path = match kind.as_str() {
        "data" => state.core.data_dir.clone(),
        "log" => state.core.data_dir.join("logs").join("att-app.log"),
        "models" => {
            std::fs::create_dir_all(&state.core.models_dir).map_err(|error| format!("无法创建模型目录：{error}"))?;
            state.core.models_dir.clone()
        }
        "update" => state.core.updater.downloaded_path().ok_or("还没有下载好的更新安装包")?,
        other => return Err(format!("未知路径类型：{other}")),
    };
    crate::permissions::reveal(&path)
}

// ---------------- 调试 ----------------

#[tauri::command]
pub fn debug_log(scope: String, message: String, details: Option<Value>) {
    tracing::info!("[{scope}] {message} {details:?}");
}

#[cfg(test)]
mod tests {
    use super::resolve_clipboard_image_path;

    #[test]
    fn 剪贴板图片路径_相对路径锚定且拒绝越界() {
        let root = std::env::temp_dir().join(format!("att-clip-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join("data");
        std::fs::create_dir_all(data_dir.join("clipboard")).unwrap();
        std::fs::write(data_dir.join("clipboard").join("a.png"), b"x").unwrap();
        std::fs::write(root.join("secret.png"), b"x").unwrap();

        let ok = resolve_clipboard_image_path(&data_dir, "a.png").unwrap();
        assert!(ok.ends_with("clipboard/a.png") || ok.ends_with("clipboard\\a.png"));
        // `..` 逃逸：旧实现的 starts_with 分段比较会放行
        assert_eq!(resolve_clipboard_image_path(&data_dir, "../../secret.png").unwrap_err(), "图片路径越界");
        // 绝对路径指向数据目录外
        let outside = root.join("secret.png");
        assert_eq!(resolve_clipboard_image_path(&data_dir, outside.to_str().unwrap()).unwrap_err(), "图片路径越界");
        assert!(resolve_clipboard_image_path(&data_dir, "  ").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
