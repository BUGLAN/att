//! 本地模型应用内下载：后台任务、断点续传、SHA-256 校验、解压与原子安装。
//!
//! 下载先落在 `<模型目录>/.downloads/<模型 id>/`，全部校验通过后才 rename 到最终位置：
//! 中途失败/取消不会留下一个看起来完整的模型目录，下次点下载时按已有字节发 Range 续传。

use super::catalog::{self, DownloadSource, Engine, FileSpec, ModelSpec};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 下载总超时按「两次收到数据」计算：大模型几十秒没有新块也不算失败。
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(IDLE_TIMEOUT)
        .user_agent(concat!("att-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("模型下载 HTTP 客户端构建失败")
});

/// 下载任务的公开状态（`/api/models` 与 `/api/models/jobs/{id}`）。
#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub id: String,
    pub model_id: String,
    /// `downloading` / `verifying` / `extracting` / `installing` / `done` / `error` / `cancelling` / `cancelled`
    pub status: &'static str,
    pub received: u64,
    pub total: u64,
    pub current_file: Option<String>,
    /// 最近一段速度（字节/秒），进度条旁边展示。
    pub speed_bps: f64,
    pub files_done: usize,
    pub files_total: usize,
    /// 下载 Whisper 时是否顺带安装了 Silero VAD。
    pub vad_included: bool,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl JobSnapshot {
    pub fn is_active(&self) -> bool {
        matches!(self.status, "downloading" | "verifying" | "extracting" | "installing" | "cancelling")
    }
}

struct JobInner {
    snapshot: JobSnapshot,
    last_bytes: u64,
    last_at: Instant,
    speed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelRequest {
    Accepted,
    Locked,
    Finished,
}

/// 一个下载任务；取消标记与进度都在这里，任务结束后仍可在任务表里查到最终状态。
pub struct Job {
    inner: Mutex<JobInner>,
    cancel: AtomicBool,
    cancel_notify: tokio::sync::Notify,
}

impl Job {
    fn new(id: String, model_id: String, total: u64, files_total: usize, vad_included: bool) -> Self {
        Self {
            inner: Mutex::new(JobInner {
                snapshot: JobSnapshot {
                    id,
                    model_id,
                    status: "downloading",
                    received: 0,
                    total,
                    current_file: None,
                    speed_bps: 0.0,
                    files_done: 0,
                    files_total,
                    vad_included,
                    error: None,
                    started_at: crate::now_rfc3339(),
                    finished_at: None,
                },
                last_bytes: 0,
                last_at: Instant::now(),
                speed: 0.0,
            }),
            cancel: AtomicBool::new(false),
            cancel_notify: tokio::sync::Notify::new(),
        }
    }

    pub fn snapshot(&self) -> JobSnapshot {
        self.inner.lock().unwrap_or_else(|error| error.into_inner()).snapshot.clone()
    }

    fn update(&self, apply: impl FnOnce(&mut JobInner)) {
        apply(&mut self.inner.lock().unwrap_or_else(|error| error.into_inner()));
    }

    fn set_status(&self, status: &'static str) {
        self.update(|inner| inner.snapshot.status = status);
    }

    fn set_current_file(&self, file: Option<&str>) {
        self.update(|inner| inner.snapshot.current_file = file.map(str::to_string));
    }

    fn set_received(&self, value: u64) {
        self.update(|inner| {
            let received = value.min(inner.snapshot.total);
            inner.snapshot.received = received;
            let now = Instant::now();
            let elapsed = now.duration_since(inner.last_at);
            let moved = received.saturating_sub(inner.last_bytes);
            if elapsed >= Duration::from_millis(250) && moved > 0 {
                inner.speed = moved as f64 / elapsed.as_secs_f64();
                inner.last_bytes = received;
                inner.last_at = now;
                inner.snapshot.speed_bps = inner.speed;
            }
        });
    }

    /// 每个文件开始/结束时重置速度窗口，避免把上一个文件的字节算进这一段。
    fn reset_speed_meter(&self) {
        self.update(|inner| {
            inner.last_bytes = inner.snapshot.received;
            inner.last_at = Instant::now();
            inner.speed = 0.0;
            inner.snapshot.speed_bps = 0.0;
        });
    }

