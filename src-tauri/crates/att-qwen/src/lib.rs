//! 本地 Qwen3-ASR / SenseVoice 引擎（ONNX 版，走 sherpa-onnx）。
//!
//! 与 Python 版的差异：
//!
//! | Python（transformers/vLLM） | 本 crate（sherpa-onnx + ONNX） |
//! | --- | --- |
//! | `Qwen3ASRModel.from_pretrained(...)`（torch，1.7B） | ONNX 导出的 `Qwen3-ASR-0.6B-int8`（约 940MB，CPU 上 RTF≈0.1） |
//! | 需要 Python + torch + CUDA 运行时 | 进程内推理，只依赖 sherpa-onnx 动态库（运行期加载） |
//! | `max_new_tokens` / `return_time_stamps` | `max_new_tokens`；ONNX 模型不返回时间戳，分段由静音切点给出 |
//! | 长文件按 `max_audio_seconds` 静音分块（`_silence_cut_points` + `_plan_chunks`） | 同算法复刻 |
//! | 语言可指定（`language="Chinese"`） | sherpa 的 Qwen3-ASR 配置没有语言参数，由模型自行判定 |
//!
//! 推理库在安装包里随 `bundle.resources` 分发；开发态放 `models/sherpa-onnx/`，也可用
//! `ATT_SHERPA_ONNX_DIR` 覆盖。模型仍不随包：放 `models/qwen3-asr-0.6B-int8/`（或 `ATT_MODELS_DIR`），
//! 缺失时给出可直接照做的提示。
//!
//! SenseVoice 与 Qwen3-ASR 共用同一份 sherpa-onnx 动态库和离线识别 C API，只是
//! `SherpaOnnxOfflineRecognizerConfig` 换成 `model_config.sense_voice`。它单模型支持
//! 中文、粤语、英语、日语、韩语，适合 CPU 上快速转写；同样按静音点分块处理长音频。

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub mod vad;

pub use vad::SileroVad;

/// 16kHz：录音链路与模型输入采样率。
pub const SAMPLE_RATE: usize = 16_000;
/// 分块转写被进度回调叫停（文件转写取消）时的错误文案。
const CANCELLED: &str = "转写已取消";
/// 模型必需文件（sherpa-onnx Qwen3-ASR 导出目录的固定布局）。
pub const MODEL_FILES: [&str; 3] = ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"];
/// SenseVoice 模型目录的必需文件。
pub const SENSE_VOICE_MODEL_FILES: [&str; 2] = ["model.int8.onnx", "tokens.txt"];
/// 错误缓冲区长度（与 shim.c 的 ATT_ERR_LEN 一致）。
const ERR_LEN: usize = 1024;

/// Qwen3-ASR 解码器 KV cache 的总序列长度（提示词、音频和生成 token 共用）。
pub const QWEN3_ASR_MAX_TOTAL_LEN: i64 = 512;
/// Qwen3-ASR 音频卷积前端每秒产生的 token 数。
pub const QWEN3_ASR_AUDIO_TOKENS_PER_SECOND: i64 = 13;
/// `max_new_tokens` 的允许范围。
pub const QWEN3_ASR_MAX_NEW_TOKENS_RANGE: (i64, i64) = (32, QWEN3_ASR_MAX_TOTAL_LEN);
/// `max_audio_seconds` 的允许范围。上限由 KV cache 总长度和每秒音频 token 数确定。
pub const QWEN3_ASR_MAX_AUDIO_SECONDS_RANGE: (i64, i64) =
    (5, QWEN3_ASR_MAX_TOTAL_LEN / QWEN3_ASR_AUDIO_TOKENS_PER_SECOND);

// 静音切点参数：与 Python `QwenASRProvider` 的常量一致
const SILENCE_WINDOW_MS: usize = 20;
const SILENCE_MIN_MS: usize = 500;
const SILENCE_RELATIVE_THRESHOLD: f64 = 0.02;

/// 引擎配置（由设置页的 `qwen_asr_*` 键 + 模型/推理库目录组装）。
#[derive(Debug, Clone)]
pub struct QwenConfig {
    /// `qwen_asr_model`：模型目录、HuggingFace 名字或相对模型根的目录名。
    pub model: String,
    /// 单块音频上限（秒），对应 Python `qwen_asr_max_audio_seconds`。
    pub max_audio_seconds: i64,
    /// 单次生成的最大 token 数，对应 Python `qwen_asr_max_new_tokens`。
    pub max_new_tokens: i64,
    /// ONNX Runtime 线程数（0 = 取默认 2）。
    pub num_threads: i64,
    /// 执行设备 provider：`cpu` | `cuda` | `coreml`（由设置页设备选择决定）。
    pub provider: String,
    /// 模型根目录（`models/`）。
    pub models_dir: PathBuf,
    /// 推理库目录覆盖；`None` 时按 [`resolve_lib_dir`] 探测。
    pub lib_dir: Option<PathBuf>,
    /// 随安装包分发的推理库目录（壳层由 `resource_dir/sherpa-onnx` 传入）；`None` 时跳过。
    pub bundled_lib_dir: Option<PathBuf>,
}

impl QwenConfig {
    pub fn new(model: impl Into<String>, models_dir: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            max_audio_seconds: 30,
            max_new_tokens: 512,
            num_threads: 2,
            provider: "cpu".into(),
            models_dir: models_dir.into(),
            lib_dir: None,
            bundled_lib_dir: None,
        }
    }
}

