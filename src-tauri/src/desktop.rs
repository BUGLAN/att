//! 桌面联动：模拟粘贴 / 模拟键盘输入到焦点窗口、录音期间系统静音（架构文档 §9）。
//!
//! 与旧版 Python helper 行为对齐：
//! - 粘贴：延迟 120ms 后发送 Cmd/Ctrl+V（macOS 需「辅助功能」权限，开发态归属启动它的终端）
//! - 静音：macOS 走 osascript（与旧 audio_helper.py 相同）；Windows 走 Core Audio 端点静音，
//!   每次调用都重新取当前默认输出设备（旧版 pycaw 句柄启动时固化，切换耳机后操作的是旧设备）

use std::time::Duration;

/// 粘贴前的延迟：给焦点窗口留出处理剪贴板变更的时间（旧版同值）。
const PASTE_DELAY: Duration = Duration::from_millis(120);

/// 写剪贴板文本。
pub fn write_clipboard(text: &str) -> Result<(), String> {
    // 本应用写入不进剪贴板历史（监控据此跳过）
    crate::clipboard::mark_internal_write(text);
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text.to_string()).map_err(|e| e.to_string())
}

/// 模拟系统粘贴快捷键（阻塞约 200ms，调用方放到阻塞线程）。
pub fn paste_active() -> Result<(), String> {
    std::thread::sleep(PASTE_DELAY);
    platform::paste()
}

/// 模拟键盘输入前的检查（macOS：辅助功能授权）。
pub fn ensure_typing_ready() -> Result<(), String> {
    platform::ensure_typing_ready()
}

/// 以 Unicode 键盘事件把文本打进焦点窗口（不含换行：换行由调用方走粘贴，见 typing.rs）。
pub fn type_unicode(text: &str) -> Result<(), String> {
    platform::type_unicode(text)
}

/// 模拟按 `count` 次退格。
pub fn press_backspace(count: usize) -> Result<(), String> {
    platform::press_backspace(count)
}

/// 查询 / 设置系统输出静音。不支持的平台返回 None。
pub fn output_muted() -> Option<bool> {
    platform::output_muted()
}