    fn finish_file(&self) {
        self.update(|inner| {
            inner.snapshot.files_done += 1;
            inner.snapshot.current_file = None;
        });
    }

    fn finish(&self, status: &'static str, error: Option<String>) {
        self.update(|inner| {
            inner.snapshot.status = status;
            inner.snapshot.error = error;
            inner.snapshot.finished_at = Some(crate::now_rfc3339());
            inner.snapshot.speed_bps = 0.0;
            inner.snapshot.current_file = None;
        });
    }

    fn request_cancel(&self) -> CancelRequest {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        match inner.snapshot.status {
            "downloading" | "verifying" => {
                self.cancel.store(true, Ordering::Relaxed);
                inner.snapshot.status = "cancelling";
                CancelRequest::Accepted
            }
            "extracting" | "installing" => CancelRequest::Locked,
            _ => CancelRequest::Finished,
        }
    }

    /// 在同一把状态锁内完成「确认未取消」和「进入不可取消安装阶段」。
    fn begin_install(&self, status: &'static str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if self.cancel.load(Ordering::Relaxed) {
            return false;
        }
        inner.snapshot.status = status;
        true
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 开始下载时的业务错误：`status` 决定 HTTP 状态码（400 参数/状态不对，409 已有任务）。
#[derive(Debug, Clone)]
pub struct StartError {
    pub status: u16,
    pub message: String,
}

impl StartError {
    fn bad(message: impl Into<String>) -> Self {
        Self { status: 400, message: message.into() }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self { status: 409, message: message.into() }
    }
}

/// 删除失败的业务状态；边界层直接映射 HTTP，不靠错误字符串猜状态码。
#[derive(Debug, Clone)]
pub struct DeleteError {
    pub status: u16,
    pub message: String,
}

impl DeleteError {
    fn bad(message: impl Into<String>) -> Self {
        Self { status: 400, message: message.into() }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self { status: 409, message: message.into() }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self { status: 500, message: message.into() }
    }
}

struct ManagerInner {
    jobs: HashMap<String, Arc<Job>>,
    current: Option<String>,
    latest: Option<String>,
}

/// 进程内模型下载管理器（挂在 `CoreState::model_manager`）。
pub struct ModelManager {
    models_dir: PathBuf,
    inner: Mutex<ManagerInner>,
    /// 串行化启动与删除，保证「检查无任务」和「登记新任务」之间没有窗口。
    operation: Mutex<()>,
}

impl ModelManager {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            inner: Mutex::new(ManagerInner { jobs: HashMap::new(), current: None, latest: None }),
            operation: Mutex::new(()),
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn lock_operation(&self) -> std::sync::MutexGuard<'_, ()> {
        self.operation.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn latest_snapshot(&self) -> Option<JobSnapshot> {
        let latest = self.lock().latest.clone()?;
        self.job(&latest)
    }

    pub fn job(&self, id: &str) -> Option<JobSnapshot> {
        self.lock().jobs.get(id).map(|job| job.snapshot())
    }

    /// `/api/models` 的响应：清单项 + 安装状态 + 最近一次下载任务。
    pub fn list(&self, source: DownloadSource) -> Value {
        let models: Vec<Value> = catalog::MODELS
            .iter()
            .map(|spec| {
                let status = catalog::status(spec, &self.models_dir);
                json!({
                    "id": spec.id,
                    "engine": spec.engine.as_str(),
                    "engine_label": spec.engine.label(),
                    "name": spec.name,
                    "title": spec.title,
                    "description": spec.description,
                    "config_key": spec.config_key,
                    "select_value": spec.select_value,
                    "status": status.state,
                    "installed": status.state == "installed",
                    "partial": status.state == "partial",
                    "download_size": spec.download_size(),
                    "installed_bytes": status.installed_bytes,
                    "dir": status.primary.as_ref().map(|path| path.display().to_string()),
                    "target": spec.target_path(&self.models_dir).display().to_string(),
                    "mirror_supported": spec.mirror_supported(),
                    "official_only": !spec.mirror_supported(),
                    "archive": spec.package,
                })
            })
            .collect();
        json!({
            "source": source.as_str(),
            "models_dir": self.models_dir.display().to_string(),
            "models": models,
            "job": self.latest_snapshot(),
        })
    }

