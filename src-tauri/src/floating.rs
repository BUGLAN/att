//! 录音悬浮窗（屏幕底部居中的极简胶囊：一枚与托盘同源的印章 + 至多一行文字）。
//!
//! 由录音 Actor 直接驱动（不经过主窗口前端：主窗口在后台/隐藏时同样可见）。
//! 页面 floating.html 里的 `window.floatingAPI` 由 floating.js 在 Tauri 下垫片实现：
//! 订阅 `floating:state`、挂载时 `get_floating_state` 拉取最近状态、关闭走 `hide_floating_window`。
//!
//! 点击穿透：窗口比胶囊大一圈（给阴影和生长留位置），透明的部分不能挡住下面应用的点击——
//! 用户多半正对着屏幕底部的输入框说话。页面上报胶囊所在区域（`floating_hit_rect`），
//! 窗口显示期间按光标是否落在胶囊上切换 `set_ignore_cursor_events`。

use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

pub const LABEL: &str = "floating";
/// 窗口按胶囊最大尺寸（约 400×54，单行）再留出阴影边；胶囊在窗口内贴底居中、按内容横向生长
const WIDTH: f64 = 440.0;
const HEIGHT: f64 = 84.0;
const BOTTOM_OFFSET: f64 = 24.0;
/// 回到待命后等胶囊淡完（floating.css .capsule.leaving 的 opacity 180ms / transform 200ms）再隐藏窗口
const HIDE_AFTER_FADE: Duration = Duration::from_millis(240);
/// 点击穿透的光标检查间隔
const HIT_POLL: Duration = Duration::from_millis(40);
/// 命中区域向外放宽的余量（CSS px）：贴着胶囊边缘也能点到 ✕
const HIT_MARGIN: f64 = 4.0;

static LAST_STATE: Mutex<Option<Value>> = Mutex::new(None);
/// 胶囊在窗口里的区域 [x, y, 宽, 高]（CSS px，相对窗口左上角）；None = 胶囊不可见，整窗穿透
static HIT_RECT: Mutex<Option<[f64; 4]>> = Mutex::new(None);

/// 窗口显示时唤醒点击穿透循环（Notify 会留住一次唤醒，不怕循环还没开始等）
fn shown_signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

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
    tauri::async_runtime::spawn(click_through_loop(app.clone()));
    Ok(())
}

/// 页面上报胶囊区域（尺寸变化、进入 / 退出淡出时）。
pub fn set_hit_rect(rect: Option<[f64; 4]>) {
    if let Ok(mut slot) = HIT_RECT.lock() {
        *slot = rect;
    }
}

fn hits(rect: [f64; 4], x: f64, y: f64) -> bool {
    let [left, top, width, height] = rect;
    x >= left - HIT_MARGIN && x <= left + width + HIT_MARGIN && y >= top - HIT_MARGIN && y <= top + height + HIT_MARGIN
}

/// 光标相对窗口左上角的 CSS px 坐标。
fn cursor_in_window(window: &tauri::WebviewWindow, primary_scale: f64) -> Option<(f64, f64)> {
    let cursor = window.cursor_position().ok()?;
    let origin = window.inner_position().ok()?;
    let scale = window.scale_factor().ok()?;
    // macOS：tao 的光标按主显示器缩放、窗口位置按窗口所在屏缩放各自放大成物理像素，先各自除回点（= CSS px）
    #[cfg(target_os = "macos")]
    let point = (cursor.x / primary_scale - origin.x as f64 / scale, cursor.y / primary_scale - origin.y as f64 / scale);
    // Windows：两者都是全局物理像素，相减后按窗口缩放换成 CSS px
    #[cfg(not(target_os = "macos"))]
    let point = {
        let _ = primary_scale;
        ((cursor.x - origin.x as f64) / scale, (cursor.y - origin.y as f64) / scale)
    };
    Some(point)
}