/// SenseVoice 引擎配置（由设置页的 `sense_voice_*` 键 + 模型/推理库目录组装）。
#[derive(Debug, Clone)]
pub struct SenseVoiceConfig {
    /// `sense_voice_model`：模型目录、相对模型根的目录名或已知模型名。
    pub model: String,
    /// `asr_language` 归一后的语言提示；不属于 zh/en/ja/ko/yue 时传 `auto`。
    pub language: String,
    /// 是否启用逆文本归一化（数字、标点等）。
    pub use_itn: bool,
    /// ONNX Runtime 线程数（0 = 取默认 2）。
    pub num_threads: i64,
    /// 执行设备 provider：`cpu` | `cuda` | `coreml`（由设置页设备选择决定）。
    pub provider: String,
    /// 单块音频上限（秒）。
    pub max_audio_seconds: i64,
    /// 模型根目录（`models/`）。
    pub models_dir: PathBuf,
    /// 推理库目录覆盖；`None` 时按 [`resolve_sense_voice_lib_dir`] 探测。
    pub lib_dir: Option<PathBuf>,
    /// 随安装包分发的推理库目录。
    pub bundled_lib_dir: Option<PathBuf>,
}

impl SenseVoiceConfig {
    pub fn new(model: impl Into<String>, models_dir: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            language: "auto".into(),
            use_itn: true,
            num_threads: 2,
            provider: "cpu".into(),
            max_audio_seconds: 30,
            models_dir: models_dir.into(),
            lib_dir: None,
            bundled_lib_dir: None,
        }
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
    pub duration: f64,
    pub elapsed: f64,
}

/// 缺模型时错误文案的首行结论（首页、诊断页只显示这一句，命令留给设置页）。
pub const MODEL_MISSING: &str = "本地 Qwen3-ASR 模型未安装";
/// 缺 sherpa-onnx 推理库时错误文案的首行结论。
pub const LIB_MISSING: &str = "本地 Qwen3-ASR 推理库未安装";
/// SenseVoice 缺模型时错误文案的首行结论。
pub const SENSE_VOICE_MODEL_MISSING: &str = "本地 SenseVoice 模型未安装";
/// SenseVoice 缺 sherpa-onnx 推理库时错误文案的首行结论。
pub const SENSE_VOICE_LIB_MISSING: &str = "本地 SenseVoice 推理库未安装";
/// 模型目录是否像一份 sherpa-onnx Qwen3-ASR 导出（三个 onnx + tokenizer 目录）。
pub fn looks_like_model_dir(path: &Path) -> bool {
    path.is_dir() && MODEL_FILES.iter().all(|name| path.join(name).is_file()) && path.join("tokenizer").is_dir()
}

/// 模型目录是否像一份 sherpa-onnx SenseVoice 导出（模型 + 词表）。
pub fn looks_like_sense_voice_model_dir(path: &Path) -> bool {
    path.is_dir() && SENSE_VOICE_MODEL_FILES.iter().all(|name| path.join(name).is_file())
}

/// 推理库在当前平台的动态库文件名。
pub fn library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sherpa-onnx-c-api.dll"
    } else if cfg!(target_os = "macos") {
        "libsherpa-onnx-c-api.dylib"
    } else {
        "libsherpa-onnx-c-api.so"
    }
}

/// 指定 ONNX Runtime 执行设备对应的独立 provider 动态库文件名。
pub fn provider_library_file_name(provider: &str) -> Option<&'static str> {
    match provider {
        "cuda" => Some(if cfg!(target_os = "windows") {
            "onnxruntime_providers_cuda.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime_providers_cuda.dylib"
        } else {
            "libonnxruntime_providers_cuda.so"
        }),
        "coreml" => Some(if cfg!(target_os = "windows") {
            "onnxruntime_providers_coreml.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime_providers_coreml.dylib"
        } else {
            "libonnxruntime_providers_coreml.so"
        }),
        _ => None,
    }
}

/// 推理库目录是否实际带有指定执行设备需要的独立 provider 动态库。
pub fn provider_available(lib_dir: &Path, provider: &str) -> bool {
    provider == "cpu" || provider_library_file_name(provider).map(|name| lib_dir.join(name).is_file()).unwrap_or(false)
}

/// 列出模型目录候选（不判存在）。
pub fn model_candidates(model: &str, models_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from(model)];
    let short = model.replace('\\', "/");
    let short = short.rsplit('/').next().unwrap_or(model).to_string();
    candidates.push(models_dir.join(&short));
    // HF 缓存把 '.' 换成 '___'
    candidates.push(models_dir.join(short.replace('.', "___")));
    for name in ["qwen3-asr-0.6B-int8", "qwen3_asr_onnx", "qwen3-asr"] {
        candidates.push(models_dir.join(name));
    }
    candidates
}

/// 解析模型目录：先按候选顺序找，最后兜底扫描模型根目录下的子目录。
pub fn resolve_model_dir(config: &QwenConfig) -> Result<PathBuf, String> {
    let candidates = model_candidates(&config.model, &config.models_dir);
    for candidate in &candidates {
        if looks_like_model_dir(candidate) {
            return Ok(candidate.clone());
        }
    }
    if let Ok(entries) = std::fs::read_dir(&config.models_dir) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        dirs.sort();
        // 优先挑 Qwen 相关的目录名，避免误选其它 ONNX 模型
        if let Some(found) = dirs.iter().find(|dir| {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            name.contains("qwen") && looks_like_model_dir(dir)
        }) {
            return Ok(found.clone());
        }
    }
    tracing::debug!(
        "未找到本地 Qwen3-ASR 模型「{}」，已尝试：{}",
        config.model,
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
    );
    // 首行一句结论（首页只显示这一句）；路径单独缩进，设置页可直接复制。
    let target = config.models_dir.join("qwen3-asr-0.6B-int8");
    Err(format!(
        "{MODEL_MISSING}\n\
         在「设置 → 识别引擎」的模型列表里下载；离线时解压到：\n\
         \t{}\n\
         目录内需要 {} 与 tokenizer/。",
        target.display(),
        MODEL_FILES.join("、"),
    ))
}

/// 列出 SenseVoice 模型目录候选（不判存在）。
pub fn sense_voice_model_candidates(model: &str, models_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from(model)];
    let short = model.replace('\\', "/");
    let short = short.rsplit('/').next().unwrap_or(model).to_string();
    candidates.push(models_dir.join(&short));
    for name in [
        "sense-voice-zh-en-ja-ko-yue-int8",
        "sensevoice-zh-en-ja-ko-yue-int8",
        "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
    ] {
        candidates.push(models_dir.join(name));
    }
    candidates
}

