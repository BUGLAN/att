//! DeepSeek 语音识别（官方 `POST /v1/tools/asr`，在线批量，不支持流式）。
//!
//! 官方约束（api-docs.deepseek.com/zh-cn/guides/asr，2026-11 核对）：
//! - multipart `file`，16 位 PCM WAV 或 Ogg Opus，按文件内容判断；
//! - WAV 采样率 8000～48000 Hz，单/双声道（双声道只识别左声道）；
//! - 单次最大 8 MiB、上传需在 10 分钟内完成、音频最长 120 秒；
//! - 没有语种参数，语种由音频自动识别；响应 `{request_id, text, duration_ms}`；
//! - 鉴权 `Authorization: Bearer <DeepSeek API Key>`。
//!
//! 录音超过 120 秒时按 [`MAX_CHUNK_SAMPLES`] 切块顺序识别，再按行拼接，避免整段被 413 拒绝。

use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = "https://api.deepseek.com/v1/tools/asr";

/// 接口上限 120 秒；留 5 秒余量，同时 115 秒的 16k/16bit 单声道 WAV 约 3.7 MiB，远小于 8 MiB。
pub const MAX_CHUNK_SECONDS: usize = 115;
pub const SAMPLE_RATE: usize = 16_000;
pub const MAX_CHUNK_SAMPLES: usize = MAX_CHUNK_SECONDS * SAMPLE_RATE;

#[derive(Clone)]
pub struct DeepSeekConfig {
    pub api_key: String,
    pub endpoint: String,
    pub timeout: Duration,
}

impl std::fmt::Debug for DeepSeekConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepSeekConfig")
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

impl DeepSeekConfig {
    pub fn ensure(&self) -> Result<(), String> {
        if self.api_key.trim().is_empty() {
            return Err("未配置 DeepSeek API Key，请在「设置 → 识别引擎」中填写".into());
        }
        if self.endpoint.trim().is_empty() {
            return Err("未配置 DeepSeek 语音识别端点地址".into());
        }
        if !(self.endpoint.starts_with("http://") || self.endpoint.starts_with("https://")) {
            return Err("DeepSeek 语音识别端点必须以 http:// 或 https:// 开头".into());
        }
        Ok(())
    }
}

/// 上游 HTTP 状态 → 可执行的中文提示。任何状态都不回显响应体：
/// 上游或用户代理可能把 `Authorization` 原样写进错误体，响应体会进入记录与前端。
fn classify_status(status: u16) -> String {
    match status {
        400 => "DeepSeek 拒绝了音频格式（HTTP 400）：需要 16 位 PCM WAV 或 Ogg Opus".into(),
        401 => "DeepSeek API Key 无效或已过期（HTTP 401），请重新填写".into(),
        402 => "DeepSeek 账户余额不足（HTTP 402），请充值后重试".into(),
        403 => "DeepSeek 拒绝了该请求（HTTP 403），请确认 API Key 有语音识别权限".into(),
        404 => "DeepSeek 语音识别端点不存在（HTTP 404），请检查端点地址设置".into(),
        408 => "音频上传超时（HTTP 408），请检查网络后重试".into(),
        413 => "音频超过 8 MiB 上限（HTTP 413），请缩短录音".into(),
        422 => "DeepSeek 无法处理该请求（HTTP 422），请检查音频格式与请求参数".into(),
        429 => "DeepSeek 并发请求数超过上限（HTTP 429），请稍后重试".into(),
        503 => "DeepSeek 服务繁忙（HTTP 503），请稍后重试".into(),
        504 => "DeepSeek 识别超时（HTTP 504），请缩短音频或稍后重试".into(),
        500..=599 => format!("DeepSeek 服务故障（HTTP {status}），请稍后重试"),
        _ => format!("DeepSeek 语音识别失败（HTTP {status}），请稍后重试"),
    }
}

/// 识别失败：上游 HTTP 状态（没走到 HTTP 层时为 `None`）+ 可执行的中文提示。
#[derive(Debug, Clone, PartialEq)]
pub struct DeepSeekError {
    pub status: Option<u16>,
    pub message: String,
}

impl DeepSeekError {
    fn local(message: impl Into<String>) -> Self {
        Self { status: None, message: message.into() }
    }
}

/// 上传一段 16k 单声道 WAV 并返回识别文本。
pub async fn transcribe_wav(config: &DeepSeekConfig, wav: Vec<u8>) -> Result<String, String> {
    transcribe_wav_detailed(config, wav).await.map_err(|error| error.message)
}

