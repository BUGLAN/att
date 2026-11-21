//! 系统权限状态（设置页「系统 / 权限」）：录音、全局快捷键、自动粘贴依赖的授权。
//!
//! 只查询、不触发系统弹窗（弹窗由真正使用该能力时触发）；「打开系统设置」直达对应的隐私面板。
//! 开发态这些权限归属启动 `tauri dev` 的终端应用。
//!
//! 更新后首次启动的引导：macOS 的「输入监控」「辅助功能」授权按代码签名绑定，ad-hoc 签名每次构建都变，
//! 应用内更新后旧授权不再生效，系统设置里旧条目却仍显示开启。启动时比较上次运行的版本，
//! 更新后首次启动且这两项没生效时，主窗口弹一次引导（见 [`update_notice`]）。

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
        if unsafe { CGPreflightListenEventAccess() } {
            "granted"
        } else {
            "denied"
        }
    }

    pub fn accessibility() -> &'static str {
        if unsafe { AXIsProcessTrusted() } {
            "granted"
        } else {
            "denied"
        }
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

/// 更新后可能失效、要引导重新授权的两项（授权按代码签名绑定）。
const RELAUNCH_SENSITIVE: [&str; 2] = ["input_monitoring", "accessibility"];

/// 本次启动是不是「更新后首次启动」：是则返回更新前的版本（更早的版本没记过版本号时为空串）。
///
/// `had_settings`：启动前数据目录里已有 settings.json——没记过版本号时据此区分「全新安装」与「从旧版本升级」。
pub fn previous_version_if_updated(last_run: &str, current: &str, had_settings: bool) -> Option<String> {
    let last = last_run.trim();
    if last.is_empty() {
        return had_settings.then(String::new);
    }
    (last != current.trim()).then(|| last.to_string())
}

/// 从权限清单里挑出要引导重新授权的项：输入监控、辅助功能里没生效的。
fn relaunch_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter(|item| {
            let id = item["id"].as_str().unwrap_or_default();
            RELAUNCH_SENSITIVE.contains(&id) && item["state"] != "granted" && item["state"] != "not_required"
        })
        .cloned()
        .collect()
}

/// 更新后首次启动的权限引导：`previous` 为 None 表示不是更新后首次启动（或引导已关掉）。
/// `show` 为 true 时 `items` 是要重新授权的项；权限都正常就不提示。
pub fn update_notice(previous: Option<&str>, current: &str) -> Value {
    let items = match previous {
        Some(_) if cfg!(target_os = "macos") => {
            let status = status();
            relaunch_items(status["items"].as_array().map(Vec::as_slice).unwrap_or_default())
        }
        _ => Vec::new(),
    };
    json!({
        "show": !items.is_empty(),
        "previous_version": previous,
        "current_version": current,
        "items": items,
    })
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
        std::process::Command::new("explorer")
            .arg("ms-settings:privacy-microphone")
            .spawn()
            .map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 更新后首次启动_按上次运行版本判断() {
        assert_eq!(previous_version_if_updated("2.8.0", "2.9.0", true).as_deref(), Some("2.8.0"));
        assert_eq!(previous_version_if_updated("2.9.0", "2.9.0", true), None, "同一版本再次启动不算");
        assert_eq!(previous_version_if_updated("", "2.9.0", false), None, "全新安装不算");
        assert_eq!(
            previous_version_if_updated("", "2.9.0", true).as_deref(),
            Some(""),
            "从没记过版本号的旧版本升级上来算，更新前版本未知"
        );
    }

    #[test]
    fn 引导项_只挑输入监控与辅助功能里没生效的() {
        let items = vec![
            item("microphone", "麦克风", "", "denied"),
            item("input_monitoring", "输入监控", "", "denied"),
            item("accessibility", "辅助功能", "", "granted"),
        ];
        let picked = relaunch_items(&items);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0]["id"], "input_monitoring");
        assert!(relaunch_items(&[item("accessibility", "辅助功能", "", "not_required")]).is_empty());
    }

    #[test]
    fn 不是更新后首次启动_不提示() {
        let notice = update_notice(None, "2.9.0");
        assert_eq!(notice["show"], false);
        assert_eq!(notice["items"], json!([]));
        assert_eq!(notice["current_version"], "2.9.0");
    }
}
