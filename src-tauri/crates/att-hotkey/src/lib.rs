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

pub use att_core::settings::hotkey::HotkeyAction;
use att_core::settings::hotkey::{HotkeyBinding, ModSet};

/// 本应用自己模拟的键盘事件（自动输入、粘贴、Windows 的掩护键）带的标记：
/// macOS 写进事件的 `EVENT_SOURCE_USER_DATA`，Windows 写进 `KEYBDINPUT.dwExtraInfo`。
///
/// 热键监听见到它一律跳过：自动输入为了不和仍按着的快捷键组合成命令，会把修饰键清零（粘贴只带 ⌘ / Ctrl），
/// 这些修饰键状态若被当成用户的真实键态，按住录音时就会被判成「松开」，紧接着又被判成「按下」开出新的一段。
pub const SYNTHETIC_EVENT_TAG: i64 = 0x4154_5453;

/// USB HID Usage ID：Escape（键盘页 0x07）。平台钩子把各自的键码映射到它。
pub const HID_ESCAPE: u16 = 0x29;

/// 监听器匹配的组合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub action: HotkeyAction,
    pub modifiers: ModSet,
    pub hid_key: Option<u16>,
}

impl HotkeySpec {
    pub fn from_binding(binding: &HotkeyBinding) -> Self {
        Self { action: binding.action, modifiers: binding.modifiers, hid_key: binding.hid_key }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Pressed,
    Released,
}
