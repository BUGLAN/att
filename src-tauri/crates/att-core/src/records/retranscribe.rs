//! 重新识别：读记录的录音 → 按指定（或当前设置的）引擎整段识别 → 写回记录。
//!
//! - 受理时先做完能当场判定的检查（记录在不在、有没有音频、引擎能不能用、音频能不能解码），
//!   再把记录置为 `transcribing`、在后台任务里识别；前端按记录状态轮询结果。
//! - 识别成功覆盖原文并清空优化结果（与「重新录一次」语义一致，不保留多个版本）；
//!   失败只把状态标成 failed 并写明原因，原有文字不动，可以换引擎再试。
//! - 同一条记录同时只跑一个；应用中途退出时，下次启动由 [`recover_interrupted`] 把卡在识别中的记录标成失败。

use super::{Record, RecordError, RecordStatus};
use crate::state::CoreState;
use crate::stt::{self, BatchTranscript, Engine};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// 应用退出打断了重新识别时写进记录的原因。
pub const INTERRUPTED: &str = "重新识别被中断（应用已退出），可以再次重新识别";
/// 识别结果为空时写进记录的原因（与录音收尾的提示一致）。
pub const EMPTY_RESULT: &str = "没有识别到语音内容";

/// 进行中的重新识别（记录 id 集合，只在内存里）。
#[derive(Default)]
pub struct RetranscribeJobs {
    active: Mutex<HashSet<String>>,
}

impl RetranscribeJobs {
    fn lock(&self) -> MutexGuard<'_, HashSet<String>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.lock().contains(id)
    }

    /// 占住这条记录；已有一个在跑时返回 false。
    fn try_begin(&self, id: &str) -> bool {
        self.lock().insert(id.to_string())
    }

    fn finish(&self, id: &str) {
        self.lock().remove(id);
    }

    pub fn is_any_active(&self) -> bool {
        !self.lock().is_empty()
    }
}

/// 后台任务结束（含 panic、受理中途出错）时释放占用。
struct JobGuard {
    core: Arc<CoreState>,
    id: String,
    _model_gate: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.core.retranscribe.finish(&self.id);
    }
}

/// 受理失败的原因；[`RetranscribeError::status`] 是对应的 HTTP 状态码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetranscribeError {
    /// 记录不存在
    NotFound,
    /// 记录没有音频（优化选中文本建的文本记录、录音落盘失败、音频文件已不在）
    NoAudio,
    /// 这条记录正在重新识别或正在优化
    Busy(&'static str),
    /// 不认识的 provider
    UnknownEngine(String),
    /// 引擎不可用（本地模型缺失、在线凭据未配置）：现有的一句结论
    EngineUnavailable(String),
    /// 音频读不出来
    BadAudio(String),
    /// 写记录失败
    Storage(String),
}

impl RetranscribeError {
    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound | Self::NoAudio => 404,
            Self::Busy(_) | Self::EngineUnavailable(_) => 409,
            Self::UnknownEngine(_) | Self::BadAudio(_) => 422,
            Self::Storage(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "记录不存在".into(),
            Self::NoAudio => "这条记录没有录音音频，无法重新识别".into(),
            Self::Busy(reason) => (*reason).into(),
            Self::UnknownEngine(provider) => {
                format!("不支持的识别引擎：{provider}（可选 {}）", stt::PROVIDERS.join(" / "))
            }
            Self::EngineUnavailable(message) => message.clone(),
            Self::BadAudio(message) => format!("录音读取失败：{message}"),
            Self::Storage(message) => format!("保存记录失败：{message}"),
        }
    }
}

impl From<RecordError> for RetranscribeError {
    fn from(error: RecordError) -> Self {
        match error {
            RecordError::NotFound => Self::NotFound,
            other => Self::Storage(other.to_string()),
        }
    }
}

/// 归一 provider：空 = 当前设置的引擎；设置里的空串按豆包处理（与 [`Engine::online_from_settings`] 一致）。
fn normalize_provider(requested: Option<&str>, current: &str) -> String {
    let provider = requested.map(str::trim).filter(|p| !p.is_empty()).unwrap_or(current.trim());
    if provider.is_empty() {
        "doubao_asr".into()
    } else {
        provider.to_string()
    }
}

