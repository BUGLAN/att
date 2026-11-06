//! macOS 菜单栏：中文的应用菜单 / 编辑 / 窗口 / 帮助。
//!
//! 不设菜单时 Tauri 会在 macOS 上装一套英文默认菜单（File / Edit / View / Window / Help），
//! 这里换成同样作用的中文版：
//! - 编辑菜单里的撤销、重做、剪切、复制、粘贴、全选是系统预置项，WebView 输入框里的 ⌘Z / ⌘X / ⌘C / ⌘V / ⌘A
//!   靠它们生效（快捷键随预置项固定，与标题文字无关），不能删；
//! - 应用菜单保留关于、服务、隐藏 / 隐藏其他 / 全部显示、退出（⌘Q）；
//! - 窗口菜单保留最小化、缩放、全屏、关闭窗口（⌘W 只隐藏主窗口，见 lib.rs 的 CloseRequested），
//!   并按 Tauri 的约定 id 交给系统当「窗口」菜单（自动列出打开的窗口）；
//! - 帮助菜单为空，系统会在里面放菜单搜索框。
//!
//! 系统还会自动往编辑 / 窗口菜单里加项（开始听写、表情与符号、全部最小化、填充、居中……），
//! 它们的语言跟随 Info.plist 声明的本地化（已声明 zh_CN），不在这里控制。
//!
//! 只在 macOS 装：Windows / Linux 上 Tauri 本来就不加默认菜单，主窗口无边框，保持没有菜单栏。

use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Runtime};

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let package = app.package_info();
    let name = package.name.clone();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(package.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|publisher| vec![publisher]),
        ..Default::default()
    };

    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, Some(&format!("关于{name}")), Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("服务"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&format!("隐藏{name}")))?,
            &PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?,
            &PredefinedMenuItem::show_all(app, Some("全部显示"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some(&format!("退出{name}")))?,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("撤销"))?,
            &PredefinedMenuItem::redo(app, Some("重做"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("剪切"))?,
            &PredefinedMenuItem::copy(app, Some("复制"))?,
            &PredefinedMenuItem::paste(app, Some("粘贴"))?,
            &PredefinedMenuItem::select_all(app, Some("全选"))?,
        ],
    )?;
    let window_menu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("最小化"))?,
            &PredefinedMenuItem::maximize(app, Some("缩放"))?,
            &PredefinedMenuItem::fullscreen(app, Some("切换全屏"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("关闭窗口"))?,
        ],
    )?;
    let help_menu = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "帮助", true, &[])?;

    Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu, &help_menu])
}
