//! 本地文件转写（移植自 Python `app/services/file_transcription_service.py`）。
//!
//! - POST /api/transcribe：缓存命中秒回；短文件同步 200；长文件或开启 LLM 校对 → 202 任务号；
//! - 错误码契约：404 文件不存在 / 415 无法解码 / 413 超时长上限 / 409 引擎或校对不可用 /
//!   503 引擎忙（Retry-After）/ 500 其他；
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
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
}

#[derive(Default)]
pub struct FileTranscriber {
    jobs: Mutex<HashMap<String, Job>>,
    /// 单并发闸门（引擎/上游配额有限；忙时 503）
    busy: tokio::sync::Mutex<()>,
    /// 缓存（懒加载）；None = 尚未读盘
    cache: Mutex<Option<Map<String, Value>>>,
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn variant_key(model_id: Option<&str>) -> String {
    format!("{}@{}", model_id.unwrap_or("default"), proofread::PROMPT_VERSION)
}

/// 引擎解析：显式 > auto（本地优先）；本地模型/推理库缺失、豆包凭据缺失 → 409。
///
/// 本地引擎只做**文件是否就位**的探测，不触发模型加载（加载发生在真正转写时）。
fn resolve_engine(core: &CoreState, requested: &str) -> Result<&'static str, TranscribeError> {
    use crate::stt::local::LocalKind;

    let whisper = core.local_models.status(core, LocalKind::Whisper);
    let qwen = core.local_models.status(core, LocalKind::Qwen);
    let doubao_ready = crate::stt::doubao_config(core).ensure().is_ok();
    let unavailable = |kind: LocalKind, status: &crate::stt::local::LocalStatus| {
        TranscribeError::new(409, format!("{}不可用：{}", kind.label(), status.error.clone().unwrap_or_default()))
    };

    match requested.trim().to_lowercase().as_str() {
        // 与 Python 版一致：auto = 本地优先（配置指向本地引擎就用它，否则回落本地 Whisper，最后才用在线引擎）
        "auto" | "" => {
            if let Some(kind) = crate::stt::local_kind(&core.settings().asr_provider) {
                let ready = if kind == LocalKind::Whisper { whisper.available } else { qwen.available };
                if ready {
                    return Ok(kind.provider());
                }
            }
            if whisper.available {
                Ok("whisper")
            } else if qwen.available {
                Ok("qwen_asr")
            } else if doubao_ready {
                Ok("doubao_asr")
            } else {
                Err(TranscribeError::new(
                    409,
                    "当前没有可用的转写引擎：本地 Whisper / Qwen3-ASR 模型未就位，豆包凭据也未配置",
                ))
            }
        }
        "whisper" if whisper.available => Ok("whisper"),
        "whisper" => Err(unavailable(LocalKind::Whisper, &whisper)),
        "qwen_asr" if qwen.available => Ok("qwen_asr"),
        "qwen_asr" => Err(unavailable(LocalKind::Qwen, &qwen)),
        "doubao_asr" if doubao_ready => Ok("doubao_asr"),
        "doubao_asr" => Err(TranscribeError::new(409, "豆包凭据未配置：请在 ATT 设置页填写 Access Token 后重试")),
        other => Err(TranscribeError::new(422, format!("不支持的 engine：{other}（可选 auto/whisper/qwen_asr/doubao_asr）"))),
    }
}

