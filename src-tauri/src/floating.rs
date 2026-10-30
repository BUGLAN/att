//! 录音悬浮窗（屏幕底部居中的胶囊条，移植自 electron/floatingWindow）。
//!
//! 由录音 Actor 直接驱动（不经过主窗口前端：主窗口在后台/隐藏时同样可见）。
//! 页面 floating.html 里的 `window.floatingAPI` 由 floating.js 在 Tauri 下垫片实现：
//! 订阅 `floating:state`、挂载时 `get_floating_state` 拉取最近状态、关闭走 `hide_floating_window`。

use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const LABEL: &str = "floating";
const WIDTH: f64 = 512.0;
const HEIGHT: f64 = 40.0;
const BOTTOM_OFFSET: f64 = 32.0;

static LAST_STATE: Mutex<Option<Value>> = Mutex::new(None);

/// 预创建隐藏的悬浮窗（首次录音时立即可显示，不等页面加载）。
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
    let builder = tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("floating.html".into()))
        .title("ATT Floating")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .shadow(false)
        .visible(false);
    builder.build()?;
    Ok(())
}

fn position_bottom_center(window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else { return };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let (x, y) = (area.position.x as f64 / scale, area.position.y as f64 / scale);
    let (w, h) = (area.size.width as f64 / scale, area.size.height as f64 / scale);
    let _ = window.set_position(tauri::LogicalPosition::new(x + (w - WIDTH) / 2.0, y + h - HEIGHT - BOTTOM_OFFSET));
}

/// 推送状态；`show=true` 时同时显示（不抢焦点，避免打断用户正在输入的窗口）。
pub fn update(app: &AppHandle, state: Value, show: bool) {
    if let Ok(mut last) = LAST_STATE.lock() {
        *last = Some(state.clone());
    }
    let Some(window) = app.get_webview_window(LABEL) else { return };
    if show && !window.is_visible().unwrap_or(false) {
        position_bottom_center(&window);
        let _ = window.show();
    }
    let _ = app.emit_to(LABEL, "floating:state", state);
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    if let Ok(mut last) = LAST_STATE.lock() {
        *last = Some(json!({ "phase": "idle", "text": "", "duration": 0 }));
    }
}

pub fn last_state() -> Value {
    LAST_STATE
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_else(|| json!({ "phase": "idle", "text": "", "duration": 0 }))
}

/// 录音 Actor 状态 → 悬浮窗状态（悬浮窗页面的相位词表：recording/transcribing/complete/error）。
pub fn from_recording_state(payload: &Value) -> Option<(Value, bool)> {
    let mode = payload["mode"].as_str().unwrap_or("single");
    let message = payload["message"].as_str().unwrap_or("");
    let state = |phase: &str, text: &str| json!({ "phase": phase, "mode": mode, "text": text, "duration": 0 });
    match payload["phase"].as_str()? {
        "connecting" | "recording" => Some((state("recording", ""), true)),
        "finalizing" => Some((state("transcribing", ""), true)),
        "completed" => {
            let text = payload["text"].as_str().filter(|t| !t.is_empty()).unwrap_or(message);
            Some((state("complete", text), true))
        }
        "error" => Some((state("error", message), true)),
        _ => None,
    }
}