    /// 起一个后台下载任务；同一时刻只允许一个（大文件争抢磁盘与带宽）。
    pub fn start(self: &Arc<Self>, model_id: &str, source: DownloadSource) -> Result<JobSnapshot, StartError> {
        let _operation = self.lock_operation();
        let spec = catalog::find(model_id).ok_or_else(|| StartError::bad(format!("未知的模型：{model_id}")))?;
        let status = catalog::status(spec, &self.models_dir);
        if status.state == "installed" {
            return Err(StartError::bad(format!("{} 已安装", spec.title)));
        }
        let mut plan: Vec<PlanItem> = spec.files.iter().map(|file| PlanItem { file, vad: false }).collect();
        let mut vad_included = false;
        if spec.engine == Engine::Whisper && !catalog::vad_installed(&self.models_dir) {
            let vad = catalog::vad_spec();
            plan.extend(vad.files.iter().map(|file| PlanItem { file, vad: true }));
            vad_included = true;
        }
        let total = plan.iter().map(|item| item.file.size).sum();
        let id = format!("job-{}", uuid::Uuid::new_v4().simple());
        let job = Arc::new(Job::new(id.clone(), spec.id.to_string(), total, plan.len(), vad_included));
        {
            let mut inner = self.lock();
            if let Some(current) = inner.current.as_deref().and_then(|id| inner.jobs.get(id)) {
                let snapshot = current.snapshot();
                if snapshot.is_active() {
                    return Err(StartError::conflict(format!(
                        "已有下载任务在运行（{}），请先等它完成或取消",
                        snapshot.model_id
                    )));
                }
            }
            inner.jobs.insert(id.clone(), job.clone());
            inner.current = Some(id.clone());
            inner.latest = Some(id.clone());
            // 只留最近 20 个任务，避免长期运行时任务表无限增长
            if inner.jobs.len() > 20 {
                let mut ordered: Vec<(String, String)> =
                    inner.jobs.iter().map(|(id, job)| (id.clone(), job.snapshot().started_at)).collect();
                ordered.sort_by(|left, right| left.1.cmp(&right.1));
                for (old_id, _) in ordered.into_iter().take(inner.jobs.len() - 20) {
                    inner.jobs.remove(&old_id);
                }
            }
        }
        let snapshot = job.snapshot();
        let manager = self.clone();
        let spec: &'static ModelSpec = spec;
        tokio::spawn(async move {
            run_job(manager.clone(), job.clone(), spec, source, plan).await;
            let mut inner = manager.lock();
            if inner.current.as_deref() == Some(job.snapshot().id.as_str()) {
                inner.current = None;
            }
        });
        Ok(snapshot)
    }

    /// 取消任务：只置标记，后台任务在下一个数据块前退出，已下字节保留以便续传。
    pub fn cancel(&self, job_id: &str) -> Result<JobSnapshot, StartError> {
        let job = self
            .lock()
            .jobs
            .get(job_id)
            .cloned()
            .ok_or_else(|| StartError { status: 404, message: format!("没有这个下载任务：{job_id}") })?;
        match job.request_cancel() {
            CancelRequest::Locked => {
                return Err(StartError::conflict("模型已进入解压或安装阶段，完成后才能继续操作"));
            }
            CancelRequest::Finished => return Ok(job.snapshot()),
            CancelRequest::Accepted => {}
        }
        job.cancel_notify.notify_one();
        Ok(job.snapshot())
    }

