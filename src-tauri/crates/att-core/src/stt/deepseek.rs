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

use std::ops::Range;
use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = "https://api.deepseek.com/v1/tools/asr";

/// 接口上限 120 秒；留 5 秒余量，同时 115 秒的 16k/16bit 单声道 WAV 约 3.7 MiB，远小于 8 MiB。
pub const MAX_CHUNK_SECONDS: usize = 115;
pub const SAMPLE_RATE: usize = 16_000;
pub const MAX_CHUNK_SAMPLES: usize = MAX_CHUNK_SECONDS * SAMPLE_RATE;
/// 尾块不足 1 秒时直接跳过：单独发一次请求既容易触发上游格式校验，也会放大最长录音的切块数。
pub const MIN_TAIL_SAMPLES: usize = SAMPLE_RATE;
/// 单个分段最多请求 3 次（首次 + 两次重试）；只重试暂时性错误。
pub const MAX_ATTEMPTS_PER_CHUNK: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(250);

/// 一段识别结果在整段音频中的位置（秒）。
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// 整段 PCM 的分段识别结果：成功分段的文本，以及降级时保留给调用方的警告。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PcmTranscript {
    pub text: String,
    pub segments: Vec<TranscriptSegment>,
    pub warnings: Vec<String>,
}

/// 按 16k 单声道样本数算需要几次串行上传，与 [`transcribe_pcm`] 的切块一致（空音频为 0 块）。
pub fn chunk_count(samples: usize) -> usize {
    chunk_ranges(samples).len()
}

/// 按录音时长估算切块数（收尾看门狗用）；不足一块也算一块。
pub fn chunk_count_for_duration(recorded: Duration) -> usize {
    let samples = (recorded.as_secs_f64() * SAMPLE_RATE as f64).ceil() as usize;
    chunk_count(samples).max(1)
}

/// 单块请求实际使用的超时：设置值限制在 10～600 秒（与 [`crate::stt::deepseek_config`] 共用）。
pub fn request_timeout_seconds(configured: i64) -> u64 {
    configured.clamp(10, 600) as u64
}

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

    /// 暂时性错误：网络异常、超时、限流与服务端故障。
    fn retryable(&self) -> bool {
        matches!(self.status, None | Some(200 | 408 | 429 | 500..=599))
    }

    /// 鉴权、权限与端点错误继续请求也不会变好，分段循环不再尝试后续分段。
    fn fatal(&self) -> bool {
        matches!(self.status, Some(401 | 403 | 404))
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
    let response =
        client.post(config.endpoint.trim()).bearer_auth(&config.api_key).multipart(form).send().await.map_err(
            |error| {
                DeepSeekError::local(if error.is_timeout() {
                    "DeepSeek 识别请求超时，请稍后重试".to_string()
                } else if error.is_connect() {
                    format!("无法连接 DeepSeek 语音识别服务（{}），请检查网络或代理设置", config.endpoint)
                } else {
                    "DeepSeek 识别请求失败，请稍后重试".to_string()
                })
            },
        )?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    if status != 200 {
        return Err(DeepSeekError { status: Some(status), message: classify_status(status) });
    }
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| DeepSeekError {
        status: Some(status),
        message: "DeepSeek 返回了无法解析的响应（非 JSON）".into(),
    })?;
    Ok(value.get("text").and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string())
}

#[derive(Debug, Clone, PartialEq)]
struct ChunkPlan {
    ranges: Vec<Range<usize>>,
    skipped_tail: usize,
}

/// 切块：每块不超过 `chunk_samples`；最后一个不足 1 秒的残块跳过，避免单独上传亚秒 WAV。
fn plan_chunks(samples_len: usize, chunk_samples: usize, min_tail_samples: usize) -> ChunkPlan {
    if samples_len == 0 || chunk_samples == 0 {
        return ChunkPlan { ranges: Vec::new(), skipped_tail: samples_len };
    }
    if samples_len <= chunk_samples {
        return ChunkPlan { ranges: std::iter::once(0..samples_len).collect(), skipped_tail: 0 };
    }
    let mut used = (samples_len / chunk_samples) * chunk_samples;
    let tail = samples_len - used;
    if tail >= min_tail_samples {
        used += tail;
    }
    let ranges = (0..used).step_by(chunk_samples).map(|start| start..(start + chunk_samples).min(used)).collect();
    ChunkPlan { ranges, skipped_tail: samples_len - used }
}

