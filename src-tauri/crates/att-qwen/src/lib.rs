//! 本地 Qwen3-ASR 引擎（ONNX 版，走 sherpa-onnx）。
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
//! 推理库与模型都不随仓库分发：推理库放 `models/sherpa-onnx/`（或 `ATT_SHERPA_ONNX_DIR`），
//! 模型放 `models/qwen3-asr-0.6B-int8/`（或 `ATT_MODELS_DIR`），缺失时给出可直接照做的提示。

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub mod vad;

pub use vad::SileroVad;

/// 16kHz：录音链路与模型输入采样率。
pub const SAMPLE_RATE: usize = 16_000;
/// 模型必需文件（sherpa-onnx Qwen3-ASR 导出目录的固定布局）。
pub const MODEL_FILES: [&str; 3] = ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"];
/// 错误缓冲区长度（与 shim.c 的 ATT_ERR_LEN 一致）。
const ERR_LEN: usize = 1024;

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
    /// 模型根目录（`models/`）。
    pub models_dir: PathBuf,
    /// 推理库目录覆盖；`None` 时按 [`resolve_lib_dir`] 探测。
    pub lib_dir: Option<PathBuf>,
}

impl QwenConfig {
    pub fn new(model: impl Into<String>, models_dir: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            max_audio_seconds: 30,
            max_new_tokens: 512,
            num_threads: 2,
            models_dir: models_dir.into(),
            lib_dir: None,
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

/// 模型目录是否像一份 sherpa-onnx Qwen3-ASR 导出（三个 onnx + tokenizer 目录）。
pub fn looks_like_model_dir(path: &Path) -> bool {
    path.is_dir()
        && MODEL_FILES.iter().all(|name| path.join(name).is_file())
        && path.join("tokenizer").is_dir()
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
    Err(format!(
        "未找到本地 Qwen3-ASR 模型「{}」：请用下面命令下载并解压到 {}（可用环境变量 ATT_MODELS_DIR 指定根目录）：\n\
         \twget https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2\n\
         \ttar xvf sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2 -C {}\n\
         目录内需要 {} 与 tokenizer/",
        config.model,
        config.models_dir.display(),
        config.models_dir.display(),
        MODEL_FILES.join("、"),
    ))
}

/// 解析 sherpa-onnx 推理库目录：显式配置 > `ATT_SHERPA_ONNX_DIR` > `<models>/sherpa-onnx`。
pub fn resolve_lib_dir(config: &QwenConfig) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(dir) = &config.lib_dir {
        candidates.push(dir.clone());
    }
    if let Ok(dir) = std::env::var("ATT_SHERPA_ONNX_DIR") {
        if !dir.trim().is_empty() {
            candidates.push(PathBuf::from(dir));
        }
    }
    candidates.push(config.models_dir.join("sherpa-onnx"));
    let library = library_file_name();
    for candidate in &candidates {
        if candidate.join(library).is_file() {
            return Ok(candidate.clone());
        }
    }
    let package = library_package_name();
    Err(format!(
        "未找到 sherpa-onnx 推理库（缺少 {}）：请把 sherpa-onnx 的共享库放到 {}（或用环境变量 ATT_SHERPA_ONNX_DIR 指定），例如：\n\
         \twget https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/{package}\n\
         \tmkdir -p {} && tar xvf {package} --strip-components=2 -C {}",
        library,
        config.models_dir.join("sherpa-onnx").display(),
        config.models_dir.join("sherpa-onnx").display(),
        config.models_dir.join("sherpa-onnx").display(),
    ))
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
pub fn plan_chunks(samples_len: usize, sample_rate: usize, max_audio_seconds: i64, cut_points: &[usize]) -> Vec<(usize, usize)> {
    let limit = max_audio_seconds.max(1) as usize * sample_rate;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < samples_len {
        let limit_end = (start + limit).min(samples_len);
        if limit_end >= samples_len {
            chunks.push((start, samples_len));
            break;
        }
        let end = cut_points
            .iter()
            .copied()
            .filter(|cut| *cut > start && *cut <= limit_end)
            .next_back()
            .unwrap_or(limit_end);
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
    /// sherpa-onnx 的 recognizer 本身是可变状态，串行调用
    gate: Mutex<()>,
}

// 句柄只在 C 侧使用；串行由 gate 保证
unsafe impl Send for LocalQwen {}
unsafe impl Sync for LocalQwen {}

impl std::fmt::Debug for LocalQwen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalQwen")
            .field("model_dir", &self.model_dir)
            .field("lib_dir", &self.lib_dir)
            .finish()
    }
}

fn to_c_path(path: &Path) -> Result<CString, String> {
    let text = path.to_string_lossy().to_string();
    CString::new(text.clone()).map_err(|_| format!("路径包含非法字符（NUL）：{text}"))
}

impl LocalQwen {
    /// 加载识别器（同步、耗时 1~2s）。
    pub fn load(config: &QwenConfig) -> Result<Self, String> {
        let model_dir = resolve_model_dir(config)?;
        let lib_dir = resolve_lib_dir(config)?;
        let started = Instant::now();

        let lib_dir_c = to_c_path(&lib_dir)?;
        let conv_frontend = to_c_path(&model_dir.join(MODEL_FILES[0]))?;
        let encoder = to_c_path(&model_dir.join(MODEL_FILES[1]))?;
        let decoder = to_c_path(&model_dir.join(MODEL_FILES[2]))?;
        let tokenizer = to_c_path(&model_dir.join("tokenizer"))?;

        let mut err = [0 as c_char; ERR_LEN];
        let handle = unsafe {
            att_qwen_create(
                lib_dir_c.as_ptr(),
                conv_frontend.as_ptr(),
                encoder.as_ptr(),
                decoder.as_ptr(),
                tokenizer.as_ptr(),
                config.num_threads.clamp(0, 64) as i32,
                config.max_new_tokens.clamp(16, 4096) as i32,
                err.as_mut_ptr(),
                ERR_LEN as i32,
            )
        };
        if handle.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() { "加载 Qwen3-ASR 失败（未知原因）".into() } else { reason });
        }
        tracing::info!(
            "本地 Qwen3-ASR 模型已加载：{}（推理库 {}，{:.2}s）",
            model_dir.display(),
            lib_dir.display(),
            started.elapsed().as_secs_f64()
        );
        Ok(Self {
            handle,
            model_dir,
            lib_dir,
            max_audio_seconds: config.max_audio_seconds.clamp(5, 120),
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
            return Err(if reason.trim().is_empty() { "Qwen3-ASR 推理失败（未知原因）".into() } else { reason });
        }
        let text = unsafe { CStr::from_ptr(text_ptr) }.to_string_lossy().to_string();
        unsafe { att_qwen_free_text(text_ptr) };
        Ok(text.trim().to_string())
    }

