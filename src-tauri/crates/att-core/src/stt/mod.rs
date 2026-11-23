//! 语音识别引擎。
//!
//! - [`sauc`]：火山 SAUC 二进制协议编解码（纯函数）
//! - [`doubao`]：豆包会话（流式推流 / 批量转写）
//! - [`doubao_web`]：豆包网页匿名语音（游客初始化 + 网页 WebSocket，在线批量）
//! - [`doubao_ime`]：豆包输入法语音（虚拟设备注册 + 输入法 WebSocket，在线批量）
//! - [`hotwords`]：个人词表的归一化与 token 预算（目前只有豆包使用）
//! - [`chatgpt`]：ChatGPT 听写（在线批量，不支持流式）
//! - [`deepseek`]：DeepSeek 语音识别（在线批量，不支持流式）
//! - [`local`]：本地引擎装配与常驻池（faster-whisper / Qwen3-ASR，进程内推理）
//! - [`wetype`]：微信输入法语音（仅 Windows，直接注入输入法进程）
//! - [`verify`]：在线引擎验证（凭据握手 / 匿名连通性，区分「已配置」与「可用」）
//!
//! 在线引擎（豆包 / 豆包网页 / ChatGPT / DeepSeek）凭据齐备或匿名地址可用即就绪；本地引擎要在后台线程加载模型（large-v3 在 CPU 上
//! 要十几秒），所以 [`Engine::resolve`] 只校验模型文件、随即在后台预加载（[`preload_local`]），
//! 录音立刻开始；真正推理时（[`Engine::transcribe_pcm`] / [`transcribe_local`]）再取常驻句柄，
//! 若还在加载就在 blocking 线程上等它加载完。

pub mod chatgpt;
pub mod deepseek;
pub mod doubao;
pub mod doubao_web;
pub mod doubao_ime;
pub mod hotwords;
pub mod local;
pub mod sauc;
pub mod verify;
#[cfg(windows)]
pub mod wetype;
#[cfg(any(windows, test))]
pub(super) mod wetype_stream;

use crate::state::CoreState;
use local::LocalKind;
use std::sync::Arc;

/// 微信输入法语音的轻量状态（不 attach，设置页可安全轮询）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WetypeStatus {
    pub available: bool,
    pub pid: Option<u32>,
    pub message: String,
    pub platform_supported: bool,
}

/// 微信输入法语音是否可用：只枚举进程，供首页、文件转写和引擎列表使用。
pub fn wetype_status(core: &CoreState) -> WetypeStatus {
    #[cfg(windows)]
    {
        if core.platform == crate::Platform::Windows {
            wetype::status(&wetype_config(core))
        } else {
            WetypeStatus {
                available: false,
                pid: None,
                message: "微信输入法语音仅支持 Windows".into(),
                platform_supported: false,
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = core;
        WetypeStatus {
            available: false,
            pid: None,
            message: "微信输入法语音仅支持 Windows".into(),
            platform_supported: false,
        }
    }
}

/// 分块识别被进度回调叫停（文件转写取消）时的错误文案。
pub const TRANSCRIBE_CANCELLED: &str = "转写已取消";

/// 分块识别的进度回调：参数是（已处理样本数, 总样本数）；返回 false 表示取消，停在下一块之前。
pub type ChunkProgress = Box<dyn FnMut(usize, usize) -> bool + Send>;

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
        if value.trim().is_empty() {
            fallback.to_string()
        } else {
            value.trim().to_string()
        }
    };
    doubao::DoubaoConfig {
        api_key,
        resource_id: non_empty(&settings.doubao_asr_resource_id, sauc::DEFAULT_RESOURCE_ID),
        endpoint: non_empty(&settings.doubao_asr_endpoint, sauc::DEFAULT_ENDPOINT),
        end_window_size: settings.doubao_asr_end_window_size,
        hotwords: settings.asr_hotwords.clone(),
    }
}

