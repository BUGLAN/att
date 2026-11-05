//! 检查更新：查询发布渠道的最新正式版本，与当前版本比较；可下载对应平台的安装包。
//!
//! - 更新源：GitHub Releases 的 `releases/latest` 接口（与 `.github/workflows/release.yml`
//!   发布渠道一致，只返回非草稿、非预发布的最新版）。`ATT_UPDATE_URL` 可覆盖（测试 / 自建镜像）。
//! - 应用内更新（桌面版）：桌面壳注入 [`InAppUpdater`]（Tauri updater 插件），下载经签名校验的更新包，
//!   用户点「重启并更新」后安装并重启；更新包与清单 `latest.json` 由发布工作流签名产出。
//! - 退路：没有应用内更新器（纯服务 / 测试）或它失败时，下载完整安装包到 `<数据目录>/updates/`，
//!   由用户自己打开安装。
//! - 状态（上次检查结果 + 下载进度）只在内存里；「自动下载更新」开关存 `settings.json`
//!   的 `auto_download_updates`。

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

pub const DEFAULT_RELEASES_URL: &str = "https://api.github.com/repos/BUGLAN/att/releases/latest";
/// 检查请求的总超时（设置页按钮不应长时间转圈）
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// 下载时两次收到数据之间的最长等待
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(DOWNLOAD_IDLE_TIMEOUT)
        // GitHub API 要求带 User-Agent
        .user_agent(concat!("att-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("HTTP 客户端构建失败")
});

/// 更新源地址：`ATT_UPDATE_URL` 优先。
pub fn releases_url() -> String {
    std::env::var("ATT_UPDATE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASES_URL.to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// 一次检查的结果。`status`：`available`（有新版本）/ `latest`（已是最新）/ `error`（检查失败，看 `error`）。
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub status: &'static str,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_name: Option<String>,
    pub release_url: Option<String>,
    pub notes: Option<String>,
    pub published_at: Option<String>,
    /// 适合本机的安装包；有新版本但没有对应平台的包时为 None
    pub asset: Option<ReleaseAsset>,
    pub error: Option<String>,
    pub checked_at: String,
}

/// 下载状态。`status`：`idle` / `downloading` / `done` / `installing` / `error`。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadState {
    pub status: &'static str,
    /// `in_app`：应用内更新包（下载完可「重启并更新」）；`installer`：完整安装包（下载完由用户打开）
    pub mode: &'static str,
    pub version: Option<String>,
    pub file_name: Option<String>,
    pub path: Option<String>,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self { status: "idle", mode: MODE_INSTALLER, version: None, file_name: None, path: None, received: 0, total: None, error: None }
    }
}

pub const MODE_IN_APP: &str = "in_app";
pub const MODE_INSTALLER: &str = "installer";

/// 应用内更新器（桌面壳实现，见 `src-tauri/src/app_update.rs`）：下载与安装都在后台进行，
/// 进度与结果经 [`Updater::in_app_progress`] / [`Updater::in_app_finished`] 写回，前端照常轮询。
pub trait InAppUpdater: Send + Sync {
    /// 开始后台下载最新版的更新包（状态已由 [`Updater`] 占位为 downloading）。
    fn start_download(&self);
    /// 安装已下载的更新包并重启应用（Windows 上交给安装程序、当前进程退出）；失败时回写 error。
    fn install(&self);
}

/// 已占位、待执行的下载。
#[derive(Debug)]
pub struct PendingDownload {
    url: String,
    target: PathBuf,
}

#[derive(Default)]
struct UpdaterState {
    last_check: Option<CheckResult>,
    download: DownloadState,
}

/// 更新服务：无持久状态，进程内共享（挂在 `CoreState::updater`）。
pub struct Updater {
    url: String,
    download_dir: PathBuf,
    state: Mutex<UpdaterState>,
    in_app: std::sync::OnceLock<Arc<dyn InAppUpdater>>,
}