pub fn set_output_muted(muted: bool) -> bool {
    platform::set_output_muted(muted)
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use std::process::Command;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    }

    /// 是否已获「辅助功能」授权；未授权时弹出系统引导（只弹一次，由系统去重）。
    fn accessibility_trusted() -> bool {
        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
        unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
    }

    pub fn paste() -> Result<(), String> {
        if !accessibility_trusted() {
            return Err("自动粘贴需要「辅助功能」权限：请在 系统设置 → 隐私与安全性 → 辅助功能 中允许本应用（开发态为启动它的终端）".into());
        }
        const KEY_V: u16 = 0x09;
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "创建键盘事件源失败".to_string())?;
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(source.clone(), KEY_V, down).map_err(|_| "创建键盘事件失败".to_string())?;
            event.set_flags(CGEventFlags::CGEventFlagCommand);
            event.post(CGEventTapLocation::HID);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }

    /// 单个键盘事件携带的 UTF-16 上限：Chromium 系应用超出部分会被截掉。
    const MAX_UNITS_PER_EVENT: usize = 20;
    /// 事件间隔：太密时部分应用会丢字或乱序。
    const TYPE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(6);

    pub fn ensure_typing_ready() -> Result<(), String> {
        if accessibility_trusted() {
            Ok(())
        } else {
            Err("自动输入需要「辅助功能」权限：请在 系统设置 → 隐私与安全性 → 辅助功能 中允许本应用（开发态为启动它的终端）".into())
        }
    }

    fn event_source() -> Result<CGEventSource, String> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "创建键盘事件源失败".to_string())
    }

    /// 只发按下事件携带文本；修饰键清零，避免与仍按着的快捷键组合成命令。
    fn post_units(source: &CGEventSource, units: &[u16]) -> Result<(), String> {
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true).map_err(|_| "创建键盘事件失败".to_string())?;
        down.set_flags(CGEventFlags::CGEventFlagNull);
        down.set_string_from_utf16_unchecked(units);
        down.post(CGEventTapLocation::HID);
        let up = CGEvent::new_keyboard_event(source.clone(), 0, false).map_err(|_| "创建键盘事件失败".to_string())?;
        up.set_flags(CGEventFlags::CGEventFlagNull);
        up.post(CGEventTapLocation::HID);
        std::thread::sleep(TYPE_INTERVAL);
        Ok(())
    }

    pub fn type_unicode(text: &str) -> Result<(), String> {
        let source = event_source()?;
        let mut units: Vec<u16> = Vec::with_capacity(MAX_UNITS_PER_EVENT);
        let mut buf = [0u16; 2];
        for ch in text.chars() {
            let encoded = ch.encode_utf16(&mut buf);
            // 按字符切块，不把代理对拆到两个事件里
            if units.len() + encoded.len() > MAX_UNITS_PER_EVENT {
                post_units(&source, &units)?;
                units.clear();
            }
            units.extend_from_slice(encoded);
        }
        if !units.is_empty() {
            post_units(&source, &units)?;
        }
        Ok(())
    }

    pub fn press_backspace(count: usize) -> Result<(), String> {
        const KEY_DELETE: u16 = 0x33;
        let source = event_source()?;
        for _ in 0..count {
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(source.clone(), KEY_DELETE, down).map_err(|_| "创建键盘事件失败".to_string())?;
                event.set_flags(CGEventFlags::CGEventFlagNull);
                event.post(CGEventTapLocation::HID);
            }
            std::thread::sleep(TYPE_INTERVAL);
        }
        Ok(())
    }

    fn osascript(script: &str) -> Option<String> {
        let output = Command::new("osascript").args(["-e", script]).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn output_muted() -> Option<bool> {
        osascript("output muted of (get volume settings)").map(|s| s == "true")
    }

    pub fn set_output_muted(muted: bool) -> bool {
        osascript(&format!("set volume output muted {muted}")).is_some()
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_BACK, VK_CONTROL,
    };

    fn key(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    pub fn paste() -> Result<(), String> {
        const VK_V: u16 = 0x56;
        let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err("模拟粘贴失败（SendInput 被拦截）".into());
        }
        Ok(())
    }

    fn unicode(unit: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        // 分批发送：单次过长时部分应用会丢字
        for batch in inputs.chunks(64) {
            let sent = unsafe { SendInput(batch.len() as u32, batch.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
            if sent as usize != batch.len() {
                return Err("模拟输入失败（SendInput 被拦截）".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(6));
        }
        Ok(())
    }

    pub fn ensure_typing_ready() -> Result<(), String> {
        Ok(())
    }

    pub fn type_unicode(text: &str) -> Result<(), String> {
        let inputs: Vec<INPUT> = text.encode_utf16().flat_map(|unit| [unicode(unit, false), unicode(unit, true)]).collect();
        send(&inputs)
    }

    pub fn press_backspace(count: usize) -> Result<(), String> {
        let inputs: Vec<INPUT> = (0..count).flat_map(|_| [key(VK_BACK, false), key(VK_BACK, true)]).collect();
        send(&inputs)
    }

    /// 当前默认输出设备的音量控制（COM 在调用线程按需初始化，调用方都在阻塞线程里）。
    fn endpoint_volume() -> Option<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume> {
        use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
        unsafe {
            // 已初始化（S_FALSE / RPC_E_CHANGED_MODE）都不影响后续调用
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            device.Activate(CLSCTX_ALL, None).ok()
        }
    }

    pub fn output_muted() -> Option<bool> {
        let volume = endpoint_volume()?;
        unsafe { volume.GetMute().ok().map(|muted| muted.as_bool()) }
    }

    pub fn set_output_muted(muted: bool) -> bool {
        let Some(volume) = endpoint_volume() else { return false };
        unsafe { volume.SetMute(muted, std::ptr::null()).is_ok() }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub fn paste() -> Result<(), String> {
        Err("当前平台暂不支持自动粘贴".into())
    }

    pub fn ensure_typing_ready() -> Result<(), String> {
        Err("当前平台暂不支持自动输入".into())
    }

    pub fn type_unicode(_text: &str) -> Result<(), String> {
        Err("当前平台暂不支持自动输入".into())
    }

    pub fn press_backspace(_count: usize) -> Result<(), String> {
        Err("当前平台暂不支持自动输入".into())
    }

    pub fn output_muted() -> Option<bool> {
        None
    }

    pub fn set_output_muted(_muted: bool) -> bool {
        false
    }
}