/// 解析 SenseVoice 模型目录：先按候选顺序找，最后兜底扫描名称含 sense 的模型目录。
pub fn resolve_sense_voice_model_dir(config: &SenseVoiceConfig) -> Result<PathBuf, String> {
    let candidates = sense_voice_model_candidates(&config.model, &config.models_dir);
    for candidate in &candidates {
        if looks_like_sense_voice_model_dir(candidate) {
            return Ok(candidate.clone());
        }
    }
    if let Ok(entries) = std::fs::read_dir(&config.models_dir) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        dirs.sort();
        if let Some(found) = dirs.iter().find(|dir| {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            name.contains("sense") && looks_like_sense_voice_model_dir(dir)
        }) {
            return Ok(found.clone());
        }
    }
    tracing::debug!(
        "未找到本地 SenseVoice 模型「{}」，已尝试：{}",
        config.model,
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
    );
    let target = config.models_dir.join("sense-voice-zh-en-ja-ko-yue-int8");
    Err(format!(
        "{SENSE_VOICE_MODEL_MISSING}\n\
         在「设置 → 识别引擎」的模型列表里下载；离线时解压到：\n\
         \t{}\n\
         目录内需要 {}。",
        target.display(),
        SENSE_VOICE_MODEL_FILES.join("、"),
    ))
}

#[cfg(test)]
/// 列出推理库候选目录（不判存在）：显式配置 > `ATT_SHERPA_ONNX_DIR` > 安装包资源 > `<models>/sherpa-onnx`。
fn lib_dir_candidates(config: &QwenConfig, env_dir: Option<PathBuf>) -> Vec<PathBuf> {
    lib_dir_candidates_for(config.lib_dir.as_deref(), config.bundled_lib_dir.as_deref(), &config.models_dir, env_dir)
}

fn lib_dir_candidates_for(
    lib_dir: Option<&Path>,
    bundled_lib_dir: Option<&Path>,
    models_dir: &Path,
    env_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = lib_dir {
        candidates.push(dir.to_path_buf());
    }
    if let Some(dir) = env_dir {
        candidates.push(dir);
    }
    if let Some(dir) = bundled_lib_dir {
        candidates.push(dir.to_path_buf());
    }
    candidates.push(models_dir.join("sherpa-onnx"));
    candidates
}

/// 解析 sherpa-onnx 推理库目录：显式配置 > `ATT_SHERPA_ONNX_DIR` > 安装包资源 > `<models>/sherpa-onnx`。
pub fn resolve_lib_dir(config: &QwenConfig) -> Result<PathBuf, String> {
    resolve_lib_dir_for(config.lib_dir.as_deref(), config.bundled_lib_dir.as_deref(), &config.models_dir, LIB_MISSING)
}

/// SenseVoice 与 Qwen 共用同一份 sherpa-onnx 推理库，只是缺库文案按引擎区分。
pub fn resolve_sense_voice_lib_dir(config: &SenseVoiceConfig) -> Result<PathBuf, String> {
    resolve_lib_dir_for(
        config.lib_dir.as_deref(),
        config.bundled_lib_dir.as_deref(),
        &config.models_dir,
        SENSE_VOICE_LIB_MISSING,
    )
}

fn resolve_lib_dir_for(
    lib_dir: Option<&Path>,
    bundled_lib_dir: Option<&Path>,
    models_dir: &Path,
    missing_summary: &str,
) -> Result<PathBuf, String> {
    let env_dir = std::env::var("ATT_SHERPA_ONNX_DIR").ok().filter(|dir| !dir.trim().is_empty()).map(PathBuf::from);
    let candidates = lib_dir_candidates_for(lib_dir, bundled_lib_dir, models_dir, env_dir);
    let library = library_file_name();
    for candidate in &candidates {
        if candidate.join(library).is_file() {
            return Ok(candidate.clone());
        }
    }
    let package = library_package_name();
    let target = models_dir.join("sherpa-onnx");
    tracing::debug!(
        "未找到 sherpa-onnx 推理库 {library}，已尝试：{}",
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("、")
    );
    Err(missing_lib_hint(missing_summary, library, package, &target))
}

/// 开发态缺推理库时的安装提示。Windows 用 PowerShell 可直接执行的 `curl.exe` / `tar.exe`；
/// 旧提示里的 `wget` 在 PowerShell 5.1 中只是 `Invoke-WebRequest` 的别名、不会落盘，`&&` 也不是合法运算符。
fn missing_lib_hint(summary: &str, library: &str, package: &str, target: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        format!(
            "{summary}\n\
             缺少 {library}，用下面的 PowerShell 命令下载到推理库目录：\n\
             \tNew-Item -ItemType Directory -Force -Path \"{}\" | Out-Null\n\
             \tcurl.exe -L \"https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/{package}\" -o \"$env:TEMP\\{package}\"\n\
             \ttar.exe -xjf \"$env:TEMP\\{package}\" --strip-components=2 -C \"{}\"",
            target.display(),
            target.display(),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!(
            "{summary}\n\
             缺少 {library}，用下面的命令下载到推理库目录：\n\
             \twget https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/{package}\n\
             \tmkdir -p \"{}\" && tar xvf {package} --strip-components=2 -C \"{}\"",
            target.display(),
            target.display(),
        )
    }
}

/// 当前平台对应的 sherpa-onnx 共享库发布包（Release 页 assets 名）。
fn library_package_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sherpa-onnx-v2.0.1-win-x64-shared-MD-Release-lib.tar.bz2"
    } else if cfg!(target_os = "macos") {
        "sherpa-onnx-v2.0.1-osx-universal2-shared-lib.tar.bz2"
    } else {
        "sherpa-onnx-v2.0.1-linux-x64-shared-lib.tar.bz2"
    }
}