impl Updater {
    pub fn new(data_dir: &Path) -> Self {
        Self::with_url(data_dir, releases_url())
    }

    pub fn with_url(data_dir: &Path, url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            download_dir: data_dir.join("updates"),
            state: Mutex::new(UpdaterState::default()),
            in_app: std::sync::OnceLock::new(),
        }
    }

    /// 桌面壳启动时注入应用内更新器（只能注入一次）。
    pub fn set_in_app(&self, driver: Arc<dyn InAppUpdater>) {
        let _ = self.in_app.set(driver);
    }

    pub fn has_in_app(&self) -> bool {
        self.in_app.get().is_some()
    }

    /// 后台开始下载上次检查到的新版本（设置页按钮 / 自动下载共用）。
    ///
    /// 有应用内更新器且没要求 `installer` 时下载更新包，否则下载完整安装包；
    /// 已在下载、或同一版本已下好时什么都不做；不能下载时返回原因。
    pub fn start_download(self: &Arc<Self>, installer: bool) -> Result<(), String> {
        match self.in_app.get().filter(|_| !installer) {
            Some(driver) => {
                if self.begin_in_app()? {
                    driver.start_download();
                }
                Ok(())
            }
            None => {
                if let Some(pending) = self.begin_download()? {
                    let updater = self.clone();
                    tokio::spawn(async move {
                        updater.run_download(pending).await;
                    });
                }
                Ok(())
            }
        }
    }

    /// 应用内下载占位；返回 false 表示已在下载或同一版本已下好（不重复下载）。
    fn begin_in_app(&self) -> Result<bool, String> {
        let mut state = self.lock();
        let check = state.last_check.clone().filter(|c| c.status == "available").ok_or("没有可下载的新版本，请先检查更新")?;
        let download = &state.download;
        if download.status == "downloading" || download.status == "installing" {
            return Ok(false);
        }
        if download.status == "done" && download.mode == MODE_IN_APP && download.version == check.latest_version {
            return Ok(false);
        }
        state.download = DownloadState {
            status: "downloading",
            mode: MODE_IN_APP,
            version: check.latest_version.clone(),
            ..DownloadState::default()
        };
        Ok(true)
    }

    /// 应用内下载进度：累加本次收到的字节，`total` 为整包大小（已知时）。
    pub fn in_app_progress(&self, received: u64, total: Option<u64>) {
        let mut state = self.lock();
        if state.download.mode == MODE_IN_APP && state.download.status == "downloading" {
            state.download.received += received;
            if total.is_some() {
                state.download.total = total;
            }
        }
    }

    /// 应用内下载 / 安装结束：成功时记下实际下载到的版本（以更新清单为准），失败记原因。
    pub fn in_app_finished(&self, result: Result<String, String>) {
        let mut state = self.lock();
        state.download.mode = MODE_IN_APP;
        match result {
            Ok(version) => {
                tracing::info!("更新包已下载：v{version}");
                state.download.status = "done";
                state.download.version = Some(version);
                state.download.error = None;
            }
            Err(error) => {
                tracing::warn!("应用内更新失败：{error}");
                state.download.status = "error";
                state.download.error = Some(error);
            }
        }
    }

    /// 安装已下载的更新包并重启（只适用于应用内下载完成的状态）。
    pub fn install(&self) -> Result<(), String> {
        let driver = self.in_app.get().ok_or("当前环境不支持应用内更新，请下载安装包手动更新")?;
        {
            let mut state = self.lock();
            if state.download.mode != MODE_IN_APP || state.download.status != "done" {
                return Err("更新包还没下载完成".into());
            }
            state.download.status = "installing";
        }
        tracing::info!("开始安装更新并重启");
        driver.install();
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, UpdaterState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn last_check(&self) -> Option<CheckResult> {
        self.lock().last_check.clone()
    }

    pub fn download_state(&self) -> DownloadState {
        self.lock().download.clone()
    }

    /// 已下载完成的安装包路径（供「在文件管理器中显示」）。
    pub fn downloaded_path(&self) -> Option<PathBuf> {
        let state = self.lock();
        (state.download.status == "done").then(|| state.download.path.clone().map(PathBuf::from)).flatten()
    }

    /// 发起一次检查；失败不返回 Err，而是 `status = "error"` 并带上原因（前端统一展示）。
    pub async fn check(&self, current_version: &str) -> CheckResult {
        let result = match fetch_latest(&self.url).await {
            Ok(release) => evaluate(current_version, release, std::env::consts::OS, std::env::consts::ARCH),
            Err(error) => {
                tracing::warn!("检查更新失败：{error}");
                CheckResult::failed(current_version, error)
            }
        };
        if result.status != "error" {
            tracing::info!(
                "检查更新：当前 {} · 最新 {} · {}",
                result.current_version,
                result.latest_version.as_deref().unwrap_or("-"),
                result.status
            );
        }
        self.lock().last_check = Some(result.clone());
        result
    }

    /// 同步占位开始下载（状态立即变为 `downloading`）：上次检查有可下载的新版本时返回待执行的下载任务；
    /// 正在下载、或同一版本已下载完成且文件还在时返回 None（不重复下载）；不能下载时返回原因。
    pub fn begin_download(&self) -> Result<Option<PendingDownload>, String> {
        let mut state = self.lock();
        let check = state.last_check.clone().filter(|c| c.status == "available").ok_or("没有可下载的新版本，请先检查更新")?;
        let asset = check.asset.clone().ok_or("发布里没有适合本机的安装包，请到发布页手动下载")?;
        if state.download.status == "downloading" {
            return Ok(None);
        }
        let target = self.download_dir.join(sanitize_file_name(&asset.name));
        if state.download.status == "done"
            && state.download.version == check.latest_version
            && target.is_file()
        {
            return Ok(None);
        }
        state.download = DownloadState {
            status: "downloading",
            mode: MODE_INSTALLER,
            version: check.latest_version.clone(),
            file_name: Some(asset.name.clone()),
            path: Some(target.display().to_string()),
            received: 0,
            total: (asset.size > 0).then_some(asset.size),
            error: None,
        };
        Ok(Some(PendingDownload { url: asset.url, target }))
    }

    /// 下载上次检查到的新版本安装包（等到完成）。
    pub async fn download(&self) -> Result<DownloadState, String> {
        match self.begin_download()? {
            Some(pending) => Ok(self.run_download(pending).await),
            None => Ok(self.download_state()),
        }
    }

    /// 执行 [`Self::begin_download`] 占好的下载；调用方通常 spawn 它，再轮询 [`Self::download_state`]。
    pub async fn run_download(&self, pending: PendingDownload) -> DownloadState {
        let PendingDownload { url, target } = pending;
        let outcome = self.stream_to_file(&url, &target).await;
        let mut state = self.lock();
        match outcome {
            Ok(()) => {
                state.download.status = "done";
                tracing::info!("更新安装包已下载：{}", target.display());
            }
            Err(error) => {
                tracing::warn!("下载更新失败：{error}");
                state.download.status = "error";
                state.download.error = Some(error);
            }
        }
        state.download.clone()
    }

    async fn stream_to_file(&self, url: &str, target: &Path) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        std::fs::create_dir_all(&self.download_dir).map_err(|e| format!("无法创建下载目录：{e}"))?;
        let mut response = CLIENT
            .get(url)
            .header(reqwest::header::ACCEPT, "application/octet-stream")
            .send()
            .await
            .map_err(|e| describe_request_error(&e))?;
        if !response.status().is_success() {
            return Err(format!("下载地址返回 HTTP {}", response.status().as_u16()));
        }
        if let Some(total) = response.content_length() {
            self.lock().download.total = Some(total);
        }
        // 先写 .part，完整后再改名：中途失败不会留下一个看起来完整的安装包
        let partial = target.with_extension(format!(
            "{}.part",
            target.extension().and_then(|e| e.to_str()).unwrap_or("bin")
        ));
        let result = async {
            let mut file = tokio::fs::File::create(&partial).await.map_err(|e| format!("无法写入安装包：{e}"))?;
            while let Some(chunk) = response.chunk().await.map_err(|e| describe_request_error(&e))? {
                file.write_all(&chunk).await.map_err(|e| format!("无法写入安装包：{e}"))?;
                self.lock().download.received += chunk.len() as u64;
            }
            file.flush().await.map_err(|e| format!("无法写入安装包：{e}"))?;
            drop(file);
            let state = self.lock().download.clone();
            if let Some(total) = state.total {
                if state.received != total {
                    return Err(format!("下载不完整：{} / {} 字节", state.received, total));
                }
            }
            tokio::fs::rename(&partial, target).await.map_err(|e| format!("无法保存安装包：{e}"))
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result
    }

    /// 前端展示用的完整状态。
    pub fn snapshot(&self, current_version: &str, auto_download: bool) -> Value {
        let state = self.lock();
        serde_json::json!({
            "current_version": current_version,
            "auto_download": auto_download,
            "in_app": self.has_in_app(),
            "source": self.url,
            "check": state.last_check,
            "download": state.download,
        })
    }
}

