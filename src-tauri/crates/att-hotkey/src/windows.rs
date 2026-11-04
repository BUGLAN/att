//! Windows 底层键盘监听：WH_KEYBOARD_LL（无需特权）。
//!
//! 命中快捷键的主键事件在钩子里吞掉（返回非零），不再送给前台应用。

use crate::listener::{Shared, Source};
use crate::SYNTHETIC_EVENT_TAG;
use att_core::settings::hotkey::{hid_from_char, ModSet};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// 未分配的虚拟键码：吞掉 Alt / Win 组合的主键后补发一次，免得单独的 Alt / Win 松开时激活菜单栏或开始菜单
const VK_MASK_KEY: u16 = 0xE8;

fn is_modifier(vk: u32) -> bool {
    matches!(
        vk,
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU
            | VK_LWIN | VK_RWIN
    )
}

fn is_down(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// 系统真实修饰键状态（防粘滞轮询用）。
pub fn physical_modifiers() -> Option<ModSet> {
    Some(modifiers_with(None))
}

/// 修饰键状态：以系统真实键态为准，`current` 为钩子正在处理的那个键（及其新状态）。
///
/// 不能自己记账（旧实现用静态数组随事件翻转）：Win+L 锁屏、UAC 等安全桌面会吞掉
/// 松开事件，记账值从此卡在「按下」——默认 Win+Alt 绑定下单按 Alt 就会误触发录音。
/// 低级钩子回调时系统键态尚未包含当前这次按键，因此当前键用事件自身的状态覆盖。
fn modifiers_with(current: Option<(u32, bool)>) -> ModSet {
    let current = current.map(|(vk, down)| (canonical_modifier(vk), down));
    let down = |vk: u32| match current {
        Some((current_vk, current_down)) if current_vk == vk => current_down,
        _ => is_down(vk),
    };
    ModSet::new(
        down(VK_LCONTROL) || down(VK_RCONTROL),
        down(VK_LMENU) || down(VK_RMENU),
        down(VK_LSHIFT) || down(VK_RSHIFT),
        down(VK_LWIN) || down(VK_RWIN),
    )
}

/// 通用修饰键码归到左侧键（低级钩子通常已给出左右区分的键码）。
fn canonical_modifier(vk: u32) -> u32 {
    match vk {
        VK_CONTROL => VK_LCONTROL,
        VK_MENU => VK_LMENU,
        VK_SHIFT => VK_LSHIFT,
        other => other,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(shared) = SHARED.get() {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);
            let message = wparam as u32;
            // 自己模拟的按键（掩护键、自动输入、粘贴的 Ctrl+V）不参与匹配：
            // 否则掩护键会被当成「又按了一个键」，粘贴的 Ctrl 会让按住的组合被判成松开
            if info.vkCode == VK_MASK_KEY as u32 || info.dwExtraInfo == SYNTHETIC_EVENT_TAG as usize {
                return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
            }
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            if down || up {
                let vk = info.vkCode;
                if is_modifier(vk) {
                    shared.on_modifiers(modifiers_with(Some((vk, down))), Source::Modifiers);
                } else {
                    // 非修饰键事件也顺带校正一次修饰键（与 macOS 每个事件带完整 flags 对齐）
                    shared.on_modifiers(modifiers_with(None), Source::Key);
                    let hid = char::from_u32(vk)
                        .filter(|c| c.is_ascii_alphanumeric())
                        .and_then(hid_from_char);
                    if shared.on_key(vk, hid, down) {
                        if down {
                            let mods = modifiers_with(None);
                            if mods.alt || mods.meta {
                                send_mask_key();
                            }
                        }
                        return 1;
                    }
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

fn send_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VK_MASK_KEY, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: SYNTHETIC_EVENT_TAG as usize },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

/// 在专用线程上安装低级键盘钩子并泵消息。
pub fn spawn(shared: Arc<Shared>) -> Result<(), String> {
    if SHARED.set(shared).is_err() {
        return Err("键盘监听已启动".into());
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("att-hotkey-windows".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0);
            if hook.is_null() {
                let _ = ready_tx.send(Err("安装键盘钩子失败".into()));
                return;
            }
            crate::listener::set_suppresses_keys(true);
            let _ = ready_tx.send(Ok(()));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .map_err(|e| format!("无法启动热键线程: {e}"))?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or_else(|_| Err("键盘监听启动超时".into()))
}
