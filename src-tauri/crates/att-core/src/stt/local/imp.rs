//! 本地引擎的装配与常驻池（faster-whisper / Qwen3-ASR）。
//!
//! 两个引擎都是**进程内推理**，模型加载要几秒、单次推理要几百毫秒到几秒，所以这里做三件事：
//!
//! 1. **常驻**：加载过的模型留在池里（`CoreState.local_models`），录音与文件转写共用一份；
//! 2. **设置指纹**：模型/设备/分块参数一变就丢弃旧实例，下次使用时按新设置重载；
//! 3. **可读的就绪性**：`/api/health`、`/api/model-service` 只看文件是否就位、模型是否已加载，
//!    不触发加载，避免状态轮询把显存/内存顶满。
//!
//! 另外 Whisper 走一条「有 VAD 就用 VAD」的前置路径：Python 版 `vad_filter=True` 是 Silero VAD，
//! 少了它，噪声/音乐素材上 large-v3 会整段输出固定幻觉文本（实测过）。VAD 模型缺失时自动退回整段推理。

use crate::settings::UserSettings;
use crate::state::CoreState;
use att_qwen::{LocalQwen, QwenConfig, SileroVad};
use att_whisper::{LocalWhisper, WhisperConfig, SAMPLE_RATE};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 本地 Whisper 的 provider 标识（与设置 `asr_provider` 一致）。
pub const WHISPER: &str = "whisper";
/// 本地 Qwen3-ASR 的 provider 标识。
pub const QWEN: &str = "qwen_asr";
/// VAD 区间前后各留的余量（对应 Python `vad_parameters.speech_pad_ms=200`）。
const VAD_PAD_MS: usize = 200;
/// 相邻语音区间间隔小于该值时合并，避免同一句话被切成两段重复识别。
const VAD_MERGE_GAP_MS: usize = 400;
/// 合并后的区间不超过该时长（Whisper 本身按 30 秒窗口推理）：长文件的进度与取消按区间推进，
/// 连续讲话合成一整块会让进度长时间不动、取消也要等整块推理完。只在 VAD 的静音间隙处断开。
const VAD_MERGE_MAX_SECONDS: usize = 30;
/// 没有 VAD 时，超过该时长的音频按能量静音点切成 ≤[`VAD_MERGE_MAX_SECONDS`] 的块逐块推理（文件转写的进度与取消
/// 靠它推进）；更短的（口述录音）照旧整段推理，行为不变。
const NO_VAD_CHUNK_ABOVE_SECONDS: usize = 60;

/// 本地引擎种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Whisper,
    Qwen,
}

impl LocalKind {
    /// provider 字符串 → 本地引擎种类（在线引擎返回 `None`）。
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider.trim() {
            WHISPER => Some(Self::Whisper),
            QWEN => Some(Self::Qwen),
            _ => None,
        }
    }

    pub fn provider(&self) -> &'static str {
        match self {
            Self::Whisper => WHISPER,
            Self::Qwen => QWEN,
        }
    }

    /// 界面文案（错误提示里用）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Whisper => "本地 Whisper",
            Self::Qwen => "本地 Qwen3-ASR",
        }
    }
}

/// 一段本地转写结果（两个引擎的结果在这里归一，避免上层按引擎分叉）。
#[derive(Debug, Clone)]
pub struct LocalTranscript {
    pub text: String,
    pub segments: Vec<LocalSegment>,
    pub duration: f64,
    pub elapsed: f64,
    pub device: String,
}

/// 转写分段（秒）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// 已加载的本地引擎（句柄在各 crate 内维护，这里只持有 Arc）。
#[derive(Debug, Clone)]
pub struct LocalEngine {
    kind: LocalKind,
    whisper: Option<Arc<LocalWhisper>>,
    qwen: Option<Arc<LocalQwen>>,
    /// 仅供 Whisper：先把非语音段剔掉（Silero VAD）
    vad: Option<Arc<SileroVad>>,
    model_dir: PathBuf,
}