impl CheckResult {
    fn failed(current_version: &str, error: String) -> Self {
        Self {
            status: "error",
            current_version: current_version.to_string(),
            latest_version: None,
            release_name: None,
            release_url: None,
            notes: None,
            published_at: None,
            asset: None,
            error: Some(error),
            checked_at: crate::now_rfc3339(),
        }
    }
}

/// 检查一次；有新版本且开启了自动下载时在后台开始下载。设置页按钮与启动时的自动检查共用。
pub async fn check_and_maybe_download(core: Arc<crate::state::CoreState>) -> CheckResult {
    let result = core.updater.check(&core.app_version).await;
    // 自动下载：有应用内更新器时下更新包（装不装仍由用户点「重启并更新」），否则下完整安装包
    let downloadable = core.updater.has_in_app() || result.asset.is_some();
    if result.status == "available" && downloadable && core.settings().auto_download_updates {
        if let Err(reason) = core.updater.start_download(false) {
            tracing::warn!("自动下载更新未开始：{reason}");
        }
    }
    result
}

// ---------------- 拉取与解析 ----------------

/// 发布信息里我们关心的字段。
#[derive(Debug, Clone, Default)]
pub struct Release {
    pub tag: String,
    pub name: Option<String>,
    pub html_url: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

async fn fetch_latest(url: &str) -> Result<Release, String> {
    let response = CLIENT
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(CHECK_TIMEOUT)
        .send()
        .await
        .map_err(|e| describe_request_error(&e))?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|e| describe_request_error(&e))?;
    if !(200..300).contains(&status) {
        return Err(describe_status(status, &text));
    }
    parse_release(&text)
}

