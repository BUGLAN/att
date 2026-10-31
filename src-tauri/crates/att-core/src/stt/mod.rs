//! 语音识别引擎。
//!
//! - [`sauc`]：火山 SAUC 二进制协议编解码（纯函数）
//! - [`doubao`]：豆包会话（流式推流 / 批量转写）
//! - [`chatgpt`]：ChatGPT 听写（在线批量，不支持流式）
//! - [`local`]：本地引擎装配与常驻池（faster-whisper / Qwen3-ASR，进程内推理）
//!
//! 在线引擎（豆包 / ChatGPT）凭据齐备即就绪；本地引擎要在后台线程加载模型，
//! 因此录制入口与文件转写都用 [`Engine::resolve`] / [`transcribe_local`] 这两个异步入口，
//! 而不是同步的 [`Engine::online_from_settings`]。

pub mod chatgpt;
pub mod doubao;
pub mod local;
pub mod sauc;

use crate::state::CoreState;
use local::LocalKind;
use std::sync::Arc;

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

/// provider 是否是本地引擎（`whisper` / `qwen_asr`）。
pub fn local_kind(provider: &str) -> Option<LocalKind> {
    LocalKind::from_provider(provider)
}

/// 录音使用的引擎（按设置 `asr_provider` 选择；本地引擎持有已加载的模型句柄）。
#[derive(Clone, Debug)]
pub enum Engine {
    Doubao(doubao::DoubaoConfig),
    ChatGpt(chatgpt::ChatGptConfig),
    Local(local::LocalEngine),
}

impl Engine {
    /// 只解析在线引擎（本地引擎要加载模型，见 [`Engine::resolve`]）。
    pub fn online_from_settings(core: &CoreState) -> Result<Self, String> {
        let provider = core.settings().asr_provider;
        match provider.trim() {
            "chatgpt_asr" => {
                let config = chatgpt_config(core);
                config.ensure()?;
                Ok(Engine::ChatGpt(config))
            }
            "doubao_asr" | "" => {
                let config = doubao_config(core);
                config.ensure().map_err(|error| error.to_string())?;
                Ok(Engine::Doubao(config))
            }
            other if LocalKind::from_provider(other).is_some() => Err(format!(
                "{} 是本地引擎，请在异步入口（Engine::resolve）中加载",
                LocalKind::from_provider(other).map(|kind| kind.label()).unwrap_or(other)
            )),
            other => Err(format!("未知的 ASR 引擎：{other}")),
        }
    }

    /// 按当前设置解析引擎：本地引擎在后台线程加载模型（首次使用约 1~5 秒）。
    pub async fn resolve(core: Arc<CoreState>) -> Result<Self, String> {
        let provider = core.settings().asr_provider;
        match LocalKind::from_provider(&provider) {
            Some(kind) => {
                let loader = core.clone();
                let engine = tokio::task::spawn_blocking(move || loader.local_models.get(&loader, kind))
                    .await
                    .map_err(|error| format!("加载本地模型任务失败：{error}"))??;
                Ok(Engine::Local(engine))
            }
            None => Self::online_from_settings(&core),
        }
    }

    pub fn supports_streaming(&self) -> bool {
        matches!(self, Engine::Doubao(_))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Engine::Doubao(_) => "豆包语音",
            Engine::ChatGpt(_) => "ChatGPT 听写",
            Engine::Local(engine) => engine.label(),
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
            Engine::Local(engine) => {
                // 本地推理是纯 CPU/GPU 计算，必须离开异步运行时线程
                let engine = engine.clone();
                let language = language.to_string();
                let samples = samples.to_vec();
                tokio::task::spawn_blocking(move || engine.transcribe(&language, &samples).map(|outcome| outcome.text))
                    .await
                    .map_err(|error| format!("本地引擎任务失败：{error}"))?
            }
        }
    }
}

/// 本地引擎整段转写（阻塞推理丢到 blocking 线程池），返回 `(engine 标签, 结果)`。
pub async fn transcribe_local(
    core: Arc<CoreState>,
    kind: LocalKind,
    language: String,
    samples: Vec<i16>,
) -> Result<(String, local::LocalTranscript), String> {
    tokio::task::spawn_blocking(move || {
        let engine = core.local_models.get(&core, kind)?;
        let model_label = engine.model_label();
        let transcript = engine.transcribe(&language, &samples)?;
        Ok::<_, String>((model_label, transcript))
    })
    .await
    .map_err(|error| format!("本地转写任务失败：{error}"))?
}

/// 预热本地引擎（`/api/model-service/start`）：加载模型并跑一次极短推理。
pub async fn warmup_local(core: Arc<CoreState>, kind: LocalKind) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let engine = core.local_models.get(&core, kind)?;
        engine.warmup();
        Ok::<_, String>(format!("{} 已就绪（{}）", engine.label(), engine.device()))
    })
    .await
    .map_err(|error| format!("预热任务失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_kind_识别本地_provider() {
        assert_eq!(local_kind("whisper"), Some(LocalKind::Whisper));
        assert_eq!(local_kind("qwen_asr"), Some(LocalKind::Qwen));
        assert_eq!(local_kind("doubao_asr"), None);
        assert_eq!(local_kind("chatgpt_asr"), None);
    }
}
