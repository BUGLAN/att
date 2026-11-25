//! 本地文件转写（移植自 Python `app/services/file_transcription_service.py`）。
//!
//! - POST /api/transcribe：缓存命中秒回；短文件同步 200；长文件、开启 LLM 校对或请求 `async_job` → 202 任务号；
//! - 后台任务可取消（`cancel_job`）：解码、分块识别、校对窗口之间检查取消令牌，取消后状态为 `cancelled`、
//!   不再写缓存并释放单并发闸门；识别阶段的 `progress` 为 `{stage: "asr", done_seconds, total_seconds}`，
//!   校对阶段为 `{window_done, window_total}`；
//! - 错误码契约：404 文件不存在 / 415 无法解码 / 413 超时长上限 / 409 引擎或校对不可用 /
//!   503 引擎忙（Retry-After）/ 500 其他；
//! - 转写完成存进记录集：无音频的文本记录，`source_file` 是原文件名；记录 id 记在缓存引擎桶的 `record_id`，
//!   同一文件同一引擎只建一条（记录还在就只更新校对稿，被删了才重建），响应里带 `record_id`；
//! - 缓存 `data/transcribe_file_cache.json`（与 Python 同格式：sha256 → engines → 引擎桶 → 校对变体）；
//! - 引擎：豆包（在线，ffmpeg 解码为 16k PCM 后按会话上限分段推流）；本地 Whisper（faster-whisper）
//!   与本地 Qwen3-ASR（sherpa-onnx ONNX）走 [`crate::stt::transcribe_local`]，分段时间戳原样返回。

pub mod media;
pub mod proofread;

use crate::state::CoreState;
use crate::storage::json_store;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OwnedMutexGuard;

const MAX_DURATION_SECONDS: f64 = 7200.0;
const ASYNC_THRESHOLD_SECONDS: f64 = 1800.0;
const BUSY_RETRY_AFTER_SECONDS: u64 = 30;
const CACHE_SIZE: usize = 200;
/// 豆包单会话时长上限（超长分段，保守 30 分钟，与 Python 默认一致）
const DOUBAO_SESSION_SECONDS: usize = 1800;
const JOB_KEEP_SECONDS: f64 = 30.0 * 60.0;
const JOB_MAX_ENTRIES: usize = 100;
const PROOFREAD_VARIANTS_PER_ENGINE: usize = 8;
const SAMPLE_RATE: usize = 16_000;

#[derive(Debug, Clone)]
pub struct TranscribeError {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<u64>,
}

impl TranscribeError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), retry_after: None }
    }

    fn busy(message: &str) -> Self {
        Self { status: 503, message: message.into(), retry_after: Some(BUSY_RETRY_AFTER_SECONDS) }
    }

    /// 任务被用户取消（只出现在后台任务内部，不作为 HTTP 响应返回）。
    fn cancelled() -> Self {
        Self::new(499, crate::stt::TRANSCRIBE_CANCELLED)
    }
}

/// 后台转写任务的取消令牌：取消接口置位；引擎在分块之间查 [`Self::is_cancelled`]，
/// 解码、在线识别、校对这些异步等待处用 [`Self::cancelled`] 直接打断。
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// 等到被取消为止（已取消时立即返回）。
    pub async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        // 先登记等待再查标志：避免查完标志、登记之前恰好发生的取消被漏掉
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }

    /// 跑一个异步步骤，取消时立即放弃它（drop 掉 future：ffmpeg 子进程随之结束、在线连接随之关闭）。
    async fn run<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, TranscribeError>>,
    ) -> Result<T, TranscribeError> {
        tokio::select! {
            // 先看取消：已取消时不再让工作跑一步
            biased;
            _ = self.cancelled() => Err(TranscribeError::cancelled()),
            outcome = work => outcome,
        }
    }
}

/// 识别阶段的钩子：取消令牌 + 进度上报（已识别到的音频秒数）。同步请求用 [`AsrHook::none`]。
#[derive(Clone)]
struct AsrHook {
    token: CancelToken,
    report: Arc<dyn Fn(f64) + Send + Sync>,
}

impl AsrHook {
    fn none() -> Self {
        Self { token: CancelToken::default(), report: Arc::new(|_| {}) }
    }

    /// 上报进度并返回是否继续（分块回调用）。
    fn tick(&self, done_seconds: f64) -> bool {
        (self.report)(done_seconds);
        !self.token.is_cancelled()
    }
}

