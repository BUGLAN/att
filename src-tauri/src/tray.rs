//! 系统托盘：四态图标（待命 = 品牌标「墨」，录音 / 转写 / 断开 = 状态章「聽 轉 斷」）+
//! 开始 / 停止录音、打开主窗口 + 最近 10 条记录菜单（点击 = 复制并粘贴）+「优化预设」子菜单
//! （全部预设，当前默认打勾，点选即切换）+ 设置 + 退出。
//!
//! 菜单直接读 Rust 记录库（不依赖主窗口前端是否加载），录音/优化落库后由 Actor 调 [`refresh`]；
//! 前端的 `update_tray_menu` 也只触发一次重建。

use att_core::state::CoreState;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "main";
const RECENT_LIMIT: usize = 10;
const LABEL_MAX_CHARS: usize = 40;

/// 当前图标状态（避免重复设置同一张图）。
static CURRENT: Mutex<&str> = Mutex::new("");

/// Windows 托盘位图档位：系统托盘小图标边长 = 16 × 显示缩放（100%~300%），
/// 每档都由 scripts/generate_brand_icons.py 按像素单独出图，避免系统把 32px 缩成 16 / 20px 发糊。
#[cfg(windows)]
const WIN_SIZES: [u32; 6] = [16, 20, 24, 32, 40, 48];

/// 按主屏缩放挑「不小于 16×缩放」的最小档（只缩小不放大；读不到缩放给最大档）。
#[cfg(windows)]
fn win_tray_px(app: &AppHandle) -> u32 {
    let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(3.0);
    let want = (16.0 * scale).round() as u32;
    WIN_SIZES.iter().copied().find(|&px| px >= want).unwrap_or(48)
}

