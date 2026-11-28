//! 关闭 `local-engines` feature 时的空实现：接口保持可用，统一返回「此构建不含本地引擎」。

use crate::state::CoreState;
use std::path::PathBuf;
use std::time::Duration;

/// 本地 Whisper 的 provider 标识（与设置 `asr_provider` 一致）。
pub const WHISPER: &str = "whisper";
/// 本地 Qwen3-ASR 的 provider 标识。
pub const QWEN: &str = "qwen_asr";
/// 本地 SenseVoice 的 provider 标识。
pub const SENSE_VOICE: &str = "sense_voice";
/// 本地引擎种类（与开启 feature 时保持一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Whisper,
    Qwen,
    SenseVoice,
}

impl LocalKind {
    pub const ALL: [Self; 3] = [Self::Whisper, Self::Qwen, Self::SenseVoice];

    /// provider 字符串 → 本地引擎种类（在线引擎返回 `None`）。
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider.trim() {
            WHISPER => Some(Self::Whisper),
            QWEN => Some(Self::Qwen),
            SENSE_VOICE => Some(Self::SenseVoice),
            _ => None,
        }
    }

    pub fn provider(&self) -> &'static str {
        match self {
            Self::Whisper => WHISPER,
            Self::Qwen => QWEN,
            Self::SenseVoice => SENSE_VOICE,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Whisper => "本地 Whisper",
            Self::Qwen => "本地 Qwen3-ASR",
            Self::SenseVoice => "本地 SenseVoice",
        }
    }
}

/// 一段本地转写结果（类型与开启 feature 时一致，实际不会被成功构造）。
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

/// 已加载的本地引擎句柄；关闭 feature 时永远是「不可用」。
#[derive(Debug, Clone)]
pub struct LocalEngine {
    kind: LocalKind,
}

impl LocalEngine {
    pub fn kind(&self) -> LocalKind {
        self.kind
    }

    pub fn label(&self) -> &'static str {
        self.kind.label()
    }

    pub fn has_vad(&self) -> bool {
        false
    }

    pub fn device(&self) -> String {
        "unavailable".to_string()
    }

    pub fn model_label(&self) -> String {
        self.kind.provider().to_string()
    }

    pub fn warmup(&self) {}

    pub fn transcribe(&self, _language: &str, _samples: &[i16]) -> Result<LocalTranscript, String> {
        Err(LOCAL_ENGINES_DISABLED.into())
    }

    pub fn transcribe_with_progress(
        &self,
        _language: &str,
        _samples: &[i16],
        _on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<LocalTranscript, String> {
        Err(LOCAL_ENGINES_DISABLED.into())
    }
}

/// 本地引擎就绪性（关闭 feature 时永远不可用）。
#[derive(Debug, Clone)]
pub struct LocalStatus {
    pub available: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub error_detail: Option<String>,
    pub load_error: Option<String>,
    pub model_dir: Option<PathBuf>,
    pub device: Option<String>,
}

/// 把引擎 crate 的两段式错误拆成（首行结论, 详情）：详情为空时返回 `None`。
pub fn split_hint(error: &str) -> (String, Option<String>) {
    let trimmed = error.trim_end();
    match trimmed.split_once('\n') {
        Some((summary, detail)) if !detail.trim().is_empty() => (summary.trim().to_string(), Some(detail.to_string())),
        Some((summary, _)) => (summary.trim().to_string(), None),
        None => (trimmed.trim().to_string(), None),
    }
}

/// 本地引擎常驻池的空实现：状态恒为不可用，加载请求统一报错。
#[derive(Default)]
pub struct LocalModels;

impl LocalModels {
    pub fn is_loaded(&self, _kind: LocalKind) -> bool {
        false
    }

    pub fn get(&self, _core: &CoreState, _kind: LocalKind) -> Result<LocalEngine, String> {
        Err(LOCAL_ENGINES_DISABLED.into())
    }

    pub fn is_loading(&self, _kind: LocalKind) -> bool {
        false
    }

    pub fn enqueue_preload(&self, _kind: LocalKind) -> bool {
        false
    }

    pub fn finish_preload(&self, _kind: LocalKind) {}

    pub fn unload(&self, _kind: Option<LocalKind>) {}

    pub fn unload_idle(&self, _threshold: Duration, _externally_busy: bool) -> usize {
        0
    }

    pub fn status(&self, _core: &CoreState, _kind: LocalKind) -> LocalStatus {
        LocalStatus {
            available: false,
            loaded: false,
            loading: false,
            error: Some(LOCAL_ENGINES_DISABLED.into()),
            error_detail: None,
            load_error: None,
            model_dir: None,
            device: None,
        }
    }

    pub fn vad_loaded(&self) -> bool {
        false
    }

    pub fn unload_vad(&self) {}
}