impl From<media::MediaError> for TranscribeError {
    fn from(error: media::MediaError) -> Self {
        match error {
            media::MediaError::ToolMissing(_) => Self::new(500, error.to_string()),
            media::MediaError::Undecodable(message) => Self::new(415, message),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub path: String,
    pub language: String,
    pub want_segments: bool,
    pub engine: String,
    pub proofread: bool,
    pub proofread_model_id: Option<String>,
    pub proofread_options: Option<Value>,
    /// 要求一律走后台任务（缓存命中仍秒回）：前端需要进度与取消时打开
    pub async_job: bool,
}

pub enum Outcome {
    /// 同步结果（含缓存命中）
    Result(Value),
    /// 已受理的异步任务号（202）
    Job(String),
}

struct Job {
    status: &'static str,
    stage: &'static str,
    progress: Option<Value>,
    result: Option<Value>,
    error: String,
    created_at: f64,
    cancel: CancelToken,
}

impl Job {
    fn snapshot(&self) -> Value {
        json!({
            "status": self.status,
            "stage": self.stage,
            "progress": self.progress,
            "result": self.result,
            "error": self.error,
            "cancel_requested": self.status == "running" && self.cancel.is_cancelled(),
        })
    }
}

#[derive(Default)]
pub struct FileTranscriber {
    jobs: Mutex<HashMap<String, Job>>,
    /// 单并发闸门（引擎/上游配额有限；忙碌时 503）。
    ///
    /// 受理异步任务时必须把 guard 交给后台任务持有，不能在 `start_job` 返回时释放；
    /// 否则并发请求会重复通过受理，随后真正执行时才以「引擎忙」失败。
    busy: Arc<tokio::sync::Mutex<()>>,
    /// 缓存（懒加载）；None = 尚未读盘
    cache: Mutex<Option<Map<String, Value>>>,
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn variant_key(model_id: Option<&str>) -> String {
    format!("{}@{}", model_id.unwrap_or("default"), proofread::PROMPT_VERSION)
}

/// 引擎解析：显式 > auto（本地优先，其次豆包、DeepSeek、豆包网页）；本地模型/推理库缺失、在线凭据缺失 → 409。
///
/// 本地引擎只做**文件是否就位**的探测，不触发模型加载（加载发生在真正转写时）。
fn resolve_engine(core: &CoreState, requested: &str) -> Result<&'static str, TranscribeError> {
    use crate::stt::local::LocalKind;

    let whisper = core.local_models.status(core, LocalKind::Whisper);
    let qwen = core.local_models.status(core, LocalKind::Qwen);
    let sense_voice = core.local_models.status(core, LocalKind::SenseVoice);
    let doubao_ready = crate::stt::doubao_config(core).ensure().is_ok();
    let doubao_web_ready = crate::stt::doubao_web_config(core).ensure().is_ok();
    let doubao_ime_ready = crate::stt::doubao_ime_config(core).ensure().is_ok();
    let deepseek_ready = crate::stt::deepseek_config(core).ensure().is_ok();
    let wetype_ready = crate::stt::Engine::online_for_provider(core, "wetype_asr").is_ok();
    // 只给一句结论 + 去哪下载：完整目录与文件要求仍放在设置页折叠详情里
    let unavailable = |kind: LocalKind, status: &crate::stt::local::LocalStatus| {
        let summary = status.error.clone().unwrap_or_else(|| format!("{}不可用", kind.label()));
        TranscribeError::new(409, format!("{summary}，可在「设置 → 识别引擎」下载模型"))
    };

    match requested.trim().to_lowercase().as_str() {
        // auto = 本地优先（配置指向本地引擎就用它，否则回落本地 Whisper / Qwen / SenseVoice），
        // 最后才用在线引擎：豆包 → DeepSeek → 豆包网页（后者无需凭据）
        "auto" | "" => {
            if let Some(kind) = crate::stt::local_kind(&core.settings().asr_provider) {
                let ready = match kind {
                    LocalKind::Whisper => whisper.available,
                    LocalKind::Qwen => qwen.available,
                    LocalKind::SenseVoice => sense_voice.available,
                };
                if ready {
                    return Ok(kind.provider());
                }
            }
            if whisper.available {
                Ok("whisper")
            } else if qwen.available {
                Ok("qwen_asr")
            } else if sense_voice.available {
                Ok("sense_voice")
            } else if doubao_ready {
                Ok("doubao_asr")
            } else if deepseek_ready {
                Ok("deepseek_asr")
            } else if doubao_web_ready {
                Ok("doubao_web_asr")
            } else if doubao_ime_ready {
                Ok("doubao_ime_asr")
            } else if wetype_ready {
                Ok("wetype_asr")
            } else {
                Err(TranscribeError::new(
                    409,
                    "当前没有可用的转写引擎：本地模型未就位，豆包、DeepSeek 凭据未配置，豆包网页不可用，也未检测到微信输入法",
                ))
            }
        }
        "whisper" if whisper.available => Ok("whisper"),
        "whisper" => Err(unavailable(LocalKind::Whisper, &whisper)),
        "qwen_asr" if qwen.available => Ok("qwen_asr"),
        "qwen_asr" => Err(unavailable(LocalKind::Qwen, &qwen)),
        "sense_voice" if sense_voice.available => Ok("sense_voice"),
        "sense_voice" => Err(unavailable(LocalKind::SenseVoice, &sense_voice)),
        "doubao_asr" if doubao_ready => Ok("doubao_asr"),
        "doubao_asr" => Err(TranscribeError::new(409, "豆包语音未配置 Access Token，请在「设置 → 识别引擎」中填写")),
        "doubao_web_asr" if doubao_web_ready => Ok("doubao_web_asr"),
        "doubao_web_asr" => Err(TranscribeError::new(409, "豆包网页语音当前不可用，请稍后重试")),
        "doubao_ime_asr" if doubao_ime_ready => Ok("doubao_ime_asr"),
        "doubao_ime_asr" => Err(TranscribeError::new(409, "豆包输入法语音当前不可用，请稍后重试")),
        "deepseek_asr" if deepseek_ready => Ok("deepseek_asr"),
        "deepseek_asr" => {
            Err(TranscribeError::new(409, "DeepSeek 语音识别未配置 API Key，请在「设置 → 识别引擎」中填写"))
        }
        "wetype_asr" => {
            #[cfg(windows)]
            {
                if wetype_ready {
                    Ok("wetype_asr")
                } else {
                    Err(TranscribeError::new(
                        409,
                        crate::stt::wetype_config(core).ensure().err().unwrap_or_else(|| "请先启动微信输入法".into()),
                    ))
                }
            }
            #[cfg(not(windows))]
            {
                Err(TranscribeError::new(409, "微信输入法语音仅支持 Windows"))
            }
        }
        other => Err(TranscribeError::new(
            422,
            format!(
                "不支持的 engine：{other}（可选 auto/whisper/qwen_asr/sense_voice/doubao_asr/doubao_web_asr/doubao_ime_asr/deepseek_asr/wetype_asr）"
            ),
        )),
    }
}

/// 路径校验：绝对路径 → canonicalize → 文件存在 → 扩展名可识别。
///
/// 数据面以 `X-ATT-Token` 作为唯一授权（路由层在进入这里之前已经校验）：令牌有效即可转写
/// 任意可读的本地音视频文件，不再要求文件在界面里登记过或位于配置的目录内。界面里的
/// 文件白名单只用于防误操作，不参与这里的判断。
fn validate_path(raw: &str) -> Result<PathBuf, TranscribeError> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Err(TranscribeError::new(404, "缺少文件路径"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(TranscribeError::new(404, format!("必须是本地绝对路径：{raw}")));
    }
    // canonicalize 同时消掉 `..` 与符号链接；不存在 / 无权限都按「文件不存在」返回
    let path = std::fs::canonicalize(&path)
        .map(crate::state::tidy)
        .map_err(|_| TranscribeError::new(404, format!("文件不存在：{raw}")))?;
    if !path.is_file() {
        return Err(TranscribeError::new(404, format!("文件不存在：{raw}")));
    }
    if !media::is_supported_extension(&path) {
        let suffix =
            path.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_else(|| "无后缀".into());
        return Err(TranscribeError::new(415, format!("无法解码：不支持的文件类型「{suffix}」")));
    }
    Ok(path)
}

async fn content_hash(path: &Path) -> Result<String, TranscribeError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            digest.update(&buffer[..n]);
        }
        Ok(crate::clipboard::hex(&digest.finalize()))
    })
    .await
    .map_err(|e| TranscribeError::new(500, e.to_string()))?
    .map_err(|e| TranscribeError::new(404, format!("读取文件失败：{e}")))
}

/// 按接口契约整形响应：text 恒返回，segments 仅在需要时返回；校对变体命中时换成校对版。
fn shape_payload(entry: &Value, want_segments: bool, cached: bool, variant: Option<&Value>) -> Value {
    let mut payload = json!({
        "text": entry.get("text").and_then(Value::as_str).unwrap_or(""),
        "duration_seconds": entry.get("duration_seconds").and_then(Value::as_f64).unwrap_or(0.0),
        "engine": entry.get("engine").and_then(Value::as_str).unwrap_or(""),
        "cached": cached,
    });
    if let Some(billing) = entry.get("billing").filter(|b| b.is_object()) {
        payload["billing"] = billing.clone();
    }
    for key in ["raw_text", "proofread", "warnings"] {
        if let Some(value) = entry.get(key) {
            payload[key] = value.clone();
        }
    }
    let mut segments = entry.get("segments").cloned().unwrap_or_else(|| json!([]));
    if let Some(variant) = variant {
        payload["raw_text"] = payload["text"].clone();
        if let Some(text) = variant.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            payload["text"] = json!(text);
        }
        if let Some(variant_segments) =
            variant.get("segments").filter(|s| s.as_array().map(|a| !a.is_empty()).unwrap_or(false))
        {
            segments = variant_segments.clone();
        }
        payload["proofread"] = json!({
            "applied": true,
            "llm_model": variant.get("model_label").and_then(Value::as_str).filter(|s| !s.is_empty())
                .or_else(|| variant.get("model_id").and_then(Value::as_str)).unwrap_or("default"),
            "windows": variant.get("windows").and_then(Value::as_i64).unwrap_or(0),
            "requests": variant.get("requests").and_then(Value::as_i64).unwrap_or(0),
            "fallback_batches": variant.get("fallback_batches").cloned().unwrap_or_else(|| json!([])),
            "prompt_version": proofread::PROMPT_VERSION,
        });
    }
    if want_segments {
        payload["segments"] = segments;
    }
    payload
}

