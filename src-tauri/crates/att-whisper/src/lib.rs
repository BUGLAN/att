//! 本地 faster-whisper 引擎（移植自 Python `app/stt/providers/whisper_provider.py`）。
//!
//! 推理走 [CTranslate2] 的 Rust 绑定 `ct2rs`，**直接复用 Python 版下载好的 CTranslate2 模型目录**
//! （`models/faster_whisper`，内含 `model.bin`），不需要重新下载 ggml/ONNX 权重。
//!
//! 与 Python 版的对应关系：
//!
//! | Python（faster-whisper） | 本 crate |
//! | --- | --- |
//! | `WhisperModel(model_size, device, compute_type)` | [`LocalWhisper::load`]，CUDA 失败回退 CPU int8 |
//! | `_resolve_model_source()` 的本地目录 / HF snapshots 探测 | [`resolve_model_dir`] |
//! | `beam_size=5, temperature=0, condition_on_previous_text=False` | `WhisperOptions::default()`（贪心/beam，不串提示词） |
//! | `word_timestamps=True` | [`LocalWhisper::transcribe`] 走 `generate_segments`（含词级时间戳） |
//! | `vad_filter=True, min_silence_duration_ms=500, speech_pad_ms=200` | [`trim_silence`] 能量法裁掉**首尾**长静音（CTranslate2 本身不含 Silero VAD） |
//! | `info.duration / language / language_probability` | 时长取原始采样数；语言按设置或模型自检；概率 CTranslate2 不返回，置 1.0 |
//!
//! CTranslate2 的 Whisper 解码器不提供 Python faster-whisper 的 `vad_filter`（那是 Python 侧用
//! Silero ONNX 实现的），所以这里用能量阈值裁剪首尾静音来近似：录音开头/结尾的静音是 Whisper
//! 幻觉（“字幕由…提供”）的主要来源，裁掉即可，不做句间切分。
//!
//! [CTranslate2]: https://github.com/OpenNMT/CTranslate2

use ct2rs::sys::{ComputeType, Device, WhisperOptions};
use ct2rs::{Config as Ct2Config, Whisper};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 16kHz：Whisper 的输入采样率（与录音链路一致）。
pub const SAMPLE_RATE: usize = 16_000;
/// 静音裁剪窗口（20ms，与 Python Qwen 分块的能量检测一致）。
const SILENCE_WINDOW_MS: usize = 20;
/// 判静音的相对阈值：窗口 RMS 低于全局 95 分位的 2%（约 -34dB）。
const SILENCE_RELATIVE_THRESHOLD: f64 = 0.02;
/// 裁剪时首尾保留的静音余量（对应 Python `speech_pad_ms=200`）。
const SILENCE_PAD_MS: usize = 200;
/// 少于该长度（毫秒）视为纯静音，不送推理。
const MIN_SPEECH_MS: usize = 200;

/// 引擎配置（由设置页的 `whisper_*` 键 + 模型目录组装）。
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// `whisper_model`：`large-v3` 这类短名，或直接是模型目录路径。
    pub model: String,
    /// `whisper_device`：`auto` | `cuda` | `cpu`。
    pub device: String,
    /// 计算精度：`auto` | `int8` | `float16` | `float32` | `int8_float16` …
    pub compute_type: String,
    /// 模型根目录（`models/`，或 `ATT_MODELS_DIR` 指定的目录）。
    pub models_dir: PathBuf,
}

impl WhisperConfig {
    pub fn new(model: impl Into<String>, device: impl Into<String>, models_dir: impl Into<PathBuf>) -> Self {
        Self { model: model.into(), device: device.into(), compute_type: "auto".into(), models_dir: models_dir.into() }
    }

    pub fn with_compute_type(mut self, compute_type: impl Into<String>) -> Self {
        self.compute_type = compute_type.into();
        self
    }
}

/// 一个转写分段（秒）。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// 一次转写结果。
#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub segments: Vec<Segment>,
    /// 音频时长（秒，按 16kHz 采样数换算）
    pub duration: f64,
    /// 生效的计算设备描述（`cuda:float16` / `cpu:int8` …），用于健康面板与日志
    pub device: String,
    /// 推理耗时（秒）
    pub elapsed: f64,
}