    /// 删除已安装模型；校验通过后才调用 `prepare` 卸载正在占文件的引擎。
    pub fn delete(&self, model_id: &str, prepare: impl FnOnce(Engine)) -> Result<(), DeleteError> {
        let _operation = self.lock_operation();
        let spec = catalog::find(model_id).ok_or_else(|| DeleteError::bad(format!("未知的模型：{model_id}")))?;
        {
            let inner = self.lock();
            if let Some(job) = inner.current.as_deref().and_then(|id| inner.jobs.get(id)) {
                let job = job.snapshot();
                if job.is_active()
                    && (job.model_id == spec.id || (job.vad_included && spec.id == catalog::vad_spec().id))
                {
                    return Err(DeleteError::conflict("模型正在下载，请先取消或等它完成"));
                }
            }
        }
        let paths = catalog::existing_paths(spec, &self.models_dir);
        if paths.is_empty() {
            return Err(DeleteError::bad(format!("{} 未安装", spec.title)));
        }
        prepare(spec.engine);
        for path in &paths {
            if path.is_dir() {
                std::fs::remove_dir_all(path)
                    .map_err(|error| DeleteError::internal(format!("删除 {} 失败：{error}", path.display())))?;
            } else if path.is_file() {
                std::fs::remove_file(path)
                    .map_err(|error| DeleteError::internal(format!("删除 {} 失败：{error}", path.display())))?;
            }
        }
        let _ = std::fs::remove_dir_all(self.download_dir(spec));
        if spec.engine == Engine::Vad {
            let _ = std::fs::remove_dir_all(self.vad_download_dir());
        }
        Ok(())
    }

    fn download_dir(&self, spec: &ModelSpec) -> PathBuf {
        self.models_dir.join(".downloads").join(spec.id)
    }

    /// VAD 的续传目录由 Whisper 任务与单独下载共用，避免同一文件两套半成品。
    fn vad_download_dir(&self) -> PathBuf {
        self.models_dir.join(".downloads").join("_vad")
    }
}

#[derive(Clone, Copy)]
struct PlanItem {
    file: &'static FileSpec,
    vad: bool,
}

async fn run_job(
    manager: Arc<ModelManager>,
    job: Arc<Job>,
    spec: &'static ModelSpec,
    source: DownloadSource,
    plan: Vec<PlanItem>,
) {
    match execute(&manager, &job, spec, source, &plan).await {
        Ok(()) => {
            job.finish("done", None);
            tracing::info!("本地模型下载完成：{}", spec.id);
        }
        Err(JobError::Cancelled) => {
            job.finish("cancelled", None);
            tracing::info!("本地模型下载已取消：{}（已下载部分保留，可续传）", spec.id);
        }
        Err(JobError::Failed(error)) => {
            tracing::warn!("本地模型下载失败（{}）：{error}", spec.id);
            job.finish("error", Some(error));
        }
    }
}

#[derive(Debug)]
enum JobError {
    Cancelled,
    Failed(String),
}

impl From<String> for JobError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

async fn execute(
    manager: &ModelManager,
    job: &Job,
    spec: &'static ModelSpec,
    source: DownloadSource,
    plan: &[PlanItem],
) -> Result<(), JobError> {
    let models_dir = &manager.models_dir;
    let download_dir = manager.download_dir(spec);
    // 目录里的文件按文件名区分，重复使用即可；旧目录不删是为了续传
    std::fs::create_dir_all(&download_dir).map_err(|error| format!("无法创建下载目录：{error}"))?;
    let file_root = download_dir.join("files");
    std::fs::create_dir_all(&file_root).map_err(|error| format!("无法创建下载目录：{error}"))?;
    let vad_root = manager.vad_download_dir();
    let uses_vad_root = spec.engine == Engine::Vad || plan.iter().any(|item| item.vad);
    if uses_vad_root {
        std::fs::create_dir_all(&vad_root).map_err(|error| format!("无法创建下载目录：{error}"))?;
    }

    let mut completed = 0u64;
    for item in plan {
        if job.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        let dest = if item.vad || spec.engine == Engine::Vad {
            vad_root.join(item.file.path)
        } else if spec.single_file || spec.package {
            download_dir.join(item.file.path)
        } else {
            file_root.join(item.file.path)
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("无法创建下载目录：{error}"))?;
        }
        job.set_current_file(Some(item.file.path));
        let url = spec.file_url(item.file, source);
        let base = completed;
        let existing = std::fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0).min(item.file.size);
        job.set_received(base.saturating_add(existing));
        job.reset_speed_meter();
        fetch_file(&url, &dest, item.file.size, item.file.sha256, &job.cancel, &job.cancel_notify, |received| {
            job.set_received(base.saturating_add(received))
        })
        .await?;
        completed = completed.saturating_add(item.file.size);
        job.set_received(completed);
        job.reset_speed_meter();
        job.finish_file();
    }

    job.set_status("verifying");
    if job.is_cancelled() {
        return Err(JobError::Cancelled);
    }