/// 静音切点（采样位置）：长静音段的中点。
///
/// 切在静音中点意味着边界两侧都留有静音余量，不会切掉词首词尾（Python 版注释同样考虑）。
pub fn silence_cut_points(samples: &[i16], sample_rate: usize) -> Vec<usize> {
    let window = (sample_rate * SILENCE_WINDOW_MS / 1000).max(1);
    let usable = samples.len() / window;
    if usable == 0 {
        return Vec::new();
    }
    let mut rms = Vec::with_capacity(usable);
    for index in 0..usable {
        let frame = &samples[index * window..(index + 1) * window];
        let sum: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
        rms.push((sum / window as f64).sqrt());
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let threshold = (percentile * SILENCE_RELATIVE_THRESHOLD).max(1.0);
    let min_windows = (SILENCE_MIN_MS / SILENCE_WINDOW_MS).max(1);

    let mut cuts = Vec::new();
    let mut run_start: Option<usize> = None;
    for (index, value) in rms.iter().enumerate() {
        let silent = *value <= threshold;
        if silent && run_start.is_none() {
            run_start = Some(index);
        } else if !silent {
            if let Some(start) = run_start.take() {
                if index - start >= min_windows {
                    cuts.push((start + index) / 2 * window);
                }
            }
        }
    }
    if let Some(start) = run_start {
        if usable - start >= min_windows {
            cuts.push((start + usable) / 2 * window);
        }
    }
    cuts
}

/// 贪心装箱：每块不超过 `max_audio_seconds`，优先在窗内最靠右的静音点切，无静音点就硬切。
pub fn plan_chunks(
    samples_len: usize,
    sample_rate: usize,
    max_audio_seconds: i64,
    cut_points: &[usize],
) -> Vec<(usize, usize)> {
    let limit = max_audio_seconds.max(1) as usize * sample_rate;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < samples_len {
        let limit_end = (start + limit).min(samples_len);
        if limit_end >= samples_len {
            chunks.push((start, samples_len));
            break;
        }
        let end = cut_points.iter().copied().rfind(|cut| *cut > start && *cut <= limit_end).unwrap_or(limit_end);
        chunks.push((start, end));
        start = end;
    }
    chunks
}

// ---- sherpa-onnx C 桥（见 src/shim.c）----

#[repr(C)]
struct AttQwen {
    _private: [u8; 0],
}

extern "C" {
    fn att_qwen_create(
        lib_dir: *const c_char,
        conv_frontend: *const c_char,
        encoder: *const c_char,
        decoder: *const c_char,
        tokenizer: *const c_char,
        num_threads: i32,
        provider: *const c_char,
        max_total_len: i32,
        max_new_tokens: i32,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut AttQwen;
    fn att_qwen_transcribe(
        handle: *mut AttQwen,
        samples: *const f32,
        n: i32,
        sample_rate: i32,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut c_char;
    fn att_qwen_free_text(text: *mut c_char);
    fn att_qwen_destroy(handle: *mut AttQwen);
}

/// 已加载的本地 Qwen3-ASR（常驻；识别调用内部串行化）。
pub struct LocalQwen {
    handle: *mut AttQwen,
    model_dir: PathBuf,
    lib_dir: PathBuf,
    max_audio_seconds: i64,
    /// 生效的执行设备（cpu / cuda / coreml）。
    provider: String,
    /// sherpa-onnx 的 recognizer 本身是可变状态，串行调用
    gate: Mutex<()>,
}

// 句柄只在 C 侧使用；串行由 gate 保证
unsafe impl Send for LocalQwen {}
unsafe impl Sync for LocalQwen {}

impl std::fmt::Debug for LocalQwen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalQwen").field("model_dir", &self.model_dir).field("lib_dir", &self.lib_dir).finish()
    }
}

fn to_c_path(path: &Path) -> Result<CString, String> {
    let text = path.to_string_lossy().to_string();
    CString::new(text.clone()).map_err(|_| format!("路径包含非法字符（NUL）：{text}"))
}

/// 传给 Qwen C 桥的一组模型路径。
struct QwenPaths<'a> {
    lib_dir: &'a CString,
    conv_frontend: &'a CString,
    encoder: &'a CString,
    decoder: &'a CString,
    tokenizer: &'a CString,
}

/// 用指定 provider 建 Qwen 识别器（provider 字符串在调用期间有效即可）。
fn qwen_create_handle(paths: &QwenPaths<'_>, config: &QwenConfig, provider: &str, err: *mut c_char) -> *mut AttQwen {
    let provider_c = CString::new(provider).unwrap_or_else(|_| CString::new("cpu").unwrap());
    unsafe {
        att_qwen_create(
            paths.lib_dir.as_ptr(),
            paths.conv_frontend.as_ptr(),
            paths.encoder.as_ptr(),
            paths.decoder.as_ptr(),
            paths.tokenizer.as_ptr(),
            config.num_threads.clamp(0, 64) as i32,
            provider_c.as_ptr(),
            QWEN3_ASR_MAX_TOTAL_LEN as i32,
            config.max_new_tokens as i32,
            err,
            ERR_LEN as i32,
        )
    }
}

fn validate_qwen_config(config: &QwenConfig) -> Result<(), String> {
    let (new_min, new_max) = QWEN3_ASR_MAX_NEW_TOKENS_RANGE;
    if !(new_min..=new_max).contains(&config.max_new_tokens) {
        return Err(format!("Qwen3-ASR max_new_tokens 必须在 {new_min}~{new_max} 之间"));
    }
    let (audio_min, audio_max) = QWEN3_ASR_MAX_AUDIO_SECONDS_RANGE;
    if !(audio_min..=audio_max).contains(&config.max_audio_seconds) {
        return Err(format!("Qwen3-ASR max_audio_seconds 必须在 {audio_min}~{audio_max} 秒之间"));
    }
    Ok(())
}