/// 窗口显示期间轮询光标：落在胶囊上才接收鼠标，其余透明部分让点击穿透到下面的应用。
async fn click_through_loop(app: AppHandle) {
    loop {
        shown_signal().notified().await;
        let Some(window) = app.get_webview_window(LABEL) else { continue };
        let primary_scale = window.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
        let mut ignoring = None;
        while window.is_visible().unwrap_or(false) {
            let rect = HIT_RECT.lock().ok().and_then(|r| *r);
            let inside = match (rect, cursor_in_window(&window, primary_scale)) {
                (Some(rect), Some((x, y))) => hits(rect, x, y),
                // 量不到光标：宁可接收点击，别让 ✕ 点不到
                (Some(_), None) => true,
                (None, _) => false,
            };
            if ignoring != Some(!inside) {
                let _ = window.set_ignore_cursor_events(!inside);
                if !inside && ignoring.is_some() {
                    // 穿透后页面收不到 mouseleave：补一个，免得悬停态卡住（结果态的收起会一直暂停）
                    let _ = app.emit_to(LABEL, "floating:pointer-out", ());
                }
                ignoring = Some(!inside);
            }
            tokio::time::sleep(HIT_POLL).await;
        }
    }
}

/// 放到鼠标所在显示器的底部居中（多屏时跟着用户正在看的那块屏），取不到再退回主显示器。
fn position_bottom_center(window: &tauri::WebviewWindow) {
    let Some(monitor) = monitor_under_cursor(window).or_else(|| window.primary_monitor().ok().flatten()) else { return };
    // 先在该显示器的物理像素里算好（窗口的物理尺寸按它落到的那块屏的缩放算）
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let x = area.position.x as f64 + (area.size.width as f64 - WIDTH * scale) / 2.0;
    let y = area.position.y as f64 + area.size.height as f64 - (HEIGHT + BOTTOM_OFFSET) * scale;
    // macOS 的全局坐标是点：各显示器的物理坐标是按各自缩放放大的，除回该屏缩放才是全局坐标；
    // Windows 的物理坐标本身就是全局虚拟桌面坐标（逻辑坐标会按窗口当前所在屏的缩放换算，跨屏混合缩放时算偏）
    #[cfg(target_os = "macos")]
    let _ = window.set_position(tauri::LogicalPosition::new(x / scale, y / scale));
    #[cfg(not(target_os = "macos"))]
    let _ = window.set_position(tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

fn monitor_under_cursor(window: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let cursor = window.cursor_position().ok()?;
    // macOS：tao 把光标坐标按主显示器缩放放大成了物理像素，查显示器却按点比（CGDisplayBounds），先除回去
    #[cfg(target_os = "macos")]
    let cursor = {
        let scale = window.primary_monitor().ok()??.scale_factor();
        tauri::PhysicalPosition::new(cursor.x / scale, cursor.y / scale)
    };
    window.monitor_from_point(cursor.x, cursor.y).ok().flatten()
}

fn idle_state() -> Value {
    json!({ "phase": "idle", "text": "", "duration": 0 })
}

fn phase_of(state: Option<&Value>) -> &str {
    state.and_then(|s| s["phase"].as_str()).unwrap_or("idle")
}

/// 会话已结束（待命 / 成 / 敗）：只有这时才允许悬浮窗自己收起。
fn is_settled(state: Option<&Value>) -> bool {
    matches!(phase_of(state), "idle" | "complete" | "error")
}

/// 建连成功（connecting → recording）在悬浮窗里是同一个状态：沿用已经出来的字，
/// 否则 Actor 补发的这条不带文字的「录音中」会把识别中间结果清空，字重新墨晕一遍。
fn carry_live_text(mut next: Value, last: Option<&Value>) -> Value {
    if let Some(last) = last.filter(|last| phase_of(Some(last)) == "recording" && last["mode"] == next["mode"]) {
        if phase_of(Some(&next)) == "recording" && next["text"].as_str().unwrap_or("").is_empty() {
            next["text"] = last["text"].clone();
            next["pending"] = last["pending"].clone();
        }
    }
    next
}

/// 悬浮窗状态的唯一写入口：在锁内读最近状态、决定新状态并推送。
/// 持锁推送保证页面收到的顺序与这里写入的顺序一致（emit 只投递脚本，不等主线程，持锁无死锁风险；
/// 窗口显隐要等主线程，一律放到锁外）。`decide` 返回 None 表示放弃这次写入。
fn publish(app: &AppHandle, decide: impl FnOnce(Option<&Value>) -> Option<Value>) -> bool {
    let Ok(mut last) = LAST_STATE.lock() else { return false };
    let Some(next) = decide(last.as_ref()) else { return false };
    *last = Some(next.clone());
    let _ = app.emit_to(LABEL, "floating:state", next);
    true
}

fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    if !window.is_visible().unwrap_or(false) {
        position_bottom_center(&window);
        let _ = window.show();
        shown_signal().notify_one();
    }
}

/// 推送状态；`show=true` 时同时显示（不抢焦点，避免打断用户正在输入的窗口）。
pub fn update(app: &AppHandle, state: Value, show: bool) {
    if !show {
        publish(app, |last| Some(carry_live_text(state, last)));
        return;
    }
    // 先写状态、后判断可见性：与 [`dismiss`] 的复查配合，新会话不会被并发的收起藏掉。
    // 但要等窗口显示出来再推给页面：页面在隐藏期间收到新会话就会提前重建胶囊、播入场动效，
    // 窗口出现那一下会闪一帧；显示时页面仍是透明的待命态，胶囊才从透明淡入
    if let Ok(mut last) = LAST_STATE.lock() {
        let next = carry_live_text(state, last.as_ref());
        *last = Some(next);
    }
    show_window(app);
    // 推此刻最新的状态：期间若有更新的写入（中间结果 / 收起），以它为准，页面不会收到倒序的旧状态
    publish(app, |last| last.cloned());
}

/// 识别中间结果（流式 / 多段）：只在悬浮窗仍处于「聽」时生效。
/// 识别回调与录音 Actor 不在同一线程，松开后 Actor 已推「轉」，晚到的中间结果不能再把它推回「聽」。
pub fn update_partial(app: &AppHandle, state: Value) {
    publish(app, |last| (phase_of(last) == "recording").then_some(state));
}

/// 收起（取消 / 没说话 / 托盘切换）：页面先回到待命淡出胶囊，淡完再隐藏窗口。
///
/// 不能当场隐藏：系统会留着窗口隐藏前的最后一帧，下次显示时先露出这张旧胶囊（比如「聽」），
/// 页面随后换成新会话、胶囊从透明重新淡入，看起来就是闪一下。
/// 淡出期间若已开始新一次录音，[`dismiss`] 见会话未结束就不再隐藏。
pub fn hide(app: &AppHandle) {
    publish(app, |_| Some(idle_state()));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(HIDE_AFTER_FADE).await;
        // 与收起命令一样在主线程上跑：判断、隐藏、复查之间不会插进窗口的显示
        let main = app.clone();
        let _ = app.run_on_main_thread(move || dismiss(&main));
    });
}