impl FileTranscriber {
    /// 是否有转写在跑（同步或异步任务持有并发闸门）：删除模型前据此拒绝。
    pub fn is_busy(&self) -> bool {
        self.busy.try_lock().is_err()
    }

    fn cache_path(core: &CoreState) -> PathBuf {
        core.data_dir.join("transcribe_file_cache.json")
    }

    fn with_cache<T>(&self, core: &CoreState, f: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let cache =
            guard.get_or_insert_with(|| match json_store::read_json_lenient::<Value>(&Self::cache_path(core)) {
                Value::Object(map) => map.into_iter().filter(|(_, v)| v.is_object()).collect(),
                _ => Map::new(),
            });
        f(cache)
    }

    fn save_cache(core: &CoreState, cache: &Map<String, Value>) {
        if let Err(error) = json_store::write_json_atomic(&Self::cache_path(core), cache) {
            tracing::warn!("转写缓存落盘失败: {error}");
        }
    }

    /// 按（内容哈希, 引擎）查缓存；命中刷新最近使用时间。
    fn cache_lookup(&self, core: &CoreState, hash: &str, engine: &str) -> Option<Value> {
        self.with_cache(core, |cache| {
            let entry = cache.get_mut(hash)?.as_object_mut()?;
            let bucket = entry.get("engines")?.get(engine)?.clone();
            entry.insert("_last_used".into(), json!(now_secs()));
            Some(bucket)
        })
    }

    /// 写入引擎桶；按整文件条目做 LRU 淘汰（最近使用时间最旧的先淘汰）。
    fn cache_store(&self, core: &CoreState, hash: &str, engine: &str, payload: &Value) {
        self.with_cache(core, |cache| {
            let entry = cache.entry(hash.to_string()).or_insert_with(|| json!({ "engines": {} }));
            if !entry.get("engines").map(Value::is_object).unwrap_or(false) {
                entry["engines"] = json!({});
            }
            let mut bucket = payload.clone();
            if let Some(object) = bucket.as_object_mut() {
                object.retain(|k, _| !k.starts_with('_'));
                object.insert("cached_at".into(), json!(now_secs()));
            }
            entry["engines"][engine] = bucket;
            entry["_last_used"] = json!(now_secs());
            while cache.len() > CACHE_SIZE {
                let oldest =
                    cache.iter().min_by(|a, b| last_used(a.1).total_cmp(&last_used(b.1))).map(|(k, _)| k.clone());
                match oldest {
                    Some(key) => cache.remove(&key),
                    None => break,
                };
            }
            Self::save_cache(core, cache);
        });
    }

    fn store_proofread_variant(
        &self,
        core: &CoreState,
        hash: &str,
        engine: &str,
        model_id: Option<&str>,
        result: &proofread::ProofreadResult,
    ) {
        self.with_cache(core, |cache| {
            let Some(bucket) = cache.get_mut(hash).and_then(|e| e.get_mut("engines")).and_then(|e| e.get_mut(engine))
            else {
                return;
            };
            if !bucket.get("proofread").map(Value::is_object).unwrap_or(false) {
                bucket["proofread"] = json!({});
            }
            let variants = bucket["proofread"].as_object_mut().expect("刚确保为对象");
            variants.insert(
                variant_key(model_id),
                json!({
                    "text": result.text,
                    "segments": result.segments,
                    "windows": result.windows,
                    "requests": result.requests,
                    "fallback_batches": result.fallback_batches,
                    "model_id": result.model_id,
                    "model_label": result.model_label,
                }),
            );
            while variants.len() > PROOFREAD_VARIANTS_PER_ENGINE {
                let first = variants.keys().next().cloned();
                if let Some(key) = first {
                    variants.remove(&key);
                }
            }
            Self::save_cache(core, cache);
        });
    }

    /// 缓存引擎桶里记着的记录 id（转写结果存进记录集时写入）。
    fn bucket_record_id(&self, core: &CoreState, hash: &str, engine: &str) -> Option<String> {
        self.with_cache(core, |cache| {
            cache.get(hash)?.get("engines")?.get(engine)?.get("record_id")?.as_str().map(str::to_string)
        })
    }

    fn set_bucket_record_id(&self, core: &CoreState, hash: &str, engine: &str, record_id: &str) {
        self.with_cache(core, |cache| {
            let Some(bucket) = cache.get_mut(hash).and_then(|e| e.get_mut("engines")).and_then(|e| e.get_mut(engine))
            else {
                return;
            };
            bucket["record_id"] = json!(record_id);
            Self::save_cache(core, cache);
        });
    }

    pub fn get_job(&self, job_id: &str) -> Option<Value> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut jobs);
        jobs.get(job_id).map(Job::snapshot)
    }

    /// 取消后台任务：置位取消令牌并返回当前快照（`cancel_requested=true`）；任务在下一个检查点退出后
    /// 状态变为 `cancelled`、释放执行权。已结束的任务原样返回。不存在 → None。
    pub fn cancel_job(&self, job_id: &str) -> Option<Value> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs.get(job_id)?;
        if job.status == "running" && !job.cancel.is_cancelled() {
            tracing::info!("转写任务 {job_id} 收到取消请求（阶段 {}）", job.stage);
            job.cancel.cancel();
        }
        Some(job.snapshot())
    }

    fn update_job(&self, job_id: &str, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get_mut(job_id) {
            f(job);
        }
    }

    /// 取得本次执行权；返回的 owned guard 必须存活到引擎工作结束。
    fn try_acquire_busy(&self) -> Result<OwnedMutexGuard<()>, TranscribeError> {
        self.busy.clone().try_lock_owned().map_err(|_| TranscribeError::busy("转写引擎忙，请稍后重试"))
    }
}

fn last_used(entry: &Value) -> f64 {
    entry.get("_last_used").and_then(Value::as_f64).unwrap_or_else(|| {
        entry
            .get("engines")
            .and_then(Value::as_object)
            .map(|engines| {
                engines.values().filter_map(|b| b.get("cached_at").and_then(Value::as_f64)).fold(0.0, f64::max)
            })
            .unwrap_or(0.0)
    })
}

fn prune(jobs: &mut HashMap<String, Job>) {
    let now = now_secs();
    jobs.retain(|_, job| {
        !(matches!(job.status, "done" | "failed" | "cancelled") && now - job.created_at > JOB_KEEP_SECONDS)
    });
}

