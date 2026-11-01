//! 主窗口圆角：与录音悬浮窗胶囊同一个半径（src/styles/tokens.css 的 --r-md，10px）。
//!
//! 主窗口无边框（自绘标题栏），macOS 上无边框窗口是直角，和全站 10px 的容器圆角对不上。
//! 做法：窗口背景透明、内容视图图层按半径裁切（网页内容随之裁掉四角），系统阴影跟着裁切后的形状走。
//! 全屏时取消圆角，贴满屏幕。
//!
//! Windows 11：无边框窗口请 DWM 画系统圆角（约 8px，抗锯齿、保留系统阴影，最大化 / 全屏时 DWM 自动取消）。
//! DWM 不能自定半径；要精确 10px 只能透明窗口 + 网页裁切，会丢系统阴影、缩放边缘也难处理，不划算。
//! Windows 10 不支持这个属性，调用失败即保持直角（与系统其他窗口一致）。

/// 与悬浮窗胶囊（floating.css .capsule 的 --r-md）一致
const WINDOW_RADIUS: f64 = 10.0;

/// 按当前是否全屏设置圆角；需在主线程调用（窗口创建处 / 窗口事件回调里）。
pub fn apply(window: &tauri::Window) {
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    platform::round(window, if fullscreen { 0.0 } else { WINDOW_RADIUS });
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    pub fn round(window: &tauri::Window, radius: f64) {
        let Ok(ns_window) = window.ns_window() else { return };
        let ns_window = ns_window as *mut AnyObject;
        let Some(color) = AnyClass::get(c"NSColor") else { return };
        unsafe {
            // 四角露出的部分要透明（窗口创建时的深色背景是为了防白闪，网页自身背景仍是深色）
            let clear: *mut AnyObject = msg_send![color, clearColor];
            let _: () = msg_send![ns_window, setOpaque: false];
            let _: () = msg_send![ns_window, setBackgroundColor: clear];
            let view: *mut AnyObject = msg_send![ns_window, contentView];
            if view.is_null() {
                return;
            }
            let _: () = msg_send![view, setWantsLayer: true];
            let layer: *mut AnyObject = msg_send![view, layer];
            if layer.is_null() {
                return;
            }
            let _: () = msg_send![layer, setCornerRadius: radius];
            let _: () = msg_send![layer, setMasksToBounds: true];
            // 阴影按新的形状重算
            let _: () = msg_send![ns_window, invalidateShadow];
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE,
    };

    pub fn round(window: &tauri::Window, radius: f64) {
        let Ok(hwnd) = window.hwnd() else { return };
        let preference: DWM_WINDOW_CORNER_PREFERENCE = if radius > 0.0 { DWMWCP_ROUND } else { DWMWCP_DEFAULT };
        unsafe {
            DwmSetWindowAttribute(
                hwnd.0 as _,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub fn round(_window: &tauri::Window, _radius: f64) {}
}
