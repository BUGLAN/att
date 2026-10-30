//! 录音悬浮窗（屏幕底部居中的极简胶囊：一枚与托盘同源的印章 + 至多一行文字）。
//!
//! 由录音 Actor 直接驱动（不经过主窗口前端：主窗口在后台/隐藏时同样可见）。
//! 页面 floating.html 里的 `window.floatingAPI` 由 floating.js 在 Tauri 下垫片实现：
//! 订阅 `floating:state`、挂载时 `get_floating_state` 拉取最近状态、关闭走 `hide_floating_window`。

use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const LABEL: &str = "floating";
/// 窗口按胶囊最大尺寸（约 400×54，单行）再留出阴影边；胶囊在窗口内贴底居中、按内容横向生长
const WIDTH: f64 = 440.0;
const HEIGHT: f64 = 84.0;
const BOTTOM_OFFSET: f64 = 24.0;

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
        // 不抢焦点的窗口上，✕ 第一下点击就要生效
        .accept_first_mouse(true)
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
    let idle = json!({ "phase": "idle", "text": "", "duration": 0 });
    if let Ok(mut last) = LAST_STATE.lock() {
        *last = Some(idle.clone());
    }
    // 页面也回到待命（胶囊透明）：否则下次显示时会先闪一帧上次的胶囊，再收到新状态
    let _ = app.emit_to(LABEL, "floating:state", idle);
}

pub fn last_state() -> Value {
    LAST_STATE
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_else(|| json!({ "phase": "idle", "text": "", "duration": 0 }))
}

/// 录音 Actor 状态 → 悬浮窗状态（悬浮窗页面的相位词表：recording/transcribing/optimizing/complete/error）。
///
/// complete 带 `ok`：有结果且优化、输出都没出错才算「成」，否则「敗」并把原因放进 text。
/// 没说话 / 误触（载荷带 `silent`）不算失败：返回 None，由调用方直接收起悬浮窗。
pub fn from_recording_state(payload: &Value) -> Option<(Value, bool)> {
    let mode = payload["mode"].as_str().unwrap_or("single");
    let message = payload["message"].as_str().unwrap_or("");
    let state = |phase: &str, text: &str| json!({ "phase": phase, "mode": mode, "text": text, "duration": 0 });
    match payload["phase"].as_str()? {
        "connecting" | "recording" => Some((state("recording", ""), true)),
        "finalizing" => Some((state("transcribing", ""), true)),
        "optimizing" => Some((state("optimizing", payload["text"].as_str().unwrap_or("")), true)),
        "completed" if payload["silent"].as_bool() == Some(true) => None,
        "completed" => {
            let text = payload["text"].as_str().unwrap_or("");
            let problem = ["optimize_error", "copy_error"].iter().find_map(|key| payload[*key].as_str().filter(|e| !e.is_empty()));
            let ok = !text.is_empty() && problem.is_none();
            let shown = if ok { text } else { problem.unwrap_or(message) };
            let mut complete = state("complete", shown);
            complete["ok"] = json!(ok);
            Some((complete, true))
        }
        "error" => Some((state("error", message), true)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::from_recording_state;
    use serde_json::json;

    fn complete(payload: serde_json::Value) -> serde_json::Value {
        from_recording_state(&payload).expect("completed 应映射").0
    }

    #[test]
    fn 完成态_有结果且无错为成_否则敗并带原因() {
        let ok = complete(json!({ "phase": "completed", "mode": "streaming", "text": "优化文", "message": "录音已完成，已优化" }));
        assert_eq!(ok["phase"], "complete");
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["text"], "优化文");

        let empty = complete(json!({ "phase": "completed", "text": "", "message": "没有识别到语音内容" }));
        assert_eq!(empty["ok"], false);
        assert_eq!(empty["text"], "没有识别到语音内容");

        let optimize = complete(json!({ "phase": "completed", "text": "原文", "optimize_error": "鉴权失败", "message": "优化失败" }));
        assert_eq!(optimize["ok"], false);
        assert_eq!(optimize["text"], "鉴权失败");

        let output = complete(json!({ "phase": "completed", "text": "原文", "copy_error": "自动输入需要「辅助功能」权限", "optimize_error": null }));
        assert_eq!(output["ok"], false);
        assert_eq!(output["text"], "自动输入需要「辅助功能」权限");
    }

    #[test]
    fn 取消与没说话都不映射_由调用方隐藏悬浮窗() {
        assert!(from_recording_state(&json!({ "phase": "cancelled" })).is_none());
        assert!(from_recording_state(&json!({ "phase": "completed", "text": "", "message": "没有检测到说话", "silent": true })).is_none());
        assert!(from_recording_state(&json!({ "phase": "completed", "text": "", "message": "录音时间过短", "silent": true })).is_none());
    }
}