/// 同 [`transcribe_wav`]，失败时保留 HTTP 状态（凭据验证据此区分「Key 无效」与「暂时失败」）。
pub async fn transcribe_wav_detailed(config: &DeepSeekConfig, wav: Vec<u8>) -> Result<String, DeepSeekError> {
    config.ensure().map_err(DeepSeekError::local)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(config.timeout)
        .build()
        .map_err(|e| DeepSeekError::local(format!("HTTP 客户端初始化失败：{e}")))?;
    let file = reqwest::multipart::Part::bytes(wav)
        .file_name("recording.wav")
        .mime_str("audio/wav")
        .map_err(|e| DeepSeekError::local(e.to_string()))?;
    let form = reqwest::multipart::Form::new().part("file", file);
    tracing::info!(endpoint = %config.endpoint, "调用 DeepSeek 语音识别接口");
    let response = client
        .post(config.endpoint.trim())
        .bearer_auth(&config.api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| {
            DeepSeekError::local(if error.is_timeout() {
                "DeepSeek 识别请求超时，请稍后重试".to_string()
            } else if error.is_connect() {
                format!("无法连接 DeepSeek 语音识别服务（{}），请检查网络或代理设置", config.endpoint)
            } else {
                "DeepSeek 识别请求失败，请稍后重试".to_string()
            })
        })?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    if status != 200 {
        return Err(DeepSeekError { status: Some(status), message: classify_status(status) });
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| DeepSeekError { status: Some(status), message: "DeepSeek 返回了无法解析的响应（非 JSON）".into() })?;
    Ok(value.get("text").and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string())
}

/// 录音整段 PCM：超过 120 秒自动切块，顺序识别后按行拼接。
pub async fn transcribe_pcm(config: &DeepSeekConfig, samples: &[i16]) -> Result<String, String> {
    let mut texts = Vec::new();
    for chunk in samples.chunks(MAX_CHUNK_SAMPLES) {
        let wav = crate::audio::wav_bytes_16k(chunk).map_err(|e| format!("音频编码失败：{e}"))?;
        let text = transcribe_wav(config, wav).await?;
        if !text.is_empty() {
            texts.push(text);
        }
    }
    Ok(texts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::testing::{stub_routes, StubRoute};

    #[test]
    fn 切块上限不越过接口的_120_秒() {
        assert_eq!(MAX_CHUNK_SECONDS, 115);
        assert!(MAX_CHUNK_SAMPLES / SAMPLE_RATE < 120);
        assert!(MAX_CHUNK_SAMPLES * 2 < 8 * 1024 * 1024);
    }

    #[test]
    fn 令牌前缀可带_bearer() {
        assert_eq!(strip_bearer_prefix("  Bearer sk-a "), "sk-a");
        assert_eq!(strip_bearer_prefix("sk-b"), "sk-b");
    }

    #[tokio::test]
    async fn 上传_鉴权头与文本解析() {
        let (base, mut requests) =
            stub_routes(vec![StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":" 你好 ","duration_ms":3200}"#)]).await;
        let config = DeepSeekConfig {
            api_key: "sk-test-123456".into(),
            endpoint: format!("{base}/v1/tools/asr"),
            timeout: Duration::from_secs(5),
        };
        assert_eq!(transcribe_wav(&config, b"RIFF".to_vec()).await.unwrap(), "你好");
        let request = requests.recv().await.unwrap().to_lowercase();
        assert!(request.contains("authorization: bearer sk-test-123456"));
        assert!(request.contains("name=\"file\""));
        assert!(request.contains("filename=\"recording.wav\""));
    }

    #[tokio::test]
    async fn 状态归类_错误文案不回显密钥() {
        for (status, expect) in
            [(401, "无效"), (402, "余额"), (403, "权限"), (413, "8 MiB"), (422, "无法处理"), (429, "并发"), (503, "繁忙"), (504, "超时")]
        {
            let (base, _) = stub_routes(vec![StubRoute::json("POST /v1/tools/asr", status, r#"{"error":"x"}"#)]).await;
            let config = DeepSeekConfig {
                api_key: "sk-secret-987654".into(),
                endpoint: format!("{base}/v1/tools/asr"),
                timeout: Duration::from_secs(5),
            };
            let error = transcribe_wav_detailed(&config, b"RIFF".to_vec()).await.unwrap_err();
            assert_eq!(error.status, Some(status));
            assert!(error.message.contains(expect), "{status}: {}", error.message);
            assert!(!error.message.contains("sk-secret-987654"));
        }
    }

    #[tokio::test]
    async fn 未知状态与_422_不透传响应体里的密钥() {
        let secret = "sk-leak-4f9a2b7c";
        let body = format!(
            r#"{{"error":"bad audio","detail":"Authorization: Bearer {secret}","echo":"{secret}"}}"#
        );
        // 422 是已知状态，418 走未知状态兜底：两条路径都不能把响应体拼进文案。
        for status in [422, 418] {
            let (base, _) = stub_routes(vec![StubRoute::json("POST /v1/tools/asr", status, body.clone())]).await;
            let config = DeepSeekConfig {
                api_key: secret.into(),
                endpoint: format!("{base}/v1/tools/asr"),
                timeout: Duration::from_secs(5),
            };
            let error = transcribe_wav_detailed(&config, b"RIFF".to_vec()).await.unwrap_err();
            assert_eq!(error.status, Some(status));
            assert!(!error.message.contains(secret), "{status}: {}", error.message);
            assert!(!error.message.contains("Authorization"), "{status}: {}", error.message);
            assert!(!error.message.contains("bad audio"), "{status}: {}", error.message);
            // 记录与 HTTP 错误响应消费的是这条纯文案，同样不能带密钥。
            let message = transcribe_wav(&config, b"RIFF".to_vec()).await.unwrap_err();
            assert!(!message.contains(secret), "{status}: {message}");
        }
    }
}
