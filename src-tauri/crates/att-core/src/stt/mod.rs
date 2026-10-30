//! 语音识别引擎。
//!
//! - [`sauc`]：火山 SAUC 二进制协议编解码（纯函数）
//! - [`doubao`]：豆包会话（流式推流 / 批量转写）
//! - [`chatgpt`]：ChatGPT 听写（在线批量，不支持流式）
//!
//! 本地引擎（faster-whisper / Qwen3-ASR）依赖 Python 推理运行时，尚未移植，见 [`unsupported_provider_message`]。

pub mod chatgpt;
pub mod doubao;
pub mod sauc;

use crate::state::CoreState;

/// 从用户设置组装豆包配置。密钥优先级：用户设置（密文）> 环境变量 `DOUBAO_ASR_API_KEY`。
pub fn doubao_config(core: &CoreState) -> doubao::DoubaoConfig {
    let settings = core.settings();
    let api_key = settings
        .doubao_asr_api_key_encrypted
        .as_deref()
        .filter(|token| !token.is_empty())
        .and_then(|token| core.crypto.decrypt(token))
        .filter(|key| !key.trim().is_empty())
        .or_else(|| std::env::var("DOUBAO_ASR_API_KEY").ok())
        .unwrap_or_default();
    let non_empty = |value: &str, fallback: &str| {
        if value.trim().is_empty() { fallback.to_string() } else { value.trim().to_string() }
    };
    doubao::DoubaoConfig {
        api_key,
        resource_id: non_empty(&settings.doubao_asr_resource_id, sauc::DEFAULT_RESOURCE_ID),
        endpoint: non_empty(&settings.doubao_asr_endpoint, sauc::DEFAULT_ENDPOINT),
        end_window_size: settings.doubao_asr_end_window_size,
    }
}

/// 从用户设置组装 ChatGPT 听写配置（令牌只存密文，运行期解密）。
pub fn chatgpt_config(core: &CoreState) -> chatgpt::ChatGptConfig {
    let settings = core.settings();
    let api_key = settings
        .chatgpt_asr_api_key_encrypted
        .as_deref()
        .filter(|token| !token.is_empty())
        .and_then(|token| core.crypto.decrypt(token))
        .map(|key| chatgpt::strip_bearer_prefix(&key))
        .unwrap_or_default();
    let endpoint = settings.chatgpt_asr_endpoint.trim();
    chatgpt::ChatGptConfig {
        api_key,
        endpoint: if endpoint.is_empty() { chatgpt::DEFAULT_ENDPOINT.into() } else { endpoint.to_string() },
        timeout: std::time::Duration::from_secs(settings.chatgpt_asr_timeout_seconds.clamp(10, 600) as u64),
    }
}

/// 尚未移植到 Rust 的引擎：给出可读提示（录音入口据此拒绝开始）。
pub fn unsupported_provider_message(provider: &str) -> Option<String> {
    match provider {
        "doubao_asr" | "chatgpt_asr" => None,
        "whisper" => Some("Tauri 版暂未接入本地 faster-whisper 引擎，请在设置中切换到「豆包语音」或「ChatGPT 听写」".into()),
        "qwen_asr" => Some("Tauri 版暂未接入本地 Qwen3-ASR 引擎，请在设置中切换到「豆包语音」或「ChatGPT 听写」".into()),
        other => Some(format!("未知的 ASR 引擎：{other}")),
    }
}

/// 录音使用的在线引擎（按设置 `asr_provider` 选择）。
#[derive(Clone, Debug)]
pub enum Engine {
    Doubao(doubao::DoubaoConfig),
    ChatGpt(chatgpt::ChatGptConfig),
}

impl Engine {
    /// 按当前设置选引擎并校验配置；不可用时返回可直接展示的原因。
    pub fn from_settings(core: &CoreState) -> Result<Self, String> {
        let provider = core.settings().asr_provider;
        if let Some(message) = unsupported_provider_message(&provider) {
            return Err(message);
        }
        if provider == "chatgpt_asr" {
            let config = chatgpt_config(core);
            config.ensure()?;
            return Ok(Engine::ChatGpt(config));
        }
        let config = doubao_config(core);
        config.ensure().map_err(|e| e.to_string())?;
        Ok(Engine::Doubao(config))
    }

    pub fn supports_streaming(&self) -> bool {
        matches!(self, Engine::Doubao(_))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Engine::Doubao(_) => "豆包语音",
            Engine::ChatGpt(_) => "ChatGPT 听写",
        }
    }

    /// 整段批量识别（单段 / 多段的每一段）。
    pub async fn transcribe_pcm(&self, language: &str, samples: &[i16]) -> Result<String, String> {
        match self {
            Engine::Doubao(config) => doubao::transcribe_pcm(config, language, "", samples).await.map(|o| o.text).map_err(|e| e.to_string()),
            Engine::ChatGpt(config) => {
                let wav = crate::audio::wav_bytes_16k(samples).map_err(|e| format!("音频编码失败：{e}"))?;
                chatgpt::transcribe_wav(config, wav, language).await
            }
        }
    }
}