pub fn parse_release(text: &str) -> Result<Release, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("更新源返回的内容无法解析：{e}"))?;
    let str_field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
    let tag = str_field("tag_name").ok_or("更新源返回的发布信息缺少版本号（tag_name）")?;
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ReleaseAsset {
                        name: item.get("name")?.as_str()?.to_string(),
                        url: item.get("browser_download_url")?.as_str()?.to_string(),
                        size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        tag,
        name: str_field("name"),
        html_url: str_field("html_url"),
        body: str_field("body"),
        published_at: str_field("published_at"),
        assets,
    })
}

/// 非 2xx 的可读原因。
fn describe_status(status: u16, body: &str) -> String {
    let upstream = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string));
    let suffix = upstream.map(|m| format!("（{m}）")).unwrap_or_default();
    match status {
        404 => format!("找不到发布信息（HTTP 404）：更新源仓库不可公开访问，或还没有正式发布的版本{suffix}"),
        401 | 403 | 429 => format!("更新源拒绝访问（HTTP {status}）：可能触发了接口频率限制，请稍后再试{suffix}"),
        500..=599 => format!("更新源服务异常（HTTP {status}），请稍后再试{suffix}"),
        _ => format!("更新源返回 HTTP {status}{suffix}"),
    }
}

fn describe_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "连接更新源超时，请检查网络或代理设置".to_string()
    } else if error.is_connect() {
        format!("无法连接更新源，请检查网络或代理设置：{error}")
    } else {
        format!("请求更新源失败：{error}")
    }
}