/// ct2rs 解码前处理（mel 频谱）的参数文件。
const PREPROCESSOR_CONFIG: &str = "preprocessor_config.json";

/// 模型目录是否像一份可直接加载的 CTranslate2 Whisper 模型。
///
/// CTranslate2 转换产物必须同时有 `model.bin`（权重）与 `tokenizer.json`（ct2rs 解码用）。
pub fn looks_like_model_dir(path: &Path) -> bool {
    path.is_dir() && path.join("model.bin").is_file() && path.join("tokenizer.json").is_file()
}

/// 按 Python `_resolve_model_source()` 的顺序列出候选目录（不判存在）。
///
/// 顺序：显式目录 → 模型根目录本身 → `faster_whisper`/`faster-whisper`/`whisper`（Python 版
/// `download_root` 直接指向这几个目录，各机器上落盘位置不一）→ 每个根目录下的
/// 短名/`faster-whisper-<短名>`/`Systran--faster-whisper-<短名>`
/// → HF 缓存 `models--Systran--faster-whisper-<短名>/snapshots/<hash>`。
pub fn model_candidates(model: &str, models_dir: &Path) -> Vec<PathBuf> {
    let raw = Path::new(model);
    let short = model.replace('\\', "/");
    let short = short.rsplit('/').next().unwrap_or(model).to_string();
    let mut roots = vec![models_dir.to_path_buf()];
    for name in ["faster_whisper", "faster-whisper", "whisper"] {
        roots.push(models_dir.join(name));
    }
    let mut candidates = vec![raw.to_path_buf()];
    candidates.extend(roots.iter().cloned());
    for root in &roots {
        for name in [
            short.clone(),
            format!("faster-whisper-{short}"),
            format!("Systran--faster-whisper-{short}"),
            format!("models--Systran--faster-whisper-{short}"),
        ] {
            candidates.push(root.join(name));
        }
        let snapshot_root = root.join(format!("models--Systran--faster-whisper-{short}")).join("snapshots");
        if let Ok(entries) = std::fs::read_dir(&snapshot_root) {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
            dirs.sort();
            dirs.reverse();
            candidates.extend(dirs);
        }
    }
    candidates
}

/// 缺模型时错误文案的首行结论：首页、诊断页只显示这一句，详情（目录与获取方式）留给设置页。
pub const MODEL_MISSING: &str = "本地 Whisper 模型未安装";

/// 默认的模型存放目录（`models/faster_whisper`，也是 [`model_candidates`] 里最常见的布局）。
pub fn default_model_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("faster_whisper")
}

/// 解析出真正要加载的模型目录。
///
/// 都找不到时返回「首行结论 + 详情」两段式文案（见 [`model_missing_hint`]）；
/// 尝试过的候选路径只写 debug 日志，不进错误文案（空数据目录下有二十多条，用户看了也没法照做）。
pub fn resolve_model_dir(config: &WhisperConfig) -> Result<PathBuf, String> {
    let candidates = model_candidates(&config.model, &config.models_dir);
    if let Some(found) = candidates.iter().find(|candidate| looks_like_model_dir(candidate)) {
        return Ok(found.clone());
    }
    tracing::debug!(
        "未找到本地 Whisper 模型「{}」，已尝试：{}",
        config.model,
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
    );
    Err(model_missing_hint(config))
}