    /// 整段 PCM 转写：超过 `max_audio_seconds` 时按静音切点分块，时间戳按块偏移拼接。
    ///
    /// `language` 只用于日志（sherpa 的 Qwen3-ASR 不支持语言指定，模型自行判定）。
    pub fn transcribe(&self, language: &str, samples: &[i16]) -> Result<Transcript, String> {
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
            let offset = start as f64 / SAMPLE_RATE as f64;
            let text = self.transcribe_chunk(&samples[start..end])?;
            if text.is_empty() {
                continue;
            }
            segments.push(Segment { start: offset, end: end as f64 / SAMPLE_RATE as f64, text });
        }
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

        // 3) 缺失时提示可照做
        let root = temp_dir("missing");
        let config = QwenConfig::new("Qwen/Qwen3-ASR-1.7B", &root);
        let error = resolve_model_dir(&config).unwrap_err();
        assert!(error.contains("sherpa-onnx-qwen3-asr"), "{error}");
        assert!(error.contains("conv_frontend.onnx"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 推理库目录解析_默认_models_sherpa_onnx() {
        let root = temp_dir("lib");
        let config = QwenConfig::new("m", &root);
        assert!(resolve_lib_dir(&config).is_err(), "没有库时就该报错");
        let lib = root.join("sherpa-onnx");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(library_file_name()), b"x").unwrap();
        assert_eq!(resolve_lib_dir(&config).unwrap(), lib);
        let _ = std::fs::remove_dir_all(&root);
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
