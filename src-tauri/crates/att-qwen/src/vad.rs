//! Silero VAD（走 sherpa-onnx 的 C API，与 Qwen 引擎共用同一份动态库）。
//!
//! 为什么要它：Python faster-whisper 的 `vad_filter=True` 是 Silero VAD，用来先把「非语音」段
//! （背景音乐、室内噪声、片头片尾）剔掉再送模型。没有这一步时，large-v3 在噪声/音乐素材上会
//! 整段输出固定幻觉（实测「请不吝点赞 订阅 转发 打赏支持明镜与点点栏目」这种字幕模板文本）。
//! Rust 侧没有现成 VAD 实现，但 sherpa-onnx 自带 Silero VAD，复用它最省事。
//!
//! 使用方式：把 `silero_vad.onnx`（约 700KB，sherpa-onnx 的 `asr-models` release 里有）放到
//! `models/silero_vad.onnx`（或 `ATT_SILERO_VAD` 指定），上层按 [`SileroVad::speech_ranges`]
//! 得到的区间逐个送 Whisper。

use crate::resolve_lib_dir;
use serde::Deserialize;
use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ERR_LEN: usize = 1024;
/// VAD 模型默认文件名。
pub const VAD_MODEL_FILE: &str = "silero_vad.onnx";

#[repr(C)]
struct AttVad {
    _private: [u8; 0],
}

extern "C" {
    fn att_vad_create(
        lib_dir: *const c_char,
        model: *const c_char,
        threshold: f32,
        min_silence: f32,
        min_speech: f32,
        max_speech: f32,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut AttVad;
    fn att_vad_speech_ranges(
        handle: *mut AttVad,
        samples: *const f32,
        n: i32,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut c_char;
    fn att_vad_destroy(handle: *mut AttVad);
}

/// 解析 VAD 模型文件：`ATT_SILERO_VAD` > `<models>/silero_vad.onnx` > `<models>/sherpa-onnx/silero_vad.onnx`。
pub fn resolve_vad_model(models_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("ATT_SILERO_VAD") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    for candidate in [models_dir.join(VAD_MODEL_FILE), models_dir.join("sherpa-onnx").join(VAD_MODEL_FILE)] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "未找到 Silero VAD 模型（{}）：可用 `wget https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx -O {}` 或设置 ATT_SILERO_VAD",
        VAD_MODEL_FILE,
        models_dir.join(VAD_MODEL_FILE).display()
    ))
}

/// 一个已加载的 Silero VAD（常驻；内部串行化）。
pub struct SileroVad {
    handle: *mut AttVad,
    model: PathBuf,
    gate: Mutex<()>,
}

// 句柄只在 C 侧使用，串行由 gate 保证
unsafe impl Send for SileroVad {}
unsafe impl Sync for SileroVad {}

impl std::fmt::Debug for SileroVad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SileroVad").field("model", &self.model).finish()
    }
}

#[derive(Deserialize)]
struct RawRange(i64, i64);

impl SileroVad {
    /// 加载 VAD（需要推理库目录；`lib_dir` 为 `None` 时按 [`crate::resolve_lib_dir`] 探测）。
    pub fn load(models_dir: &Path, lib_dir: Option<&Path>) -> Result<Self, String> {
        let model = resolve_vad_model(models_dir)?;
        let config = crate::QwenConfig { lib_dir: lib_dir.map(Path::to_path_buf), ..crate::QwenConfig::new("", models_dir) };
        let lib = resolve_lib_dir(&config)?;
        let lib_c = CString::new(lib.to_string_lossy().to_string()).map_err(|_| "推理库路径含非法字符".to_string())?;
        let model_c = CString::new(model.to_string_lossy().to_string()).map_err(|_| "VAD 模型路径含非法字符".to_string())?;
        let mut err = [0 as c_char; ERR_LEN];
        let handle = unsafe {
            att_vad_create(
                lib_c.as_ptr(),
                model_c.as_ptr(),
                0.5,  // speech 概率阈值（sherpa 默认）
                0.5,  // 静音多久算断句（秒），与 Python vad_parameters.min_silence_duration_ms=500 一致
                0.25, // 太短的片段丢掉
                30.0, // 超过 30s 强制切（Whisper 的单窗上限）
                err.as_mut_ptr(),
                ERR_LEN as i32,
            )
        };
        if handle.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() { "加载 Silero VAD 失败（未知原因）".into() } else { reason });
        }
        tracing::info!("Silero VAD 已加载：{}", model.display());
        Ok(Self { handle, model, gate: Mutex::new(()) })
    }

    pub fn model(&self) -> &Path {
        &self.model
    }

    /// 语音区间（采样点，左闭右开）；无可识别语音时返回空表。
    pub fn speech_ranges(&self, samples: &[i16]) -> Result<Vec<(usize, usize)>, String> {
        let _guard = self.gate.lock().unwrap_or_else(|error| error.into_inner());
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let mono: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut err = [0 as c_char; ERR_LEN];
        let json_ptr = unsafe {
            att_vad_speech_ranges(self.handle, mono.as_ptr(), mono.len() as i32, err.as_mut_ptr(), ERR_LEN as i32)
        };
        if json_ptr.is_null() {
            let reason = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().to_string();
            return Err(if reason.trim().is_empty() { "VAD 推理失败（未知原因）".into() } else { reason });
        }
        let json = unsafe { CStr::from_ptr(json_ptr) }.to_string_lossy().to_string();
        unsafe { crate::att_qwen_free_text(json_ptr) };
        let raw: Vec<RawRange> = serde_json::from_str(&json).map_err(|error| format!("解析 VAD 结果失败：{error}（{json}）"))?;
        let ranges: Vec<(usize, usize)> = raw
            .into_iter()
            .filter(|range| range.1 > range.0 && range.0 >= 0)
            .map(|range| (range.0 as usize, range.1.min(samples.len() as i64) as usize))
            .filter(|(start, end)| end > start)
            .collect();
        Ok(ranges)
    }
}

impl Drop for SileroVad {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { att_vad_destroy(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_模型缺失时给出可照做提示() {
        let dir = std::env::temp_dir().join(format!("att-vad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = resolve_vad_model(&dir).unwrap_err();
        assert!(error.contains("silero_vad.onnx"), "{error}");
        assert!(error.contains("ATT_SILERO_VAD"), "{error}");
        std::fs::write(dir.join(VAD_MODEL_FILE), b"x").unwrap();
        assert_eq!(resolve_vad_model(&dir).unwrap(), dir.join(VAD_MODEL_FILE));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
