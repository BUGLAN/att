//! 剪贴板悬浮框：全局快捷键唤起、从屏幕底边升起的一条剪贴板卡片带（页面 clipboard-panel.html）。
//!
//! 与录音悬浮窗不同，它要接管键盘（搜索、↑↓ 选择、回车），所以显示时会抢焦点；
//! 为了让「回车粘贴」落到用户原来的输入框，唤起前记下前台应用，收起时把焦点还回去再模拟粘贴。
//! - 唤起：`toggle` / `show`（铺满光标所在屏幕的底边，推 `clipboard-panel:show` 让页面重置并刷新列表）
//! - 收起：Esc / 选中条目后由页面调用命令；点到别处（失焦）直接隐藏，不再抢回焦点
//! - 尺寸：页面上拖上边缘 / 左右边缘实时调整（水平居中），松手后记进设置
//! - 位置：按住头部空白处上下拖动改离底距离，双击回到贴底，同样记进设置

use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const LABEL: &str = "clipboard-panel";
/// 面板离屏幕左右与底边的最小距离（逻辑像素，不含阴影边）
const SCREEN_GAP: f64 = 8.0;
/// 阴影边：窗口四周透明留白（与 clipboard-panel.css 的 #app padding 一致）
const SHADOW_EDGE: f64 = 16.0;

/// 唤起前的前台应用（macOS 进程号 / Windows 窗口句柄）；收起时还焦点用
static PREVIOUS_APP: Mutex<Option<platform::AppRef>> = Mutex::new(None);

/// 预创建隐藏的悬浮框（首次唤起不等页面加载）。
pub fn create(app: &AppHandle, backend_port: u16) -> tauri::Result<()> {
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
    let init_script = format!("window.__ATT_BACKEND_PORT__ = {backend_port}; window.__ATT_IS_DESKTOP__ = true;");
    tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("clipboard-panel.html".into()))
        .title("ATT 剪贴板")
        .inner_size(960.0, 276.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .accept_first_mouse(true)
        .shadow(false)
        .visible(false)
        .initialization_script(&init_script)
        .build()?;
    Ok(())
}

pub fn is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL).is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// 快捷键入口：显示中就收起（焦点还给原应用），否则唤起。
pub fn toggle(app: &AppHandle) {
    if is_visible(app) {
        hide(app, true);
    } else {
        show(app);
    }
}

pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let front = platform::frontmost_app();
    if let Ok(mut previous) = PREVIOUS_APP.lock() {
        *previous = front;
    }
    let saved = saved_layout(app);
    if let Some(monitor) = cursor_monitor(app, &window) {
        let screen = Screen::of(&monitor);
        layout(&window, &screen, saved.width(), saved.height, saved.offset);
    }
    // 先让页面重置（清空搜索、回到第一条、刷新列表），再显示，避免闪一帧上次的状态
    let _ = app.emit_to(LABEL, "clipboard-panel:show", json!({}));
    let _ = window.show();
    let _ = window.set_focus();
}

