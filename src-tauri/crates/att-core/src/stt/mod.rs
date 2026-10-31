//! 语音识别引擎。
//!
//! - [`sauc`]：火山 SAUC 二进制协议编解码（纯函数）
//! - [`doubao`]：豆包会话（流式推流 / 批量转写）
//! - [`chatgpt`]：ChatGPT 听写（在线批量，不支持流式）
//! - [`local`]：本地引擎装配与常驻池（faster-whisper / Qwen3-ASR，进程内推理）
//!
//! 在线引擎（豆包 / ChatGPT）凭据齐备即就绪；本地引擎要在后台线程加载模型（large-v3 在 CPU 上
//! 要十几秒），所以 [`Engine::resolve`] 只校验模型文件、随即在后台预加载（[`preload_local`]），
//! 录音立刻开始；真正推理时（[`Engine::transcribe_pcm`] / [`transcribe_local`]）再取常驻句柄，
//! 若还在加载就在 blocking 线程上等它加载完。

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

/// 本地引擎的延迟句柄：录音开始时不等模型加载，推理时才从常驻池取（必要时等加载完成）。
#[derive(Clone)]
pub struct LocalHandle {
    core: Arc<CoreState>,
    kind: LocalKind,
}

impl std::fmt::Debug for LocalHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalHandle").field("kind", &self.kind).finish()
    }
}

/// 录音使用的引擎（按设置 `asr_provider` 选择）。
#[derive(Clone, Debug)]
pub enum Engine {
    Doubao(doubao::DoubaoConfig),
    ChatGpt(chatgpt::ChatGptConfig),
    Local(LocalHandle),
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
                "{} 是本地引擎，请用 Engine::resolve 解析",
                LocalKind::from_provider(other).map(|kind| kind.label()).unwrap_or(other)
            )),
            other => Err(format!("未知的 ASR 引擎：{other}")),
        }
    }

    /// 按当前设置解析引擎；不阻塞：本地引擎只校验模型文件是否就位，并在后台开始预加载。
    pub fn resolve(core: &Arc<CoreState>) -> Result<Self, String> {
        let provider = core.settings().asr_provider;
        match LocalKind::from_provider(&provider) {
            Some(kind) => {
                let status = core.local_models.status(core, kind);
                if !status.available {
                    return Err(status.error.unwrap_or_else(|| format!("{}不可用", kind.label())));
                }
                preload_local(core.clone(), kind);
                Ok(Engine::Local(LocalHandle { core: core.clone(), kind }))
            }
            None => Self::online_from_settings(core),
        }
    }

    pub fn supports_streaming(&self) -> bool {
        matches!(self, Engine::Doubao(_))
    }

    pub fn label(&self) -> &'static str {
        match self {
            Engine::Doubao(_) => "豆包语音",
            Engine::ChatGpt(_) => "ChatGPT 听写",
            Engine::Local(handle) => handle.kind.label(),
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
            Engine::Local(handle) => transcribe_local(handle.core.clone(), handle.kind, language.to_string(), samples.to_vec())
                .await
                .map(|(_, outcome)| outcome.text),
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

/// 后台预加载本地引擎（切换引擎、开始录音时调用）：已加载、正在加载或文件缺失时什么都不做。
///
/// 不在 tokio 运行时内（如同步单测）时直接跳过，等真正推理时再加载。
pub fn preload_local(core: Arc<CoreState>, kind: LocalKind) {
    let status = core.local_models.status(&core, kind);
    if status.loaded || status.loading || !status.available {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    if !core.local_models.enqueue_preload(kind) {
        return;
    }
    runtime.spawn_blocking(move || {
        match core.local_models.get(&core, kind) {
            Ok(engine) => {
                engine.warmup();
                tracing::info!("{} 预加载完成（{}）", engine.label(), engine.device());
            }
            Err(error) => tracing::warn!("{} 预加载失败：{error}", kind.label()),
        }
        core.local_models.finish_preload(kind);
    });
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