/// 缺模型提示：首行是结论；其后非缩进行是说明，Tab 缩进行是目录（前端排成可复制的代码块）。
///
/// 只给安装包用户做得到的获取方式：从 Hugging Face 下载现成的 CTranslate2 模型放进模型目录。
/// 设置里填的是目录路径时，直接指出那个目录不是有效模型。
fn model_missing_hint(config: &WhisperConfig) -> String {
    let model = config.model.trim();
    let is_path = Path::new(model).is_absolute() || model.starts_with('.') || model.contains('\\');
    if is_path {
        return format!(
            "{MODEL_MISSING}\n设置里的模型目录不是有效的 Whisper 模型（需要 model.bin 与 tokenizer.json）：\n\t{model}"
        );
    }
    // `large-v3` / `Systran/faster-whisper-large-v3` 都归一成短名
    let short = model.rsplit('/').next().unwrap_or(model);
    let short = short.strip_prefix("faster-whisper-").unwrap_or(short);
    let short = if short.is_empty() { "large-v3" } else { short };
    format!(
        "{MODEL_MISSING}\n\
         下载 https://huggingface.co/Systran/faster-whisper-{short} 的全部文件，放到这个目录：\n\
         \t{}\n\
         目录内至少要有 model.bin 与 tokenizer.json。",
        default_model_dir(&config.models_dir).display()
    )
}

/// 缺 `preprocessor_config.json` 时按 Whisper 官方默认值补写一份。
///
/// `Systran/faster-whisper-tiny/medium/…` 这些 HF 仓库不带这个文件，Python faster-whisper 缺了就用
/// 默认值，ct2rs 却硬性要求它（否则加载报 `No such file or directory`）。mel 维度：large-v3 / turbo
/// 是 128（这一代多了粤语，`lang_ids` 有 100 个），其余 80。
pub fn ensure_preprocessor_config(model_dir: &Path) -> Result<(), String> {
    let path = model_dir.join(PREPROCESSOR_CONFIG);
    if path.is_file() {
        return Ok(());
    }
    let feature_size = default_mel_bins(model_dir);
    let body = serde_json::json!({
        "chunk_length": 30,
        "feature_extractor_type": "WhisperFeatureExtractor",
        "feature_size": feature_size,
        "hop_length": 160,
        "n_fft": 400,
        "n_samples": 480000,
        "nb_max_frames": 3000,
        "padding_side": "right",
        "padding_value": 0.0,
        "processor_class": "WhisperProcessor",
        "return_attention_mask": false,
        "sampling_rate": 16000,
    });
    let body = serde_json::to_string_pretty(&body).unwrap_or_default();
    std::fs::write(&path, body)
        .map_err(|error| format!("模型目录缺少 {PREPROCESSOR_CONFIG}，自动补写失败（{}）：{error}", path.display()))?;
    tracing::info!("本地 Whisper：模型目录缺少 {PREPROCESSOR_CONFIG}，已按默认值补写（{feature_size} 维 mel）：{}", path.display());
    Ok(())
}