/// 用指定 provider 建 SenseVoice 识别器。
fn sense_voice_create_handle(
    lib_dir: &CString,
    model: &CString,
    tokens: &CString,
    language: &CString,
    config: &SenseVoiceConfig,
    provider: &str,
    err: *mut c_char,
) -> *mut AttSenseVoice {
    let provider_c = CString::new(provider).unwrap_or_else(|_| CString::new("cpu").unwrap());
    unsafe {
        att_sensevoice_create(
            lib_dir.as_ptr(),
            model.as_ptr(),
            tokens.as_ptr(),
            language.as_ptr(),
            if config.use_itn { 1 } else { 0 },
            config.num_threads.clamp(0, 64) as i32,
            provider_c.as_ptr(),
            err,
            ERR_LEN as i32,
        )
    }
}

impl LocalQwen {
    /// 生效的执行设备（cpu / cuda / coreml），用于健康面板与日志。
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// 加载识别器（同步、耗时 1~2s）。
    pub fn load(config: &QwenConfig) -> Result<Self, String> {
        validate_qwen_config(config)?;
        let model_dir = resolve_model_dir(config)?;
        let lib_dir = resolve_lib_dir(config)?;
        let started = Instant::now();

        let lib_dir_c = to_c_path(&lib_dir)?;
        let conv_frontend = to_c_path(&model_dir.join(MODEL_FILES[0]))?;
        let encoder = to_c_path(&model_dir.join(MODEL_FILES[1]))?;
        let decoder = to_c_path(&model_dir.join(MODEL_FILES[2]))?;
        let tokenizer = to_c_path(&model_dir.join("tokenizer"))?;

        let requested = if config.provider.trim().is_empty() { "cpu" } else { config.provider.trim() };
        let mut effective = if provider_available(&lib_dir, requested) {
            requested.to_string()
        } else {
            let dependency = provider_library_file_name(requested)
                .map(|name| format!("缺少 {name}"))
                .unwrap_or_else(|| "执行设备未知".to_string());
            tracing::warn!(
                "本地 Qwen3-ASR 请求设备 {requested}，但推理库目录 {} {dependency}，回退 CPU",
                lib_dir.display()
            );
            "cpu".into()
        };
        let mut err = [0 as c_char; ERR_LEN];
        let paths = QwenPaths {
            lib_dir: &lib_dir_c,
            conv_frontend: &conv_frontend,
            encoder: &encoder,
            decoder: &decoder,
            tokenizer: &tokenizer,
        };
        let mut handle = qwen_create_handle(&paths, config, &effective, err.as_mut_ptr());
        if handle.is_null() && effective != "cpu" {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            tracing::warn!("本地 Qwen3-ASR 使用 {effective} 加载失败，回退 CPU：{}", reason.trim());
            err = [0 as c_char; ERR_LEN];
            handle = qwen_create_handle(&paths, config, "cpu", err.as_mut_ptr());
            effective = "cpu".into();
        }
        if handle.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() {
                "加载 Qwen3-ASR 失败（未知原因）".into()
            } else {
                reason
            });
        }
        tracing::info!(
            "本地 Qwen3-ASR 模型已加载：{}（推理库 {}，设备 {}，{:.2}s）",
            model_dir.display(),
            lib_dir.display(),
            effective,
            started.elapsed().as_secs_f64()
        );
        Ok(Self {
            handle,
            model_dir,
            lib_dir,
            max_audio_seconds: config.max_audio_seconds,
            provider: effective,
            gate: Mutex::new(()),
        })
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn lib_dir(&self) -> &Path {
        &self.lib_dir
    }

    /// 单块识别（16k 单声道 i16），返回纯文本。
    fn transcribe_chunk(&self, samples: &[i16]) -> Result<String, String> {
        let mono: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut err = [0 as c_char; ERR_LEN];
        let text_ptr = unsafe {
            att_qwen_transcribe(
                self.handle,
                mono.as_ptr(),
                mono.len() as i32,
                SAMPLE_RATE as i32,
                err.as_mut_ptr(),
                ERR_LEN as i32,
            )
        };
        if text_ptr.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() {
                "Qwen3-ASR 推理失败（未知原因）".into()
            } else {
                reason
            });
        }
        let text = unsafe { CStr::from_ptr(text_ptr) }.to_string_lossy().to_string();
        unsafe { att_qwen_free_text(text_ptr) };
        Ok(text.trim().to_string())
    }

    /// 整段 PCM 转写：超过 `max_audio_seconds` 时按静音切点分块，时间戳按块偏移拼接。
    ///
    /// `language` 只用于日志（sherpa 的 Qwen3-ASR 不支持语言指定，模型自行判定）。
    pub fn transcribe(&self, language: &str, samples: &[i16]) -> Result<Transcript, String> {
        self.transcribe_with_progress(language, samples, &mut |_, _| true)
    }

    /// 同 [`Self::transcribe`]，每块推理前回调（已处理样本数, 总样本数），全部完成后再回调一次（总数, 总数）；
    /// 回调返回 false 时不再推理后面的块，返回 [`CANCELLED`]。
    pub fn transcribe_with_progress(
        &self,
        language: &str,
        samples: &[i16],
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Transcript, String> {
        let _guard = self.gate.lock().unwrap_or_else(|error| error.into_inner());
        let started = Instant::now();
        let duration = samples.len() as f64 / SAMPLE_RATE as f64;
        if samples.is_empty() {
            return Ok(Transcript { text: String::new(), segments: Vec::new(), duration: 0.0, elapsed: 0.0 });
        }
        let cut_points = silence_cut_points(samples, SAMPLE_RATE);
        let chunks = plan_chunks(samples.len(), SAMPLE_RATE, self.max_audio_seconds, &cut_points);
        tracing::info!(
            "本地 Qwen3-ASR 转写：语言={language}，时长 {duration:.1}s，分 {} 块（每块 ≤{}s）",
            chunks.len(),
            self.max_audio_seconds
        );

        let mut segments = Vec::new();
        for (start, end) in chunks {
            if !on_chunk(start, samples.len()) {
                return Err(CANCELLED.to_string());
            }
            let offset = start as f64 / SAMPLE_RATE as f64;
            let text = self.transcribe_chunk(&samples[start..end])?;
            if text.is_empty() {
                continue;
            }
            segments.push(Segment { start: offset, end: end as f64 / SAMPLE_RATE as f64, text });
        }
        on_chunk(samples.len(), samples.len());
        let text = segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>().join(" ");
        let elapsed = started.elapsed().as_secs_f64();
        tracing::info!("本地 Qwen3-ASR 转写完成：{} 段，耗时 {elapsed:.2}s", segments.len());
        Ok(Transcript { text, segments, duration, elapsed })
    }

    /// 预热：跑一次 0.5s 静音，触发 ONNX Runtime 图初始化（对齐 Python `warmup()`）。
    pub fn warmup(&self) {
        let samples = vec![0i16; SAMPLE_RATE / 2];
        if let Err(error) = self.transcribe("zh", &samples) {
            tracing::warn!("本地 Qwen3-ASR 预热失败：{error}");
        }
    }
}

