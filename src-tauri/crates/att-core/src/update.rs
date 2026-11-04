//! 检查更新：查询发布渠道的最新正式版本，与当前版本比较；可下载对应平台的安装包。
//!
//! - 更新源：GitHub Releases 的 `releases/latest` 接口（与 `.github/workflows/release.yml`
//!   发布渠道一致，只返回非草稿、非预发布的最新版）。`ATT_UPDATE_URL` 可覆盖（测试 / 自建镜像）。
//! - 只下载不安装：安装包落到 `<数据目录>/updates/`，由用户自己打开安装（不改动正在运行的程序，
//!   也不需要更新签名基础设施）。
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

/// 下载状态。`status`：`idle` / `downloading` / `done` / `error`。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadState {
    pub status: &'static str,
    pub version: Option<String>,
    pub file_name: Option<String>,
    pub path: Option<String>,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self { status: "idle", version: None, file_name: None, path: None, received: 0, total: None, error: None }
    }
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
}

impl Updater {
    pub fn new(data_dir: &Path) -> Self {
        Self::with_url(data_dir, releases_url())
    }

    pub fn with_url(data_dir: &Path, url: impl Into<String>) -> Self {
        Self { url: url.into(), download_dir: data_dir.join("updates"), state: Mutex::new(UpdaterState::default()) }
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
    if result.status == "available" && result.asset.is_some() && core.settings().auto_download_updates {
        if let Ok(Some(pending)) = core.updater.begin_download() {
            let core = core.clone();
            tokio::spawn(async move {
                core.updater.run_download(pending).await;
            });
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