/// 推断 mel 维度：优先看 config.json 的 `lang_ids` 数量（large-v3 起为 100），读不到再看目录名。
fn default_mel_bins(model_dir: &Path) -> usize {
    let lang_count = std::fs::read_to_string(model_dir.join("config.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|config| config.get("lang_ids").and_then(|ids| ids.as_array()).map(|ids| ids.len()));
    match lang_count {
        Some(count) if count >= 100 => 128,
        Some(_) => 80,
        None => {
            let name = model_dir.to_string_lossy().to_lowercase();
            if name.contains("large-v3") || name.contains("turbo") { 128 } else { 80 }
        }
    }
}

/// 设置里的设备 + 精度 → CTranslate2 的 (设备, 精度)。
///
/// `auto` 的语义与 Python 默认一致：有 CUDA 用 GPU（float16），否则 CPU（int8）。
pub fn resolve_precision(device: &str, compute_type: &str, cuda_built: bool) -> (Device, ComputeType, &'static str) {
    let requested_device = device.trim().to_lowercase();
    let want_cuda = match requested_device.as_str() {
        "cuda" | "gpu" => true,
        "cpu" => false,
        // auto：只有编译期带 CUDA 才尝试 GPU，避免无谓的加载失败
        _ => cuda_built,
    };
    let compute = compute_type.trim().to_lowercase();
    let compute = if compute.is_empty() || compute == "auto" {
        if want_cuda { "float16".to_string() } else { "int8".to_string() }
    } else {
        compute
    };
    let compute_type = match compute.as_str() {
        "int8" => ComputeType::INT8,
        "int8_float32" => ComputeType::INT8_FLOAT32,
        "int8_float16" => ComputeType::INT8_FLOAT16,
        "int8_bfloat16" => ComputeType::INT8_BFLOAT16,
        "int16" => ComputeType::INT16,
        "float16" | "half" => ComputeType::FLOAT16,
        "float32" | "float" => ComputeType::FLOAT32,
        "bfloat16" => ComputeType::BFLOAT16,
        _ => ComputeType::DEFAULT,
    };
    let device = if want_cuda { Device::CUDA } else { Device::CPU };
    (device, compute_type, if want_cuda { "cuda" } else { "cpu" })
}

/// 能量法静音裁剪：返回要保留的采样区间 `[start, end)`（已加首尾余量）。
///
/// 纯静音（或短于 `MIN_SPEECH_MS`）返回 `None`，调用方直接给空结果，避免 Whisper 幻觉。
pub fn trim_silence(samples: &[i16], sample_rate: usize) -> Option<(usize, usize)> {
    let window = (sample_rate * SILENCE_WINDOW_MS / 1000).max(1);
    let usable = samples.len() / window;
    if usable == 0 {
        return None;
    }
    let mut rms = Vec::with_capacity(usable);
    for index in 0..usable {
        let frame = &samples[index * window..(index + 1) * window];
        let sum: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
        rms.push((sum / window as f64).sqrt());
    }
    // 95 分位近似：排序后取高分位，抗少数爆音
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let threshold = (percentile * SILENCE_RELATIVE_THRESHOLD).max(1.0);

    let first = rms.iter().position(|value| *value > threshold)?;
    let last = rms.iter().rposition(|value| *value > threshold)?;
    let pad = sample_rate * SILENCE_PAD_MS / 1000;
    let start = (first * window).saturating_sub(pad);
    let end = (((last + 1) * window) + pad).min(samples.len());
    if end.saturating_sub(start) < sample_rate * MIN_SPEECH_MS / 1000 {
        return None;
    }
    Some((start, end))
}

/// 已加载的本地 Whisper（常驻，直到设置变化或显式卸载）。
pub struct LocalWhisper {
    whisper: Whisper,
    device: String,
    sampling_rate: usize,
    model_dir: PathBuf,
    /// 录音与文件转写可能同时要推理：串行化，避免 CPU/显存互相拖慢
    gate: std::sync::Mutex<()>,
}

impl std::fmt::Debug for LocalWhisper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalWhisper")
            .field("model_dir", &self.model_dir)
            .field("device", &self.device)
            .finish()
    }
}

impl LocalWhisper {
    /// 加载模型（同步、耗时数秒）；CUDA 加载失败自动回退 CPU + int8（对齐 Python `_load_model`）。
    pub fn load(config: &WhisperConfig) -> Result<Self, String> {
        let model_dir = resolve_model_dir(config)?;
        ensure_preprocessor_config(&model_dir)?;
        let started = Instant::now();
        let cuda_built = cfg!(feature = "cuda");
        let (mut device, mut compute, mut label) = resolve_precision(&config.device, &config.compute_type, cuda_built);
        let build = |device: Device, compute: ComputeType| {
            Whisper::new(&model_dir, Ct2Config { device, compute_type: compute, ..Default::default() })
        };
        let whisper = match build(device, compute) {
            Ok(whisper) => whisper,
            Err(error) if device == Device::CUDA => {
                tracing::warn!(
                    "本地 Whisper CUDA 加载失败（{error}），回退 CPU + int8：{}",
                    model_dir.display()
                );
                device = Device::CPU;
                compute = ComputeType::INT8;
                label = "cpu";
                build(device, compute).map_err(|error| {
                    format!("加载本地 Whisper 模型失败（{}）：{error}", model_dir.display())
                })?
            }
            Err(error) => {
                return Err(format!("加载本地 Whisper 模型失败（{}）：{error}", model_dir.display()));
            }
        };
        let sampling_rate = whisper.sampling_rate();
        tracing::info!(
            "本地 Whisper 模型已加载：{}（{label}，{:.2}s）",
            model_dir.display(),
            started.elapsed().as_secs_f64()
        );
        let compute_label = match compute {
            ComputeType::INT8 => "int8",
            ComputeType::FLOAT16 => "float16",
            ComputeType::FLOAT32 => "float32",
            ComputeType::INT8_FLOAT16 => "int8_float16",
            ComputeType::INT8_FLOAT32 => "int8_float32",
            ComputeType::INT8_BFLOAT16 => "int8_bfloat16",
            ComputeType::BFLOAT16 => "bfloat16",
            ComputeType::INT16 => "int16",
            _ => "default",
        };
        Ok(Self { whisper, device: format!("{label}:{compute_label}"), sampling_rate, model_dir, gate: std::sync::Mutex::new(()) })
    }

