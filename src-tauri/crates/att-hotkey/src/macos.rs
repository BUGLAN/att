//! macOS 底层键盘监听：CGEventTap（listen-only，需要「输入监控」权限）。

use crate::listener::Shared;
use att_core::settings::hotkey::{hid_from_char, ModSet};
use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
}

/// kCGEventSourceStateHIDSystemState
const HID_SYSTEM_STATE: i32 = 1;

static TAP_PORT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

fn mods_from_flags(flags: CGEventFlags) -> ModSet {
    ModSet::new(
        flags.contains(CGEventFlags::CGEventFlagControl),
        flags.contains(CGEventFlags::CGEventFlagAlternate),
        flags.contains(CGEventFlags::CGEventFlagShift),
        flags.contains(CGEventFlags::CGEventFlagCommand),
    )
}

/// 系统真实修饰键状态（防粘滞轮询用）。
pub fn physical_modifiers() -> Option<ModSet> {
    let raw = unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) };
    Some(mods_from_flags(CGEventFlags::from_bits_truncate(raw)))
}

/// ANSI 虚拟键码 → 字母数字（仅用于映射到 HID 主键；其余键只参与「有无其他键」判断）。
fn char_from_keycode(code: i64) -> Option<char> {
    const TABLE: &[(i64, char)] = &[
        (0x00, 'a'), (0x01, 's'), (0x02, 'd'), (0x03, 'f'), (0x04, 'h'), (0x05, 'g'), (0x06, 'z'),
        (0x07, 'x'), (0x08, 'c'), (0x09, 'v'), (0x0B, 'b'), (0x0C, 'q'), (0x0D, 'w'), (0x0E, 'e'),
        (0x0F, 'r'), (0x10, 'y'), (0x11, 't'), (0x12, '1'), (0x13, '2'), (0x14, '3'), (0x15, '4'),
        (0x16, '6'), (0x17, '5'), (0x19, '9'), (0x1A, '7'), (0x1C, '8'), (0x1D, '0'), (0x1F, 'o'),
        (0x20, 'u'), (0x22, 'i'), (0x23, 'p'), (0x25, 'l'), (0x26, 'j'), (0x28, 'k'), (0x2D, 'n'),
        (0x2E, 'm'),
    ];
    TABLE.iter().find(|(k, _)| *k == code).map(|(_, c)| *c)
}

/// 在专用线程上安装事件监听并运行 CFRunLoop。权限缺失时返回 Err（附引导文案）。
pub fn spawn(shared: Arc<Shared>) -> Result<(), String> {
    if !unsafe { CGPreflightListenEventAccess() } {
        // 触发系统授权弹窗（首次）；授权后需重启应用才能生效
        let granted = unsafe { CGRequestListenEventAccess() };
        if !granted {
            return Err("全局快捷键需要「输入监控」权限：请在 系统设置 → 隐私与安全性 → 输入监控 中允许本应用（开发态为启动它的终端），然后重启应用".into());
        }
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("att-hotkey-macos".into())
        .spawn(move || {
            let callback_shared = shared.clone();
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
                move |_proxy, event_type, event| {
                    match event_type {
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                            // 系统因超时停用了监听：立即重新启用（否则热键静默失效）
                            let port = TAP_PORT.load(Ordering::SeqCst);
                            if !port.is_null() {
                                unsafe { CGEventTapEnable(port, true) };
                            }
                        }
                        CGEventType::FlagsChanged => {
                            callback_shared.on_modifiers(mods_from_flags(event.get_flags()));
                        }
                        CGEventType::KeyDown | CGEventType::KeyUp => {
                            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                            let hid = char_from_keycode(code).and_then(hid_from_char);
                            callback_shared.on_modifiers(mods_from_flags(event.get_flags()));
                            callback_shared.on_key(code as u32, hid, matches!(event_type, CGEventType::KeyDown));
                        }
                        _ => {}
                    }
                    CallbackResult::Keep
                },
            );
            let tap = match tap {
                Ok(tap) => tap,
                Err(()) => {
                    let _ = ready_tx.send(Err("创建键盘监听失败：请在 系统设置 → 隐私与安全性 → 输入监控 中允许本应用后重启".into()));
                    return;
                }
            };
            TAP_PORT.store(tap.mach_port().as_concrete_TypeRef() as *mut _, Ordering::SeqCst);
            let source = match tap.mach_port().create_runloop_source(0) {
                Ok(source) => source,
                Err(()) => {
                    let _ = ready_tx.send(Err("创建键盘监听运行循环失败".into()));
                    return;
                }
            };
            CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
            tap.enable();
            let _ = ready_tx.send(Ok(()));
            CFRunLoop::run_current();
        })
        .map_err(|e| format!("无法启动热键线程: {e}"))?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or_else(|_| Err("键盘监听启动超时".into()))
}
