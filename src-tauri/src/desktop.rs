//! 桌面联动：模拟粘贴到焦点窗口、录音期间系统静音（架构文档 §9）。
//!
//! 与旧版 Python helper 行为对齐：
//! - 粘贴：延迟 120ms 后发送 Cmd/Ctrl+V（macOS 需「辅助功能」权限，开发态归属启动它的终端）
//! - 静音：macOS 走 osascript（与旧 audio_helper.py 相同）；Windows 端点静音尚未移植

use std::time::Duration;

/// 粘贴前的延迟：给焦点窗口留出处理剪贴板变更的时间（旧版同值）。
const PASTE_DELAY: Duration = Duration::from_millis(120);

/// 写剪贴板文本。
pub fn write_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text.to_string()).map_err(|e| e.to_string())
}

/// 模拟系统粘贴快捷键（阻塞约 200ms，调用方放到阻塞线程）。
pub fn paste_active() -> Result<(), String> {
    std::thread::sleep(PASTE_DELAY);
    platform::paste()
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
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
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

    pub fn output_muted() -> Option<bool> {
        None
    }

    pub fn set_output_muted(_muted: bool) -> bool {
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub fn paste() -> Result<(), String> {
        Err("当前平台暂不支持自动粘贴".into())
    }

    pub fn output_muted() -> Option<bool> {
        None
    }

    pub fn set_output_muted(_muted: bool) -> bool {
        false
    }
}