/// 路径校验：绝对路径 + 文件存在 + 扩展名可识别。
fn validate_path(raw: &str) -> Result<PathBuf, TranscribeError> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Err(TranscribeError::new(404, "缺少文件路径"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(TranscribeError::new(404, format!("必须是本地绝对路径：{raw}")));
    }
    if !path.is_file() {
        return Err(TranscribeError::new(404, format!("文件不存在：{raw}")));
    }
    if !media::is_supported_extension(&path) {
        let suffix = path.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_else(|| "无后缀".into());
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
    for key in ["raw_text", "proofread"] {
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
        if let Some(variant_segments) = variant.get("segments").filter(|s| s.as_array().map(|a| !a.is_empty()).unwrap_or(false)) {
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
    fn cache_path(core: &CoreState) -> PathBuf {
        core.data_dir.join("transcribe_file_cache.json")
    }

    fn with_cache<T>(&self, core: &CoreState, f: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let cache = guard.get_or_insert_with(|| match json_store::read_json_lenient::<Value>(&Self::cache_path(core)) {
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
                let oldest = cache
                    .iter()
                    .min_by(|a, b| last_used(a.1).total_cmp(&last_used(b.1)))
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(key) => cache.remove(&key),
                    None => break,
                };
            }
            Self::save_cache(core, cache);
        });
    }

    fn store_proofread_variant(&self, core: &CoreState, hash: &str, engine: &str, model_id: Option<&str>, result: &proofread::ProofreadResult) {
        self.with_cache(core, |cache| {
            let Some(bucket) = cache.get_mut(hash).and_then(|e| e.get_mut("engines")).and_then(|e| e.get_mut(engine)) else { return };
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

    pub fn get_job(&self, job_id: &str) -> Option<Value> {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut jobs);
        jobs.get(job_id).map(|job| {
            json!({ "status": job.status, "stage": job.stage, "progress": job.progress, "result": job.result, "error": job.error })
        })
    }

    fn update_job(&self, job_id: &str, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get_mut(job_id) {
            f(job);
        }
    }
}

fn last_used(entry: &Value) -> f64 {
    entry.get("_last_used").and_then(Value::as_f64).unwrap_or_else(|| {
        entry
            .get("engines")
            .and_then(Value::as_object)
            .map(|engines| engines.values().filter_map(|b| b.get("cached_at").and_then(Value::as_f64)).fold(0.0, f64::max))
            .unwrap_or(0.0)
    })
}

fn prune(jobs: &mut HashMap<String, Job>) {
    let now = now_secs();
    jobs.retain(|_, job| !(matches!(job.status, "done" | "failed") && now - job.created_at > JOB_KEEP_SECONDS));
}

/// 处理一次 POST /api/transcribe。
pub async fn handle(core: Arc<CoreState>, request: TranscribeRequest) -> Result<Outcome, TranscribeError> {
    let engine = resolve_engine(&core, &request.engine)?;
    if request.proofread {
        if let Err(error) = core.llm.resolve_runtime(request.proofread_model_id.as_deref()) {
            return Err(TranscribeError::new(409, format!("LLM 校对不可用：{}。请先在设置页配置 LLM，或关闭校对", error.user_message)));
        }
    }
    let path = validate_path(&request.path)?;
    let hash = content_hash(&path).await?;
    let transcriber = &core.transcriber;

    if let Some(cached) = transcriber.cache_lookup(&core, &hash, engine) {
        let variant = request
            .proofread
            .then(|| cached.get("proofread").and_then(|p| p.get(variant_key(request.proofread_model_id.as_deref()))).cloned())
            .flatten();
        if !request.proofread || variant.is_some() {
            tracing::info!("文件转写缓存命中: {}（{engine}）", path.display());
            return Ok(Outcome::Result(shape_payload(&cached, request.want_segments, true, variant.as_ref())));
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
    if request.proofread || duration > ASYNC_THRESHOLD_SECONDS {
        return start_job(core.clone(), request, path, hash, duration, engine).map(Outcome::Job);
    }
    let payload = run_transcription(&core, &path, &request.language, duration, &hash, engine).await?;
    Ok(Outcome::Result(shape_payload(&payload, request.want_segments, false, None)))
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
    let _guard = core.transcriber.busy.try_lock().map_err(|_| TranscribeError::busy("转写引擎忙，请稍后重试"))?;
    let samples = media::decode_pcm16k(path).await?;
    if samples.is_empty() {
        return Err(TranscribeError::new(415, "无法解码：没有音频数据"));
    }
    let payload = match crate::stt::local_kind(engine) {
        // 本地引擎（Whisper / Qwen3-ASR）：分段时间戳原样进入 payload，供校对窗口与前端使用
        Some(kind) => {
            let (model_label, transcript) =
                crate::stt::transcribe_local(core.clone(), kind, language.to_string(), samples)
                    .await
                    .map_err(|error| TranscribeError::new(500, format!("转写失败：{error}")))?;
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
        None => run_doubao(core, language, &samples, duration, engine).await?,
    };
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
) -> Result<Value, TranscribeError> {
    let config = crate::stt::doubao_config(core);
    let session_samples = DOUBAO_SESSION_SECONDS * SAMPLE_RATE;
    let mut segments = Vec::new();
    let mut billing_ms = 0i64;
    for (index, chunk) in samples.chunks(session_samples).enumerate() {
        let offset = (index * session_samples) as f64 / SAMPLE_RATE as f64;
        let outcome = crate::stt::doubao::transcribe_pcm(&config, language, "", chunk)
            .await
            .map_err(|e| TranscribeError::new(500, format!("转写失败：{e}")))?;
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
        payload["billing"] = json!({ "audio_duration_seconds": (billing_ms as f64 / 1000.0 * 1000.0).round() / 1000.0 });
    }
    Ok(payload)
}

fn start_job(core: Arc<CoreState>, request: TranscribeRequest, path: PathBuf, hash: String, duration: f64, engine: &'static str) -> Result<String, TranscribeError> {
    if core.transcriber.busy.try_lock().is_err() {
        return Err(TranscribeError::busy("转写引擎忙，请稍后重试"));
    }
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    {
        let mut jobs = core.transcriber.jobs.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut jobs);
        if jobs.len() >= JOB_MAX_ENTRIES {
            return Err(TranscribeError::busy("转写任务表已满，请稍后重试"));
        }
        jobs.insert(
            job_id.clone(),
            Job { status: "running", stage: "asr", progress: None, result: None, error: String::new(), created_at: now_secs() },
        );
    }
    tracing::info!("转写任务已受理 {job_id}（{engine}，校对={}，{duration:.0}s）", request.proofread);
    let id = job_id.clone();
    tokio::spawn(async move {
        let outcome = execute_job(&core, &id, &request, &path, &hash, duration, engine).await;
        core.transcriber.update_job(&id, |job| match outcome {
            Ok(result) => {
                job.stage = "done";
                job.status = "done";
                job.result = Some(result);
            }
            Err(error) => {
                job.status = "failed";
                job.error = error.message;
            }
        });
    });
    Ok(job_id)
}

/// 后台任务体：ASR（缓存命中可跳过）→（可选）校对；校对整体降级不算失败。
async fn execute_job(
    core: &Arc<CoreState>,
    job_id: &str,
    request: &TranscribeRequest,
    path: &Path,
    hash: &str,
    duration: f64,
    engine: &str,
) -> Result<Value, TranscribeError> {
    let mut payload = match core.transcriber.cache_lookup(core, hash, engine) {
        Some(bucket) => bucket,
        None => run_transcription(core, path, &request.language, duration, hash, engine).await?,
    };
    if request.proofread {
        core.transcriber.update_job(job_id, |job| job.stage = "proofreading");
        let segments = payload.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
        let options = proofread::ProofreadOptions::from_request(request.proofread_options.as_ref());
        let progress_core = core.clone();
        let progress_id = job_id.to_string();
        let result = proofread::run(core, request.proofread_model_id.as_deref(), options, segments, move |done, total| {
            progress_core.transcriber.update_job(&progress_id, |job| job.progress = Some(json!({ "window_done": done, "window_total": total })));
        })
        .await
        .map_err(|e| TranscribeError::new(409, format!("LLM 校对不可用：{}", e.user_message)))?;
        payload["raw_text"] = payload.get("text").cloned().unwrap_or_else(|| json!(""));
        if result.applied {
            payload["text"] = json!(result.text);
            payload["segments"] = json!(result.segments);
            // 校对成功才写缓存变体（降级不写，下次可重试校对）
            core.transcriber.store_proofread_variant(core, hash, engine, request.proofread_model_id.as_deref(), &result);
        }
        payload["proofread"] = result.meta();
    }
    Ok(shape_payload(&payload, request.want_segments, false, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 响应整形_校对变体覆盖正文() {
        let entry = json!({"text": "原文", "duration_seconds": 3.0, "engine": "doubao_asr:bigmodel",
            "segments": [{"start": 0.0, "end": 3.0, "text": "原文"}], "billing": {"audio_duration_seconds": 3.0}});
        let plain = shape_payload(&entry, false, true, None);
        assert_eq!(plain["cached"], true);
        assert!(plain.get("segments").is_none());
        let variant = json!({"text": "校对文", "segments": [{"start": 0.0, "end": 3.0, "text": "校对文"}], "windows": 1, "requests": 1, "model_label": "DS"});
        let shaped = shape_payload(&entry, true, true, Some(&variant));
        assert_eq!(shaped["text"], "校对文");
        assert_eq!(shaped["raw_text"], "原文");
        assert_eq!(shaped["proofread"]["llm_model"], "DS");
        assert_eq!(shaped["segments"][0]["text"], "校对文");
    }

    #[test]
    fn 路径与扩展名校验() {
        assert_eq!(validate_path("").unwrap_err().status, 404);
        assert_eq!(validate_path("relative.mp3").unwrap_err().status, 404);
        let dir = std::env::temp_dir().join(format!("att-ft-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("a.txt");
        std::fs::write(&text, b"x").unwrap();
        assert_eq!(validate_path(text.to_str().unwrap()).unwrap_err().status, 415);
        let audio = dir.join("a.MP3");
        std::fs::write(&audio, b"x").unwrap();
        assert!(validate_path(&format!("\"{}\"", audio.display())).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