/// 处理一次 POST /api/transcribe。
pub async fn handle(core: Arc<CoreState>, request: TranscribeRequest) -> Result<Outcome, TranscribeError> {
    let engine = resolve_engine(&core, &request.engine)?;
    if request.proofread {
        if let Err(error) = core.llm.resolve_runtime(request.proofread_model_id.as_deref()) {
            return Err(TranscribeError::new(
                409,
                format!("LLM 校对不可用：{}。请先在设置页配置 LLM，或关闭校对", error.user_message),
            ));
        }
    }
    let path = validate_path(&request.path)?;
    let hash = content_hash(&path).await?;
    let transcriber = &core.transcriber;

    if let Some(cached) = transcriber.cache_lookup(&core, &hash, engine) {
        let variant = request
            .proofread
            .then(|| {
                cached.get("proofread").and_then(|p| p.get(variant_key(request.proofread_model_id.as_deref()))).cloned()
            })
            .flatten();
        if !request.proofread || variant.is_some() {
            tracing::info!("文件转写缓存命中: {}（{engine}）", path.display());
            let payload = shape_payload(&cached, request.want_segments, true, variant.as_ref());
            return Ok(Outcome::Result(save_record(&core, &request, &path, &hash, engine, payload)));
        }
        // ASR 命中但校对变体缺失 → 走 202 只跑校对阶段
    }

    let duration = media::probe_duration(&path).await?;
    if duration > MAX_DURATION_SECONDS {
        return Err(TranscribeError::new(
            413,
            format!("音频时长约 {:.0} 分钟，超过上限 {:.0} 分钟", duration / 60.0, MAX_DURATION_SECONDS / 60.0),
        ));
    }
    if request.async_job || request.proofread || duration > ASYNC_THRESHOLD_SECONDS {
        return start_job(core.clone(), request, path, hash, duration, engine).map(Outcome::Job);
    }
    let payload = run_transcription(&core, &path, &request.language, duration, &hash, engine).await?;
    let payload = shape_payload(&payload, request.want_segments, false, None);
    Ok(Outcome::Result(save_record(&core, &request, &path, &hash, engine, payload)))
}

/// 单并发执行一次转写并写缓存；忙时 503。
async fn run_transcription(
    core: &Arc<CoreState>,
    path: &Path,
    language: &str,
    duration: f64,
    hash: &str,
    engine: &str,
) -> Result<Value, TranscribeError> {
    let _model_gate = if crate::stt::local_kind(engine).is_some() {
        Some(
            core.model_gate
                .clone()
                .try_read_owned()
                .map_err(|_| TranscribeError::busy("本地模型正在维护，请稍后重试"))?,
        )
    } else {
        None
    };
    let _permit = core.transcriber.try_acquire_busy()?;
    run_transcription_locked(core, path, language, duration, hash, engine, &AsrHook::none()).await
}

/// 在调用方已持有执行许可时跑一次转写并写缓存；许可必须覆盖完整任务生命周期。
///
/// 被取消时返回 [`TranscribeError::cancelled`]、不写缓存；本地引擎要等正在推理的那一块结束才返回，
/// 这样调用方释放执行许可时引擎确实空闲了。
async fn run_transcription_locked(
    core: &Arc<CoreState>,
    path: &Path,
    language: &str,
    duration: f64,
    hash: &str,
    engine: &str,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    let samples = hook.token.run(async { Ok(media::decode_pcm16k(path).await?) }).await?;
    if samples.is_empty() {
        return Err(TranscribeError::new(415, "无法解码：没有音频数据"));
    }
    let payload = match crate::stt::local_kind(engine) {
        // 本地引擎（Whisper / Qwen3-ASR）：分段时间戳原样进入 payload，供校对窗口与前端使用
        Some(kind) => {
            let chunk_hook = hook.clone();
            let on_chunk: crate::stt::ChunkProgress =
                Box::new(move |done, _total| chunk_hook.tick(done as f64 / SAMPLE_RATE as f64));
            let outcome =
                crate::stt::transcribe_local_with_progress(core.clone(), kind, language.to_string(), samples, on_chunk)
                    .await;
            if hook.token.is_cancelled() {
                return Err(TranscribeError::cancelled());
            }
            let (model_label, transcript) =
                outcome.map_err(|error| TranscribeError::new(500, format!("转写失败：{error}")))?;
            let segments = transcript
                .segments
                .iter()
                .map(|segment| json!({ "start": segment.start, "end": segment.end, "text": segment.text }))
                .collect::<Vec<_>>();
            json!({
                "text": transcript.text,
                "duration_seconds": if duration > 0.0 { duration } else { transcript.duration },
                "engine": model_label,
                "segments": segments,
            })
        }
        None => match engine {
            "deepseek_asr" => hook.token.run(run_deepseek(core, &samples, duration, hook)).await?,
            "doubao_web_asr" => hook.token.run(run_doubao_web(core, &samples, duration, hook)).await?,
            "doubao_ime_asr" => hook.token.run(run_doubao_ime(core, &samples, duration, hook)).await?,
            "wetype_asr" => hook.token.run(run_wetype(core, &samples, duration, hook)).await?,
            _ => hook.token.run(run_doubao(core, language, &samples, duration, engine, hook)).await?,
        },
    };
    if hook.token.is_cancelled() {
        return Err(TranscribeError::cancelled());
    }
    core.transcriber.cache_store(core, hash, engine, &payload);
    Ok(payload)
}

/// 豆包路径：按单会话上限分段推流，段偏移拼成时间戳。
async fn run_doubao(
    core: &CoreState,
    language: &str,
    samples: &[i16],
    duration: f64,
    engine: &str,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    let config = crate::stt::doubao_config(core);
    let session_samples = DOUBAO_SESSION_SECONDS * SAMPLE_RATE;
    let mut segments = Vec::new();
    let mut billing_ms = 0i64;
    for (index, chunk) in samples.chunks(session_samples).enumerate() {
        let offset = (index * session_samples) as f64 / SAMPLE_RATE as f64;
        // 进度按已推送的音频秒数算（推流比实时快，推完后还要等服务端吐完结果）
        let mut on_push = |pushed: usize| hook.tick(offset + pushed as f64 / SAMPLE_RATE as f64);
        let outcome = crate::stt::doubao::transcribe_pcm_with_progress(&config, language, "", chunk, &mut on_push)
            .await
            .map_err(|e| {
                if hook.token.is_cancelled() {
                    TranscribeError::cancelled()
                } else {
                    TranscribeError::new(500, format!("转写失败：{e}"))
                }
            })?;
        billing_ms += outcome.audio_duration_ms.max(0);
        if !outcome.text.is_empty() {
            segments.push(json!({
                "start": offset,
                "end": offset + chunk.len() as f64 / SAMPLE_RATE as f64,
                "text": outcome.text,
            }));
        }
    }
    let text = segments.iter().filter_map(|s| s["text"].as_str()).collect::<Vec<_>>().join(" ");
    let mut payload = json!({
        "text": text,
        "duration_seconds": if duration > 0.0 { duration } else { samples.len() as f64 / SAMPLE_RATE as f64 },
        "engine": format!("{engine}:bigmodel"),
        "segments": segments,
    });
    if billing_ms > 0 {
        payload["billing"] =
            json!({ "audio_duration_seconds": (billing_ms as f64 / 1000.0 * 1000.0).round() / 1000.0 });
    }
    Ok(payload)
}

/// DeepSeek 路径：接口单次最长 120 秒，按 115 秒切块顺序识别，段偏移拼成时间戳。
async fn run_deepseek(
    core: &CoreState,
    samples: &[i16],
    duration: f64,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    let config = crate::stt::deepseek_config(core);
    let mut on_chunk = |done: usize, _total: usize| hook.tick(done as f64 / SAMPLE_RATE as f64);
    let transcript =
        crate::stt::deepseek::transcribe_pcm_with_progress(&config, samples, &mut on_chunk).await.map_err(|error| {
            if hook.token.is_cancelled() {
                TranscribeError::cancelled()
            } else {
                TranscribeError::new(500, format!("转写失败：{error}"))
            }
        })?;
    let segments = transcript
        .segments
        .iter()
        .map(|segment| json!({ "start": segment.start, "end": segment.end, "text": segment.text }))
        .collect::<Vec<_>>();
    let mut payload = json!({
        "text": transcript.text,
        "duration_seconds": if duration > 0.0 { duration } else { samples.len() as f64 / SAMPLE_RATE as f64 },
        "engine": "deepseek-asr",
        "segments": segments,
    });
    if !transcript.warnings.is_empty() {
        payload["warnings"] = json!(transcript.warnings);
    }
    Ok(payload)
}