/// 受理一次重新识别：检查通过后把记录置为 `transcribing`、在后台识别，返回置位后的记录。
///
/// `provider` 为空时用当前设置的引擎。须在 tokio 运行时里调用（识别任务 spawn 在当前运行时上）。
pub async fn start(core: Arc<CoreState>, id: &str, provider: Option<&str>) -> Result<Record, RetranscribeError> {
    let audio = core.records.audio_file(id)?.ok_or(RetranscribeError::NoAudio)?;
    let settings = core.settings();
    let provider = normalize_provider(provider, &settings.asr_provider);
    if !stt::PROVIDERS.contains(&provider.as_str()) {
        return Err(RetranscribeError::UnknownEngine(provider));
    }
    if core.optimizer.is_active(id) {
        return Err(RetranscribeError::Busy("这条记录正在优化，结束后再重新识别"));
    }
    if core.retranscribe.is_active(id) {
        return Err(RetranscribeError::Busy("这条记录正在重新识别"));
    }
    let model_gate = if stt::local_kind(&provider).is_some() {
        Some(
            core.model_gate
                .clone()
                .try_read_owned()
                .map_err(|_| RetranscribeError::Busy("本地模型正在维护，稍后再重新识别"))?,
        )
    } else {
        None
    };
    let engine = Engine::resolve_provider(&core, &provider).map_err(RetranscribeError::EngineUnavailable)?;
    // 当场解码一遍：音频坏了直接报错，不把记录置为识别中
    let samples = tokio::task::spawn_blocking(move || crate::audio::read_audio_16k_mono(&audio))
        .await
        .map_err(|error| RetranscribeError::BadAudio(error.to_string()))?
        .map_err(|error| RetranscribeError::BadAudio(error.to_string()))?;
    if samples.is_empty() {
        return Err(RetranscribeError::BadAudio("音频是空的".into()));
    }

    if !core.retranscribe.try_begin(id) {
        return Err(RetranscribeError::Busy("这条记录正在重新识别"));
    }
    let guard = JobGuard { core: core.clone(), id: id.to_string(), _model_gate: model_gate };
    let marked = core.records.patch(id, |record| record.status = RecordStatus::Transcribing)?;
    // 重新识别可以换引擎：设置里的语言是按当前引擎选的，换到不能指定语言的引擎时记为 auto
    let language = stt::effective_language(&provider, &settings.asr_language);
    tracing::info!("重新识别开始（记录 {id}，{}，{:.1}s）", engine.label(), samples.len() as f64 / 16_000.0);
    tokio::spawn(async move {
        let guard = guard;
        run(&guard.core, &guard.id, engine, language, samples).await;
    });
    Ok(marked)
}

/// 后台识别并写回记录。
async fn run(core: &CoreState, id: &str, engine: Engine, language: String, samples: Vec<i16>) {
    let started = Instant::now();
    let result = engine.transcribe_pcm(&language, &samples).await;
    let elapsed = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    let provider = engine.provider();
    match core.records.patch(id, |record| apply_outcome(record, provider, &language, result, elapsed)) {
        Ok(record) => {
            tracing::info!("重新识别结束（记录 {id}，{}，{}，{elapsed:.2}s）", engine.label(), record.status.as_str())
        }
        // 识别期间记录被删了：结果无处可写，丢弃即可
        Err(error) => tracing::warn!("重新识别结果写回失败（记录 {id}）：{error}"),
    }
}

/// 把识别结果写进记录：成功覆盖原文、清空优化结果；失败（含识别出空文本）只改状态与原因。
/// 两种情况都记下这次用的引擎与语言。
fn apply_outcome(
    record: &mut Record,
    provider: &str,
    language: &str,
    result: Result<BatchTranscript, String>,
    elapsed: f64,
) {
    record.engine = Some(provider.to_string());
    record.language = Some(language.to_string());
    match result {
        Ok(transcript) if !transcript.text.trim().is_empty() => {
            record.raw_text = Some(transcript.text);
            record.optimized_text = None;
            record.optimize_time = None;
            record.is_modified = false;
            record.status = RecordStatus::Transcribed;
            // 部分分段失败等降级原因照常记下（与录音收尾一致）
            record.error = transcript.warning;
            record.processing_time = Some(elapsed);
        }
        Ok(_) => {
            record.status = RecordStatus::Failed;
            record.error = Some(EMPTY_RESULT.into());
        }
        Err(message) => {
            record.status = RecordStatus::Failed;
            record.error = Some(message);
        }
    }
}