impl Drop for LocalQwen {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { att_qwen_destroy(self.handle) };
            self.handle = std::ptr::null_mut();
            tracing::info!("本地 Qwen3-ASR 模型已卸载");
        }
    }
}

// ---- SenseVoice C 桥（见 src/shim.c）----

#[repr(C)]
struct AttSenseVoice {
    _private: [u8; 0],
}

extern "C" {
    fn att_sensevoice_create(
        lib_dir: *const c_char,
        model: *const c_char,
        tokens: *const c_char,
        language: *const c_char,
        use_itn: i32,
        num_threads: i32,
        provider: *const c_char,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut AttSenseVoice;
    fn att_sensevoice_transcribe(
        handle: *mut AttSenseVoice,
        samples: *const f32,
        n: i32,
        sample_rate: i32,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut c_char;
    fn att_sensevoice_free_text(text: *mut c_char);
    fn att_sensevoice_destroy(handle: *mut AttSenseVoice);
}

/// 已加载的本地 SenseVoice（常驻；识别调用内部串行化）。
pub struct LocalSenseVoice {
    handle: *mut AttSenseVoice,
    model_dir: PathBuf,
    lib_dir: PathBuf,
    max_audio_seconds: i64,
    /// 生效的执行设备（cpu / cuda / coreml）。
    provider: String,
    gate: Mutex<()>,
}

unsafe impl Send for LocalSenseVoice {}
unsafe impl Sync for LocalSenseVoice {}

impl std::fmt::Debug for LocalSenseVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSenseVoice").field("model_dir", &self.model_dir).field("lib_dir", &self.lib_dir).finish()
    }
}

impl LocalSenseVoice {
    /// 生效的执行设备（cpu / cuda / coreml），用于健康面板与日志。
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// 加载识别器（同步）。
    pub fn load(config: &SenseVoiceConfig) -> Result<Self, String> {
        let model_dir = resolve_sense_voice_model_dir(config)?;
        let lib_dir = resolve_sense_voice_lib_dir(config)?;
        let started = Instant::now();
        let lib_dir_c = to_c_path(&lib_dir)?;
        let model = to_c_path(&model_dir.join(SENSE_VOICE_MODEL_FILES[0]))?;
        let tokens = to_c_path(&model_dir.join(SENSE_VOICE_MODEL_FILES[1]))?;
        let language = CString::new(if config.language.trim().is_empty() { "auto" } else { config.language.trim() })
            .map_err(|_| "语言参数包含非法字符（NUL）".to_string())?;

        let requested = if config.provider.trim().is_empty() { "cpu" } else { config.provider.trim() };
        let mut effective = if provider_available(&lib_dir, requested) {
            requested.to_string()
        } else {
            let dependency = provider_library_file_name(requested)
                .map(|name| format!("缺少 {name}"))
                .unwrap_or_else(|| "执行设备未知".to_string());
            tracing::warn!(
                "本地 SenseVoice 请求设备 {requested}，但推理库目录 {} {dependency}，回退 CPU",
                lib_dir.display()
            );
            "cpu".into()
        };
        let mut err = [0 as c_char; ERR_LEN];
        let mut handle =
            sense_voice_create_handle(&lib_dir_c, &model, &tokens, &language, config, &effective, err.as_mut_ptr());
        if handle.is_null() && effective != "cpu" {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            tracing::warn!("本地 SenseVoice 使用 {effective} 加载失败，回退 CPU：{}", reason.trim());
            err = [0 as c_char; ERR_LEN];
            handle = sense_voice_create_handle(&lib_dir_c, &model, &tokens, &language, config, "cpu", err.as_mut_ptr());
            effective = "cpu".into();
        }
        if handle.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() {
                "加载 SenseVoice 失败（未知原因）".into()
            } else {
                reason
            });
        }
        tracing::info!(
            "本地 SenseVoice 模型已加载：{}（推理库 {}，设备 {}，{:.2}s）",
            model_dir.display(),
            lib_dir.display(),
            effective,
            started.elapsed().as_secs_f64()
        );
        Ok(Self {
            handle,
            model_dir,
            lib_dir,
            max_audio_seconds: config.max_audio_seconds.clamp(5, 120),
            provider: effective,
            gate: Mutex::new(()),
        })
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    pub fn lib_dir(&self) -> &Path {
        &self.lib_dir
    }

    fn transcribe_chunk(&self, samples: &[i16]) -> Result<String, String> {
        let mono: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut err = [0 as c_char; ERR_LEN];
        let text_ptr = unsafe {
            att_sensevoice_transcribe(
                self.handle,
                mono.as_ptr(),
                mono.len() as i32,
                SAMPLE_RATE as i32,
                err.as_mut_ptr(),
                ERR_LEN as i32,
            )
        };
        if text_ptr.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() {
                "SenseVoice 推理失败（未知原因）".into()
            } else {
                reason
            });
        }
        let text = unsafe { CStr::from_ptr(text_ptr) }.to_string_lossy().to_string();
        unsafe { att_sensevoice_free_text(text_ptr) };
        Ok(text.trim().to_string())
    }

    /// 整段 PCM 转写：超过 `max_audio_seconds` 时按静音切点分块，时间戳按块偏移拼接。
    pub fn transcribe(&self, language: &str, samples: &[i16]) -> Result<Transcript, String> {
        self.transcribe_with_progress(language, samples, &mut |_, _| true)
    }

    /// 同 [`Self::transcribe`]，用法与本地 Qwen 的进度回调一致。
    pub fn transcribe_with_progress(
        &self,
        language: &str,
        samples: &[i16],
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Transcript, String> {
        let _guard = self.gate.lock().unwrap_or_else(|error| error.into_inner());
        let started = Instant::now();
        let duration = samples.len() as f64 / SAMPLE_RATE as f64;
        if samples.is_empty() {
            return Ok(Transcript { text: String::new(), segments: Vec::new(), duration: 0.0, elapsed: 0.0 });
        }
        let cut_points = silence_cut_points(samples, SAMPLE_RATE);
        let chunks = plan_chunks(samples.len(), SAMPLE_RATE, self.max_audio_seconds, &cut_points);
        tracing::info!(
            "本地 SenseVoice 转写：语言={language}，时长 {duration:.1}s，分 {} 块（每块 ≤{}s）",
            chunks.len(),
            self.max_audio_seconds
        );

        let mut segments = Vec::new();
        for (start, end) in chunks {
            if !on_chunk(start, samples.len()) {
                return Err(CANCELLED.to_string());
            }
            let offset = start as f64 / SAMPLE_RATE as f64;
            let text = self.transcribe_chunk(&samples[start..end])?;
            if text.is_empty() {
                continue;
            }
            segments.push(Segment { start: offset, end: end as f64 / SAMPLE_RATE as f64, text });
        }
        on_chunk(samples.len(), samples.len());
        let text = segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>().join(" ");
        let elapsed = started.elapsed().as_secs_f64();
        tracing::info!("本地 SenseVoice 转写完成：{} 段，耗时 {elapsed:.2}s", segments.len());
        Ok(Transcript { text, segments, duration, elapsed })
    }

    /// 预热：跑一次 0.5s 静音，触发 ONNX Runtime 图初始化。
    pub fn warmup(&self) {
        let samples = vec![0i16; SAMPLE_RATE / 2];
        if let Err(error) = self.transcribe("auto", &samples) {
            tracing::warn!("本地 SenseVoice 预热失败：{error}");
        }
    }
}