/// 结合当前版本与平台得出检查结果（纯函数，便于测试）。
pub fn evaluate(current_version: &str, release: Release, os: &str, arch: &str) -> CheckResult {
    let latest = release.tag.trim_start_matches(['v', 'V']).to_string();
    let newer = is_newer(&latest, current_version);
    CheckResult {
        status: if newer { "available" } else { "latest" },
        current_version: current_version.to_string(),
        asset: if newer { pick_asset(&release.assets, os, arch) } else { None },
        latest_version: Some(latest),
        release_name: release.name,
        release_url: release.html_url,
        notes: release.body,
        published_at: release.published_at,
        error: None,
        checked_at: crate::now_rfc3339(),
    }
}

/// 解析 `1.2.3` / `v1.2.3-beta.1`：数字段 + 可选预发布标记。
fn parse_version(text: &str) -> Option<(Vec<u64>, Option<String>)> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let text = text.split('+').next()?; // 忽略构建元数据
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_string())),
        None => (text, None),
    };
    let parts = core.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    (!parts.is_empty()).then_some((parts, pre))
}

/// `candidate` 是否比 `current` 新（解析失败按「不是更新」处理，避免误报）。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse_version(candidate), parse_version(current)) else {
        return false;
    };
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    // 数字相同：正式版比预发布新；都是预发布时按字典序
    match (a_pre, b_pre) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

/// 选择适合本机的安装包：macOS → `.dmg`，Windows → `-setup.exe`（其次 `.msi`），Linux → `.AppImage` / `.deb`；
/// 架构必须匹配（宁可不给，也不下错架构的包）。
pub fn pick_asset(assets: &[ReleaseAsset], os: &str, arch: &str) -> Option<ReleaseAsset> {
    let arch_tokens: &[&str] = match arch {
        "aarch64" => &["aarch64", "arm64"],
        "x86_64" => &["x64", "x86_64", "amd64"],
        _ => &[],
    };
    let suffixes: &[&str] = match os {
        "macos" => &[".dmg"],
        "windows" => &["-setup.exe", ".msi"],
        "linux" => &[".appimage", ".deb"],
        _ => &[],
    };
    suffixes.iter().find_map(|suffix| {
        assets
            .iter()
            .find(|a| {
                let name = a.name.to_ascii_lowercase();
                name.ends_with(suffix) && arch_tokens.iter().any(|t| name.contains(t))
            })
            .cloned()
    })
}