impl LocalEngine {
    fn whisper(engine: Arc<LocalWhisper>, vad: Option<Arc<SileroVad>>) -> Self {
        Self {
            kind: LocalKind::Whisper,
            model_dir: engine.model_dir().to_path_buf(),
            whisper: Some(engine),
            qwen: None,
            vad,
        }
    }

    fn qwen(engine: Arc<LocalQwen>) -> Self {
        Self {
            kind: LocalKind::Qwen,
            model_dir: engine.model_dir().to_path_buf(),
            whisper: None,
            qwen: Some(engine),
            vad: None,
        }
    }

    pub fn kind(&self) -> LocalKind {
        self.kind
    }

    pub fn label(&self) -> &'static str {
        self.kind.label()
    }

    /// 是否挂了 VAD（设置页/日志展示用）。
    pub fn has_vad(&self) -> bool {
        self.vad.is_some()
    }

    /// 设备/精度描述（`cuda:float16` / `cpu:int8` / `cpu:onnx`）。
    pub fn device(&self) -> String {
        match (&self.whisper, &self.qwen) {
            (Some(engine), _) => engine.device().to_string(),
            (_, Some(_)) => "cpu:onnx".to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// 写入 payload 的 `engine` 字段（与 Python `f"{engine}:{model_name}"` 同形）。
    pub fn model_label(&self) -> String {
        let model = self.model_dir.file_name().and_then(|name| name.to_str()).unwrap_or("local");
        format!("{}:{model}", self.kind.provider())
    }

    /// 预热（加载后跑一次极短推理，把 ONNX/CTranslate2 的首次开销提前吃掉）。
    pub fn warmup(&self) {
        match (&self.whisper, &self.qwen) {
            (Some(engine), _) => engine.warmup(),
            (_, Some(engine)) => engine.warmup(),
            _ => {}
        }
    }

    /// 阻塞式整段转写（调用方负责丢进 `spawn_blocking`）。
    pub fn transcribe(&self, language: &str, samples: &[i16]) -> Result<LocalTranscript, String> {
        self.transcribe_with_progress(language, samples, &mut |_, _| true)
    }

    /// 同 [`Self::transcribe`]，按分块（Whisper 的 VAD 区间、Qwen 的分块）回调进度：
    /// 每块推理前回调（已处理样本数, 总样本数），完成后回调（总数, 总数）；回调返回 false 时停在下一块之前，
    /// 返回 [`crate::stt::TRANSCRIBE_CANCELLED`]。正在推理的那一块无法中途打断。
    pub fn transcribe_with_progress(
        &self,
        language: &str,
        samples: &[i16],
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<LocalTranscript, String> {
        match (&self.whisper, &self.qwen) {
            (Some(whisper), _) => transcribe_whisper(whisper, self.vad.as_deref(), language, samples, on_chunk),
            (_, Some(qwen)) => {
                qwen.transcribe_with_progress(language, samples, on_chunk).map(|outcome| LocalTranscript {
                    text: outcome.text,
                    segments: outcome
                        .segments
                        .into_iter()
                        .map(|segment| LocalSegment { start: segment.start, end: segment.end, text: segment.text })
                        .collect(),
                    duration: outcome.duration,
                    elapsed: outcome.elapsed,
                    device: "cpu:onnx".to_string(),
                })
            }
            _ => Err("本地引擎未就绪".into()),
        }
    }
}

/// Whisper 结果 → 统一结果结构。
fn from_whisper(outcome: att_whisper::Transcript) -> LocalTranscript {
    LocalTranscript {
        text: outcome.text,
        segments: outcome
            .segments
            .into_iter()
            .map(|segment| LocalSegment { start: segment.start, end: segment.end, text: segment.text })
            .collect(),
        duration: outcome.duration,
        elapsed: outcome.elapsed,
        device: outcome.device,
    }
}

/// Whisper 转写：有 VAD 就先按语音区间切，逐段推理后按偏移拼回时间戳。
///
/// 无 VAD 模型时退回整段推理（与没有 `vad_filter` 的 Python 版行为一致）。
fn transcribe_whisper(
    whisper: &LocalWhisper,
    vad: Option<&SileroVad>,
    language: &str,
    samples: &[i16],
    on_chunk: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<LocalTranscript, String> {
    let duration = samples.len() as f64 / SAMPLE_RATE as f64;
    let total = samples.len();
    // 没有 VAD：短音频整段推理（只有开始与结束两个进度点）；长音频按静音点切块，进度与取消逐块推进
    let without_vad = |on_chunk: &mut dyn FnMut(usize, usize) -> bool| {
        if total > NO_VAD_CHUNK_ABOVE_SECONDS * SAMPLE_RATE {
            let cuts = att_qwen::silence_cut_points(samples, SAMPLE_RATE);
            let chunks = att_qwen::plan_chunks(total, SAMPLE_RATE, VAD_MERGE_MAX_SECONDS as i64, &cuts);
            tracing::info!("本地 Whisper（无 VAD）：{duration:.1}s 音频按静音点分 {} 块推理", chunks.len());
            return transcribe_whisper_chunks(whisper, language, samples, chunks, on_chunk, "静音点分块");
        }
        if !on_chunk(0, total) {
            return Err(crate::stt::TRANSCRIBE_CANCELLED.to_string());
        }
        let outcome = whisper.transcribe(language, samples).map(from_whisper)?;
        on_chunk(total, total);
        Ok(outcome)
    };
    let Some(vad) = vad else {
        return without_vad(on_chunk);
    };
    let ranges = match vad.speech_ranges(samples) {
        Ok(ranges) => ranges,
        Err(error) => {
            tracing::warn!("Silero VAD 失败，退回无 VAD 识别：{error}");
            return without_vad(on_chunk);
        }
    };
    if ranges.is_empty() {
        tracing::info!("Silero VAD 未检测到语音，跳过推理");
        return Ok(LocalTranscript {
            text: String::new(),
            segments: Vec::new(),
            duration,
            elapsed: 0.0,
            device: whisper.device().to_string(),
        });
    }
    let pad = SAMPLE_RATE * VAD_PAD_MS / 1000;
    let merge_gap = SAMPLE_RATE * VAD_MERGE_GAP_MS / 1000;
    let merged = merge_speech_ranges(ranges, samples.len(), pad, merge_gap, SAMPLE_RATE * VAD_MERGE_MAX_SECONDS);
    transcribe_whisper_chunks(whisper, language, samples, merged, on_chunk, "VAD 分段")
}

/// 逐块推理并按块偏移拼回时间戳；每块之前回调进度，回调返回 false 时停下（取消）。
fn transcribe_whisper_chunks(
    whisper: &LocalWhisper,
    language: &str,
    samples: &[i16],
    chunks: Vec<(usize, usize)>,
    on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    how: &str,
) -> Result<LocalTranscript, String> {
    let duration = samples.len() as f64 / SAMPLE_RATE as f64;
    let total = samples.len();
    let mut segments: Vec<LocalSegment> = Vec::new();
    let mut elapsed = 0.0;
    for (start, end) in chunks {
        if end <= start {
            continue;
        }
        if !on_chunk(start, total) {
            return Err(crate::stt::TRANSCRIBE_CANCELLED.to_string());
        }
        let offset = start as f64 / SAMPLE_RATE as f64;
        let outcome = whisper.transcribe(language, &samples[start..end])?;
        elapsed += outcome.elapsed;
        for segment in outcome.segments {
            segments.push(LocalSegment {
                start: segment.start + offset,
                end: segment.end + offset,
                text: segment.text,
            });
        }
    }
    on_chunk(total, total);
    let text = segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>().join(" ");
    tracing::info!("本地 Whisper（{how}）完成：{} 段文本，音频 {duration:.1}s，推理 {elapsed:.2}s", segments.len());
    Ok(LocalTranscript { text, segments, duration, elapsed, device: whisper.device().to_string() })
}

/// VAD 语音区间加余量后合并：间隔小于 `merge_gap` 的相邻区间合成一块，但合并后不超过 `max_len`
/// （单个区间本身超长时保持原样，不在语音中间硬切）。
fn merge_speech_ranges(
    ranges: Vec<(usize, usize)>,
    len: usize,
    pad: usize,
    merge_gap: usize,
    max_len: usize,
) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        let start = start.saturating_sub(pad);
        let end = (end + pad).min(len);
        match merged.last_mut() {
            Some(last) if start <= last.1 + merge_gap && end.max(last.1) - last.0 <= max_len => {
                last.1 = last.1.max(end)
            }
            // 与上一块重叠（余量相交）但合并会超长：从上一块末尾接着切，避免重复识别同一段
            Some(last) if start < last.1 => {
                let from = last.1;
                if end > from {
                    merged.push((from, end));
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// 池中一项：加载时的设置指纹 + 句柄。
struct Cached<T> {
    engine: Arc<T>,
    fingerprint: String,
}

/// 本地引擎常驻池（懒加载、设置变化自动重建、可显式卸载）。
#[derive(Default)]
pub struct LocalModels {
    whisper: Mutex<Option<Cached<LocalWhisper>>>,
    qwen: Mutex<Option<Cached<LocalQwen>>>,
    /// Silero VAD（可选；只跟着模型根目录走，加载失败就不启用）
    vad: Mutex<Option<Arc<SileroVad>>>,
    /// 加载串行化：并发请求不会同时加载两份模型
    load_gate: Mutex<()>,
    /// 正在加载的引擎（加载闸门保证同一时刻至多一个）：设置页/首页据此显示「加载中」
    loading: Mutex<Option<LocalKind>>,
    /// 已排队、后台线程还没真正开始加载的预加载（与 `loading` 一起算「加载中」，
    /// 否则切换引擎后立刻轮询会先看到一次「未加载」再跳成「加载中」）
    queued: Mutex<Option<LocalKind>>,
    /// 最近一次加载失败：(引擎, 设置指纹, 原因)；设置一变指纹就对不上，自动作废
    failure: Mutex<Option<(LocalKind, String, String)>>,
}

/// 加载期间把 `loading` 置位，离开作用域（含 panic 展开）时复位，避免状态卡在「加载中」。
struct LoadingMark<'a>(&'a Mutex<Option<LocalKind>>);

impl<'a> LoadingMark<'a> {
    fn new(slot: &'a Mutex<Option<LocalKind>>, kind: LocalKind) -> Self {
        *slot.lock().unwrap_or_else(|error| error.into_inner()) = Some(kind);
        Self(slot)
    }
}

impl Drop for LoadingMark<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }
}

/// 本地引擎就绪性（供 /api/health 与设置页展示；不触发加载）。
#[derive(Debug, Clone)]
pub struct LocalStatus {
    /// 模型（以及 Qwen 的推理库）文件是否就位
    pub available: bool,
    /// 是否已加载常驻
    pub loaded: bool,
    /// 是否正在后台加载
    pub loading: bool,
    /// 不可用原因（available=false 时）：只有一句结论，如「本地 Whisper 模型未安装」，
    /// 首页、诊断页、录音与文件转写的报错都只用这一句
    pub error: Option<String>,
    /// 不可用时的详情：模型目录与获取方式（Tab 缩进的行是命令），只在设置页折叠展示
    pub error_detail: Option<String>,
    /// 当前设置下最近一次加载失败的原因（文件在、但模型/推理库加载不起来）
    pub load_error: Option<String>,
    /// 模型目录（可用时）
    pub model_dir: Option<PathBuf>,
    /// 设备描述（已加载时）
    pub device: Option<String>,
}

/// 把引擎 crate 的两段式错误拆成（首行结论, 详情）：详情为空时返回 `None`。
///
/// 引擎 crate 约定：首行一句结论，其后是说明与 Tab 缩进的命令（见 `att_whisper::resolve_model_dir`）。
pub fn split_hint(error: &str) -> (String, Option<String>) {
    let trimmed = error.trim_end();
    match trimmed.split_once('\n') {
        Some((summary, detail)) if !detail.trim().is_empty() => (summary.trim().to_string(), Some(detail.to_string())),
        Some((summary, _)) => (summary.trim().to_string(), None),
        None => (trimmed.trim().to_string(), None),
    }
}

/// 设置指纹：这些键一变就要重载模型（与 `MODEL_RUNTIME_KEYS` 对齐）。
fn fingerprint(kind: LocalKind, settings: &UserSettings, models_dir: &std::path::Path) -> String {
    let mut parts: Vec<String> = match kind {
        LocalKind::Whisper => vec![
            format!("model={}", settings.whisper_model),
            format!("device={}", settings.whisper_device),
            format!("compute={}", whisper_compute_type()),
        ],
        LocalKind::Qwen => vec![
            format!("model={}", settings.qwen_asr_model),
            format!("max_new_tokens={}", settings.qwen_asr_max_new_tokens),
            format!("max_audio_seconds={}", settings.qwen_asr_max_audio_seconds),
        ],
    };
    parts.push(format!("models_dir={}", models_dir.display()));
    parts.join("|")
}

/// Whisper 计算精度：默认 auto（CUDA→float16，CPU→int8），可用 `ATT_WHISPER_COMPUTE_TYPE` 覆盖。
fn whisper_compute_type() -> String {
    std::env::var("ATT_WHISPER_COMPUTE_TYPE").unwrap_or_else(|_| "auto".into())
}

fn whisper_config(core: &CoreState) -> WhisperConfig {
    let settings = core.settings();
    WhisperConfig::new(settings.whisper_model.clone(), settings.whisper_device.clone(), core.models_dir.clone())
        .with_compute_type(whisper_compute_type())
}

fn qwen_config(core: &CoreState) -> QwenConfig {
    let settings = core.settings();
    let mut config = QwenConfig::new(settings.qwen_asr_model.clone(), core.models_dir.clone());
    config.bundled_lib_dir = core.resource_dir.as_ref().map(|dir| dir.join("sherpa-onnx"));
    config.max_new_tokens = settings.qwen_asr_max_new_tokens;
    config.max_audio_seconds = settings.qwen_asr_max_audio_seconds;
    // 默认 2 线程：与 sherpa-onnx 官方示例一致，再高在 CPU 上收益有限
    config.num_threads = std::env::var("ATT_QWEN_THREADS").ok().and_then(|value| value.parse().ok()).unwrap_or(2);
    config
}

impl LocalModels {
    /// 指定引擎是否已加载常驻。
    pub fn is_loaded(&self, kind: LocalKind) -> bool {
        match kind {
            LocalKind::Whisper => self.whisper.lock().unwrap_or_else(|error| error.into_inner()).is_some(),
            LocalKind::Qwen => self.qwen.lock().unwrap_or_else(|error| error.into_inner()).is_some(),
        }
    }

    /// Silero VAD 句柄（懒加载；模型缺失或加载失败时返回 `None`，调用方自动退回整段推理）。
    fn vad(&self, core: &CoreState) -> Option<Arc<SileroVad>> {
        if let Some(vad) = self.vad.lock().unwrap_or_else(|error| error.into_inner()).as_ref() {
            return Some(vad.clone());
        }
        match SileroVad::load(&qwen_config(core)) {
            Ok(vad) => {
                let vad = Arc::new(vad);
                *self.vad.lock().unwrap_or_else(|error| error.into_inner()) = Some(vad.clone());
                Some(vad)
            }
            Err(error) => {
                tracing::warn!("Silero VAD 不可用，Whisper 将整段识别：{error}");
                None
            }
        }
    }

    /// 取（必要时加载）引擎；阻塞，调用方应在 `spawn_blocking` 中调用。
    pub fn get(&self, core: &CoreState, kind: LocalKind) -> Result<LocalEngine, String> {
        let settings = core.settings();
        let expected = fingerprint(kind, &settings, &core.models_dir);
        // 快路径：已加载且设置未变
        if let Some(engine) = self.cached(kind, &expected) {
            return Ok(engine);
        }
        let _guard = self.load_gate.lock().unwrap_or_else(|error| error.into_inner());
        // 二次检查：等锁期间可能已被其它请求加载
        if let Some(engine) = self.cached(kind, &expected) {
            return Ok(engine);
        }
        self.invalidate(kind);
        let _mark = LoadingMark::new(&self.loading, kind);
        let result = self.load(core, kind, expected.clone());
        let mut failure = self.failure.lock().unwrap_or_else(|error| error.into_inner());
        match &result {
            Ok(_) => *failure = None,
            Err(error) => *failure = Some((kind, expected, error.clone())),
        }
        result
    }

    fn load(&self, core: &CoreState, kind: LocalKind, fingerprint: String) -> Result<LocalEngine, String> {
        match kind {
            LocalKind::Whisper => {
                let engine = Arc::new(LocalWhisper::load(&whisper_config(core))?);
                let vad = self.vad(core);
                *self.whisper.lock().unwrap_or_else(|error| error.into_inner()) =
                    Some(Cached { engine: engine.clone(), fingerprint });
                Ok(LocalEngine::whisper(engine, vad))
            }
            LocalKind::Qwen => {
                let engine = Arc::new(LocalQwen::load(&qwen_config(core))?);
                *self.qwen.lock().unwrap_or_else(|error| error.into_inner()) =
                    Some(Cached { engine: engine.clone(), fingerprint });
                Ok(LocalEngine::qwen(engine))
            }
        }
    }

    /// 指定引擎是否正在后台加载（含已排队的预加载）。
    pub fn is_loading(&self, kind: LocalKind) -> bool {
        *self.loading.lock().unwrap_or_else(|error| error.into_inner()) == Some(kind)
            || *self.queued.lock().unwrap_or_else(|error| error.into_inner()) == Some(kind)
    }

    /// 登记一次预加载（[`crate::stt::preload_local`] 用）；已有同引擎排队时返回 `false`。
    pub fn enqueue_preload(&self, kind: LocalKind) -> bool {
        let mut queued = self.queued.lock().unwrap_or_else(|error| error.into_inner());
        if *queued == Some(kind) {
            return false;
        }
        *queued = Some(kind);
        true
    }

    /// 预加载结束（成功或失败）后清掉排队标记。
    pub fn finish_preload(&self, kind: LocalKind) {
        let mut queued = self.queued.lock().unwrap_or_else(|error| error.into_inner());
        if *queued == Some(kind) {
            *queued = None;
        }
    }

    fn cached(&self, kind: LocalKind, expected: &str) -> Option<LocalEngine> {
        match kind {
            LocalKind::Whisper => {
                let guard = self.whisper.lock().unwrap_or_else(|error| error.into_inner());
                let cached = guard.as_ref().filter(|cached| cached.fingerprint == expected)?;
                let vad = self.vad.lock().unwrap_or_else(|error| error.into_inner()).clone();
                Some(LocalEngine::whisper(cached.engine.clone(), vad))
            }
            LocalKind::Qwen => {
                let guard = self.qwen.lock().unwrap_or_else(|error| error.into_inner());
                let cached = guard.as_ref().filter(|cached| cached.fingerprint == expected)?;
                Some(LocalEngine::qwen(cached.engine.clone()))
            }
        }
    }

    fn invalidate(&self, kind: LocalKind) {
        match kind {
            LocalKind::Whisper => *self.whisper.lock().unwrap_or_else(|error| error.into_inner()) = None,
            LocalKind::Qwen => *self.qwen.lock().unwrap_or_else(|error| error.into_inner()) = None,
        }
    }

    /// 卸载（`None` = 全部）：`/api/model-service/stop` 与设置页「释放模型」用。
    pub fn unload(&self, kind: Option<LocalKind>) {
        match kind {
            Some(kind) => self.invalidate(kind),
            None => {
                self.invalidate(LocalKind::Whisper);
                self.invalidate(LocalKind::Qwen);
                *self.vad.lock().unwrap_or_else(|error| error.into_inner()) = None;
            }
        }
    }

    /// 就绪性探测：只看文件与池状态，绝不加载模型。
    pub fn status(&self, core: &CoreState, kind: LocalKind) -> LocalStatus {
        let settings = core.settings();
        let expected = fingerprint(kind, &settings, &core.models_dir);
        let loaded = self.cached(kind, &expected).is_some();
        let loading = self.is_loading(kind);
        let load_error = self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|(failed, fingerprint, _)| *failed == kind && *fingerprint == expected)
            .map(|(_, _, error)| error.clone());
        let unavailable = |error: String| {
            let (summary, detail) = split_hint(&error);
            LocalStatus {
                available: false,
                loaded,
                loading,
                error: Some(summary),
                error_detail: detail,
                load_error: None,
                model_dir: None,
                device: None,
            }
        };
        match kind {
            LocalKind::Whisper => {
                let config = whisper_config(core);
                match att_whisper::resolve_model_dir(&config) {
                    Ok(dir) => LocalStatus {
                        available: true,
                        loaded,
                        loading,
                        error: None,
                        error_detail: None,
                        load_error,
                        model_dir: Some(dir),
                        device: self.device_of(kind),
                    },
                    Err(error) => unavailable(error),
                }
            }
            LocalKind::Qwen => {
                let config = qwen_config(core);
                let model = att_qwen::resolve_model_dir(&config);
                let lib = att_qwen::resolve_lib_dir(&config);
                match (model, lib) {
                    (Ok(dir), Ok(_)) => LocalStatus {
                        available: true,
                        loaded,
                        loading,
                        error: None,
                        error_detail: None,
                        load_error,
                        model_dir: Some(dir),
                        device: self.device_of(kind),
                    },
                    (Err(error), _) | (_, Err(error)) => unavailable(error),
                }
            }
        }
    }

    /// VAD 是否已加载（健康面板/测试用）。
    pub fn vad_loaded(&self) -> bool {
        self.vad.lock().unwrap_or_else(|error| error.into_inner()).is_some()
    }

    /// 只释放 VAD（删除/替换 `silero_vad.onnx` 时用，不影响已加载的识别引擎）。
    pub fn unload_vad(&self) {
        *self.vad.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }

    fn device_of(&self, kind: LocalKind) -> Option<String> {
        match kind {
            LocalKind::Whisper => self
                .whisper
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|cached| cached.engine.device().to_string()),
            LocalKind::Qwen => {
                self.qwen.lock().unwrap_or_else(|error| error.into_inner()).as_ref().map(|_| "cpu:onnx".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;

    fn temp_core(name: &str) -> (PathBuf, CoreState) {
        let dir = std::env::temp_dir().join(format!("att-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, Platform::Windows).unwrap();
        (dir, core)
    }

    #[test]
    fn vad_区间合并_不超过上限且不重叠() {
        // 间隔小 → 合并
        assert_eq!(merge_speech_ranges(vec![(100, 200), (250, 300)], 1000, 10, 100, 1000), vec![(90, 310)]);
        // 合并后超过上限 → 断开；余量相交的部分从上一块末尾接着切，不重复识别
        let merged = merge_speech_ranges(vec![(100, 200), (205, 300)], 1000, 10, 100, 150);
        assert_eq!(merged, vec![(90, 210), (210, 310)]);
        // 单个区间本身超长时保持原样
        assert_eq!(merge_speech_ranges(vec![(0, 900)], 1000, 0, 100, 150), vec![(0, 900)]);
        // 间隔大 → 各自成块
        assert_eq!(merge_speech_ranges(vec![(0, 100), (500, 600)], 1000, 0, 100, 1000), vec![(0, 100), (500, 600)]);
    }

    #[test]
    fn provider_映射() {
        assert_eq!(LocalKind::from_provider("whisper"), Some(LocalKind::Whisper));
        assert_eq!(LocalKind::from_provider("qwen_asr"), Some(LocalKind::Qwen));
        assert_eq!(LocalKind::from_provider("doubao_asr"), None);
        assert_eq!(LocalKind::Whisper.label(), "本地 Whisper");
    }

    #[test]
    fn 就绪性_缺模型时结论一句_详情另给() {
        let (dir, core) = temp_core("status");
        let models = LocalModels::default();
        let status = models.status(&core, LocalKind::Whisper);
        assert!(!status.available);
        assert!(!status.loaded);
        assert_eq!(status.error.as_deref(), Some(att_whisper::MODEL_MISSING));
        let detail = status.error_detail.unwrap();
        assert!(detail.contains(&core.models_dir.display().to_string()), "详情给出模型目录：{detail}");

        let status = models.status(&core, LocalKind::Qwen);
        assert!(!status.available);
        assert_eq!(status.error.as_deref(), Some(att_qwen::MODEL_MISSING));
        assert!(status.error_detail.unwrap().contains("设置 → 识别引擎"), "详情指向应用内下载");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 错误拆分_首行结论与详情() {
        assert_eq!(split_hint("结论\n说明\n\tcmd\n"), ("结论".to_string(), Some("说明\n\tcmd".to_string())));
        assert_eq!(split_hint("只有一句"), ("只有一句".to_string(), None));
        assert_eq!(split_hint("结论\n  \n"), ("结论".to_string(), None));
    }

    #[test]
    fn 设置指纹_模型或设备变化即失效() {
        let (dir, core) = temp_core("fingerprint");
        let models_dir = core.models_dir.clone();
        let settings = core.settings();
        let base = fingerprint(LocalKind::Whisper, &settings, &models_dir);
        let mut changed = settings.clone();
        changed.whisper_device = "cpu".into();
        assert_ne!(base, fingerprint(LocalKind::Whisper, &changed, &models_dir));
        let mut changed = settings.clone();
        changed.whisper_model = "small".into();
        assert_ne!(base, fingerprint(LocalKind::Whisper, &changed, &models_dir));
        // Qwen 只看自己的键：改 Whisper 设置不应影响 Qwen 指纹
        let qwen_a = fingerprint(LocalKind::Qwen, &settings, &models_dir);
        assert_eq!(qwen_a, fingerprint(LocalKind::Qwen, &changed, &models_dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 加载失败_记录原因且设置变化后作废() {
        let (dir, core) = temp_core("failure");
        let models = LocalModels::default();
        // 伪造一份「文件齐全但内容损坏」的 Whisper 模型：探测判可用，真正加载会失败
        let model_dir = core.models_dir.join("faster_whisper");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("model.bin"), b"broken").unwrap();
        std::fs::write(model_dir.join("tokenizer.json"), b"{}").unwrap();
        assert!(models.status(&core, LocalKind::Whisper).available);

        assert!(models.get(&core, LocalKind::Whisper).is_err());
        let status = models.status(&core, LocalKind::Whisper);
        assert!(!status.loading, "失败后不应停在加载中");
        assert!(!status.loaded);
        assert!(status.load_error.unwrap().contains("加载本地 Whisper 模型失败"));

        // 换模型后旧的失败原因不再适用
        core.apply_settings_update(&serde_json::json!({ "whisper_model": "small" })).unwrap();
        assert!(models.status(&core, LocalKind::Whisper).load_error.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vad_缺失时不阻塞引擎使用() {
        let (dir, core) = temp_core("vad");
        let models = LocalModels::default();
        // 没有 silero_vad.onnx：取 VAD 应返回 None 而不是 Err
        assert!(models.vad(&core).is_none());
        assert!(!models.vad_loaded());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
