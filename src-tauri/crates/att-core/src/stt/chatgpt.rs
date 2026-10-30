//! ChatGPT 听写（移植自 Python `chatgpt_asr_provider.py`，原实现参考 Wangnov/codex-asr，MIT）。
//!
//! - 端点 `POST https://chatgpt.com/backend-api/transcribe`，鉴权是 **ChatGPT 账号 access token**
//!   （网页登录态），不是 platform.openai.com 的 API Key；
//! - multipart 只带 `file` 与可选 `language`；鉴权头四件套缺一会被边缘风控拦成 403 HTML；
//! - 响应 `{"text": …}`，text 为空是合法结果（没有语音）；非流式、无时间戳；
//! - token 等同于账号登录态：只活在内存里，绝不进日志与错误文案。

use base64::Engine as _;
use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = "https://chatgpt.com/backend-api/transcribe";
const ORIGINATOR: &str = "Codex Desktop";
const DESKTOP_VERSION: &str = "26.429.30905";

#[derive(Clone)]
pub struct ChatGptConfig {
    pub api_key: String,
    pub endpoint: String,
    pub timeout: Duration,
}

impl std::fmt::Debug for ChatGptConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGptConfig")
            .field("api_key_set", &!self.api_key.is_empty())
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

/// 容忍用户连着 `Bearer ` 前缀一起粘贴。
pub fn strip_bearer_prefix(token: &str) -> String {
    let value = token.trim();
    match value.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => value[7..].trim().to_string(),
        _ => value.to_string(),
    }
}

/// 从 JWT payload 提取 chatgpt_account_id；失败返回空串（此时不带该头）。
pub fn account_id_from_access_token(token: &str) -> String {
    let Some(payload) = token.split('.').nth(1) else { return String::new() };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')) else {
        return String::new();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|claims| claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl ChatGptConfig {
    pub fn ensure(&self) -> Result<(), String> {
        if self.api_key.trim().is_empty() {
            return Err("未配置 ChatGPT 访问令牌，请在「设置 → 语音识别」中填写后重试".into());
        }
        if self.endpoint.trim().is_empty() {
            return Err("未配置 ChatGPT 听写端点地址".into());
        }
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) {
            return Err("ChatGPT 听写端点必须以 http:// 或 https:// 开头".into());
        }
        Ok(())
    }

    fn user_agent() -> String {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        format!("{ORIGINATOR}/{DESKTOP_VERSION} ({os}; {})", std::env::consts::ARCH)
    }
}

/// 上游 HTTP 状态 → 可执行的中文提示（不回显完整 body）。
fn classify_status(status: u16, body: &str) -> String {
    let is_html = body.trim_start().starts_with('<');
    match status {
        401 => "ChatGPT 访问令牌无效或已过期（HTTP 401），请重新获取令牌后更新设置".into(),
        403 if is_html => "请求被 ChatGPT 风控拦截（HTTP 403），请稍后重试；频繁出现请更换网络环境或重新登录获取新令牌".into(),
        403 => "ChatGPT 拒绝了该请求（HTTP 403），令牌可能缺少听写权限，请重新获取".into(),
        404 => "听写端点不存在（HTTP 404），请检查端点地址设置".into(),
        413 => "音频文件过大，被听写接口拒绝（HTTP 413）".into(),
        429 => "听写请求被限流（HTTP 429），请稍后重试".into(),
        500..=599 => format!("听写服务故障（HTTP {status}），请稍后重试"),
        _ => {
            let detail: String = body.trim().chars().take(200).collect();
            if detail.is_empty() { format!("听写请求失败（HTTP {status}）") } else { format!("听写请求失败（HTTP {status}）：{detail}") }
        }
    }
}

/// 上传一段 16k 单声道 WAV 并返回识别文本。
pub async fn transcribe_wav(config: &ChatGptConfig, wav: Vec<u8>, language: &str) -> Result<String, String> {
    config.ensure()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(config.timeout)
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败：{e}"))?;
    let file = reqwest::multipart::Part::bytes(wav).file_name("recording.wav").mime_str("audio/wav").map_err(|e| e.to_string())?;
    let mut form = reqwest::multipart::Form::new().part("file", file);
    let language = language.trim();
    if !language.is_empty() && !language.eq_ignore_ascii_case("auto") {
        form = form.text("language", language.to_string());
    }
    let mut request = client
        .post(config.endpoint.trim())
        .bearer_auth(&config.api_key)
        .header("originator", ORIGINATOR)
        .header("User-Agent", ChatGptConfig::user_agent())
        .multipart(form);
    let account_id = account_id_from_access_token(&config.api_key);
    if !account_id.is_empty() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    tracing::info!(endpoint = %config.endpoint, "调用 ChatGPT 听写接口");
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "听写请求超时，请稍后重试".to_string()
        } else if error.is_connect() {
            format!("无法连接听写服务（{}），请检查网络或代理设置", config.endpoint)
        } else {
            "听写请求失败，请稍后重试".to_string()
        }
    })?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    if status != 200 {
        return Err(classify_status(status, &body));
    }
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| "听写接口返回了无法解析的响应（非 JSON）".to_string())?;
    Ok(value.get("text").and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::testing::{stub_routes, StubRoute};

    #[test]
    fn 令牌前缀与账号提取() {
        assert_eq!(strip_bearer_prefix("  Bearer abc "), "abc");
        assert_eq!(strip_bearer_prefix("abc"), "abc");
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"}}"#);
        assert_eq!(account_id_from_access_token(&format!("h.{claims}.s")), "acct-1");
        assert_eq!(account_id_from_access_token("not-a-jwt"), "");
    }

    #[tokio::test]
    async fn 上传_四件套鉴权头与状态归类() {
        let (base, mut requests) = stub_routes(vec![StubRoute::json("POST /transcribe", 200, r#"{"text":" 你好 "}"#)]).await;
        let config = ChatGptConfig { api_key: "tok-123456".into(), endpoint: format!("{base}/transcribe"), timeout: Duration::from_secs(5) };
        assert_eq!(transcribe_wav(&config, b"RIFF".to_vec(), "zh").await.unwrap(), "你好");
        let request = requests.recv().await.unwrap().to_lowercase();
        assert!(request.contains("authorization: bearer tok-123456"));
        assert!(request.contains("originator: codex desktop"));
        assert!(request.contains("user-agent: codex desktop/"));
        assert!(request.contains("name=\"language\""));
        assert!(classify_status(403, "<html>").contains("风控"));
        assert!(classify_status(401, "").contains("过期"));
    }
}