/// 豆包网页匿名路径：实时速率推流；引擎内部按 13 分钟会话上限自动分段。
async fn run_doubao_web(
    core: &CoreState,
    samples: &[i16],
    duration: f64,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    let config = crate::stt::doubao_web_config(core);
    let mut on_push = |pushed: usize| hook.tick(pushed as f64 / SAMPLE_RATE as f64);
    let text = crate::stt::doubao_web::transcribe_pcm_with_progress(&config, samples, &mut on_push).await.map_err(
        |error| {
            if hook.token.is_cancelled() {
                TranscribeError::cancelled()
            } else {
                TranscribeError::new(500, format!("转写失败：{error}"))
            }
        },
    )?;
    Ok(doubao_web_payload(text, duration, samples.len()))
}

fn doubao_web_payload(text: String, duration: f64, samples: usize) -> Value {
    let end = if duration > 0.0 { duration } else { samples as f64 / SAMPLE_RATE as f64 };
    // 校对链路从 segments 取文本；豆包网页不返回时间戳，但必须给一个覆盖整段的 segment，
    // 否则开启 LLM 校对时会误判成 asr_empty 并静默跳过。
    let segments = if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![json!({ "start": 0.0, "end": end, "text": text.clone() })]
    };
    json!({
        "text": text,
        "duration_seconds": end,
        "engine": "doubao_web_asr",
        "segments": segments,
    })
}

/// 豆包输入法语音：整段发送；协议层按段返回中间稿/最终稿，客户端已按段累积成完整正文。
async fn run_doubao_ime(
    core: &CoreState,
    samples: &[i16],
    duration: f64,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    let config = crate::stt::doubao_ime_config(core);
    let mut on_push = |pushed: usize| hook.tick(pushed as f64 / SAMPLE_RATE as f64);
    let text = crate::stt::doubao_ime::transcribe_pcm_with_progress(&config, samples, &mut on_push).await.map_err(
        |error| {
            if hook.token.is_cancelled() {
                TranscribeError::cancelled()
            } else {
                TranscribeError::new(500, format!("转写失败：{error}"))
            }
        },
    )?;
    Ok(doubao_ime_payload(text, duration, samples.len()))
}

fn doubao_ime_payload(text: String, duration: f64, samples: usize) -> Value {
    let end = if duration > 0.0 { duration } else { samples as f64 / SAMPLE_RATE as f64 };
    let segments = if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![json!({ "start": 0.0, "end": end, "text": text.clone() })]
    };
    json!({
        "text": text,
        "duration_seconds": end,
        "engine": "doubao_ime_asr",
        "segments": segments,
    })
}

/// 微信输入法语音：整段发送，协议层只返回最终文字，不伪造时间戳。
async fn run_wetype(
    core: &CoreState,
    samples: &[i16],
    duration: f64,
    hook: &AsrHook,
) -> Result<Value, TranscribeError> {
    #[cfg(windows)]
    {
        hook.tick(0.0);
        let config = crate::stt::wetype_config(core);
        let text = crate::stt::wetype::transcribe_pcm(&config, samples).await.map_err(|error| {
            if hook.token.is_cancelled() {
                TranscribeError::cancelled()
            } else {
                TranscribeError::new(500, format!("转写失败：{error}"))
            }
        })?;
        hook.tick(duration);
        let segments = (!text.trim().is_empty())
            .then(|| vec![json!({ "start": 0.0, "end": duration, "text": text })])
            .unwrap_or_default();
        Ok(json!({
            "text": text,
            "duration_seconds": if duration > 0.0 { duration } else { samples.len() as f64 / SAMPLE_RATE as f64 },
            "engine": "wetype_asr",
            "segments": segments,
        }))
    }
    #[cfg(not(windows))]
    {
        let _ = (core, samples, duration, hook);
        Err(TranscribeError::new(409, "微信输入法语音仅支持 Windows"))
    }
}

fn start_job(
    core: Arc<CoreState>,
    request: TranscribeRequest,
    path: PathBuf,
    hash: String,
    duration: f64,
    engine: &'static str,
) -> Result<String, TranscribeError> {
    let model_gate = if crate::stt::local_kind(engine).is_some() {
        Some(
            core.model_gate
                .clone()
                .try_read_owned()
                .map_err(|_| TranscribeError::busy("本地模型正在维护，请稍后重试"))?,
        )
    } else {
        None
    };
    // 受理即取得执行权；guard 随任务移动，直到 ASR / 校对全部结束才释放。
    let permit = core.transcriber.try_acquire_busy()?;
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    let cancel = CancelToken::default();
    {
        let mut jobs = core.transcriber.jobs.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut jobs);
        if jobs.len() >= JOB_MAX_ENTRIES {
            return Err(TranscribeError::busy("转写任务表已满，请稍后重试"));
        }
        jobs.insert(
            job_id.clone(),
            Job {
                status: "running",
                stage: "asr",
                progress: None,
                result: None,
                error: String::new(),
                created_at: now_secs(),
                cancel: cancel.clone(),
            },
        );
    }
    tracing::info!("转写任务已受理 {job_id}（{engine}，校对={}，{duration:.0}s）", request.proofread);
    let id = job_id.clone();
    tokio::spawn(async move {
        // guard 随后台任务存活，覆盖 ASR 与校对；成功、失败或 panic 时才释放。
        let _model_gate = model_gate;
        let _permit = permit;
        let outcome = execute_job(&core, &id, &request, &path, &hash, duration, engine, &cancel).await;
        drop(_permit);
        core.transcriber.update_job(&id, |job| match outcome {
            Ok(result) => {
                job.stage = "done";
                job.status = "done";
                job.result = Some(result);
            }
            Err(_) if cancel.is_cancelled() => {
                tracing::info!("转写任务 {id} 已取消（阶段 {}）", job.stage);
                job.status = "cancelled";
                job.error = crate::stt::TRANSCRIBE_CANCELLED.to_string();
            }
            Err(error) => {
                job.status = "failed";
                job.error = error.message;
            }
        });
    });
    Ok(job_id)
}

/// 识别进度写进任务表：`{stage: "asr", done_seconds, total_seconds}`（秒，保留一位小数）。
///
/// 豆包逐帧推流时每 200ms 音频回调一次，这里只在前进满 1 秒或到达末尾时才写，避免反复改任务表。
fn asr_progress_reporter(core: Arc<CoreState>, job_id: String, duration: f64) -> Arc<dyn Fn(f64) + Send + Sync> {
    let total_seconds = (duration.max(0.0) * 10.0).round() / 10.0;
    let last = Mutex::new(f64::NEG_INFINITY);
    Arc::new(move |done_seconds: f64| {
        let mut done = (done_seconds.max(0.0) * 10.0).round() / 10.0;
        if total_seconds > 0.0 {
            done = done.min(total_seconds);
        }
        {
            let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
            let at_end = total_seconds > 0.0 && done >= total_seconds;
            if done - *last < 1.0 && !(at_end && done > *last) {
                return;
            }
            *last = done;
        }
        core.transcriber.update_job(&job_id, |job| {
            job.progress = Some(json!({ "stage": "asr", "done_seconds": done, "total_seconds": total_seconds }))
        });
    })
}