/// 收起；`restore_focus` 时把焦点还给唤起前的应用（Esc / 选中条目），失焦隐藏时不还。
///
/// 必须先还焦点再隐藏：先隐藏的话，本应用在那一瞬间处于激活态却没有可见窗口，
/// 系统会先切到桌面再切回原应用，看起来是一次闪烁。
pub fn hide(app: &AppHandle, restore_focus: bool) {
    let previous = PREVIOUS_APP.lock().ok().and_then(|mut p| p.take());
    if restore_focus {
        if let Some(previous) = previous {
            platform::activate(previous);
        }
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}

/// 失焦（点到别处 / 切走）：直接隐藏。
pub fn on_blur(app: &AppHandle) {
    if is_visible(app) {
        hide(app, false);
    }
}

/// 页面需要的上下文：回车行为、唤起快捷键与记住的尺寸（每次唤起时读，设置页改了立即生效）。
pub fn context(core: &att_core::state::CoreState) -> Value {
    use att_core::settings::hotkey::HotkeyAction;
    let settings = core.settings();
    json!({
        "enter_action": settings.clipboard_panel_enter_action,
        "hotkey_display": settings
            .binding_of(HotkeyAction::ClipboardPanel)
            .map(|b| b.display(core.platform))
            .unwrap_or_default(),
        "height": settings.clipboard_panel_height,
        "max_width": settings.clipboard_panel_max_width,
        "bottom_offset": settings.clipboard_panel_bottom_offset,
    })
}

/// 拖动调整：按面板尺寸（逻辑像素，不含阴影边）重新布局，仍贴底居中；`save` 时记进设置。
/// 宽度拉到屏幕可用宽度即记为「铺满」（0），换到更宽的屏幕也跟着铺满。返回夹取后的实际尺寸。
pub fn resize(app: &AppHandle, width: f64, height: f64, save: bool) -> Value {
    let Some(window) = app.get_webview_window(LABEL) else { return json!(null) };
    let Some(monitor) = window.current_monitor().ok().flatten() else { return json!(null) };
    let screen = Screen::of(&monitor);
    // 改尺寸时保持离底距离不变：拖上边缘就是只往上长
    let (width, height, _) = layout(&window, &screen, width, height, saved_layout(app).offset);
    if save {
        let max_width = if width >= screen.available_width() - 1.0 { 0 } else { width.round() as i64 };
        save_settings(app, |s| s.set_clipboard_panel_size(max_width, height.round() as i64));
    }
    json!({ "width": width, "height": height })
}

/// 上下拖动：按离底距离重新布局（尺寸用记住的），`save` 时记进设置。返回夹取后的实际离底距离。
pub fn move_to(app: &AppHandle, bottom_offset: f64, save: bool) -> Value {
    let Some(window) = app.get_webview_window(LABEL) else { return json!(null) };
    let Some(monitor) = window.current_monitor().ok().flatten() else { return json!(null) };
    let saved = saved_layout(app);
    let (_, _, offset) = layout(&window, &Screen::of(&monitor), saved.width(), saved.height, bottom_offset);
    if save {
        save_settings(app, |s| s.set_clipboard_panel_offset(offset.round() as i64));
    }
    json!({ "bottom_offset": offset })
}

/// 恢复默认尺寸；悬浮框正显示着就当场重排。
pub fn reset_size(app: &AppHandle) {
    save_settings(app, |s| s.reset_clipboard_panel_size());
    relayout_if_visible(app);
}

/// 恢复默认位置（贴底）；悬浮框正显示着就当场重排。
pub fn reset_position(app: &AppHandle) {
    save_settings(app, |s| s.reset_clipboard_panel_position());
    relayout_if_visible(app);
}

fn save_settings(app: &AppHandle, change: impl FnOnce(&mut att_core::settings::UserSettings)) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        if let Err(error) = state.core.update_settings(change) {
            tracing::warn!("保存剪贴板悬浮框布局失败: {error}");
        }
    }
}