    // 先装模型，再装 VAD：进入安装阶段后不可取消，避免留下模型与 VAD 不一致的半成品。
    if spec.single_file {
        if !job.begin_install("installing") {
            return Err(JobError::Cancelled);
        }
        let temp = if spec.engine == Engine::Vad {
            vad_root.join(spec.files[0].path)
        } else {
            download_dir.join(spec.files[0].path)
        };
        let target = spec.target_path(models_dir);
        tokio::task::spawn_blocking(move || replace_file(&temp, &target))
            .await
            .map_err(|error| format!("安装模型失败：{error}"))??;
    } else if spec.package {
        if !job.begin_install("extracting") {
            return Err(JobError::Cancelled);
        }
        let archive = download_dir.join(spec.files[0].path);
        let extract_dir = download_dir.join("extract");
        let required: Vec<&'static str> = spec.required.to_vec();
        let required_dir = spec.required_dir;
        let found = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            let _ = std::fs::remove_dir_all(&extract_dir);
            extract_tar_bz2(&archive, &extract_dir)?;
            catalog::find_packaged_dir(&extract_dir, &required, required_dir)
                .ok_or_else(|| "压缩包解压后没有找到完整的模型目录".to_string())
        })
        .await
        .map_err(|error| format!("解压失败：{error}"))??;
        let target = spec.target_path(models_dir);
        tokio::task::spawn_blocking(move || replace_dir(&found, &target))
            .await
            .map_err(|error| format!("安装模型失败：{error}"))??;
    } else {
        if !job.begin_install("installing") {
            return Err(JobError::Cancelled);
        }
        let target = spec.target_path(models_dir);
        tokio::task::spawn_blocking(move || replace_dir(&file_root, &target))
            .await
            .map_err(|error| format!("安装模型失败：{error}"))??;
    }

    // 一旦进入安装阶段就完成整组安装。若此时才收到取消，仍按完成收尾，
    // 避免留下“模型已就位但随附 VAD 缺失、再次下载却被判定已安装”的半成品。
    if plan.iter().any(|item| item.vad) {
        job.set_status("installing");
        let temp = vad_root.join(catalog::vad_spec().target);
        let target = models_dir.join(catalog::vad_spec().target);
        tokio::task::spawn_blocking(move || replace_file(&temp, &target))
            .await
            .map_err(|error| format!("安装 VAD 失败：{error}"))??;
    }
    let _ = std::fs::remove_dir_all(&download_dir);
    if uses_vad_root {
        let _ = std::fs::remove_dir_all(&vad_root);
    }
    Ok(())
}

/// 已验证的临时文件换到目标位置（Windows 上 rename 不能覆盖已有文件，先删）。
fn replace_file(temp: &Path, target: &Path) -> Result<(), String> {
    if !temp.is_file() {
        return Err(format!("临时文件不存在：{}", temp.display()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建模型目录：{error}"))?;
    }
    let backup = target.with_file_name(format!(
        "{}.bak-{}",
        target.file_name().and_then(|name| name.to_str()).unwrap_or("model"),
        uuid::Uuid::new_v4().simple()
    ));
    let had_target = target.exists();
    if had_target {
        std::fs::rename(target, &backup).map_err(|error| format!("无法替换旧文件：{error}"))?;
    }
    match std::fs::rename(temp, target) {
        Ok(()) => {
            if had_target {
                let _ = std::fs::remove_file(&backup);
            }
            Ok(())
        }
        Err(error) => {
            if had_target {
                let _ = std::fs::rename(&backup, target);
            }
            Err(format!("无法保存模型文件：{error}"))
        }
    }
}

/// 已校验的临时目录原子换到目标位置：先把旧目录改名备份，成功后删备份。
fn replace_dir(temp: &Path, target: &Path) -> Result<(), String> {
    if !temp.is_dir() {
        return Err(format!("临时目录不存在：{}", temp.display()));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建模型目录：{error}"))?;
    }
    let backup = target.with_file_name(format!(
        "{}.bak-{}",
        target.file_name().and_then(|name| name.to_str()).unwrap_or("model"),
        uuid::Uuid::new_v4().simple()
    ));
    let had_target = target.exists();
    if had_target {
        std::fs::rename(target, &backup).map_err(|error| format!("无法替换旧模型目录：{error}"))?;
    }
    match std::fs::rename(temp, target) {
        Ok(()) => {
            if had_target {
                let _ = std::fs::remove_dir_all(&backup);
            }
            Ok(())
        }
        Err(error) => {
            if had_target {
                let _ = std::fs::rename(&backup, target);
            }
            Err(format!("无法保存模型目录：{error}"))
        }
    }
}

/// 解压 `.tar.bz2`；逐条校验路径，拒绝绝对路径与 `..`（防压缩包目录穿越）。
pub fn extract_tar_bz2(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|error| format!("无法创建解压目录：{error}"))?;
    let file = std::fs::File::open(archive).map_err(|error| format!("无法打开压缩包：{error}"))?;
    let decoder = bzip2::read::BzDecoder::new(BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().map_err(|error| format!("无法读取压缩包：{error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("压缩包损坏：{error}"))?;
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(format!("压缩包内含不支持的条目类型：{entry_type:?}"));
        }
        let path = entry.path().map_err(|error| format!("压缩包内文件名非法：{error}"))?.to_path_buf();
        let unsafe_path = path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)));
        if unsafe_path {
            return Err(format!("压缩包内含不安全路径：{}", path.display()));
        }
        let unpacked = entry.unpack_in(dest).map_err(|error| format!("解压失败：{error}"))?;
        if !unpacked {
            return Err(format!("压缩包内含不安全路径：{}", path.display()));
        }
    }
    Ok(())
}