/// 下载文件名只保留安全字符，防止发布资产名里带路径分隔符。
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim_start_matches('.').to_string();
    if cleaned.is_empty() { "update.bin".into() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::testing::{stub_routes, StubRoute};

    fn assets() -> Vec<ReleaseAsset> {
        ["_2.6.0_aarch64.dmg", "_2.6.0_x64-setup.exe", "_2.6.0_x64_zh-CN.msi", "_aarch64.app.tar.gz"]
            .iter()
            .map(|name| ReleaseAsset { name: name.to_string(), url: format!("https://x/{name}"), size: 10 })
            .collect()
    }

    #[test]
    fn 版本比较() {
        assert!(is_newer("2.6.0", "2.5.1"));
        assert!(is_newer("v2.10.0", "2.9.9"), "按数字比较而不是字符串");
        assert!(is_newer("2.5.1", "2.5.1-beta.2"), "正式版比同号预发布新");
        assert!(!is_newer("2.5.1", "2.5.1"));
        assert!(!is_newer("2.5.0", "2.5.1"));
        assert!(!is_newer("2.5", "2.5.0"), "缺省段按 0");
        assert!(!is_newer("nightly", "2.5.1"), "解析不了不当作更新");
    }

    #[test]
    fn 选择安装包_按平台与架构() {
        let list = assets();
        assert_eq!(pick_asset(&list, "macos", "aarch64").unwrap().name, "_2.6.0_aarch64.dmg");
        assert!(pick_asset(&list, "macos", "x86_64").is_none(), "没有 Intel 包时不给错架构的包");
        assert_eq!(pick_asset(&list, "windows", "x86_64").unwrap().name, "_2.6.0_x64-setup.exe");
        assert!(pick_asset(&list, "linux", "x86_64").is_none());
        assert_eq!(sanitize_file_name("../evil/名.dmg"), "_evil_名.dmg");
    }

    #[test]
    fn 解析与判定() {
        let release = parse_release(r#"{"tag_name":"v2.6.0","name":"v2.6.0","html_url":"https://h","body":"修复若干问题","assets":[{"name":"_2.6.0_aarch64.dmg","browser_download_url":"https://d","size":7}]}"#).unwrap();
        let result = evaluate("2.5.1", release.clone(), "macos", "aarch64");
        assert_eq!(result.status, "available");
        assert_eq!(result.latest_version.as_deref(), Some("2.6.0"));
        assert_eq!(result.asset.unwrap().url, "https://d");
        assert_eq!(evaluate("2.6.0", release, "macos", "aarch64").status, "latest");
        assert!(parse_release(r#"{"name":"x"}"#).unwrap_err().contains("tag_name"));
        assert!(parse_release("<html>").unwrap_err().contains("无法解析"));
        assert!(describe_status(404, r#"{"message":"Not Found"}"#).contains("HTTP 404"));
        assert!(describe_status(403, "").contains("频率限制"));
    }

    /// 记录调用次数的假应用内更新器（真正的下载 / 安装在桌面壳里，这里只验状态机）
    #[derive(Default)]
    struct FakeInApp {
        downloads: std::sync::atomic::AtomicUsize,
        installs: std::sync::atomic::AtomicUsize,
    }

    impl InAppUpdater for FakeInApp {
        fn start_download(&self) {
            self.downloads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn install(&self) {
            self.installs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn 应用内更新_下载占位_进度_完成后可安装_失败可退回安装包() {
        use std::sync::atomic::Ordering::SeqCst;
        let dir = std::env::temp_dir().join(format!("att-update-inapp-{}", uuid::Uuid::new_v4()));
        let (base, _rx) = stub_routes(vec![StubRoute::json(
            "GET /latest ",
            200,
            r#"{"tag_name":"v9.9.9","assets":[]}"#,
        )])
        .await;
        let updater = Arc::new(Updater::with_url(&dir, format!("{base}/latest")));
        assert!(updater.start_download(false).is_err(), "没检查过不能下载");
        assert!(updater.install().is_err(), "没有应用内更新器时不能安装");

        let fake = Arc::new(FakeInApp::default());
        updater.set_in_app(fake.clone());
        assert_eq!(updater.snapshot("2.6.1", false)["in_app"], true);
        assert_eq!(updater.check("2.6.1").await.status, "available");
        assert!(updater.install().is_err(), "还没下载完不能安装");

        // 发布里没有本机安装包也能走应用内更新
        updater.start_download(false).unwrap();
        updater.start_download(false).unwrap();
        assert_eq!(fake.downloads.load(SeqCst), 1, "下载中再点不重复下载");
        let state = updater.download_state();
        assert_eq!((state.status, state.mode, state.version.as_deref()), ("downloading", MODE_IN_APP, Some("9.9.9")));

        updater.in_app_progress(400, Some(1000));
        updater.in_app_progress(600, None);
        let state = updater.download_state();
        assert_eq!((state.received, state.total), (1000, Some(1000)));

        updater.in_app_finished(Ok("9.9.9".into()));
        assert_eq!(updater.download_state().status, "done");
        updater.start_download(false).unwrap();
        assert_eq!(fake.downloads.load(SeqCst), 1, "同一版本已下好不重复下载");

        updater.install().unwrap();
        assert_eq!(updater.download_state().status, "installing");
        assert_eq!(fake.installs.load(SeqCst), 1);
        assert!(updater.start_download(false).is_ok() && fake.downloads.load(SeqCst) == 1, "安装中不再下载");

        // 安装失败：回到 error，可重试下载；也可显式改为下载安装包（这里发布里没有，给出原因）
        updater.in_app_finished(Err("安装更新失败：x".into()));
        let state = updater.download_state();
        assert_eq!((state.status, state.error.as_deref()), ("error", Some("安装更新失败：x")));
        updater.start_download(false).unwrap();
        assert_eq!(fake.downloads.load(SeqCst), 2, "失败后可重新下载");
        let reason = updater.start_download(true).unwrap_err();
        assert!(reason.contains("没有适合本机的安装包"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 检查_有新版本_最新_失败三种结果并可下载() {
        let dir = std::env::temp_dir().join(format!("att-update-{}", uuid::Uuid::new_v4()));
        let (base, _rx) = stub_routes(vec![
            StubRoute::json("GET /missing ", 404, r#"{"message":"Not Found"}"#),
            StubRoute { request_line: "GET /pkg.dmg ", body_contains: None, status: 200, content_type: "application/octet-stream", body: "PKGDATA".into() },
        ])
        .await;
        // 发布信息里的下载地址要指向上面的桩，所以发布 JSON 由第二个桩提供
        let release_json = format!(
            r#"{{"tag_name":"v9.9.9","assets":[{{"name":"Luomo_9.9.9_{arch}.dmg","browser_download_url":"{base}/pkg.dmg","size":7}},{{"name":"Luomo_9.9.9_{arch_win}-setup.exe","browser_download_url":"{base}/pkg.dmg","size":7}}]}}"#,
            arch = if std::env::consts::ARCH == "aarch64" { "aarch64" } else { "x64" },
            arch_win = "x64",
        );
        let (release_base, _rx2) = stub_routes(vec![StubRoute::json("GET /latest ", 200, release_json)]).await;

        // 有新版本
        let updater = Updater::with_url(&dir, format!("{release_base}/latest"));
        let result = updater.check("2.5.1").await;
        assert_eq!(result.status, "available", "{result:?}");
        assert_eq!(result.latest_version.as_deref(), Some("9.9.9"));
        if result.asset.is_some() {
            let state = updater.download().await.unwrap();
            assert_eq!(state.status, "done", "{state:?}");
            let path = updater.downloaded_path().unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "PKGDATA");
            assert!(!path.with_extension("dmg.part").exists());
        }

        // 已是最新
        assert_eq!(updater.check("9.9.9").await.status, "latest");

        // 失败：404 带原因；连不上带原因
        let missing = Updater::with_url(&dir, format!("{base}/missing")).check("2.5.1").await;
        assert_eq!(missing.status, "error");
        assert!(missing.error.unwrap().contains("HTTP 404"));
        let offline = Updater::with_url(&dir, "http://127.0.0.1:9/latest").check("2.5.1").await;
        assert_eq!(offline.status, "error");
        assert!(offline.error.unwrap().contains("更新源"));
        // 没检查到新版本时不能下载
        assert!(Updater::with_url(&dir, "http://127.0.0.1:9/").download().await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