/// 录音整段 PCM 的切块范围：115.1 秒只发一块（丢掉 0.1 秒尾块），230.1 秒只发两块。
pub fn chunk_ranges(samples_len: usize) -> Vec<Range<usize>> {
    plan_chunks(samples_len, MAX_CHUNK_SAMPLES, MIN_TAIL_SAMPLES).ranges
}

/// 单个分段的重试：只重试暂时性错误，最多 `MAX_ATTEMPTS_PER_CHUNK` 次。
async fn transcribe_chunk_with_retry(
    config: &DeepSeekConfig,
    wav: &[u8],
    index: usize,
    total: usize,
) -> Result<String, DeepSeekError> {
    let mut delay = RETRY_BASE_DELAY;
    for attempt in 1..=MAX_ATTEMPTS_PER_CHUNK {
        match transcribe_wav_detailed(config, wav.to_vec()).await {
            Ok(text) => return Ok(text),
            Err(error) => {
                if attempt == MAX_ATTEMPTS_PER_CHUNK || !error.retryable() {
                    return Err(error);
                }
                tracing::warn!(
                    "DeepSeek 第 {index}/{total} 段第 {attempt} 次请求暂时失败，{}ms 后重试：{}",
                    delay.as_millis(),
                    error.message
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
            }
        }
    }
    unreachable!("重试循环至少返回一次结果")
}

/// 录音整段 PCM：超过 120 秒自动切块，顺序识别后按行拼接；单段失败保留其他段文本。
pub async fn transcribe_pcm(config: &DeepSeekConfig, samples: &[i16]) -> Result<String, String> {
    transcribe_pcm_detailed(config, samples).await.map(|transcript| transcript.text)
}

/// 同 [`transcribe_pcm`]，返回分段位置与降级警告，供文件转写把缺段信息带回响应。
pub async fn transcribe_pcm_detailed(config: &DeepSeekConfig, samples: &[i16]) -> Result<PcmTranscript, String> {
    transcribe_pcm_with_limits(config, samples, MAX_CHUNK_SAMPLES, MIN_TAIL_SAMPLES).await
}

async fn transcribe_pcm_with_limits(
    config: &DeepSeekConfig,
    samples: &[i16],
    chunk_samples: usize,
    min_tail_samples: usize,
) -> Result<PcmTranscript, String> {
    config.ensure()?;
    let plan = plan_chunks(samples.len(), chunk_samples, min_tail_samples);
    let total = plan.ranges.len();
    let mut transcript = PcmTranscript::default();
    if plan.skipped_tail > 0 {
        let warning = format!(
            "末尾 {:.1} 秒音频未上传（不足 {:.1} 秒，避免单独切块）",
            plan.skipped_tail as f64 / SAMPLE_RATE as f64,
            min_tail_samples as f64 / SAMPLE_RATE as f64
        );
        tracing::warn!("{warning}");
        transcript.warnings.push(warning);
    }

    let mut succeeded = 0usize;
    let mut failures = Vec::new();
    for (index, range) in plan.ranges.into_iter().enumerate() {
        let wav = match crate::audio::wav_bytes_16k(&samples[range.clone()]) {
            Ok(wav) => wav,
            Err(error) => return Err(format!("音频编码失败：{error}")),
        };
        match transcribe_chunk_with_retry(config, &wav, index + 1, total).await {
            Ok(text) => {
                succeeded += 1;
                if !text.is_empty() {
                    transcript.segments.push(TranscriptSegment {
                        start: range.start as f64 / SAMPLE_RATE as f64,
                        end: range.end as f64 / SAMPLE_RATE as f64,
                        text,
                    });
                }
            }
            Err(error) => {
                let warning = format!("第 {}/{} 段识别失败：{}", index + 1, total, error.message);
                tracing::warn!("{warning}");
                transcript.warnings.push(warning.clone());
                failures.push(warning);
                let fatal = error.fatal();
                if fatal {
                    break;
                }
            }
        }
    }

    if succeeded == 0 && !failures.is_empty() {
        return Err(if failures.len() == 1 {
            failures.remove(0)
        } else {
            format!("DeepSeek 分段识别没有可用结果：{}", failures.join("；"))
        });
    }
    transcript.text = transcript.segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>().join("\n");
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::testing::{stub_routes, StubRoute};

    fn config(base: &str) -> DeepSeekConfig {
        DeepSeekConfig {
            api_key: "sk-test-123456".into(),
            endpoint: format!("{base}/v1/tools/asr"),
            timeout: Duration::from_secs(5),
        }
    }

    fn request_count(requests: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> usize {
        let mut count = 0;
        while requests.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    #[test]
    fn 切块上限不越过接口的_120_秒() {
        assert_eq!(MAX_CHUNK_SECONDS, 115);
        assert!(MAX_CHUNK_SAMPLES / SAMPLE_RATE < 120);
        assert!(MAX_CHUNK_SAMPLES * 2 < 8 * 1024 * 1024);
    }

    #[test]
    fn 切块数与串行上传次数一致() {
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(MAX_CHUNK_SAMPLES), 1);
        assert_eq!(chunk_count(MAX_CHUNK_SAMPLES + 1), 1, "亚秒尾块跳过，不再单独上传");

        assert_eq!(chunk_count_for_duration(Duration::from_secs(115)), 1);
        assert_eq!(chunk_count_for_duration(Duration::from_secs(116)), 2);
        assert_eq!(chunk_count_for_duration(Duration::from_secs(600)), 6);
        assert_eq!(chunk_count_for_duration(Duration::from_secs(1800)), 16);
        assert_eq!(chunk_count_for_duration(Duration::from_secs(7200)), 63);
        // 空录音也按一块算，避免收尾预算退化成 0
        assert_eq!(chunk_count_for_duration(Duration::ZERO), 1);

        for samples in [
            0,
            1,
            MAX_CHUNK_SAMPLES,
            MAX_CHUNK_SAMPLES + 1,
            MAX_CHUNK_SAMPLES + SAMPLE_RATE,
            MAX_CHUNK_SAMPLES * 2 + 1_600,
        ] {
            assert_eq!(chunk_count(samples), chunk_ranges(samples).len(), "{samples} 样本");
        }
    }

    #[test]
    fn 单块超时与配置范围一致() {
        assert_eq!(request_timeout_seconds(0), 10);
        assert_eq!(request_timeout_seconds(120), 120);
        assert_eq!(request_timeout_seconds(900), 600);
    }

    #[test]
    fn 切块计划_115点1和230点1秒都跳过亚秒尾块() {
        let one = plan_chunks(MAX_CHUNK_SAMPLES + 1_600, MAX_CHUNK_SAMPLES, MIN_TAIL_SAMPLES);
        assert_eq!(one.ranges, vec![0..MAX_CHUNK_SAMPLES]);
        assert_eq!(one.skipped_tail, 1_600);

        let two = plan_chunks(MAX_CHUNK_SAMPLES * 2 + 1_600, MAX_CHUNK_SAMPLES, MIN_TAIL_SAMPLES);
        assert_eq!(two.ranges.len(), 2);
        assert_eq!(two.ranges[0], 0..MAX_CHUNK_SAMPLES);
        assert_eq!(two.ranges[1], MAX_CHUNK_SAMPLES..MAX_CHUNK_SAMPLES * 2);
        assert_eq!(two.skipped_tail, 1_600);

        let one_second_tail = plan_chunks(MAX_CHUNK_SAMPLES + SAMPLE_RATE, MAX_CHUNK_SAMPLES, MIN_TAIL_SAMPLES);
        assert_eq!(one_second_tail.ranges.len(), 2, "满 1 秒的尾块仍然保留");
        assert_eq!(one_second_tail.skipped_tail, 0);
    }

    #[tokio::test]
    async fn 长录音_115点1和230点1秒实际上传次数为一和二() {
        for (samples, expected) in [(MAX_CHUNK_SAMPLES + 1_600, 1usize), (MAX_CHUNK_SAMPLES * 2 + 1_600, 2)] {
            let (base, mut requests) =
                stub_routes(vec![StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":"片段"}"#)]).await;
            let transcript = transcribe_pcm_detailed(&config(&base), &vec![0i16; samples]).await.unwrap();
            assert_eq!(transcript.segments.len(), expected);
            assert_eq!(transcript.text.lines().count(), expected);
            assert_eq!(transcript.warnings.len(), 1, "亚秒尾块应给出跳过警告");
            assert!(transcript.warnings[0].contains("0.1 秒"), "{:?}", transcript.warnings);
            assert_eq!(request_count(&mut requests), expected);
        }
    }

    #[test]
    fn 令牌前缀可带_bearer() {
        assert_eq!(strip_bearer_prefix("  Bearer sk-a "), "sk-a");
        assert_eq!(strip_bearer_prefix("sk-b"), "sk-b");
    }

    #[tokio::test]
    async fn 上传_鉴权头与文本解析() {
        let (base, mut requests) =
            stub_routes(vec![StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":" 你好 ","duration_ms":3200}"#)])
                .await;
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
    async fn 第二块_503_有界重试后保留第一块文本() {
        let mut samples = vec![0x4141i16; 4];
        samples.extend(vec![0x4242i16; 4]);
        let (base, mut requests) = stub_routes(vec![
            StubRoute::json("POST /v1/tools/asr", 503, r#"{"error":"busy"}"#).when_body("BBBBBBBB"),
            StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":"第一段"}"#),
        ])
        .await;

        let transcript = transcribe_pcm_with_limits(&config(&base), &samples, 4, 2).await.unwrap();
        assert_eq!(transcript.text, "第一段");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.warnings.len(), 1);
        assert!(transcript.warnings[0].contains("第 2/2 段"), "{:?}", transcript.warnings);
        assert_eq!(request_count(&mut requests), 4, "第一块 1 次 + 第二块首次与两次重试");
    }

    #[tokio::test]
    async fn 尾块_400_不丢弃第一块文本也不重试() {
        let mut samples = vec![0x4141i16; 4];
        samples.extend(vec![0x4242i16; 4]);
        let (base, mut requests) = stub_routes(vec![
            StubRoute::json("POST /v1/tools/asr", 400, r#"{"error":"bad"}"#).when_body("BBBBBBBB"),
            StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":"第一段"}"#),
        ])
        .await;

        let transcript = transcribe_pcm_with_limits(&config(&base), &samples, 4, 2).await.unwrap();
        assert_eq!(transcript.text, "第一段");
        assert_eq!(transcript.segments.len(), 1);
        assert!(transcript.warnings[0].contains("HTTP 400"), "{:?}", transcript.warnings);
        assert_eq!(request_count(&mut requests), 2, "400 是确定性失败，不重试");
    }

    #[tokio::test]
    async fn 单块_400_直接失败不重试() {
        let (base, mut requests) =
            stub_routes(vec![StubRoute::json("POST /v1/tools/asr", 400, r#"{"error":"bad"}"#)]).await;

        let error = transcribe_pcm_with_limits(&config(&base), &[0x4141; 4], 4, 2).await.unwrap_err();
        assert!(error.contains("HTTP 400"), "{error}");
        assert_eq!(request_count(&mut requests), 1);
    }

    #[tokio::test]
    async fn 状态归类_错误文案不回显密钥() {
        for (status, expect) in [
            (401, "无效"),
            (402, "余额"),
            (403, "权限"),
            (413, "8 MiB"),
            (422, "无法处理"),
            (429, "并发"),
            (503, "繁忙"),
            (504, "超时"),
        ] {
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
        let body = format!(r#"{{"error":"bad audio","detail":"Authorization: Bearer {secret}","echo":"{secret}"}}"#);
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
