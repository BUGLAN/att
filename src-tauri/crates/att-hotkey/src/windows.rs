//! Windows 底层键盘监听：WH_KEYBOARD_LL（只监听不吞键，无需特权）。

use crate::listener::Shared;
use att_core::settings::hotkey::{hid_from_char, ModSet};
use std::sync::{Arc, Mutex, OnceLock};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static MODS: Mutex<[bool; 4]> = Mutex::new([false; 4]);

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

/// 修饰键槽位：0=ctrl 1=alt 2=shift 3=meta。
fn modifier_slot(vk: u32) -> Option<usize> {
    match vk {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Some(0),
        VK_MENU | VK_LMENU | VK_RMENU => Some(1),
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Some(2),
        VK_LWIN | VK_RWIN => Some(3),
        _ => None,
    }
}

fn is_down(vk: u32) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// 系统真实修饰键状态（防粘滞轮询用）。
pub fn physical_modifiers() -> Option<ModSet> {
    Some(ModSet::new(
        is_down(VK_LCONTROL) || is_down(VK_RCONTROL),
        is_down(VK_LMENU) || is_down(VK_RMENU),
        is_down(VK_LSHIFT) || is_down(VK_RSHIFT),
        is_down(VK_LWIN) || is_down(VK_RWIN),
    ))
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(shared) = SHARED.get() {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);
            let message = wparam as u32;
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            if down || up {
                let vk = info.vkCode;
                if let Some(slot) = modifier_slot(vk) {
                    let mods = {
                        let mut state = MODS.lock().unwrap_or_else(|e| e.into_inner());
                        state[slot] = down;
                        ModSet::new(state[0], state[1], state[2], state[3])
                    };
                    shared.on_modifiers(mods);
                } else {
                    let hid = char::from_u32(vk)
                        .filter(|c| c.is_ascii_alphanumeric())
                        .and_then(hid_from_char);
                    shared.on_key(vk, hid, down);
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
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