/// 从用户设置组装豆包网页语音配置。无凭据；超时按设置夹到 10–300 秒。
pub fn doubao_web_config(core: &CoreState) -> doubao_web::DoubaoWebConfig {
    doubao_web::DoubaoWebConfig::from_seconds(core.settings().doubao_web_timeout_seconds)
}

/// 从用户设置组装豆包输入法语音配置。无用户凭据；设备凭据缓存到 data 目录。
pub fn doubao_ime_config(core: &CoreState) -> doubao_ime::DoubaoImeConfig {
    doubao_ime::DoubaoImeConfig::from_seconds(core.settings().doubao_ime_timeout_seconds)
        .with_credential_path(core.data_dir.join("doubao_ime_credentials.json"))
        .with_crypto(core.crypto.clone())
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

/// 从用户设置组装 DeepSeek 语音识别配置（API Key 只存密文，运行期解密）。
pub fn deepseek_config(core: &CoreState) -> deepseek::DeepSeekConfig {
    let settings = core.settings();
    let api_key = settings
        .deepseek_asr_api_key_encrypted
        .as_deref()
        .filter(|token| !token.is_empty())
        .and_then(|token| core.crypto.decrypt(token))
        .map(|key| deepseek::strip_bearer_prefix(&key))
        .unwrap_or_default();
    let endpoint = settings.deepseek_asr_endpoint.trim();
    deepseek::DeepSeekConfig {
        api_key,
        endpoint: if endpoint.is_empty() { deepseek::DEFAULT_ENDPOINT.into() } else { endpoint.to_string() },
        timeout: std::time::Duration::from_secs(deepseek::request_timeout_seconds(
            settings.deepseek_asr_timeout_seconds,
        )),
    }
}

/// 从用户设置组装微信输入法语音配置（仅 Windows 编译）。
#[cfg(windows)]
pub fn wetype_config(core: &CoreState) -> wetype::WetypeConfig {
    let settings = core.settings();
    wetype::WetypeConfig {
        pid: u32::try_from(settings.wetype_asr_pid).ok().filter(|pid| *pid > 0),
        timeout: std::time::Duration::from_secs(settings.wetype_asr_timeout_seconds.clamp(10, 600) as u64),
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

/// 整段批量识别的结果：正文 + 可选降级原因。
///
/// DeepSeek 长录音按 115 秒切块；部分分段失败或尾块不足 1 秒被跳过时，
/// 仍返回已经识别出的正文，同时把缺段原因带给录音链路，避免界面把降级当完整成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTranscript {
    pub text: String,
    pub warning: Option<String>,
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
    DoubaoWeb(doubao_web::DoubaoWebConfig),
    DoubaoIme(doubao_ime::DoubaoImeConfig),
    ChatGpt(chatgpt::ChatGptConfig),
    DeepSeek(deepseek::DeepSeekConfig),
    Local(LocalHandle),
    #[cfg(windows)]
    WeType(wetype::WetypeConfig),
}

/// 全部识别引擎的 provider 标识（与设置 `asr_provider` 取值一致），也是「重新识别」可选引擎的顺序。
pub const PROVIDERS: [&str; 8] = [
    "doubao_asr",
    "doubao_web_asr",
    "doubao_ime_asr",
    "chatgpt_asr",
    "deepseek_asr",
    "wetype_asr",
    local::WHISPER,
    local::QWEN,
];

/// 一个识别引擎当前能不能用（「重新识别」选引擎用）：只看凭据是否齐备、本地模型文件是否就位，不发网络请求。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineChoice {
    pub provider: &'static str,
    pub label: &'static str,
    pub available: bool,
    /// 不可用时的一句结论
    pub error: Option<String>,
}

/// 列出全部识别引擎及其可用性（顺序同 [`PROVIDERS`]）。
pub fn engine_choices(core: &CoreState) -> Vec<EngineChoice> {
    PROVIDERS
        .iter()
        .map(|&provider| {
            let (label, error) = match LocalKind::from_provider(provider) {
                Some(kind) => {
                    let status = core.local_models.status(core, kind);
                    let error =
                        (!status.available).then(|| status.error.unwrap_or_else(|| format!("{}不可用", kind.label())));
                    (kind.label(), error)
                }
                // 凭据验证「确定不可用」（Token 无效等）同样不列为可用；没验过按凭据齐备处理
                None => match Engine::online_for_provider(core, provider) {
                    Ok(engine) => {
                        let invalid = if provider == "wetype_asr" {
                            None
                        } else {
                            verify::invalid_reason(core, provider)
                                .map(|reason| verify::invalid_message(provider, engine.label(), &reason))
                        };
                        (engine.label(), invalid)
                    }
                    Err(message) => (online_label(provider), Some(message)),
                },
            };
            EngineChoice { provider, label, available: error.is_none(), error }
        })
        .collect()
}

/// 这次识别实际用的语言（写进记录）：DeepSeek / Qwen3-ASR 不能指定语言、由模型自行判定，记为 `auto`；
/// 其余引擎就是设置里的识别语言。
pub fn effective_language(provider: &str, language: &str) -> String {
    match provider.trim() {
        "deepseek_asr" | "wetype_asr" | "qwen_asr" | "doubao_web_asr" | "doubao_ime_asr" => "auto".into(),
        _ => language.to_string(),
    }
}

/// 在线引擎的界面名称（凭据缺失、构造不出 [`Engine`] 时也要能显示）。
fn online_label(provider: &str) -> &'static str {
    match provider {
        "doubao_web_asr" => "豆包网页语音",
        "doubao_ime_asr" => "豆包输入法语音",
        "chatgpt_asr" => "ChatGPT 听写",
        "deepseek_asr" => "DeepSeek 语音识别",
        "wetype_asr" => "微信输入法语音",
        _ => "豆包语音",
    }
}

