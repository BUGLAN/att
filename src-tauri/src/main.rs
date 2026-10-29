// 防止 Windows 发布构建弹控制台（Tauri 惯例）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    att_app_lib::run()
}
