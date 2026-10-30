//! 语音识别引擎（阶段 3d：豆包 SAUC 在线流式 + 批量）。
//!
//! - [`sauc`]：火山 SAUC 二进制协议编解码（纯函数）
//! - [`doubao`]：豆包会话（流式推流 / 批量转写）
//!
//! 其余引擎（whisper.cpp / Qwen 桥接 / ChatGPT 听写）尚未移植，见 [`unsupported_provider_message`]。

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

/// 尚未移植到 Rust 的引擎：给出可读提示（录音入口据此拒绝开始）。
pub fn unsupported_provider_message(provider: &str) -> Option<String> {
    match provider {
        "doubao_asr" => None,
        "whisper" => Some("Tauri 版暂未接入本地 Whisper 引擎，请在设置中切换到「豆包语音」".into()),
        "qwen_asr" => Some("Tauri 版暂未接入 Qwen3-ASR 引擎，请在设置中切换到「豆包语音」".into()),
        "chatgpt_asr" => Some("Tauri 版暂未接入 ChatGPT 听写引擎，请在设置中切换到「豆包语音」".into()),
        other => Some(format!("未知的 ASR 引擎：{other}")),
    }
}