/// 后台任务体：ASR（缓存命中可跳过）→（可选）校对；校对整体降级不算失败。
///
/// 取消发生在识别阶段时什么都不写；发生在校对阶段时，已完成的识别结果照常留在缓存里
/// （换个模型重新校对不必再识别一遍），校对变体不写。
#[allow(clippy::too_many_arguments)]
async fn execute_job(
    core: &Arc<CoreState>,
    job_id: &str,
    request: &TranscribeRequest,
    path: &Path,
    hash: &str,
    duration: f64,
    engine: &str,
    cancel: &CancelToken,
) -> Result<Value, TranscribeError> {
    let mut payload = match core.transcriber.cache_lookup(core, hash, engine) {
        Some(bucket) => bucket,
        None => {
            let report = asr_progress_reporter(core.clone(), job_id.to_string(), duration);
            report(0.0);
            let hook = AsrHook { token: cancel.clone(), report };
            run_transcription_locked(core, path, &request.language, duration, hash, engine, &hook).await?
        }
    };
    if cancel.is_cancelled() {
        return Err(TranscribeError::cancelled());
    }
    if request.proofread {
        core.transcriber.update_job(job_id, |job| {
            job.stage = "proofreading";
            job.progress = None;
        });
        let segments = payload.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
        let options = proofread::ProofreadOptions::from_request(request.proofread_options.as_ref());
        let progress_core = core.clone();
        let progress_id = job_id.to_string();
        let result = cancel
            .run(async {
                proofread::run(core, request.proofread_model_id.as_deref(), options, segments, move |done, total| {
                    progress_core.transcriber.update_job(&progress_id, |job| {
                        job.progress = Some(json!({ "window_done": done, "window_total": total }))
                    });
                })
                .await
                .map_err(|e| TranscribeError::new(409, format!("LLM 校对不可用：{}", e.user_message)))
            })
            .await?;
        payload["raw_text"] = payload.get("text").cloned().unwrap_or_else(|| json!(""));
        if result.applied {
            payload["text"] = json!(result.text);
            payload["segments"] = json!(result.segments);
            // 校对成功才写缓存变体（降级不写，下次可重试校对）
            core.transcriber.store_proofread_variant(
                core,
                hash,
                engine,
                request.proofread_model_id.as_deref(),
                &result,
            );
        }
        payload["proofread"] = result.meta();
    }
    let payload = shape_payload(&payload, request.want_segments, false, None);
    Ok(save_record(core, request, path, hash, engine, payload))
}