/// 单文件下载（可续传），完成后校验大小与 SHA-256；坏文件删除，避免下次带着坏尾巴续传。
async fn fetch_file(
    url: &str,
    dest: &Path,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    cancel_notify: &tokio::sync::Notify,
    mut on_received: impl FnMut(u64),
) -> Result<(), JobError> {
    let mut existing = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    if existing > size {
        let _ = std::fs::remove_file(dest);
        existing = 0;
    }
    if existing == size && size > 0 {
        if verify_file(dest, sha256, cancel).await? {
            on_received(existing);
            return Ok(());
        }
        let _ = std::fs::remove_file(dest);
        existing = 0;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }

    // 从 0 开始也显式发 Range：Hugging Face / GitHub Release 都会返回 206，
    // 与续传走同一条服务端路径；不支持 Range 的源仍会回落为 200。
    let request = CLIENT.get(url).header(reqwest::header::RANGE, format!("bytes={existing}-"));
    let send_cancelled = cancel_notify.notified();
    tokio::pin!(send_cancelled);
    let mut response = tokio::select! {
        _ = &mut send_cancelled => return Err(JobError::Cancelled),
        response = request.send() => response.map_err(|error| JobError::Failed(describe_request_error(error)))?,
    };
    let status = response.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && existing == size {
        if verify_file(dest, sha256, cancel).await? {
            on_received(existing);
            return Ok(());
        }
        let _ = std::fs::remove_file(dest);
        return Err(JobError::Failed("已下载文件校验失败，已删除，请重新下载".into()));
    }
    let partial = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let append = partial && existing > 0;
    if append {
        let range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| JobError::Failed("续传响应缺少 Content-Range，已保留临时文件".into()))?;
        let starts_at_expected = range
            .strip_prefix("bytes ")
            .and_then(|value| value.split_once('-'))
            .and_then(|(start, _)| start.parse::<u64>().ok())
            == Some(existing);
        if !starts_at_expected {
            return Err(JobError::Failed(format!("续传响应范围不正确（{range}），已保留临时文件")));
        }
    }
    if !append && existing > 0 {
        // 服务器没按 Range 返回 206（或不支持续传）：从头写，避免旧字节拼坏文件
        existing = 0;
    }
    if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(JobError::Failed(format!("下载地址返回 HTTP {}", status.as_u16())));
    }
    if let Some(total) = response.content_length() {
        if !append && total != size && total > 0 {
            tracing::debug!("模型下载：服务器 Content-Length {} 与清单 {} 不一致（以实际完成为准）", total, size);
        }
    }

    let mut file = if append {
        std::fs::OpenOptions::new()
            .append(true)
            .open(dest)
            .map_err(|error| JobError::Failed(format!("无法写入模型文件：{error}")))?
    } else {
        std::fs::File::create(dest).map_err(|error| JobError::Failed(format!("无法写入模型文件：{error}")))?
    };
    let mut received = existing;
    on_received(received);
    let read_cancelled = cancel_notify.notified();
    tokio::pin!(read_cancelled);
    loop {
        if cancel.load(Ordering::Relaxed) {
            use std::io::Write;
            let _ = file.flush();
            return Err(JobError::Cancelled);
        }
        let maybe_chunk = tokio::select! {
            _ = &mut read_cancelled => return Err(JobError::Cancelled),
            chunk = response.chunk() => chunk.map_err(|error| JobError::Failed(describe_request_error(error)))?,
        };
        let Some(chunk) = maybe_chunk else {
            break;
        };
        use std::io::Write;
        file.write_all(&chunk).map_err(|error| JobError::Failed(format!("无法写入模型文件：{error}")))?;
        received = received.saturating_add(chunk.len() as u64);
        on_received(received);
    }
    {
        use std::io::Write;
        file.flush().map_err(|error| JobError::Failed(format!("无法写入模型文件：{error}")))?;
    }
    drop(file);
    if received != size {
        return Err(JobError::Failed(format!("下载不完整：{} / {} 字节（已保留，可续传）", received, size)));
    }
    if !verify_file(dest, sha256, cancel).await? {
        let _ = std::fs::remove_file(dest);
        return Err(JobError::Failed("SHA-256 校验失败，文件已删除；请重新下载".into()));
    }
    Ok(())
}