    /// 生效设备描述（`cuda:float16` / `cpu:int8`）。
    pub fn device(&self) -> &str {
        &self.device
    }

    /// 模型目录。
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// 解码选项：对应 Python `CHINESE_CONFIG`（beam=5，贪心/beam 不做采样）。
    fn options() -> WhisperOptions {
        WhisperOptions {
            beam_size: 5,
            patience: 1.0,
            length_penalty: 1.0,
            repetition_penalty: 1.0,
            no_repeat_ngram_size: 0,
            max_length: 448,
            sampling_topk: 1,
            sampling_temperature: 1.0,
            num_hypotheses: 1,
            return_scores: false,
            return_logits_vocab: false,
            return_no_speech_prob: false,
            max_initial_timestamp_index: 50,
            suppress_blank: true,
            suppress_tokens: vec![-1],
        }
    }

    /// 整段 PCM（16kHz、单声道、i16）转写。
    ///
    /// `language` 为 `auto`/空时交给模型自检，其余按 BCP-47 短码（`zh`/`en`…）。
    pub fn transcribe(&self, language: &str, samples: &[i16]) -> Result<Transcript, String> {
        let _guard = self.gate.lock().unwrap_or_else(|error| error.into_inner());
        let started = Instant::now();
        let duration = samples.len() as f64 / SAMPLE_RATE as f64;
        if samples.is_empty() {
            return Ok(Transcript { text: String::new(), segments: Vec::new(), duration: 0.0, device: self.device.clone(), elapsed: 0.0 });
        }
        let Some((start, end)) = trim_silence(samples, SAMPLE_RATE) else {
            tracing::info!("本地 Whisper：音频无有效语音（{}），跳过推理", self.model_dir.display());
            return Ok(Transcript { text: String::new(), segments: Vec::new(), duration, device: self.device.clone(), elapsed: 0.0 });
        };
        let offset = start as f64 / SAMPLE_RATE as f64;
        let mono: Vec<f32> = samples[start..end].iter().map(|&s| s as f32 / 32768.0).collect();
        // 采样率理论上是 16k；模型若声明别的采样率则按比例重采样（step_by 粗采样，仅兜底）
        let mono = if self.sampling_rate == SAMPLE_RATE {
            mono
        } else {
            let step = (SAMPLE_RATE / self.sampling_rate.max(1)).max(1);
            mono.into_iter().step_by(step).collect()
        };

        let normalized = language.trim().to_lowercase();
        let language_hint = match normalized.as_str() {
            "" | "auto" => None,
            other => Some(other.split(['-', '_']).next().unwrap_or(other).to_string()),
        };
        let segments = self
            .whisper
            .generate_segments(&mono, language_hint.as_deref(), &Self::options())
            .map_err(|error| format!("本地 Whisper 推理失败：{error}"))?;

        let mut out = Vec::with_capacity(segments.len());
        for segment in segments {
            let text = segment.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            out.push(Segment { start: segment.start as f64 + offset, end: segment.end as f64 + offset, text });
        }
        let text = out.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>().join(" ");
        let elapsed = started.elapsed().as_secs_f64();
        tracing::info!(
            "本地 Whisper 转写完成：{} 段，音频 {duration:.1}s，耗时 {elapsed:.2}s（{}）",
            out.len(),
            self.device
        );
        Ok(Transcript { text, segments: out, duration, device: self.device.clone(), elapsed })
    }

