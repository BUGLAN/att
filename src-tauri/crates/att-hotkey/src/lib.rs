//! 全局热键。
//!
//! 当前可用实现：[`listener`]——底层键盘监听（macOS CGEventTap / Windows WH_KEYBOARD_LL）
//! + [`matcher`] 触发状态机（支持纯修饰键组合，如旧版默认的 Win+Alt）。

pub mod listener;
#[cfg(target_os = "macos")]
mod macos;
pub mod matcher;
#[cfg(windows)]
mod windows;

pub use att_core::settings::hotkey::{HotkeyAction, HotkeySpec};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// 本应用自己模拟的键盘事件（自动输入、粘贴、Windows 的掩护键）带的标记：
/// macOS 写进事件的 `EVENT_SOURCE_USER_DATA`，Windows 写进 `KEYBDINPUT.dwExtraInfo`。
///
/// 热键监听见到它一律跳过：自动输入为了不和仍按着的快捷键组合成命令，会把修饰键清零（粘贴只带 ⌘ / Ctrl），
/// 这些修饰键状态若被当成用户的真实键态，按住录音时就会被判成「松开」，紧接着又被判成「按下」开出新的一段。
pub const SYNTHETIC_EVENT_TAG: i64 = 0x4154_5453;

#[derive(Default)]
struct SyntheticState {
    depth: usize,
    modifier_releases: BTreeSet<u32>,
}

static SYNTHETIC_STATE: OnceLock<Mutex<SyntheticState>> = OnceLock::new();

fn synthetic_state() -> &'static Mutex<SyntheticState> {
    SYNTHETIC_STATE.get_or_init(|| Mutex::new(SyntheticState::default()))
}

/// 开始一段由本进程模拟键盘输入的临界区。
///
/// Windows 钩子和防粘滞轮询在这段时间内不能采信 `GetAsyncKeyState`，否则模拟抬起修饰键
/// 会被误当成用户松开当前快捷键。允许多个调用嵌套。
pub fn begin_synthetic() {
    let mut state = synthetic_state().lock().unwrap_or_else(|error| error.into_inner());
    if state.depth == 0 {
        state.modifier_releases.clear();
    }
    state.depth += 1;
}

/// 结束模拟键盘输入的临界区；最外层结束时清掉记录。
pub fn end_synthetic() {
    let mut state = synthetic_state().lock().unwrap_or_else(|error| error.into_inner());
    if state.depth == 0 {
        return;
    }
    state.depth -= 1;
    if state.depth == 0 {
        state.modifier_releases.clear();
    }
}

#[cfg(windows)]
pub(crate) fn with_synthetic_window<R>(operation: impl FnOnce(bool, &mut BTreeSet<u32>) -> R) -> R {
    let mut state = synthetic_state().lock().unwrap_or_else(|error| error.into_inner());
    operation(state.depth > 0, &mut state.modifier_releases)
}

/// 这个修饰键是否在模拟窗口里被用户真实松开。恢复模拟按下时必须跳过。
pub fn synthetic_modifier_released(vk: u16) -> bool {
    synthetic_state().lock().unwrap_or_else(|error| error.into_inner()).modifier_releases.contains(&(vk as u32))
}

/// USB HID Usage ID：Escape（键盘页 0x07）。平台钩子把各自的键码映射到它。
pub const HID_ESCAPE: u16 = 0x29;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Pressed,
    Released,
}