impl Engine {
    /// 只解析在线引擎（本地引擎要加载模型，见 [`Engine::resolve`]）。
    pub fn online_from_settings(core: &CoreState) -> Result<Self, String> {
        Self::online_for_provider(core, &core.settings().asr_provider)
    }

    /// 按指定 provider 解析在线引擎（凭据仍取自用户设置）。
    pub fn online_for_provider(core: &CoreState, provider: &str) -> Result<Self, String> {
        match provider.trim() {
            "chatgpt_asr" => {
                let config = chatgpt_config(core);
                config.ensure()?;
                Ok(Engine::ChatGpt(config))
            }
            "deepseek_asr" => {
                let config = deepseek_config(core);
                config.ensure()?;
                Ok(Engine::DeepSeek(config))
            }
            "doubao_web_asr" => {
                let config = doubao_web_config(core);
                config.ensure().map_err(|error| error.to_string())?;
                Ok(Engine::DoubaoWeb(config))
            }
            "doubao_ime_asr" => {
                let config = doubao_ime_config(core);
                config.ensure().map_err(|error| error.to_string())?;
                Ok(Engine::DoubaoIme(config))
            }
            "wetype_asr" => {
                #[cfg(windows)]
                {
                    if core.platform != crate::Platform::Windows {
                        return Err("微信输入法语音仅支持 Windows".into());
                    }
                    let config = wetype_config(core);
                    config.ensure()?;
                    Ok(Engine::WeType(config))
                }
                #[cfg(not(windows))]
                {
                    Err("微信输入法语音仅支持 Windows".into())
                }
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
        Self::resolve_provider(core, &core.settings().asr_provider)
    }

    /// 按指定 provider 解析引擎（重新识别可以换引擎）；本地引擎同样只校验模型文件并后台预加载。
    pub fn resolve_provider(core: &Arc<CoreState>, provider: &str) -> Result<Self, String> {
        match LocalKind::from_provider(provider) {
            Some(kind) => {
                let status = core.local_models.status(core, kind);
                if !status.available {
                    return Err(status.error.unwrap_or_else(|| format!("{}不可用", kind.label())));
                }
                preload_local(core.clone(), kind);
                Ok(Engine::Local(LocalHandle { core: core.clone(), kind }))
            }
            None => Self::online_for_provider(core, provider),
        }
    }

    /// provider 标识（写进记录的 `engine` 字段，与设置 `asr_provider` 取值一致）。
    pub fn provider(&self) -> &'static str {
        match self {
            Engine::Doubao(_) => "doubao_asr",
            Engine::DoubaoWeb(_) => "doubao_web_asr",
            Engine::DoubaoIme(_) => "doubao_ime_asr",
            Engine::ChatGpt(_) => "chatgpt_asr",
            Engine::DeepSeek(_) => "deepseek_asr",
            Engine::Local(handle) => handle.kind.provider(),
            #[cfg(windows)]
            Engine::WeType(_) => "wetype_asr",
        }
    }

    pub fn supports_streaming(&self) -> bool {
        match self {
            Engine::Doubao(_) => true,
            Engine::DoubaoIme(_) => true,
            #[cfg(windows)]
            Engine::WeType(_) => true,
            _ => false,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Engine::Doubao(_) => "豆包语音",
            Engine::DoubaoWeb(_) => "豆包网页语音",
            Engine::DoubaoIme(_) => doubao_ime::LABEL,
            Engine::ChatGpt(_) => "ChatGPT 听写",
            Engine::DeepSeek(_) => "DeepSeek 语音识别",
            Engine::Local(handle) => handle.kind.label(),
            #[cfg(windows)]
            Engine::WeType(_) => wetype::LABEL,
        }
    }

    /// 整段批量识别（单段 / 多段的每一段）。
    ///
    /// 返回正文与非致命的降级原因；调用方应保留正文，并把 `warning` 当成
    /// 「识别中断，已保存已识别内容」展示与落库，而不是判成整体失败。
    pub async fn transcribe_pcm(&self, language: &str, samples: &[i16]) -> Result<BatchTranscript, String> {
        match self {
            Engine::Doubao(config) => doubao::transcribe_pcm(config, language, "", samples)
                .await
                .map(|outcome| BatchTranscript { text: outcome.text, warning: None })
                .map_err(|error| error.to_string()),
            Engine::DoubaoWeb(config) => {
                doubao_web::transcribe_pcm(config, samples).await.map(|text| BatchTranscript { text, warning: None })
            }
            Engine::DoubaoIme(config) => {
                doubao_ime::transcribe_pcm(config, samples).await.map(|text| BatchTranscript { text, warning: None })
            }
            Engine::ChatGpt(config) => {
                let wav = crate::audio::wav_bytes_16k(samples).map_err(|e| format!("音频编码失败：{e}"))?;
                chatgpt::transcribe_wav(config, wav, language).await.map(|text| BatchTranscript { text, warning: None })
            }
            Engine::DeepSeek(config) => deepseek::transcribe_pcm_detailed(config, samples).await.map(|transcript| {
                let warning = (!transcript.warnings.is_empty()).then(|| transcript.warnings.join("；"));
                BatchTranscript { text: transcript.text, warning }
            }),
            #[cfg(windows)]
            Engine::WeType(config) => {
                wetype::transcribe_pcm(config, samples).await.map(|text| BatchTranscript { text, warning: None })
            }
            Engine::Local(handle) => {
                transcribe_local(handle.core.clone(), handle.kind, language.to_string(), samples.to_vec())
                    .await
                    .map(|(_, outcome)| BatchTranscript { text: outcome.text, warning: None })
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
    transcribe_local_with_progress(core, kind, language, samples, Box::new(|_, _| true)).await
}

/// 同 [`transcribe_local`]，按分块回调进度、可在块与块之间取消（文件转写用）。
///
/// 取消只在块之间生效：返回的 future 要等 blocking 线程真正退出才结束，调用方据此释放引擎占用。
pub async fn transcribe_local_with_progress(
    core: Arc<CoreState>,
    kind: LocalKind,
    language: String,
    samples: Vec<i16>,
    mut on_chunk: ChunkProgress,
) -> Result<(String, local::LocalTranscript), String> {
    tokio::task::spawn_blocking(move || {
        let engine = core.local_models.get(&core, kind)?;
        let model_label = engine.model_label();
        let transcript = engine.transcribe_with_progress(&language, &samples, &mut *on_chunk)?;
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
        let _model_gate = core.model_gate.blocking_read();
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
        let _model_gate = core.model_gate.blocking_read();
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
    fn 识别语言_不能指定语言的引擎记为自动检测() {
        assert_eq!(effective_language("deepseek_asr", "zh"), "auto");
        assert_eq!(effective_language("qwen_asr", "en"), "auto");
        assert_eq!(effective_language("doubao_web_asr", "zh"), "auto");
        assert_eq!(effective_language("wetype_asr", "zh"), "auto");
        assert_eq!(effective_language("doubao_asr", "yue"), "yue");
        assert_eq!(effective_language("whisper", "auto"), "auto");
    }

    #[test]
    fn local_kind_识别本地_provider() {
        assert_eq!(local_kind("whisper"), Some(LocalKind::Whisper));
        assert_eq!(local_kind("qwen_asr"), Some(LocalKind::Qwen));
        assert_eq!(local_kind("doubao_asr"), None);
        assert_eq!(local_kind("doubao_web_asr"), None);
        assert_eq!(local_kind("wetype_asr"), None);
        assert_eq!(local_kind("chatgpt_asr"), None);
        assert_eq!(local_kind("deepseek_asr"), None);
    }

    #[test]
    fn engine_choices_包含无需凭据的豆包网页语音() {
        let dir = std::env::temp_dir().join(format!("att-engine-choices-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, crate::Platform::Windows).unwrap();
        let choices = engine_choices(&core);
        let web = choices.iter().find(|choice| choice.provider == "doubao_web_asr").unwrap();
        assert_eq!(web.label, "豆包网页语音");
        assert!(web.available, "无需凭据，选项本身应可用：{web:?}");
        assert!(web.error.is_none());
        let engine = Engine::online_for_provider(&core, "doubao_web_asr").unwrap();
        assert_eq!(engine.provider(), "doubao_web_asr");
        assert_eq!(engine.label(), "豆包网页语音");
        assert!(!engine.supports_streaming());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn deepseek_分段降级_engine_保留正文并返回_warning() {
        let (base, _requests) = crate::llm::testing::stub_routes(vec![crate::llm::testing::StubRoute::json(
            "POST /v1/tools/asr",
            200,
            r#"{"text":"片段"}"#,
        )])
        .await;
        // 115 秒 + 0.1 秒：尾块按规则跳过，正文仍应返回，同时带出可见的降级原因
        let samples = vec![0i16; deepseek::MAX_CHUNK_SAMPLES + 1_600];
        let engine = Engine::DeepSeek(deepseek::DeepSeekConfig {
            api_key: "sk-test-123456".into(),
            endpoint: format!("{base}/v1/tools/asr"),
            timeout: std::time::Duration::from_secs(5),
        });

        let transcript = engine.transcribe_pcm("zh", &samples).await.expect("桩识别失败");
        assert_eq!(transcript.text, "片段");
        let warning = transcript.warning.expect("尾块跳过必须带降级原因");
        assert!(warning.contains("0.1 秒"), "{warning}");
    }

    #[cfg(not(feature = "local-engines"))]
    #[test]
    fn 关闭本地引擎时_resolve_给出明确结论() {
        let dir = std::env::temp_dir().join(format!("att-no-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = Arc::new(CoreState::init(&dir, crate::Platform::current()).unwrap());
        core.apply_settings_update(&serde_json::json!({ "asr_provider": "whisper" })).unwrap();
        let error = Engine::resolve(&core).unwrap_err();
        assert_eq!(error, local::LOCAL_ENGINES_DISABLED);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
