//! 系统权限状态（设置页「系统 / 权限」）：录音、全局快捷键、自动粘贴依赖的授权。
//!
//! 只查询、不触发系统弹窗（弹窗由真正使用该能力时触发）；「打开系统设置」直达对应的隐私面板。
//! 开发态这些权限归属启动 `tauri dev` 的终端应用。

use serde_json::{json, Value};

/// 单项状态：granted / denied / unknown（尚未询问或无法查询）/ not_required（当前平台不需要）。
fn item(id: &str, label: &str, desc: &str, state: &str) -> Value {
    json!({ "id": id, "label": label, "desc": desc, "state": state })
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: *const AnyObject;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
    }

    /// AVAuthorizationStatus：0 未询问 / 1 受限 / 2 拒绝 / 3 已授权。
    pub fn microphone() -> &'static str {
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else { return "unknown" };
        let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: AVMediaTypeAudio] };
        match status {
            3 => "granted",
            1 | 2 => "denied",
            _ => "unknown",
        }
    }

    pub fn input_monitoring() -> &'static str {
        if unsafe { CGPreflightListenEventAccess() } { "granted" } else { "denied" }
    }

    pub fn accessibility() -> &'static str {
        if unsafe { AXIsProcessTrusted() } { "granted" } else { "denied" }
    }

    pub fn settings_url(id: &str) -> Option<&'static str> {
        match id {
            "microphone" => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
            "input_monitoring" => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"),
            "accessibility" => Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"),
            _ => None,
        }
    }
}

/// 当前平台的权限清单。
pub fn status() -> Value {
    #[cfg(target_os = "macos")]
    let items = vec![
        item("microphone", "麦克风", "录音必需；首次录音时系统会询问", platform::microphone()),
        item("input_monitoring", "输入监控", "全局快捷键必需；授权后需重启应用才生效", platform::input_monitoring()),
        item("accessibility", "辅助功能", "自动粘贴到焦点窗口必需", platform::accessibility()),
    ];
    #[cfg(windows)]
    let items = vec![
        item("microphone", "麦克风", "若录音无声，检查「设置 → 隐私和安全性 → 麦克风」是否允许桌面应用访问", "unknown"),
        item("input_monitoring", "全局快捷键", "Windows 无需额外授权", "not_required"),
        item("accessibility", "自动粘贴", "Windows 无需额外授权；以管理员身份运行的窗口无法接收粘贴", "not_required"),
    ];
    #[cfg(not(any(target_os = "macos", windows)))]
    let items = vec![item("microphone", "麦克风", "由系统音频服务管理", "unknown")];
    json!({ "platform": att_core::Platform::current().as_str(), "items": items })
}

/// 打开对应的系统隐私设置面板。
pub fn open_settings(id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = platform::settings_url(id).ok_or_else(|| format!("未知权限：{id}"))?;
        std::process::Command::new("open").arg(url).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(windows)]
    {
        if id != "microphone" {
            return Err("Windows 无需为该能力单独授权".into());
        }
        std::process::Command::new("explorer").arg("ms-settings:privacy-microphone").spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = id;
        Err("当前平台不支持跳转系统权限设置".into())
    }
}

/// 在文件管理器中显示文件 / 打开目录。
pub fn reveal(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("路径不存在：{}", path.display()));
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
    result.map(|_| ()).map_err(|e| e.to_string())
}