fn icon_bytes(app: &AppHandle, state: &str) -> &'static [u8] {
    #[cfg(windows)]
    {
        let px = win_tray_px(app);
        macro_rules! sized {
            ($state:literal) => {
                match px {
                    16 => &include_bytes!(concat!("../icons/tray/win-", $state, "-16.png"))[..],
                    20 => &include_bytes!(concat!("../icons/tray/win-", $state, "-20.png"))[..],
                    24 => &include_bytes!(concat!("../icons/tray/win-", $state, "-24.png"))[..],
                    32 => &include_bytes!(concat!("../icons/tray/win-", $state, "-32.png"))[..],
                    40 => &include_bytes!(concat!("../icons/tray/win-", $state, "-40.png"))[..],
                    _ => &include_bytes!(concat!("../icons/tray/win-", $state, "-48.png"))[..],
                }
            };
        }
        match state {
            "recording" => sized!("recording"),
            "transcribing" => sized!("transcribing"),
            "disconnected" => sized!("disconnected"),
            _ => sized!("idle"),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        match state {
            "recording" => include_bytes!("../icons/tray/macos-recording.png"),
            "transcribing" => include_bytes!("../icons/tray/macos-transcribing.png"),
            "disconnected" => include_bytes!("../icons/tray/macos-disconnected.png"),
            _ => include_bytes!("../icons/tray/macos-idle.png"),
        }
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
        "recording" => "落墨 - 录音中...",
        "transcribing" => "落墨 - 转写中...",
        "disconnected" => "落墨 - 未连接",
        _ => "落墨 - 就绪",
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let image = tauri::image::Image::from_bytes(icon_bytes(app, "idle"))?;
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
    if let Ok(image) = tauri::image::Image::from_bytes(icon_bytes(app, state)) {
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
    let (record_label, record_enabled) = recording_menu(&recording_phase(app));
    menu.append(&MenuItem::with_id(app, "record-toggle", record_label, record_enabled, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "open-main", "打开主窗口", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
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
    if let Some(submenu) = preset_menu(app)? {
        menu.append(&submenu)?;
    }
    menu.append(&MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?)?;
    Ok(menu)
}

/// 「优化预设」子菜单：列表顺序与设置页一致，当前生效的默认预设打勾；没有预设时不显示。
fn preset_menu(app: &AppHandle) -> tauri::Result<Option<Submenu<tauri::Wry>>> {
    let Some(core) = app.try_state::<crate::AppState>().map(|state| state.core.clone()) else { return Ok(None) };
    let presets = core.prompt_library().load_all();
    if presets.is_empty() {
        return Ok(None);
    }
    let current = att_core::prompts::effective_default_id(&presets, core.settings().default_preset_id.as_deref());
    let submenu = Submenu::with_id(app, "presets", "优化预设", true)?;
    for preset in &presets {
        let checked = current.as_deref() == Some(preset.id.as_str());
        let label = truncate(&preset.name, LABEL_MAX_CHARS);
        submenu.append(&CheckMenuItem::with_id(
            app,
            preset_item_id(&preset.id),
            label,
            true,
            checked,
            None::<&str>,
        )?)?;
    }
    Ok(Some(submenu))
}

const PRESET_ITEM_PREFIX: &str = "preset:";

fn preset_item_id(id: &str) -> String {
    format!("{PRESET_ITEM_PREFIX}{id}")
}

/// 托盘点选预设：设为默认优化预设并广播（勾选、悬浮窗、设置页同步）；预设已被删时只重建菜单。
fn select_preset(app: &AppHandle, id: &str) {
    let Some(core) = app.try_state::<crate::AppState>().map(|state| state.core.clone()) else { return };
    match core.select_default_preset(id) {
        Ok(Some(preset)) => crate::announce_default_preset(app, &preset),
        Ok(None) => refresh(app),
        Err(error) => tracing::warn!("托盘切换优化预设失败: {error}"),
    }
}

/// 当前录音相位（Actor 维护的最新快照；没有状态时按空闲处理）。
fn recording_phase(app: &AppHandle) -> String {
    app.try_state::<crate::AppState>()
        .map(|state| state.recorder.state())
        .and_then(|state| state.get("phase").and_then(|phase| phase.as_str()).map(str::to_string))
        .unwrap_or_else(|| "idle".into())
}

/// 托盘录音项：录音 / 建连中显示「停止录音」，其余显示「开始录音」；收尾中不能开始新一段。
fn recording_menu(phase: &str) -> (&'static str, bool) {
    match phase {
        "connecting" | "recording" => ("停止录音", true),
        "finalizing" => ("开始录音", false),
        _ => ("开始录音", true),
    }
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
        "record-toggle" => {
            let Some(state) = app.try_state::<crate::AppState>() else { return };
            match recording_phase(app).as_str() {
                "connecting" | "recording" => state.recorder.stop(),
                // 收尾中的菜单项是灰的；这里再兜一层，避免点到后吞掉下一次录音
                "finalizing" => {}
                _ => state.recorder.start(),
            }
        }
        "open-main" => crate::show_main_window(app),
        "settings" => {
            crate::show_main_window(app);
            let _ = app.emit_to("main", "navigate:settings", ());
        }
        _ if id.starts_with(PRESET_ITEM_PREFIX) => {
            let preset_id = id[PRESET_ITEM_PREFIX.len()..].to_string();
            // 菜单回调在主线程：写设置、重建菜单放到后台线程
            let app = app.clone();
            std::thread::spawn(move || select_preset(&app, &preset_id));
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

    #[test]
    fn 预设菜单项_id_带前缀可还原() {
        let id = preset_item_id("custom-1234");
        assert_eq!(id, "preset:custom-1234");
        assert_eq!(&id[PRESET_ITEM_PREFIX.len()..], "custom-1234");
        // 内置预设 id 本身以 preset- 开头，不会和记录菜单的 record: 混淆
        assert!(preset_item_id("preset-通用优化").starts_with(PRESET_ITEM_PREFIX));
    }

    #[test]
    fn 录音菜单项_随相位切换() {
        assert_eq!(recording_menu("connecting"), ("停止录音", true));
        assert_eq!(recording_menu("recording"), ("停止录音", true));
        assert_eq!(recording_menu("finalizing"), ("开始录音", false), "收尾中不能开始新一段");
        assert_eq!(recording_menu("optimizing"), ("开始录音", true));
        assert_eq!(recording_menu("idle"), ("开始录音", true));
    }
}