/// 结果态停留到点后由页面发起的收起：只收已结束的会话。
/// 页面的计时器与新一次录音是并发的——收起请求晚到时，新会话的悬浮窗不能被一起藏掉。
pub fn dismiss(app: &AppHandle) {
    if !publish(app, |last| is_settled(last).then(idle_state)) {
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let _ = window.hide();
    // 隐藏途中新会话已经写入状态（它若在隐藏之前查过可见性就不会再 show）：恢复显示并补推最新状态。
    // 与 update 同序：先显示、再推给页面，免得页面在隐藏期间提前重建胶囊、显示时闪一帧
    if !is_settled(Some(&last_state())) {
        position_bottom_center(&window);
        let _ = window.show();
        shown_signal().notify_one();
        publish(app, |last| last.cloned());
    }
}

/// 这次按键没生效（收尾中 / 单段优化中按了录音键）：让印章摇一下，免得用户以为热键坏了。
/// 胶囊已收起（或正在淡出）时不必提示。
pub fn nudge(app: &AppHandle) {
    if !is_settled(Some(&last_state())) {
        let _ = app.emit_to(LABEL, "floating:nudge", ());
    }
}

pub fn last_state() -> Value {
    LAST_STATE.lock().ok().and_then(|s| s.clone()).unwrap_or_else(idle_state)
}

/// 录音 Actor 状态 → 悬浮窗状态（悬浮窗页面的相位词表：recording/transcribing/optimizing/complete/error）。
///
/// optimizing：载荷里的是转写原文，放进 `draft`，整个「轉」阶段照常显示；
/// 优化稿不回填悬浮窗（停留很短，一闪而过反而突兀），优化增量也不再推过来。
///
/// complete 带 `ok`：有结果且优化、输出都没出错才算「成」，否则「敗」并把原因放进 text。
/// 没说话 / 误触（载荷带 `silent`）不算失败，取消优化也不算完成：返回 None，由调用方直接收起悬浮窗。
pub fn from_recording_state(payload: &Value) -> Option<(Value, bool)> {
    let mode = payload["mode"].as_str().unwrap_or("single");
    let message = payload["message"].as_str().unwrap_or("");
    let state = |phase: &str, text: &str| json!({ "phase": phase, "mode": mode, "text": text, "duration": 0 });
    match payload["phase"].as_str()? {
        "connecting" | "recording" => Some((state("recording", ""), true)),
        "finalizing" => Some((state("transcribing", ""), true)),
        "optimizing" => {
            let mut optimizing = state("optimizing", "");
            optimizing["draft"] = json!(payload["text"].as_str().unwrap_or(""));
            Some((optimizing, true))
        }
        "completed" if payload["silent"].as_bool() == Some(true) => None,
        // 用户点 ✕ 取消了优化：和取消录音一样直接收起，不报「成」（这次并没有把结果交出去）
        "completed" if payload["optimization_cancelled"].as_bool() == Some(true) => None,
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
    use super::{carry_live_text, from_recording_state, hits, is_settled};
    use serde_json::json;

    #[test]
    fn 建连成功补发的录音中_沿用已出的字() {
        let last = json!({ "phase": "recording", "mode": "streaming", "text": "你好世界", "pending": 2 });
        let connected = json!({ "phase": "recording", "mode": "streaming", "text": "" });
        let next = carry_live_text(connected, Some(&last));
        assert_eq!(next["text"], "你好世界");
        assert_eq!(next["pending"], 2);

        // 新会话的起点（上一次是成 / 敗 / 待命）不沿用
        let done = json!({ "phase": "complete", "mode": "streaming", "text": "上一段" });
        let fresh = carry_live_text(json!({ "phase": "recording", "mode": "streaming", "text": "" }), Some(&done));
        assert_eq!(fresh["text"], "");

        // 带字的中间结果照常覆盖
        let partial = carry_live_text(json!({ "phase": "recording", "mode": "streaming", "text": "你好" }), Some(&last));
        assert_eq!(partial["text"], "你好");
    }

    #[test]
    fn 点击穿透_胶囊区域含边缘余量() {
        let rect = [193.0, 18.0, 54.0, 54.0];
        assert!(hits(rect, 220.0, 45.0));
        assert!(hits(rect, 190.0, 16.0), "贴边余量内");
        assert!(!hits(rect, 100.0, 45.0), "左侧透明区");
        assert!(!hits(rect, 220.0, 80.0), "胶囊下方阴影区");
    }

    #[test]
    fn 只有已结束的会话允许自行收起() {
        assert!(is_settled(None));
        for phase in ["idle", "complete", "error"] {
            assert!(is_settled(Some(&json!({ "phase": phase }))), "{phase}");
        }
        for phase in ["recording", "transcribing", "optimizing"] {
            assert!(!is_settled(Some(&json!({ "phase": phase }))), "{phase}");
        }
    }

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
    fn 进入优化_原文作为草稿_不当成优化结果() {
        let (state, show) = from_recording_state(&json!({ "phase": "optimizing", "mode": "streaming", "text": "原文" })).unwrap();
        assert!(show);
        assert_eq!(state["phase"], "optimizing");
        assert_eq!(state["text"], "");
        assert_eq!(state["draft"], "原文");
    }

    #[test]
    fn 取消与没说话都不映射_由调用方隐藏悬浮窗() {
        assert!(from_recording_state(&json!({ "phase": "cancelled" })).is_none());
        let cancelled_optimize = json!({ "phase": "completed", "text": "原文", "optimization_cancelled": true, "message": "已取消优化，保留原始文本" });
        assert!(from_recording_state(&cancelled_optimize).is_none());
        assert!(from_recording_state(&json!({ "phase": "completed", "text": "", "message": "没有检测到说话", "silent": true })).is_none());
        assert!(from_recording_state(&json!({ "phase": "completed", "text": "", "message": "录音时间过短", "silent": true })).is_none());
    }
}
