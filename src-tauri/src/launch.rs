//! 开机自启与主窗口关闭策略。
//!
//! 系统登录项是开机自启的真相源：启动和应用内读取设置时都以插件查询结果回填
//! `settings.json`，用户在系统设置里删除登录项后不会继续显示为已开启。

use att_core::{state::CoreState, Platform};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// 系统登录项启动时附加的参数。主窗口创建后保持隐藏，只驻留托盘。
pub const AUTOSTART_ARG: &str = "--autostart";

fn args_have_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter().any(|arg| arg.as_ref() == AUTOSTART_ARG)
}

/// 当前进程是否由系统登录项以 `--autostart` 启动。
pub fn started_from_autostart() -> bool {
    args_have_autostart(std::env::args_os())
}

fn persist_actual(core: &CoreState, enabled: bool) -> Result<(), String> {
    if core.settings().launch_at_login == enabled {
        return Ok(());
    }
    core.update_settings(|settings| settings.launch_at_login = enabled)
        .map_err(|error| format!("保存开机自启状态失败：{error}"))
}

/// 以系统实际登录项状态回填设置并返回该状态。
pub fn reconcile_launch_at_login(app: &AppHandle, core: &CoreState) -> Result<bool, String> {
    let enabled = app.autolaunch().is_enabled().map_err(|error| format!("读取开机自启状态失败：{error}"))?;
    persist_actual(core, enabled)?;
    Ok(enabled)
}

/// 开启或关闭系统登录项，并立即以查询到的实际状态回填设置。
pub fn set_launch_at_login(app: &AppHandle, core: &CoreState, enabled: bool) -> Result<bool, String> {
    let before = app.autolaunch().is_enabled().map_err(|error| format!("读取开机自启状态失败：{error}"))?;
    if before == enabled {
        persist_actual(core, before)?;
        return Ok(before);
    }
    let operation = if enabled { app.autolaunch().enable() } else { app.autolaunch().disable() };
    if let Err(error) = operation {
        // 系统调用报错时仍可能有部分生效；能查到实际值就以它为准，避免设置文件撒谎。
        if let Ok(actual) = app.autolaunch().is_enabled() {
            let _ = persist_actual(core, actual);
        }
        return Err(format!("{}开机自启失败：{error}", if enabled { "开启" } else { "关闭" }));
    }
    let actual = app.autolaunch().is_enabled().map_err(|error| format!("确认开机自启状态失败：{error}"))?;
    persist_actual(core, actual)?;
    if actual != enabled {
        return Err(format!("系统未接受{}开机自启的修改", if enabled { "开启" } else { "关闭" }));
    }
    Ok(actual)
}

/// 主窗口收到关闭请求时是否隐藏并驻留后台。
///
/// macOS 保持原有行为，Windows 由设置决定；其他平台继续沿用关闭即退出的旧行为。
pub fn should_hide_main_window(platform: Platform, close_action: &str) -> bool {
    match platform {
        Platform::Macos => true,
        Platform::Windows => close_action != "quit",
        Platform::Linux => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自启参数只识别完整参数() {
        assert!(args_have_autostart(["att-app.exe", AUTOSTART_ARG]));
        assert!(!args_have_autostart(["att-app.exe", "--autostart=false"]));
        assert!(!args_have_autostart(["att-app.exe"]));
    }

    #[test]
    fn 主窗口关闭策略_平台与配置共同决定() {
        assert!(should_hide_main_window(Platform::Macos, "quit"));
        assert!(should_hide_main_window(Platform::Windows, "tray"));
        assert!(should_hide_main_window(Platform::Windows, "unknown"));
        assert!(!should_hide_main_window(Platform::Windows, "quit"));
        assert!(!should_hide_main_window(Platform::Linux, "tray"));
    }
}
