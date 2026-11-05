//! 应用内更新：Tauri updater 插件实现 att-core 的 [`InAppUpdater`]。
//!
//! - 检查仍由 att-core 走 GitHub API（设置页展示版本号与更新说明）；这里只负责下载与安装。
//! - 下载：插件按 `tauri.conf.json` 的 `plugins.updater.endpoints` 取发布里的 `latest.json`，
//!   下载本平台的更新包（macOS `.app.tar.gz`、Windows NSIS / MSI 安装程序）并用内置公钥校验签名，
//!   字节留在内存里，等用户点「重启并更新」。
//! - 安装：macOS 替换当前 `.app` 后重启；Windows 以 passive 模式启动安装程序，当前进程随即退出，
//!   安装完由安装程序重新打开应用。
//! - 签名私钥只在发布工作流里（GitHub Secrets），公钥写在 `tauri.conf.json`；
//!   没有对应签名的更新包会被拒绝安装。

use att_core::state::CoreState;
use att_core::update::InAppUpdater;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// 已下载、待安装的更新包
type Ready = (Update, Vec<u8>);

pub struct TauriUpdater {
    app: AppHandle,
    /// 弱引用：CoreState 持有本对象，反过来强引用会成环
    core: Weak<CoreState>,
    ready: Arc<Mutex<Option<Ready>>>,
}

/// 创建并注入到 `core.updater`（需在启动时的自动检查之前调用）。
pub fn install_driver(app: AppHandle, core: &Arc<CoreState>) {
    let driver = TauriUpdater { app, core: Arc::downgrade(core), ready: Arc::new(Mutex::new(None)) };
    core.updater.set_in_app(Arc::new(driver));
}

impl InAppUpdater for TauriUpdater {
    fn start_download(&self) {
        let (app, core, ready) = (self.app.clone(), self.core.clone(), self.ready.clone());
        tauri::async_runtime::spawn(async move {
            let result = download(&app, &core, &ready).await;
            if let Ok(version) = &result {
                // 前端提示「已下载，可重启更新」（自动下载时用户多半不在关于页）
                let _ = app.emit("update:ready", serde_json::json!({ "version": version }));
            }
            if let Some(core) = core.upgrade() {
                core.updater.in_app_finished(result);
            }
        });
    }

    fn install(&self) {
        let (app, core, ready) = (self.app.clone(), self.core.clone(), self.ready.clone());
        tauri::async_runtime::spawn(async move {
            // 让「正在安装」的状态先回到前端，再动手（Windows 上安装程序启动后进程立即退出）
            tokio::time::sleep(Duration::from_millis(300)).await;
            let pending = ready.lock().unwrap_or_else(|e| e.into_inner()).take();
            let Some((update, bytes)) = pending else {
                if let Some(core) = core.upgrade() {
                    core.updater.in_app_finished(Err("更新包已失效，请重新下载".into()));
                }
                return;
            };
            let version = update.version.clone();
            let outcome = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            match outcome {
                Ok(()) => {
                    tracing::info!("更新 v{version} 已安装，重启应用");
                    app.restart();
                }
                Err(error) => {
                    if let Some(core) = core.upgrade() {
                        core.updater.in_app_finished(Err(format!("安装更新失败：{error}")));
                    }
                }
            }
        });
    }
}

/// 按更新清单下载并校验本平台的更新包；成功返回清单里的版本号。
async fn download(app: &AppHandle, core: &Weak<CoreState>, ready: &Mutex<Option<Ready>>) -> Result<String, String> {
    let updater = app.updater().map_err(|e| format!("应用内更新不可用：{e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("读取更新清单失败：{e}"))?
        .ok_or("更新清单里还没有比当前更新的版本（新版本可能还在打包），请稍后再试，或下载安装包手动更新")?;
    let version = update.version.clone();
    tracing::info!("开始下载更新包 v{version}：{}", update.download_url);
    let progress = core.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                if let Some(core) = progress.upgrade() {
                    core.updater.in_app_progress(chunk as u64, total);
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载更新包失败：{e}"))?;
    *ready.lock().unwrap_or_else(|e| e.into_inner()) = Some((update, bytes));
    Ok(version)
}

#[cfg(test)]
mod tests {
    /// tauri.conf.json 里的 updater 配置要能被插件解析（解析失败时应用启动即报错），
    /// 且清单地址指向发布页的 latest.json（发布工作流由 tauri-action 生成）。
    #[test]
    fn updater配置可解析_清单指向最新发布() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true, "打包要产出签名更新包");
        let updater = conf["plugins"]["updater"].clone();
        let config: tauri_plugin_updater::Config = serde_json::from_value(updater).unwrap();
        assert!(!config.pubkey.trim().is_empty());
        assert_eq!(
            config.endpoints.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
            ["https://github.com/BUGLAN/att/releases/latest/download/latest.json"]
        );
    }
}