async fn verify_file(path: &Path, expected: &str, cancel: &AtomicBool) -> Result<bool, JobError> {
    use tokio::io::AsyncReadExt;

    let mut file =
        tokio::fs::File::open(path).await.map_err(|error| JobError::Failed(format!("无法读取下载文件：{error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let read =
            file.read(&mut buffer).await.map_err(|error| JobError::Failed(format!("无法读取下载文件：{error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    Ok(actual.eq_ignore_ascii_case(expected))
}

fn describe_request_error(error: reqwest::Error) -> String {
    let timeout = error.is_timeout();
    let connect = error.is_connect();
    let error = error.without_url();
    if timeout {
        "下载超时，请检查网络或代理设置（已下载部分可续传）".to_string()
    } else if connect {
        format!("无法连接下载源，请检查网络或代理设置：{error}")
    } else {
        format!("下载失败：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-model-download-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn 同一时刻只允许一个下载且活动任务不能删除() {
        let dir = temp_dir("single-job");
        let manager = Arc::new(ModelManager::new(dir.clone()));
        let first = manager.start("whisper-tiny", DownloadSource::Official).unwrap();
        let second = manager.start("whisper-base", DownloadSource::Official).unwrap_err();
        assert_eq!(second.status, 409);
        let delete = manager.delete("whisper-tiny", |_| {}).unwrap_err();
        assert_eq!(delete.status, 409);
        assert!(delete.message.contains("正在下载"));

        manager.cancel(&first.id).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if manager.job(&first.id).is_some_and(|job| !job.is_active()) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("取消后的下载任务应结束");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 未安装模型不会先释放引擎() {
        let dir = temp_dir("delete-missing");
        let manager = ModelManager::new(dir.clone());
        let prepared = std::cell::Cell::new(false);
        let error = manager.delete("whisper-small", |_| prepared.set(true)).unwrap_err();
        assert_eq!(error.status, 400);
        assert!(!prepared.get(), "校验安装状态前不能卸载引擎");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 删除未完成下载会清理暂存目录() {
        let dir = temp_dir("delete-staging");
        let staging = dir.join(".downloads").join("whisper-small");
        std::fs::create_dir_all(staging.join("files")).unwrap();
        std::fs::write(staging.join("files").join("model.bin"), vec![0u8; 16]).unwrap();
        let manager = ModelManager::new(dir.clone());
        manager.delete("whisper-small", |_| {}).unwrap();
        assert!(!staging.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 解压和安装阶段不能再取消() {
        let dir = temp_dir("cancel-installing");
        let manager = ModelManager::new(dir.clone());
        for status in ["extracting", "installing"] {
            let job = Arc::new(Job::new("job-fixed".into(), "qwen3-asr-0.6b-int8".into(), 10, 1, false));
            job.set_status(status);
            {
                let mut inner = manager.lock();
                inner.current = Some("job-fixed".into());
                inner.latest = Some("job-fixed".into());
                inner.jobs.insert("job-fixed".into(), job);
            }
            let error = manager.cancel("job-fixed").unwrap_err();
            assert_eq!(error.status, 409);
            assert!(error.message.contains("解压或安装"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 极简 HTTP 桩：一次请求，支持 Range（返回 206），记录收到的 Range 头。
    fn spawn_http_stub(body: Vec<u8>, ranges: Arc<Mutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request);
            let range = text
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("range:"))
                .map(|line| line.split_once(':').unwrap().1.trim().to_string());
            if let Some(range) = range.clone() {
                ranges.lock().unwrap_or_else(|error| error.into_inner()).push(range);
            }
            let start = range
                .as_deref()
                .and_then(|value| value.strip_prefix("bytes="))
                .and_then(|value| value.strip_suffix('-'))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0)
                .min(body.len());
            let slice = &body[start..];
            let (status, extra) = if start > 0 {
                ("206 Partial Content", format!("Content-Range: bytes {}-{}/{}\r\n", start, body.len() - 1, body.len()))
            } else {
                ("200 OK", String::new())
            };
            let head =
                format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", slice.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(slice).unwrap();
            let _ = stream.flush();
        });
        format!("http://{addr}/model.bin")
    }

    #[tokio::test]
    async fn 下载_断点续传与哈希通过() {
        let dir = temp_dir("resume");
        let body = b"hello world, this is a test model".to_vec();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_http_stub(body.clone(), ranges.clone());
        let dest = dir.join("model.bin");
        std::fs::write(&dest, &body[..10]).unwrap();
        let cancel = AtomicBool::new(false);
        let hash = sha256_hex(&body);
        fetch_file(&url, &dest, body.len() as u64, &hash, &cancel, &tokio::sync::Notify::new(), |_| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(ranges.lock().unwrap()[0], "bytes=10-");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 下载_哈希不匹配时删除坏文件() {
        let dir = temp_dir("badhash");
        let body = b"corrupted model bytes".to_vec();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_http_stub(body.clone(), ranges);
        let dest = dir.join("model.bin");
        let cancel = AtomicBool::new(false);
        let error =
            fetch_file(&url, &dest, body.len() as u64, &"0".repeat(64), &cancel, &tokio::sync::Notify::new(), |_| {})
                .await
                .unwrap_err();
        assert!(matches!(error, JobError::Failed(_)));
        assert!(!dest.exists(), "校验失败的文件不能留在下载目录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 下载_取消时不继续写() {
        let dir = temp_dir("cancel");
        let dest = dir.join("model.bin");
        let cancel = AtomicBool::new(true);
        let error = fetch_file(
            "http://127.0.0.1:1/never",
            &dest,
            4,
            &"0".repeat(64),
            &cancel,
            &tokio::sync::Notify::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, JobError::Cancelled));
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tar_bz2(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// 直接写 GNU header 的 name 字段：tar crate 的 `append_data` 会拒绝 `..`，
    /// 这里用来构造真实世界里可能出现的恶意压缩包。
    fn tar_bz2_raw_name(name: &[u8], body: &[u8]) -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        {
            let gnu = header.as_gnu_mut().unwrap();
            gnu.name[..name.len()].copy_from_slice(name);
        }
        header.set_cksum();
        builder.append(&header, body).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn 解压_保留目录结构() {
        let dir = temp_dir("extract");
        let archive = dir.join("model.tar.bz2");
        std::fs::write(&archive, tar_bz2(&[("pkg/tokenizer/a.json", b"{}")])).unwrap();
        let dest = dir.join("out");
        extract_tar_bz2(&archive, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("pkg/tokenizer/a.json")).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 解压_拒绝目录穿越() {
        let dir = temp_dir("slip");
        let archive = dir.join("evil.tar.bz2");
        std::fs::write(&archive, tar_bz2_raw_name(b"../evil.txt", b"x")).unwrap();
        let dest = dir.join("out");
        let error = extract_tar_bz2(&archive, &dest).unwrap_err();
        assert!(error.contains("不安全路径"), "{error}");
        assert!(!dir.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
