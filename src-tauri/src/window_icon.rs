//! Windows 窗口图标：按窗口所在屏幕的 DPI，从 exe 图标资源里挑对应尺寸设成大 / 小图标。
//!
//! 为什么不用 Tauri 默认的窗口图标：
//! - Tauri 只在 `generate_context!()` 里用 `GetSystemMetrics(SM_CXICON)` 取一档图标，而那时事件循环还没建、
//!   进程还没声明 DPI 感知（tao 在建事件循环时才调 `SetProcessDpiAwarenessContext`），取到的永远是 96 DPI 的 32px；
//! - 而且只发 `WM_SETICON(ICON_SMALL)`，不设 `ICON_BIG`：任务栏 / Alt+Tab 只能拿这张 32px 去放大，
//!   125% / 150% / 200% 缩放下就是糊的（icon.ico 里明明有逐尺寸做过 hinting 的 40 / 48 / 64px）。
//!
//! 这里在窗口建好后（进程已是 Per-Monitor V2）按窗口 DPI 用 `LoadImageW` 从图标资源里挑最接近的档，
//! 同时设 `ICON_SMALL`（标题栏 / 任务栏小图标）与 `ICON_BIG`（任务栏 / Alt+Tab）；
//! 窗口跨屏换缩放时（`ScaleFactorChanged`）再调一次。

/// 按窗口当前 DPI 重设窗口图标；需在主线程调用（窗口创建处 / 窗口事件回调里）。
pub fn apply(window: &tauri::Window) {
    platform::apply(window);
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON,
        WM_SETICON,
    };

    /// tauri-build 把 bundle.icon 里的 icon.ico 以这个 ID 编进 exe 资源
    const ICON_RESOURCE_ID: u16 = tauri::utils::platform::WINDOWS_APP_ICON_RESOURCE_ID;

    /// 按像素尺寸缓存已加载的 HICON（存成 usize 以便放进 static）。
    /// 图标随进程存活、不销毁：尺寸只有屏幕缩放档那几种，且 WM_SETICON 替换下来的旧句柄可能是 tao 持有的，不能乱销毁。
    static CACHE: Mutex<Option<HashMap<i32, usize>>> = Mutex::new(None);

    fn load(px: i32) -> Option<usize> {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some(&icon) = cache.get(&px) {
            return Some(icon);
        }
        // 指定尺寸时 LoadImageW 会在图标组里挑最合适的一档（有同尺寸就直接用，不缩放）；
        // 不能加 LR_SHARED：共享句柄按资源缓存，换尺寸再取会拿回第一次的那张
        let icon = unsafe {
            LoadImageW(
                GetModuleHandleW(std::ptr::null()),
                ICON_RESOURCE_ID as usize as *const u16,
                IMAGE_ICON,
                px,
                px,
                LR_DEFAULTCOLOR,
            )
        };
        if icon.is_null() {
            tracing::warn!("从 exe 资源加载 {px}px 窗口图标失败");
            return None;
        }
        cache.insert(px, icon as usize);
        Some(icon as usize)
    }

    pub fn apply(window: &tauri::Window) {
        let Ok(hwnd) = window.hwnd() else { return };
        let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
        let dpi = match unsafe { GetDpiForWindow(hwnd) } {
            0 => 96,
            dpi => dpi,
        };
        for (kind, metric, fallback) in [(ICON_SMALL, SM_CXSMICON, 16), (ICON_BIG, SM_CXICON, 32)] {
            let px = match unsafe { GetSystemMetricsForDpi(metric, dpi) } {
                n if n > 0 => n,
                _ => fallback * dpi as i32 / 96,
            };
            if let Some(icon) = load(px) {
                unsafe { SendMessageW(hwnd, WM_SETICON, kind as usize, icon as isize) };
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn apply(_window: &tauri::Window) {}
}