/// 启动时调用：上次退出时还在重新识别的记录（后台任务随进程没了）标成失败，文字原样保留。返回改了几条。
pub fn recover_interrupted(core: &CoreState) -> usize {
    core.records
        .transcribing_ids()
        .into_iter()
        .filter(|id| {
            core.records
                .patch(id, |record| {
                    // 锁内再确认一次：恢复跑的同时用户可能已经点了新的重新识别
                    if record.status == RecordStatus::Transcribing && !core.retranscribe.is_active(&record.id) {
                        record.status = RecordStatus::Failed;
                        record.error = Some(INTERRUPTED.into());
                    }
                })
                .is_ok_and(|record| record.status == RecordStatus::Failed)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(tag: &str) -> (Arc<CoreState>, std::path::PathBuf) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("att-retranscribe-{tag}-{}-{stamp}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Arc::new(CoreState::init(&dir, crate::Platform::current()).unwrap()), dir)
    }

    fn sample_record(id: &str, audio: &str) -> Record {
        Record {
            id: id.into(),
            audio_path: audio.into(),
            duration: 1.0,
            raw_text: Some("旧原文".into()),
            optimized_text: Some("旧优化".into()),
            optimize_time: Some(0.8),
            is_modified: true,
            status: RecordStatus::Optimized,
            ..Default::default()
        }
    }

    #[test]
    fn 识别成功_覆盖原文并清空优化结果() {
        let mut record = sample_record("rec-a", "a.flac");
        apply_outcome(
            &mut record,
            "deepseek_asr",
            "zh",
            Ok(BatchTranscript { text: "新原文".into(), warning: None }),
            1.234,
        );
        assert_eq!(record.raw_text.as_deref(), Some("新原文"));
        assert_eq!(record.optimized_text, None);
        assert_eq!(record.optimize_time, None);
        assert!(!record.is_modified);
        assert_eq!(record.status, RecordStatus::Transcribed);
        assert_eq!(record.error, None);
        assert_eq!(record.processing_time, Some(1.234));
        assert_eq!(record.engine.as_deref(), Some("deepseek_asr"));
        assert_eq!(record.language.as_deref(), Some("zh"));
    }

    #[test]
    fn 识别失败或为空_只改状态与原因_原文不动() {
        let mut failed = sample_record("rec-b", "b.flac");
        apply_outcome(&mut failed, "doubao_asr", "en", Err("网络连接失败".into()), 0.5);
        assert_eq!(failed.status, RecordStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("网络连接失败"));
        assert_eq!(failed.raw_text.as_deref(), Some("旧原文"), "失败不能丢掉原有文字");
        assert_eq!(failed.optimized_text.as_deref(), Some("旧优化"));
        assert_eq!(failed.engine.as_deref(), Some("doubao_asr"));
        assert_eq!(failed.language.as_deref(), Some("en"));

        let mut empty = sample_record("rec-c", "c.flac");
        apply_outcome(&mut empty, "whisper", "zh", Ok(BatchTranscript { text: "  ".into(), warning: None }), 0.5);
        assert_eq!(empty.status, RecordStatus::Failed);
        assert_eq!(empty.error.as_deref(), Some(EMPTY_RESULT));
        assert_eq!(empty.raw_text.as_deref(), Some("旧原文"));
    }

    #[test]
    fn provider_归一() {
        assert_eq!(normalize_provider(None, "whisper"), "whisper");
        assert_eq!(normalize_provider(Some("  "), "deepseek_asr"), "deepseek_asr");
        assert_eq!(normalize_provider(Some("qwen_asr"), "doubao_asr"), "qwen_asr");
        assert_eq!(normalize_provider(None, ""), "doubao_asr", "旧设置的空 provider 按豆包");
    }

    #[tokio::test]
    async fn 受理检查_记录不存在_没有音频_引擎不认识或不可用() {
        let (core, dir) = core("checks");
        assert_eq!(start(core.clone(), "rec-missing", None).await.unwrap_err(), RetranscribeError::NotFound);

        // 文本记录（没有音频）
        core.records
            .insert(Record { id: "rec-text".into(), raw_text: Some("选中原文".into()), ..Default::default() })
            .unwrap();
        let error = start(core.clone(), "rec-text", None).await.unwrap_err();
        assert_eq!(error, RetranscribeError::NoAudio);
        assert_eq!(error.status(), 404);

        // 有音频的记录：不认识的引擎 422；豆包没配凭据 409；记录状态都不变
        let audio = core.records.audio_dir().join("rec-audio.flac");
        crate::audio::write_recording_16k(&audio, &vec![0i16; 16_000]).unwrap();
        core.records.insert(sample_record("rec-audio", "rec-audio.flac")).unwrap();
        let error = start(core.clone(), "rec-audio", Some("nope")).await.unwrap_err();
        assert_eq!(error.status(), 422, "{error:?}");
        let error = start(core.clone(), "rec-audio", Some("doubao_asr")).await.unwrap_err();
        assert_eq!(error.status(), 409, "{error:?}");
        assert!(error.message().contains("豆包"), "{}", error.message());
        assert_eq!(core.records.get("rec-audio").unwrap().status, RecordStatus::Optimized);
        assert!(!core.retranscribe.is_active("rec-audio"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 启动恢复_卡在识别中的记录标成失败_进行中的不动() {
        let (core, dir) = core("recover");
        for id in ["rec-stuck", "rec-running", "rec-done"] {
            core.records.insert(sample_record(id, &format!("{id}.flac"))).unwrap();
        }
        core.records.patch("rec-stuck", |r| r.status = RecordStatus::Transcribing).unwrap();
        core.records.patch("rec-running", |r| r.status = RecordStatus::Transcribing).unwrap();
        assert!(core.retranscribe.try_begin("rec-running"));

        assert_eq!(recover_interrupted(&core), 1);
        let stuck = core.records.get("rec-stuck").unwrap();
        assert_eq!(stuck.status, RecordStatus::Failed);
        assert_eq!(stuck.error.as_deref(), Some(INTERRUPTED));
        assert_eq!(stuck.raw_text.as_deref(), Some("旧原文"), "恢复不能动文字");
        assert_eq!(core.records.get("rec-running").unwrap().status, RecordStatus::Transcribing);
        assert_eq!(core.records.get("rec-done").unwrap().status, RecordStatus::Optimized);
        core.retranscribe.finish("rec-running");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