    /// 预热：跑一次极短推理，迫使 CTranslate2 完成首批计算（对齐 Python `warmup()`）。
    ///
    /// 直接喂给模型而不走 [`Self::transcribe`]：后者会把静音裁掉、根本不进推理，预热就成了空操作。
    ///
    /// 只在 GPU 上做：CUDA/cuDNN 首次调用要初始化内核，值得提前吃掉；CPU 上编码器固定跑满 30s 窗口，
    /// large-v3 预热一次要十几秒却只省下首个请求约 2 秒（M3 实测），还会占着推理闸门拖慢紧随其后的录音。
    pub fn warmup(&self) {
        if !self.device.starts_with("cuda") {
            return;
        }
        let _guard = self.gate.lock().unwrap_or_else(|error| error.into_inner());
        let started = Instant::now();
        let mono = vec![0f32; self.sampling_rate.max(1)];
        // 贪心 + 只解几个 token：目的只是把编码器与解码器各跑一遍，不必真解码
        let options = WhisperOptions { beam_size: 1, max_length: 8, ..Self::options() };
        match self.whisper.generate_segments(&mono, Some("zh"), &options) {
            Ok(_) => tracing::info!("本地 Whisper 预热完成：{:.2}s", started.elapsed().as_secs_f64()),
            Err(error) => tracing::warn!("本地 Whisper 预热失败：{error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-whisper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_model_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.bin"), b"x").unwrap();
        std::fs::write(dir.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(dir.join("preprocessor_config.json"), b"{}").unwrap();
        dir
    }

    #[test]
    fn 模型目录判定必须有_model_bin_与_tokenizer() {
        let root = temp_dir("looks");
        let dir = make_model_dir(&root, "large-v3");
        assert!(looks_like_model_dir(&dir));
        std::fs::remove_file(dir.join("tokenizer.json")).unwrap();
        assert!(!looks_like_model_dir(&dir));
        assert!(!looks_like_model_dir(&root.join("nope")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 模型目录解析_覆盖短名与HF快照布局() {
        let root = temp_dir("resolve");
        // 1) 模型根目录本身就是模型（直接放 model.bin/tokenizer.json）
        std::fs::write(root.join("model.bin"), b"x").unwrap();
        std::fs::write(root.join("tokenizer.json"), b"{}").unwrap();
        let config = WhisperConfig::new("large-v3", "cpu", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), root.clone());
        let _ = std::fs::remove_dir_all(&root);

        // 2) faster-whisper-<短名>
        let root = temp_dir("resolve2");
        let dir = make_model_dir(&root, "faster-whisper-large-v3");
        let config = WhisperConfig::new("large-v3", "cpu", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), dir);
        let _ = std::fs::remove_dir_all(&root);

        // 3) HF 缓存 snapshots/<hash>（倒序取最新）
        let root = temp_dir("resolve3");
        let snap_root = root.join("models--Systran--faster-whisper-large-v3").join("snapshots");
        std::fs::create_dir_all(&snap_root).unwrap();
        let newest = make_model_dir(&snap_root, "bbb");
        let _older = make_model_dir(&snap_root, "aaa");
        let config = WhisperConfig::new("large-v3", "cpu", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), newest);
        let _ = std::fs::remove_dir_all(&root);

        // 4) download_root 指向 models/whisper 时的 HF 缓存（macOS 开发机的实际布局）
        let root = temp_dir("resolve4");
        let snap_root = root.join("whisper").join("models--Systran--faster-whisper-medium").join("snapshots");
        std::fs::create_dir_all(&snap_root).unwrap();
        let snapshot = make_model_dir(&snap_root, "08e178d");
        let config = WhisperConfig::new("medium", "cpu", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), snapshot);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 模型目录解析_缺失时首行结论_详情只给目录与获取方式() {
        let root = temp_dir("missing");
        let config = WhisperConfig::new("large-v3", "cpu", &root);
        let error = resolve_model_dir(&config).unwrap_err();
        let mut lines = error.lines();
        assert_eq!(lines.next(), Some(MODEL_MISSING), "首行只放一句结论：{error}");
        let detail = lines.collect::<Vec<_>>().join("\n");
        assert!(detail.contains("Systran/faster-whisper-large-v3"), "{error}");
        assert!(detail.contains(&format!("\t{}", default_model_dir(&root).display())), "目录单独成行便于复制：{error}");
        // 候选路径清单、开发者工具与环境变量都不进文案
        assert!(!error.contains("ct2-transformers-converter"), "{error}");
        assert!(!error.contains("ATT_MODELS_DIR"), "{error}");
        assert!(!error.contains("models--Systran"), "{error}");
        assert!(error.chars().count() < 300, "文案应保持简短：{} 字", error.chars().count());

        // 设置里填的是目录路径：指出那个目录
        let config = WhisperConfig::new(root.join("my-model").display().to_string(), "cpu", &root);
        let error = resolve_model_dir(&config).unwrap_err();
        assert!(error.starts_with(MODEL_MISSING), "{error}");
        assert!(error.contains("my-model"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 缺预处理配置时按模型代际补写() {
        let root = temp_dir("preprocessor");
        let old = root.join("medium");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.json"), format!("{{\"lang_ids\": {:?}}}", (0..99).collect::<Vec<_>>())).unwrap();
        ensure_preprocessor_config(&old).unwrap();
        let text = std::fs::read_to_string(old.join(PREPROCESSOR_CONFIG)).unwrap();
        let written: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written["feature_size"], 80);
        assert_eq!(written["sampling_rate"], 16000);

        let v3 = root.join("v3");
        std::fs::create_dir_all(&v3).unwrap();
        std::fs::write(v3.join("config.json"), format!("{{\"lang_ids\": {:?}}}", (0..100).collect::<Vec<_>>())).unwrap();
        ensure_preprocessor_config(&v3).unwrap();
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(v3.join(PREPROCESSOR_CONFIG)).unwrap()).unwrap();
        assert_eq!(written["feature_size"], 128);

        // 已有文件不覆盖
        std::fs::write(v3.join(PREPROCESSOR_CONFIG), "keep").unwrap();
        ensure_preprocessor_config(&v3).unwrap();
        assert_eq!(std::fs::read_to_string(v3.join(PREPROCESSOR_CONFIG)).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 精度解析_auto_按设备取默认值() {
        let (device, compute, label) = resolve_precision("auto", "auto", false);
        assert_eq!(device, Device::CPU);
        assert_eq!(compute, ComputeType::INT8);
        assert_eq!(label, "cpu");

        let (device, compute, label) = resolve_precision("auto", "auto", true);
        assert_eq!(device, Device::CUDA);
        assert_eq!(compute, ComputeType::FLOAT16);
        assert_eq!(label, "cuda");

        let (device, compute, _) = resolve_precision("cpu", "float32", true);
        assert_eq!(device, Device::CPU);
        assert_eq!(compute, ComputeType::FLOAT32);
    }

    #[test]
    fn 静音裁剪_保留语音并给出边界() {
        let mut samples = vec![0i16; SAMPLE_RATE]; // 1s 静音
        samples.extend((0..SAMPLE_RATE).map(|i| (((i as f64) * 0.05).sin() * 8000.0) as i16)); // 1s 声音
        samples.extend(vec![0i16; SAMPLE_RATE]); // 1s 静音
        let (start, end) = trim_silence(&samples, SAMPLE_RATE).unwrap();
        assert!(start >= SAMPLE_RATE - SAMPLE_RATE * 200 / 1000 - 1, "起点应贴着语音前 200ms：{start}");
        assert!(start < SAMPLE_RATE, "起点应落在语音前的余量内：{start}");
        assert!(end > 2 * SAMPLE_RATE, "终点应保留语音尾后余量：{end}");
        assert!(end <= samples.len(), "终点不得越界");
    }

    #[test]
    fn 静音裁剪_纯静音返回_none() {
        assert!(trim_silence(&vec![0i16; SAMPLE_RATE], SAMPLE_RATE).is_none());
        assert!(trim_silence(&[], SAMPLE_RATE).is_none());
        // 只有 40ms，短于最小语音时长
        assert!(trim_silence(&vec![5000i16; SAMPLE_RATE * 40 / 1000], SAMPLE_RATE).is_none());
    }
}