fn relayout_if_visible(app: &AppHandle) {
    if !is_visible(app) {
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else { return };
    if let Some(monitor) = window.current_monitor().ok().flatten() {
        let saved = saved_layout(app);
        layout(&window, &Screen::of(&monitor), saved.width(), saved.height, saved.offset);
    }
}

/// 设置里记住的布局（逻辑像素）。
struct SavedLayout {
    /// 0 = 铺满
    max_width: f64,
    height: f64,
    offset: f64,
}

impl SavedLayout {
    /// 布局用的宽度：铺满时交给 layout 按屏幕宽度夹取
    fn width(&self) -> f64 {
        if self.max_width > 0.0 { self.max_width } else { f64::INFINITY }
    }
}

fn saved_layout(app: &AppHandle) -> SavedLayout {
    app.try_state::<crate::AppState>()
        .map(|state| {
            let settings = state.core.settings();
            SavedLayout {
                max_width: settings.clipboard_panel_max_width as f64,
                height: settings.clipboard_panel_height as f64,
                offset: settings.clipboard_panel_bottom_offset as f64,
            }
        })
        .unwrap_or(SavedLayout { max_width: 1280.0, height: 244.0, offset: 0.0 })
}

/// 屏幕可用区域（逻辑像素）。
struct Screen {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Screen {
    fn of(monitor: &tauri::Monitor) -> Self {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        Self {
            left: area.position.x as f64 / scale,
            top: area.position.y as f64 / scale,
            width: area.size.width as f64 / scale,
            height: area.size.height as f64 / scale,
        }
    }

    fn available_width(&self) -> f64 {
        self.width - 2.0 * SCREEN_GAP
    }
}

/// 光标所在的屏幕（多屏时悬浮框跟着光标走）。
fn cursor_monitor(app: &AppHandle, window: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    app.cursor_position()
        .ok()
        .and_then(|cursor| app.monitor_from_point(cursor.x, cursor.y).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())
}

/// 面板水平居中、离屏幕底边 `bottom_offset`；宽高与离底距离都夹到允许范围与屏幕之内。
/// 返回实际的（面板宽，面板高，离底距离）。
fn layout(window: &tauri::WebviewWindow, screen: &Screen, width: f64, height: f64, bottom_offset: f64) -> (f64, f64, f64) {
    use att_core::settings::{CLIPBOARD_PANEL_HEIGHT_RANGE, CLIPBOARD_PANEL_WIDTH_RANGE};
    let available = screen.available_width();
    let panel_w = width.clamp(CLIPBOARD_PANEL_WIDTH_RANGE.0 as f64, CLIPBOARD_PANEL_WIDTH_RANGE.1 as f64).min(available);
    let panel_h = height
        .clamp(CLIPBOARD_PANEL_HEIGHT_RANGE.0 as f64, CLIPBOARD_PANEL_HEIGHT_RANGE.1 as f64)
        .min(screen.height - 2.0 * SCREEN_GAP);
    let _ = window.set_size(tauri::LogicalSize::new(panel_w + 2.0 * SHADOW_EDGE, panel_h + 2.0 * SHADOW_EDGE));
    // 离底距离最多到面板顶贴着屏幕上沿（换到矮屏时也不会跑出屏幕）
    let offset = bottom_offset.clamp(0.0, (screen.height - 2.0 * SCREEN_GAP - panel_h).max(0.0));
    let x = screen.left + (screen.width - panel_w) / 2.0 - SHADOW_EDGE;
    let y = screen.top + screen.height - SCREEN_GAP - offset - panel_h - SHADOW_EDGE;
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    (panel_w, panel_h, offset)
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    /// 进程号
    pub type AppRef = i32;

    /// 当前前台应用（是本应用自己时返回 None：收起后焦点自然回到本应用的窗口）。
    pub fn frontmost_app() -> Option<AppRef> {
        let class = AnyClass::get(c"NSWorkspace")?;
        let workspace: *mut AnyObject = unsafe { msg_send![class, sharedWorkspace] };
        if workspace.is_null() {
            return None;
        }
        let app: *mut AnyObject = unsafe { msg_send![workspace, frontmostApplication] };
        if app.is_null() {
            return None;
        }
        let pid: i32 = unsafe { msg_send![app, processIdentifier] };
        (pid > 0 && pid as u32 != std::process::id()).then_some(pid)
    }

    pub fn activate(pid: AppRef) {
        let Some(class) = AnyClass::get(c"NSRunningApplication") else { return };
        let app: *mut AnyObject = unsafe { msg_send![class, runningApplicationWithProcessIdentifier: pid] };
        if app.is_null() {
            return;
        }
        // 选项 0：只把它的主窗口 / 关键窗口提到前面（本应用正处于激活态，可以把激活让出去）
        let _: bool = unsafe { msg_send![app, activateWithOptions: 0usize] };
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

    /// 窗口句柄（按地址保存，句柄本身不是 Send）
    pub type AppRef = usize;

    pub fn frontmost_app() -> Option<AppRef> {
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then_some(hwnd as usize)
    }

    pub fn activate(hwnd: AppRef) {
        // 本进程此刻持有前台，按规则可以把前台让给别的窗口
        unsafe { SetForegroundWindow(hwnd as _) };
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub type AppRef = ();

    pub fn frontmost_app() -> Option<AppRef> {
        None
    }

    pub fn activate(_: AppRef) {}
}