/// 把转写结果存进记录集，并在响应里带上 `record_id`。
///
/// 无音频的文本记录：`raw_text` 是识别原文，校对成功时 `optimized_text` 是校对稿；`source_file` 是原文件名。
/// 同一文件同一引擎只建一条：记录 id 记在缓存引擎桶里，记录还在时只在有新校对稿时更新它，被删了才重建。
/// 存记录失败只写日志，不影响转写结果。
fn save_record(
    core: &CoreState,
    request: &TranscribeRequest,
    path: &Path,
    hash: &str,
    engine: &str,
    mut payload: Value,
) -> Value {
    use crate::records::{Record, RecordStatus};

    let text = payload.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    let proofread_applied =
        payload.get("proofread").and_then(|p| p.get("applied")).and_then(Value::as_bool).unwrap_or(false);
    let raw_text = match payload.get("raw_text").and_then(Value::as_str) {
        Some(raw) if proofread_applied => raw.to_string(),
        _ => text.clone(),
    };
    let optimized_text = proofread_applied.then(|| text.clone());

    let existing = core.transcriber.bucket_record_id(core, hash, engine).filter(|id| core.records.get(id).is_ok());
    let record_id = match existing {
        Some(id) => {
            // 已有记录：只在这次带来了新的校对稿时更新（用户改过的不覆盖）
            let stale = |record: &Record| {
                optimized_text
                    .as_deref()
                    .is_some_and(|optimized| !record.is_modified && record.optimized_text.as_deref() != Some(optimized))
            };
            if core.records.get(&id).is_ok_and(|record| stale(&record)) {
                let outcome = core.records.patch(&id, |record| {
                    record.optimized_text = optimized_text.clone();
                    record.status = RecordStatus::Optimized;
                });
                if let Err(error) = outcome {
                    tracing::warn!("更新文件转写记录 {id} 失败：{error}");
                }
            }
            Some(id)
        }
        None => {
            let id = crate::records::new_record_id();
            let source_file = path.file_name().map(|name| name.to_string_lossy().into_owned());
            let record = Record {
                id: id.clone(),
                duration: payload.get("duration_seconds").and_then(Value::as_f64).unwrap_or(0.0),
                status: if optimized_text.is_some() { RecordStatus::Optimized } else { RecordStatus::Transcribed },
                raw_text: Some(raw_text),
                optimized_text,
                engine: Some(engine.to_string()),
                language: Some(crate::stt::effective_language(engine, &request.language)),
                source_file,
                ..Default::default()
            };
            match core.records.insert(record) {
                Ok(()) => {
                    core.transcriber.set_bucket_record_id(core, hash, engine, &id);
                    tracing::info!("文件转写已存入记录集：{id}（{}）", path.display());
                    Some(id)
                }
                Err(error) => {
                    tracing::warn!("文件转写结果存入记录集失败：{error}");
                    None
                }
            }
        }
    };
    if let Some(id) = record_id {
        payload["record_id"] = json!(id);
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 响应整形_校对变体覆盖正文() {
        let entry = json!({"text": "原文", "duration_seconds": 3.0, "engine": "doubao_asr:bigmodel",
            "segments": [{"start": 0.0, "end": 3.0, "text": "原文"}], "billing": {"audio_duration_seconds": 3.0},
            "warnings": ["第 2/2 段识别失败"]});
        let plain = shape_payload(&entry, false, true, None);
        assert_eq!(plain["cached"], true);
        assert_eq!(plain["warnings"][0], "第 2/2 段识别失败");
        assert!(plain.get("segments").is_none());
        let variant = json!({"text": "校对文", "segments": [{"start": 0.0, "end": 3.0, "text": "校对文"}], "windows": 1, "requests": 1, "model_label": "DS"});
        let shaped = shape_payload(&entry, true, true, Some(&variant));
        assert_eq!(shaped["text"], "校对文");
        assert_eq!(shaped["raw_text"], "原文");
        assert_eq!(shaped["proofread"]["llm_model"], "DS");
        assert_eq!(shaped["segments"][0]["text"], "校对文");
    }

    #[test]
    fn 豆包网页响应_为校对提供覆盖整段的_segment() {
        let payload = doubao_web_payload("你好世界".into(), 3.5, 56_000);
        assert_eq!(payload["text"], "你好世界");
        assert_eq!(payload["engine"], "doubao_web_asr");
        assert_eq!(payload["segments"], json!([{ "start": 0.0, "end": 3.5, "text": "你好世界" }]));

        let empty = doubao_web_payload("  ".into(), 1.0, 16_000);
        assert_eq!(empty["segments"], json!([]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn 并发受理_执行许可只发给一个任务() {
        let dir = std::env::temp_dir().join(format!("att-ft-busy-{}", uuid::Uuid::new_v4()));
        let core = Arc::new(CoreState::init(&dir, crate::Platform::Macos).unwrap());
        let request = || TranscribeRequest {
            path: "unused.wav".into(),
            language: "zh".into(),
            want_segments: false,
            engine: "deepseek_asr".into(),
            proofread: false,
            proofread_model_id: None,
            proofread_options: None,
            async_job: false,
        };
        let path = PathBuf::from("unused.wav");

        // current_thread runtime 下不主动 yield，后台任务尚未开始，受理取得的 guard 仍在。
        let first =
            start_job(core.clone(), request(), path.clone(), "first-hash".into(), 3600.0, "deepseek_asr").unwrap();
        assert!(core.transcriber.is_busy(), "受理后台任务后必须能从外部观察到忙碌状态");
        let busy_job =
            start_job(core.clone(), request(), path.clone(), "second-hash".into(), 3600.0, "deepseek_asr").unwrap_err();
        assert_eq!(busy_job.status, 503);
        assert_eq!(busy_job.retry_after, Some(BUSY_RETRY_AFTER_SECONDS));

        // 同步请求在受理之后也必须被拒绝，不能抢走已返回 202 任务的执行权。
        let busy_sync = run_transcription(&core, &path, "zh", 1.0, "sync-hash", "deepseek_asr").await.unwrap_err();
        assert_eq!(busy_sync.status, 503);

        // 让已受理任务真正执行：它应因文件不存在而失败，而不是因「引擎忙」失败。
        let job = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                tokio::task::yield_now().await;
                let job = core.transcriber.get_job(&first).unwrap();
                if job["status"] != "running" {
                    break job;
                }
            }
        })
        .await
        .expect("已受理任务应结束");
        assert_eq!(job["status"], "failed");
        assert!(!job["error"].as_str().unwrap_or("").contains("引擎忙"), "{}", job["error"]);
        assert!(!core.transcriber.is_busy(), "任务结束后必须恢复空闲");

        // 任务结束后许可释放，下一批请求可以继续受理。
        let third = start_job(core.clone(), request(), path, "third-hash".into(), 3600.0, "deepseek_asr").unwrap();
        assert!(!third.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn 取消令牌_等待方被唤醒_run_立即返回取消() {
        let token = CancelToken::default();
        let waiter = tokio::spawn({
            let token = token.clone();
            async move { token.cancelled().await }
        });
        tokio::task::yield_now().await;
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter).await.expect("应被唤醒").unwrap();
        // 已取消时不再让工作跑
        let outcome = token.run(async { Ok::<_, TranscribeError>(1) }).await.unwrap_err();
        assert_eq!(outcome.status, 499);
        // 已取消后再等待立即返回
        tokio::time::timeout(std::time::Duration::from_millis(100), token.cancelled()).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn 取消任务_状态为cancelled_不写缓存_释放执行权() {
        let dir = std::env::temp_dir().join(format!("att-ft-cancel-{}", uuid::Uuid::new_v4()));
        let core = Arc::new(CoreState::init(&dir, crate::Platform::Macos).unwrap());
        let request = || TranscribeRequest {
            path: "unused.wav".into(),
            language: "zh".into(),
            want_segments: false,
            engine: "deepseek_asr".into(),
            proofread: false,
            proofread_model_id: None,
            proofread_options: None,
            async_job: true,
        };
        let path = PathBuf::from("unused.wav");
        let job_id =
            start_job(core.clone(), request(), path.clone(), "cancel-hash".into(), 3600.0, "deepseek_asr").unwrap();

        let snapshot = core.transcriber.cancel_job(&job_id).unwrap();
        assert_eq!(snapshot["status"], "running");
        assert_eq!(snapshot["cancel_requested"], true);
        assert!(core.transcriber.cancel_job("nope").is_none());

        let job = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                tokio::task::yield_now().await;
                let job = core.transcriber.get_job(&job_id).unwrap();
                if job["status"] != "running" {
                    break job;
                }
            }
        })
        .await
        .expect("取消后任务应退出");
        assert_eq!(job["status"], "cancelled");
        assert_eq!(job["cancel_requested"], false);
        assert!(core.transcriber.cache_lookup(&core, "cancel-hash", "deepseek_asr").is_none(), "取消不写缓存");

        // 再次取消已结束的任务：原样返回，不改状态
        assert_eq!(core.transcriber.cancel_job(&job_id).unwrap()["status"], "cancelled");
        // 执行权已释放，可以马上受理下一个
        assert!(start_job(core.clone(), request(), path, "next-hash".into(), 3600.0, "deepseek_asr").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 识别进度_按秒写入任务表() {
        let dir = std::env::temp_dir().join(format!("att-ft-progress-{}", uuid::Uuid::new_v4()));
        let core = Arc::new(CoreState::init(&dir, crate::Platform::Macos).unwrap());
        core.transcriber.jobs.lock().unwrap().insert(
            "job".into(),
            Job {
                status: "running",
                stage: "asr",
                progress: None,
                result: None,
                error: String::new(),
                created_at: now_secs(),
                cancel: CancelToken::default(),
            },
        );
        let progress = || core.transcriber.get_job("job").unwrap()["progress"].clone();
        let report = asr_progress_reporter(core.clone(), "job".into(), 600.04);
        report(0.0);
        assert_eq!(progress(), json!({ "stage": "asr", "done_seconds": 0.0, "total_seconds": 600.0 }));
        report(0.6);
        assert_eq!(progress()["done_seconds"], 0.0, "不满 1 秒不写");
        report(12.345);
        assert_eq!(progress()["done_seconds"], 12.3);
        report(700.0);
        assert_eq!(progress()["done_seconds"], 600.0, "不超过总时长");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 转写结果存入记录集_同一文件同一引擎只建一条() {
        let dir = std::env::temp_dir().join(format!("att-ft-record-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, crate::Platform::Macos).unwrap();
        let request = TranscribeRequest {
            path: String::new(),
            language: "zh".into(),
            want_segments: false,
            engine: "whisper".into(),
            proofread: false,
            proofread_model_id: None,
            proofread_options: None,
            async_job: true,
        };
        let path = PathBuf::from("/media/周会 录音.mp4");
        let entry = json!({ "text": "原文", "duration_seconds": 61.5, "engine": "whisper:large-v3", "segments": [] });
        core.transcriber.cache_store(&core, "h1", "whisper", &entry);

        let first = save_record(&core, &request, &path, "h1", "whisper", shape_payload(&entry, false, false, None));
        let id = first["record_id"].as_str().unwrap().to_string();
        let record = core.records.get(&id).unwrap();
        assert_eq!(record.source_file.as_deref(), Some("周会 录音.mp4"), "只存文件名，不含目录");
        assert_eq!(record.raw_text.as_deref(), Some("原文"));
        assert_eq!(record.optimized_text, None);
        assert_eq!(record.status, crate::records::RecordStatus::Transcribed);
        assert_eq!(record.duration, 61.5);
        assert!(record.audio_path.is_empty(), "无音频的文本记录");
        assert_eq!(record.engine.as_deref(), Some("whisper"));

        // 缓存命中再转一次：不重复建
        let again = save_record(&core, &request, &path, "h1", "whisper", shape_payload(&entry, false, true, None));
        assert_eq!(again["record_id"], json!(id));
        assert_eq!(core.records.list(None, 10, 0).len(), 1);

        // 之后开了校对：同一条记录补上校对稿
        let variant = json!({ "text": "校对稿", "windows": 1, "requests": 1 });
        let proofread = shape_payload(&entry, false, true, Some(&variant));
        save_record(&core, &request, &path, "h1", "whisper", proofread);
        let record = core.records.get(&id).unwrap();
        assert_eq!(record.raw_text.as_deref(), Some("原文"));
        assert_eq!(record.optimized_text.as_deref(), Some("校对稿"));
        assert_eq!(record.status, crate::records::RecordStatus::Optimized);
        assert_eq!(core.records.list(None, 10, 0).len(), 1);

        // 记录被删了：再转时重建
        core.records.delete(&id).unwrap();
        let rebuilt = save_record(&core, &request, &path, "h1", "whisper", shape_payload(&entry, false, true, None));
        assert_ne!(rebuilt["record_id"], json!(id));
        assert_eq!(core.records.list(None, 10, 0).len(), 1);
        // 换引擎是另一条
        core.transcriber.cache_store(&core, "h1", "doubao_asr", &entry);
        save_record(&core, &request, &path, "h1", "doubao_asr", shape_payload(&entry, false, false, None));
        assert_eq!(core.records.list(None, 10, 0).len(), 2);
        // 豆包网页与官方豆包必须使用独立缓存桶 / 记录，不能相互覆盖
        core.transcriber.cache_store(&core, "h1", "doubao_web_asr", &entry);
        let web =
            save_record(&core, &request, &path, "h1", "doubao_web_asr", shape_payload(&entry, false, false, None));
        assert_eq!(core.records.list(None, 10, 0).len(), 3);
        assert_ne!(web["record_id"], json!(id));
        assert!(core.transcriber.cache_lookup(&core, "h1", "doubao_asr").is_some());
        assert!(core.transcriber.cache_lookup(&core, "h1", "doubao_web_asr").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 路径与扩展名校验() {
        let dir = std::env::temp_dir().join(format!("att-ft-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(validate_path("").unwrap_err().status, 404);
        assert_eq!(validate_path("relative.mp3").unwrap_err().status, 404);
        let text = dir.join("a.txt");
        std::fs::write(&text, b"x").unwrap();
        assert_eq!(validate_path(text.to_str().unwrap()).unwrap_err().status, 415);
        let audio = dir.join("a.MP3");
        std::fs::write(&audio, b"x").unwrap();
        // 数据面只认令牌：任意可读的音频文件都通过路径校验，不再要求界面登记
        let resolved = validate_path(audio.to_str().unwrap()).expect("任意可读音频都应通过路径校验");
        assert_eq!(resolved, std::fs::canonicalize(&audio).map(crate::state::tidy).unwrap());
        assert!(validate_path(&format!("\"{}\"", audio.display())).is_ok());
        let missing = dir.join("missing.mp3");
        assert_eq!(validate_path(missing.to_str().unwrap()).unwrap_err().status, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 路径校验_canonicalize_消掉相对段() {
        let dir = std::env::temp_dir().join(format!("att-ft-canon-{}", uuid::Uuid::new_v4()));
        let media_dir = dir.join("media");
        std::fs::create_dir_all(&media_dir).unwrap();
        let audio = media_dir.join("a.mp3");
        std::fs::write(&audio, b"x").unwrap();

        let canonical = std::fs::canonicalize(&audio).map(crate::state::tidy).unwrap();
        // 带 `..` 的原始路径 canonicalize 后应得到同一个文件
        let indirect = media_dir.join("..").join("media").join("a.mp3");
        assert_eq!(validate_path(indirect.to_str().unwrap()).unwrap(), canonical);

        // 目录外的文件同样可转写（授权在令牌，不在目录）
        let outside = dir.join("outside.mp3");
        std::fs::write(&outside, b"x").unwrap();
        assert!(validate_path(outside.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 引擎解析_本地模型缺失只回一句结论() {
        let dir = std::env::temp_dir().join(format!("att-ft-engine-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, crate::Platform::Macos).unwrap();
        #[cfg(feature = "local-engines")]
        for (engine, summary) in [
            ("whisper", att_whisper::MODEL_MISSING),
            ("qwen_asr", att_qwen::MODEL_MISSING),
            ("sense_voice", att_qwen::SENSE_VOICE_MODEL_MISSING),
        ] {
            let error = resolve_engine(&core, engine).unwrap_err();
            assert_eq!(error.status, 409);
            assert!(error.message.starts_with(summary), "{}", error.message);
            assert!(!error.message.contains('\n'), "不带多行详情：{}", error.message);
            assert!(error.message.chars().count() < 60, "{}", error.message);
        }
        #[cfg(not(feature = "local-engines"))]
        for engine in ["whisper", "qwen_asr", "sense_voice"] {
            let error = resolve_engine(&core, engine).unwrap_err();
            assert_eq!(error.status, 409);
            assert!(error.message.contains(crate::stt::local::LOCAL_ENGINES_DISABLED), "{}", error.message);
        }
        assert_eq!(resolve_engine(&core, "doubao_asr").unwrap_err().status, 409, "新数据目录没有豆包凭据");
        assert_eq!(resolve_engine(&core, "doubao_web_asr").unwrap(), "doubao_web_asr", "豆包网页无需凭据");
        assert_eq!(resolve_engine(&core, "chatgpt_asr").unwrap_err().status, 422, "文件转写不支持 ChatGPT 听写");
        assert_eq!(resolve_engine(&core, "deepseek_asr").unwrap_err().status, 409, "新数据目录没有 DeepSeek 凭据");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 引擎解析_auto_先回落豆包网页_配置_deepseek_后优先_deepseek() {
        let dir = std::env::temp_dir().join(format!("att-ft-auto-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, crate::Platform::Macos).unwrap();

        // 没有本地模型，也没有豆包 / DeepSeek 凭据：最后回落到无需凭据的豆包网页
        assert_eq!(resolve_engine(&core, "auto").unwrap(), "doubao_web_asr");
        assert_eq!(resolve_engine(&core, "").unwrap(), "doubao_web_asr");

        // 只配置 DeepSeek：auto 与省略 engine 都回落到它
        core.apply_settings_update(&json!({ "deepseek_asr_api_key": "sk-test" })).unwrap();
        assert_eq!(resolve_engine(&core, "auto").unwrap(), "deepseek_asr");
        assert_eq!(resolve_engine(&core, "").unwrap(), "deepseek_asr");
        assert_eq!(resolve_engine(&core, "deepseek_asr").unwrap(), "deepseek_asr");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "需要本机运行微信输入法、ffmpeg 与 ATT_TEST_WAV"]
    async fn 真机_微信输入法_文件转写全链路() {
        let path = std::env::var("ATT_TEST_WAV").expect("请设置 ATT_TEST_WAV");
        let dir = std::env::temp_dir().join(format!("att-ft-wetype-{}", uuid::Uuid::new_v4()));
        let core = Arc::new(CoreState::init(&dir, crate::Platform::Windows).unwrap());
        core.allow_transcribe_file(std::path::Path::new(&path)).unwrap();
        let request = TranscribeRequest {
            path,
            language: "zh".into(),
            want_segments: false,
            engine: "wetype_asr".into(),
            proofread: false,
            proofread_model_id: None,
            proofread_options: None,
            async_job: false,
        };
        let outcome = handle(core.clone(), request).await.unwrap();
        let Outcome::Result(payload) = outcome else { panic!("短音频应同步返回") };
        assert_eq!(payload["engine"], json!("wetype_asr"));
        assert!(!payload["text"].as_str().unwrap_or_default().trim().is_empty());
        let id = payload["record_id"].as_str().expect("应写入记录集");
        let record = core.records.get(id).unwrap();
        assert_eq!(record.engine.as_deref(), Some("wetype_asr"));
        assert_eq!(record.language.as_deref(), Some("auto"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