impl Drop for LocalSenseVoice {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { att_sensevoice_destroy(self.handle) };
            self.handle = std::ptr::null_mut();
            tracing::info!("本地 SenseVoice 模型已卸载");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-qwen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_model_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("tokenizer")).unwrap();
        for file in MODEL_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        std::fs::write(dir.join("tokenizer").join("vocab.json"), b"{}").unwrap();
        dir
    }

    fn make_sense_voice_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for file in SENSE_VOICE_MODEL_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn 模型目录判定_需要三个_onnx_与_tokenizer() {
        let root = temp_dir("looks");
        let dir = make_model_dir(&root, "m");
        assert!(looks_like_model_dir(&dir));
        std::fs::remove_file(dir.join(MODEL_FILES[2])).unwrap();
        assert!(!looks_like_model_dir(&dir));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 模型目录解析_候选顺序与兜底扫描() {
        let root = temp_dir("resolve");
        // 1) 设置的模型名相对模型根解析
        let named = make_model_dir(&root, "Qwen3-ASR-0.6B");
        let config = QwenConfig::new("Qwen/Qwen3-ASR-0.6B", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), named);
        let _ = std::fs::remove_dir_all(&root);

        // 2) 兜底扫描：只在 models/ 下有个 qwen 命名的目录
        let root = temp_dir("resolve2");
        let scanned = make_model_dir(&root, "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25");
        let config = QwenConfig::new("Qwen/Qwen3-ASR-1.7B", &root);
        assert_eq!(resolve_model_dir(&config).unwrap(), scanned);
        let _ = std::fs::remove_dir_all(&root);

        // 3) 缺失时：首行一句结论，其后是应用内下载说明与目标目录
        let root = temp_dir("missing");
        let config = QwenConfig::new("Qwen/Qwen3-ASR-1.7B", &root);
        let error = resolve_model_dir(&config).unwrap_err();
        assert_eq!(error.lines().next(), Some(MODEL_MISSING), "{error}");
        assert!(error.contains("设置 → 识别引擎"), "{error}");
        assert!(error.contains(&format!("\t{}", root.join("qwen3-asr-0.6B-int8").display())), "{error}");
        assert!(error.contains("conv_frontend.onnx"), "{error}");
        assert!(!error.contains("ATT_MODELS_DIR"), "{error}");
        // 设置里的模型名是兼容字段（默认 1.7B，与实际用的 sherpa 0.6B int8 不符），不进文案
        assert!(!error.contains("Qwen3-ASR-1.7B"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sensevoice_模型目录判定与解析() {
        let root = temp_dir("sense-look");
        let dir = make_sense_voice_dir(&root, "sense");
        assert!(looks_like_sense_voice_model_dir(&dir));
        std::fs::remove_file(dir.join("tokens.txt")).unwrap();
        assert!(!looks_like_sense_voice_model_dir(&dir));
        let _ = std::fs::remove_dir_all(&root);

        let root = temp_dir("sense-resolve");
        let installed = make_sense_voice_dir(&root, "sense-voice-zh-en-ja-ko-yue-int8");
        let config = SenseVoiceConfig::new("sense-voice-zh-en-ja-ko-yue-int8", &root);
        assert_eq!(resolve_sense_voice_model_dir(&config).unwrap(), installed);
        let _ = std::fs::remove_dir_all(&root);

        let root = temp_dir("sense-scan");
        let packaged = make_sense_voice_dir(&root, "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17");
        let config = SenseVoiceConfig::new("missing", &root);
        assert_eq!(resolve_sense_voice_model_dir(&config).unwrap(), packaged);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sensevoice_缺模型文案指向应用内下载() {
        let root = temp_dir("sense-missing");
        let config = SenseVoiceConfig::new("sense-voice-zh-en-ja-ko-yue-int8", &root);
        let error = resolve_sense_voice_model_dir(&config).unwrap_err();
        assert_eq!(error.lines().next(), Some(SENSE_VOICE_MODEL_MISSING), "{error}");
        assert!(error.contains("设置 → 识别引擎"), "{error}");
        assert!(error.contains("model.int8.onnx"), "{error}");
        assert!(error.contains("tokens.txt"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 推理库目录解析_默认_models_sherpa_onnx() {
        let root = temp_dir("lib");
        let config = QwenConfig::new("m", &root);
        let error = resolve_lib_dir(&config).unwrap_err();
        assert_eq!(error.lines().next(), Some(LIB_MISSING), "没有库时就该报错，首行只放结论：{error}");
        assert!(error.contains(library_file_name()), "{error}");
        let expected_commands = if cfg!(target_os = "windows") { 3 } else { 2 };
        assert_eq!(error.lines().filter(|line| line.starts_with('\t')).count(), expected_commands, "{error}");
        let lib = root.join("sherpa-onnx");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(library_file_name()), b"x").unwrap();
        assert_eq!(resolve_lib_dir(&config).unwrap(), lib);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 推理库目录顺序_显式_环境_安装包_模型() {
        let root = temp_dir("lib-order");
        let explicit = root.join("explicit");
        let env = root.join("env");
        let bundled = root.join("bundle").join("sherpa-onnx");
        let config = QwenConfig {
            lib_dir: Some(explicit.clone()),
            bundled_lib_dir: Some(bundled.clone()),
            ..QwenConfig::new("m", &root)
        };
        assert_eq!(
            lib_dir_candidates(&config, Some(env.clone())),
            vec![explicit, env, bundled, root.join("sherpa-onnx")],
            "显式配置 > 环境变量 > 安装包资源 > 开发态模型目录"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 执行设备_只在对应_provider_动态库存在时可用() {
        let root = temp_dir("provider");
        assert!(provider_available(&root, "cpu"));
        assert!(!provider_available(&root, "cuda"));
        let cuda = provider_library_file_name("cuda").unwrap();
        std::fs::write(root.join(cuda), b"provider").unwrap();
        assert!(provider_available(&root, "cuda"));
        assert!(!provider_available(&root, "coreml"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn qwen_参数越界时拒绝而不是静默夹取() {
        let root = temp_dir("limits");
        let mut config = QwenConfig::new("m", &root);
        config.max_new_tokens = QWEN3_ASR_MAX_NEW_TOKENS_RANGE.1 + 1;
        assert!(validate_qwen_config(&config).unwrap_err().contains("max_new_tokens"));
        config.max_new_tokens = QWEN3_ASR_MAX_NEW_TOKENS_RANGE.1;
        config.max_audio_seconds = QWEN3_ASR_MAX_AUDIO_SECONDS_RANGE.1 + 1;
        assert!(validate_qwen_config(&config).unwrap_err().contains("max_audio_seconds"));
        config.max_audio_seconds = QWEN3_ASR_MAX_AUDIO_SECONDS_RANGE.1;
        assert!(validate_qwen_config(&config).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_缺库提示使用_powershell_可执行命令() {
        let error =
            resolve_lib_dir(&QwenConfig::new("m", std::env::temp_dir().join("att-qwen-missing-lib"))).unwrap_err();
        assert!(error.contains("curl.exe -L"), "{error}");
        assert!(error.contains("tar.exe -xjf"), "{error}");
        assert!(error.contains("New-Item -ItemType Directory -Force"), "{error}");
        assert!(!error.contains("wget"), "{error}");
        assert!(!error.contains("&&"), "{error}");
    }

    #[test]
    fn 分块规划_优先在静音点切且不超过上限() {
        let sample_rate = SAMPLE_RATE;
        // 100 秒音频，静音点每 30 秒一个
        let total = sample_rate * 100;
        let cuts = vec![sample_rate * 25, sample_rate * 50, sample_rate * 75];
        let chunks = plan_chunks(total, sample_rate, 30, &cuts);
        assert_eq!(chunks.first().unwrap().0, 0);
        // 第一块在 30s 窗内最靠右的静音点切 = 25s
        assert_eq!(chunks[0].1, sample_rate * 25);
        for (start, end) in &chunks {
            assert!(end > start);
            assert!(*end - *start <= sample_rate * 30, "每块不得超过上限");
        }
        assert_eq!(chunks.last().unwrap().1, total, "最后一块到结尾");
    }

    #[test]
    fn 分块规划_无静音点则硬切() {
        let chunks = plan_chunks(SAMPLE_RATE * 70, SAMPLE_RATE, 30, &[]);
        assert_eq!(
            chunks,
            vec![(0, SAMPLE_RATE * 30), (SAMPLE_RATE * 30, SAMPLE_RATE * 60), (SAMPLE_RATE * 60, SAMPLE_RATE * 70)]
        );
    }

    #[test]
    fn 静音切点_识别长静音的中点() {
        // 1s 声音 + 1s 静音 + 1s 声音：切点应落在中间静音段（约 1.5s）
        let mut samples: Vec<i16> = (0..SAMPLE_RATE).map(|i| (((i as f64) * 0.05).sin() * 8000.0) as i16).collect();
        samples.extend(vec![0i16; SAMPLE_RATE]);
        samples.extend((0..SAMPLE_RATE).map(|i| (((i as f64) * 0.05).sin() * 8000.0) as i16));
        let cuts = silence_cut_points(&samples, SAMPLE_RATE);
        assert_eq!(cuts.len(), 1);
        let cut = cuts[0] as f64 / SAMPLE_RATE as f64;
        assert!((1.3..1.7).contains(&cut), "切点应在中间静音段内：{cut}");
    }
}
