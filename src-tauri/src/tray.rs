//! 系统托盘（移植自 electron/tray）：四态图标 + 最近 10 条记录菜单（点击 = 复制并粘贴）+ 设置 + 退出。
//!
//! 菜单直接读 Rust 记录库（不依赖主窗口前端是否加载），录音/优化落库后由 Actor 调 [`refresh`]；
//! 前端的 `update_tray_menu` 也只触发一次重建。

use att_core::state::CoreState;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "main";
const RECENT_LIMIT: usize = 10;
const LABEL_MAX_CHARS: usize = 40;

/// 当前图标状态（避免重复设置同一张图）。
static CURRENT: Mutex<&str> = Mutex::new("");

fn icon_bytes(state: &str) -> &'static [u8] {
    #[cfg(windows)]
    match state {
        "recording" => include_bytes!("../icons/tray/win-recording.png"),
        "transcribing" => include_bytes!("../icons/tray/win-transcribing.png"),
        "disconnected" => include_bytes!("../icons/tray/win-disconnected.png"),
        _ => include_bytes!("../icons/tray/win-idle.png"),
    }
    #[cfg(not(windows))]
    match state {
        "recording" => include_bytes!("../icons/tray/macos-recording.png"),
        "transcribing" => include_bytes!("../icons/tray/macos-transcribing.png"),
        "disconnected" => include_bytes!("../icons/tray/macos-disconnected.png"),
        _ => include_bytes!("../icons/tray/macos-idle.png"),
    }
}

fn normalize(state: &str) -> &'static str {
    match state {
        "recording" => "recording",
        "transcribing" => "transcribing",
        "disconnected" => "disconnected",
        _ => "idle",
    }
}

fn tooltip(state: &str) -> &'static str {
    match state {
        "recording" => "Audio to Text - 录音中...",
        "transcribing" => "Audio to Text - 转写中...",
        "disconnected" => "Audio to Text - 未连接",
        _ => "Audio to Text - 就绪",
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let image = tauri::image::Image::from_bytes(icon_bytes("idle"))?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(image)
        .tooltip(tooltip("idle"))
        .menu(&menu)
        // macOS 菜单栏习惯左键弹菜单；Windows 左键唤起主窗口、右键弹菜单（与旧版一致）
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                if !cfg!(target_os = "macos") {
                    crate::show_main_window(tray.app_handle());
                }
            }
        })
        .build(app)?;
    if let Ok(mut current) = CURRENT.lock() {
        *current = "idle";
    }
    Ok(())
}

/// 切换图标状态：idle / recording / transcribing / disconnected（未知按 idle）。
pub fn set_icon(app: &AppHandle, state: &str) {
    let state = normalize(state);
    if let Ok(mut current) = CURRENT.lock() {
        if *current == state {
            return;
        }
        *current = state;
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    if let Ok(image) = tauri::image::Image::from_bytes(icon_bytes(state)) {
        let _ = tray.set_icon(Some(image));
    }
    let _ = tray.set_tooltip(Some(tooltip(state)));
}

/// 按记录库重建菜单（最近 10 条在前）。
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(error) => tracing::warn!("重建托盘菜单失败: {error}"),
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    let records = app
        .try_state::<crate::AppState>()
        .map(|state| state.core.records.list(None, RECENT_LIMIT, 0))
        .unwrap_or_default();
    let mut has_records = false;
    for record in &records {
        let Some(text) = record_text(record) else { continue };
        let label = format!("{} - {}", format_time(&record.created_at), truncate(text, LABEL_MAX_CHARS));
        menu.append(&MenuItem::with_id(app, format!("record:{}", record.id), label, true, None::<&str>)?)?;
        has_records = true;
    }
    if has_records {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?)?;
    Ok(menu)
}

fn record_text(record: &att_core::records::Record) -> Option<&str> {
    record
        .optimized_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .or(record.raw_text.as_deref())
        .filter(|t| !t.trim().is_empty())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "quit" => app.exit(0),
        "settings" => {
            crate::show_main_window(app);
            let _ = app.emit_to("main", "navigate:settings", ());
        }
        _ => {
            let Some(record_id) = id.strip_prefix("record:") else { return };
            let Some(core) = app.try_state::<crate::AppState>().map(|s| s.core.clone()) else { return };
            copy_and_paste_record(app.clone(), core, record_id.to_string());
        }
    }
}

/// 托盘「复制并粘贴」：写剪贴板（标记内部写入，不进剪贴板历史）→ 模拟粘贴到前台应用。
fn copy_and_paste_record(app: AppHandle, core: std::sync::Arc<CoreState>, record_id: String) {
    std::thread::spawn(move || {
        let Ok(record) = core.records.get(&record_id) else { return };
        let Some(text) = record_text(&record).map(str::to_string) else { return };
        match crate::desktop::write_clipboard(&text).and_then(|_| crate::desktop::paste_active()) {
            // 与旧版 paste-text 事件一致：主窗口提示「已复制到剪贴板」
            Ok(()) => {
                let _ = app.emit_to("main", "paste-text", ());
            }
            Err(error) => tracing::warn!("托盘复制并粘贴失败: {error}"),
        }
    });
}

/// 今天显示「时:分」，否则「M月D日」（与旧版 toLocale*('zh-CN') 输出一致）。
fn format_time(created_at: &str) -> String {
    use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Timelike};
    // Python 时代的记录是不带时区的本地时间（2026-03-25T11:45:48.455855）
    let parsed = DateTime::parse_from_rfc3339(created_at).map(|t| t.with_timezone(&Local)).ok().or_else(|| {
        NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .and_then(|naive| Local.from_local_datetime(&naive).single())
    });
    let Some(time) = parsed else {
        return String::new();
    };
    if time.date_naive() == Local::now().date_naive() {
        format!("{:02}:{:02}", time.hour(), time.minute())
    } else {
        format!("{}月{}日", time.month(), time.day())
    }
}

fn truncate(text: &str, max: usize) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max {
        return single_line;
    }
    format!("{}...", single_line.chars().take(max).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 菜单标签_截断与单行化() {
        assert_eq!(truncate("你好\n世界", 40), "你好 世界");
        assert_eq!(truncate(&"字".repeat(45), 40), format!("{}...", "字".repeat(40)));
        assert_eq!(normalize("weird"), "idle");
        assert_eq!(format_time("not-a-time"), "");
    }
}
