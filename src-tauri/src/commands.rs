//! Tauri 命令层：对旧 electronAPI 31 个方法的 1:1 替换（前端 useElectron.js 换内核不换形状）。
//!
//! 3a 范围：端口/平台/窗口/桌面设置/剪贴板/托盘与悬浮窗占位/调试日志。
//! 3b-3g：录音会话、热键、音频设备、托盘/悬浮窗真实现、粘贴模拟。

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

/// 旧版 userData/desktop-settings.json 的兼容形状：{hotkey, pause_audio_on_recording}。
pub fn desktop_settings_view(state: &AppState) -> Value {
    let settings = state.core.settings();
    json!({
        "hotkey": settings.legacy_hotkey_view(state.core.platform),
        "pause_audio_on_recording": settings.pause_audio_on_recording,
    })
}

#[tauri::command]
pub fn get_desktop_settings(state: State<'_, AppState>) -> Value {
    desktop_settings_view(&state)
}

/// 保存桌面设置（hotkey 旧形状 + 静音策略），写回唯一真相源。
#[tauri::command]
pub fn save_desktop_settings(
    state: State<'_, AppState>,
    payload: Value,
) -> Result<Value, String> {
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

/// 旧 desktop:sync-input-settings 通道：热键更新（3c 接重绑）。
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
                // 3c：这里通知热键后端重绑（当前无后端，仅日志）
                tracing::info!("热键设置已更新（重绑在 3c 生效）");
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

/// 旧 desktop:recording-state：录音开始/结束（系统静音联动在 3g 接入）。
#[tauri::command]
pub fn sync_recording_state(payload: Value) -> Value {
    let recording = payload.get("recording").and_then(|v| v.as_bool()).unwrap_or(false);
    tracing::debug!("录音状态同步（静音联动 3g 接入）: recording={recording}");
    json!({ "success": true, "skipped": true, "reason": "mute-pending-3g" })
}

// ---------------- 剪贴板 ----------------

#[tauri::command]
pub fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| err(e.to_string()))?;
    clipboard.get_text().map_err(|e| err(e.to_string()))
}

#[tauri::command]
pub fn write_clipboard_text(text: String) -> Result<bool, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| err(e.to_string()))?;
    clipboard.set_text(text).map_err(|e| err(e.to_string()))?;
    Ok(true)
}

/// 写图片剪贴板：imagePath 解析（相对路径锚定 data/clipboard/），PNG 解码为 RGBA。
#[tauri::command]
pub fn write_clipboard_image(state: State<'_, AppState>, payload: Value) -> Result<bool, String> {
    let raw_path = payload.get("imagePath").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if raw_path.is_empty() {
        return Err(err("imagePath 不能为空"));
    }
    let path = std::path::Path::new(&raw_path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.core.data_dir.join("clipboard").join(&raw_path)
    };
    // 路径安全：必须落在数据目录内
    if !resolved.starts_with(&state.core.data_dir) {
        return Err(err("图片路径越界"));
    }
    let bytes = std::fs::read(&resolved).map_err(|e| err(format!("读取图片失败: {e}")))?;
    let img = image::load_from_memory(&bytes).map_err(|e| err(format!("图片解码失败: {e}")))?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| err(e.to_string()))?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: rgba.into_raw().into(),
        })
        .map_err(|e| err(e.to_string()))?;
    Ok(true)
}

/// 模拟「粘贴」到焦点窗口（enigo，3g 接入）。
#[tauri::command]
pub fn paste_active() -> Result<Value, String> {
    Err(err("系统粘贴将在阶段 3g 接入"))
}

// ---------------- 托盘 / 悬浮窗（3g 真实现）----------------

#[tauri::command]
pub fn set_tray_icon(icon_type: String) {
    tracing::debug!("托盘图标请求（3g 接入）: {icon_type}");
}

#[tauri::command]
pub fn update_tray_menu(records: Value) {
    tracing::debug!("托盘菜单请求（3g 接入）: {} 条", records.as_array().map(Vec::len).unwrap_or(0));
}

#[tauri::command]
pub fn show_floating_window() {}

#[tauri::command]
pub fn hide_floating_window() {}

#[tauri::command]
pub fn toggle_floating_window() {}

#[tauri::command]
pub fn update_floating_window_state(payload: Value) {
    let _ = payload; // 3g：转发给悬浮窗 webview
}

// ---------------- 调试 ----------------

#[tauri::command]
pub fn debug_log(scope: String, message: String, details: Option<Value>) {
    tracing::info!("[{scope}] {message} {details:?}");
}
