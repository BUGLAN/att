//! 豆包网页匿名语音识别（移植自 `kuxiaowo/wechat-asr-bridge` 的 MIT 参考实现）。
//!
//! 链路与火山 SAUC 的 [`super::doubao`] 完全独立：
//! 1. 用全新 HTTP 客户端访问豆包首页，从 `window._ROUTER_DATA` 取得游客设备标识；
//! 2. 连接网页语音 WebSocket，按 `StartTask → StartSession → TaskRequest → EndASR → FinishSession`
//!    的固定顺序发送；
//! 3. 收集 `ASRResponse` 中 `is_interim == false` 的最终文本。
//!
//! 首页 Cookie、ttwid 与设备标识不持久化，也不转发给语音网关。这是非官方链路，
//! 网页结构或网关协议随时可能变化；错误文案只给出可执行结论，不回显上游响应体。

use super::sauc::ErrorKind;
use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use futures_util::SinkExt;
use reqwest::Url;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = SplitSink<WsStream, Message>;
type WsReader = SplitStream<WsStream>;

pub const HOMEPAGE: &str = "https://www.doubao.com/chat/";
pub const GATEWAY: &str = "wss://frontier-audio-web-ws.doubao.com/api/v2/sami/voicegenie";
/// 豆包网页前端公开的应用标识，不是账号凭据。
pub const APPKEY: &str = "GOqQpfo1fO7slHv8";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";
pub const DEFAULT_TIMEOUT_SECONDS: i64 = 30;
pub const MIN_TIMEOUT_SECONDS: i64 = 10;
pub const MAX_TIMEOUT_SECONDS: i64 = 300;

const ACCEPT_LANGUAGE: &str = "zh-CN,zh;q=0.9";
const MAX_HOMEPAGE_BYTES: u64 = 10 * 1024 * 1024;
pub const SAMPLE_RATE: usize = 16_000;
/// 参考实现一包 3200 字节，即 100ms 的 16k 单声道 PCM16。
const CHUNK_BYTES: usize = 3_200;
const CHUNK_SAMPLES: usize = CHUNK_BYTES / 2;
/// 参考实现限制单次音频不超过 25 MiB；这里按 13 分钟切会话，给协议开销留出余量。
pub const MAX_SESSION_SECONDS: usize = 780;
const MAX_SESSION_SAMPLES: usize = MAX_SESSION_SECONDS * SAMPLE_RATE;
const SUCCESS_STATUS: u64 = 20_000_000;

static START_SESSION_PAYLOAD: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "business": 1,
        "request_type": 3,
        "asr": {
            "audio_info": {
                "channel": 1,
                "format": "pcm",
                "sample_rate": 16_000,
            }
        }
    })
});

/// 豆包网页语音配置。没有 API Key；超时同时约束首页初始化、握手和每个控制事件。
#[derive(Clone)]
pub struct DoubaoWebConfig {
    pub timeout: Duration,
    pub(crate) gateway: String,
}

impl std::fmt::Debug for DoubaoWebConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoubaoWebConfig").field("timeout", &self.timeout).field("gateway", &self.gateway).finish()
    }
}

impl Default for DoubaoWebConfig {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS as u64), gateway: GATEWAY.into() }
    }
}

impl DoubaoWebConfig {
    pub fn from_seconds(seconds: i64) -> Self {
        Self {
            timeout: Duration::from_secs(seconds.clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS) as u64),
            ..Self::default()
        }
    }

    pub fn ensure(&self) -> Result<(), DoubaoWebError> {
        if self.timeout.is_zero() {
            return Err(DoubaoWebError::protocol("豆包网页语音超时时间无效，请在设置中改为 10–300 秒"));
        }
        if self.gateway.trim().is_empty() {
            return Err(DoubaoWebError::protocol("豆包网页语音地址配置无效"));
        }
        Ok(())
    }
}

/// 估算一次真实识别的总耗时上限：音频按实时速率推送，每个会话还要等首页、握手与四个控制阶段。
pub fn estimated_timeout(config: &DoubaoWebConfig, audio: Duration) -> Duration {
    let sessions = session_count_for_duration(audio);
    let per_session = config.timeout.saturating_mul(6).saturating_add(Duration::from_secs(2));
    audio.saturating_add(per_session.saturating_mul(u32::try_from(sessions).unwrap_or(u32::MAX)))
}

pub fn session_count_for_duration(audio: Duration) -> usize {
    let samples = (audio.as_secs_f64() * SAMPLE_RATE as f64).ceil() as usize;
    samples.max(1).div_ceil(MAX_SESSION_SAMPLES)
}

/// 带归一化分类的错误，供凭据验证区分「网页结构变化」与「网络暂时不可用」。
#[derive(Debug, Clone)]
pub struct DoubaoWebError {
    pub kind: ErrorKind,
    pub message: String,
}

impl DoubaoWebError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }

    fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    fn server(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Server, message)
    }

    fn input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Input, message)
    }
}

impl std::fmt::Display for DoubaoWebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DoubaoWebError {}

/// 首页取得的游客上下文；只在本次识别内存里存活。
#[derive(Clone, Debug)]
pub(crate) struct GuestContext {
    pairs: Vec<(&'static str, String)>,
}

impl GuestContext {
    fn with_device(device_id: String, version: String, aid: String) -> Self {
        Self {
            pairs: vec![
                ("api_app_key", APPKEY.into()),
                ("namespace", "VoiceGenie".into()),
                ("version_code", "20800".into()),
                ("language", "zh".into()),
                ("device_platform", "web".into()),
                ("pkg_type", "release_version".into()),
                ("pc_version", version.clone()),
                ("doubao_pc_version", version),
                ("region", "CN".into()),
                ("sys_region", "CN".into()),
                ("samantha_web", "1".into()),
                ("use-olympus-account", "1".into()),
                ("doubao_device_platform", "web".into()),
                ("aid", aid.clone()),
                ("real_aid", aid),
                ("device_id", device_id),
                ("web_platform", "browser".into()),
            ],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Envelope {
    task_id: Option<String>,
    event: Option<String>,
    status_code: Option<u64>,
    payload: Option<String>,
    data: Option<Vec<u8>>,
}

pub(crate) fn encode_request(
    event: &str,
    task_id: Option<&str>,
    payload: Option<&Value>,
    audio: Option<&[u8]>,
    sequence: Option<u64>,
) -> Vec<u8> {
    let mut packet = Vec::new();
    push_bytes_field(&mut packet, 2, APPKEY.as_bytes());
    push_bytes_field(&mut packet, 3, b"VoiceGenie");
    push_bytes_field(&mut packet, 5, event.as_bytes());
    let empty = json!({});
    let payload = payload.unwrap_or(&empty);
    push_bytes_field(&mut packet, 6, payload.to_string().as_bytes());
    if let Some(task_id) = task_id.filter(|value| !value.is_empty()) {
        push_bytes_field(&mut packet, 8, task_id.as_bytes());
    }
    if let Some(audio) = audio {
        push_bytes_field(&mut packet, 7, audio);
    }
    if let Some(sequence) = sequence {
        push_varint_field(&mut packet, 9, sequence);
    }
    packet
}

fn push_bytes_field(out: &mut Vec<u8>, number: u32, data: &[u8]) {
    push_varint(out, u64::from(number) << 3 | 2);
    push_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn push_varint_field(out: &mut Vec<u8>, number: u32, value: u64) {
    push_varint(out, u64::from(number) << 3);
    push_varint(out, value);
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value > 127 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(data: &[u8], offset: &mut usize) -> Result<u64, DoubaoWebError> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = *data.get(*offset).ok_or_else(|| DoubaoWebError::protocol("豆包网页语音响应被截断"))?;
        *offset += 1;
        if index == 9 && byte > 1 {
            return Err(DoubaoWebError::protocol("豆包网页语音响应中的整数超出范围"));
        }
        value |= u64::from(byte & 127) << (index * 7);
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(DoubaoWebError::protocol("豆包网页语音响应中的 varint 无效"))
}

fn decode_binary_response(data: &[u8]) -> Result<Envelope, DoubaoWebError> {
    let mut envelope = Envelope::default();
    let mut offset = 0usize;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        let number = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        if number == 0 {
            return Err(DoubaoWebError::protocol("豆包网页语音响应字段编号无效"));
        }
        let expected_wire = match number {
            5 | 9 => 0,
            1..=4 | 6..=8 | 10 | 11 => 2,
            _ => wire,
        };
        if number <= 11 && wire != expected_wire {
            return Err(DoubaoWebError::protocol("豆包网页语音响应字段类型不符"));
        }
        match wire {
            0 => {
                let value = read_varint(data, &mut offset)?;
                if number == 5 {
                    envelope.status_code = Some(value);
                }
            }
            1 => {
                let end = offset.checked_add(8).ok_or_else(|| DoubaoWebError::protocol("豆包网页语音响应被截断"))?;
                if end > data.len() {
                    return Err(DoubaoWebError::protocol("豆包网页语音响应被截断"));
                }
                offset = end;
            }
            2 => {
                let length = read_varint(data, &mut offset)?;
                let length =
                    usize::try_from(length).map_err(|_| DoubaoWebError::protocol("豆包网页语音响应字段过长"))?;
                let end =
                    offset.checked_add(length).ok_or_else(|| DoubaoWebError::protocol("豆包网页语音响应被截断"))?;
                if end > data.len() {
                    return Err(DoubaoWebError::protocol("豆包网页语音响应被截断"));
                }
                let bytes = &data[offset..end];
                offset = end;
                match number {
                    1 => envelope.task_id = Some(utf8_field(bytes)?),
                    4 => envelope.event = Some(utf8_field(bytes)?),
                    7 => envelope.payload = Some(utf8_field(bytes)?),
                    8 => envelope.data = Some(bytes.to_vec()),
                    _ => {}
                }
            }
            5 => {
                let end = offset.checked_add(4).ok_or_else(|| DoubaoWebError::protocol("豆包网页语音响应被截断"))?;
                if end > data.len() {
                    return Err(DoubaoWebError::protocol("豆包网页语音响应被截断"));
                }
                offset = end;
            }
            _ => return Err(DoubaoWebError::protocol("豆包网页语音响应使用了不支持的编码")),
        }
    }
    Ok(envelope)
}

fn utf8_field(bytes: &[u8]) -> Result<String, DoubaoWebError> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| DoubaoWebError::protocol("豆包网页语音响应文本不是 UTF-8"))
}

fn decode_text_response(text: &str) -> Result<Envelope, DoubaoWebError> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| DoubaoWebError::protocol("豆包网页语音响应不是有效 JSON"))?;
    let object = value.as_object().ok_or_else(|| DoubaoWebError::protocol("豆包网页语音 JSON 响应必须是对象"))?;
    let string = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(Envelope {
        task_id: string("task_id"),
        event: string("event"),
        status_code: object
            .get("status_code")
            .and_then(|value| value.as_u64().or_else(|| value.as_str().and_then(|text| text.parse().ok()))),
        payload: object.get("payload").map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }),
        ..Envelope::default()
    })
}

/// 用全新客户端读取首页。Cookie 只存在本次调用的内存 CookieJar 里，调用结束即丢弃，
/// 不读取浏览器资料、不落盘，也不会转发给 WebSocket。
pub(crate) async fn bootstrap_guest(timeout: Duration) -> Result<GuestContext, DoubaoWebError> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .cookie_store(true)
        .build()
        .map_err(|_| DoubaoWebError::network("创建豆包网页请求失败，请稍后重试"))?;
    let response = client
        .get(HOMEPAGE)
        .header("User-Agent", USER_AGENT)
        .header("Accept-Language", ACCEPT_LANGUAGE)
        .send()
        .await
        .map_err(|error| {
            tracing::debug!("访问豆包首页失败: {error}");
            DoubaoWebError::network("访问豆包网页首页失败，请检查网络后重试")
        })?;
    let status = response.status();
    if !status.is_success() {
        let code = status.as_u16();
        let kind = match code {
            404 | 410 => ErrorKind::Protocol,
            403 | 429 => ErrorKind::Quota,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::Network,
        };
        return Err(DoubaoWebError::new(kind, format!("豆包网页首页暂时不可用（HTTP {code}），请稍后重试")));
    }
    if response.content_length().is_some_and(|length| length > MAX_HOMEPAGE_BYTES) {
        return Err(DoubaoWebError::protocol("豆包网页首页响应过大，已停止解析"));
    }
    let body = response.bytes().await.map_err(|error| {
        tracing::debug!("读取豆包首页失败: {error}");
        DoubaoWebError::network("读取豆包网页首页失败，请检查网络后重试")
    })?;
    if body.len() as u64 > MAX_HOMEPAGE_BYTES {
        return Err(DoubaoWebError::protocol("豆包网页首页响应过大，已停止解析"));
    }
    let html = std::str::from_utf8(&body)
        .map_err(|_| DoubaoWebError::protocol("豆包网页首页内容不是 UTF-8，网页结构可能已改变"))?;
    parse_guest_context(html)
}

fn parse_guest_context(html: &str) -> Result<GuestContext, DoubaoWebError> {
    let marker = html
        .find("window._ROUTER_DATA")
        .ok_or_else(|| DoubaoWebError::protocol("豆包网页首页缺少游客初始化数据，网页结构可能已改变"))?;
    let after_marker = &html[marker + "window._ROUTER_DATA".len()..];
    let equals = after_marker
        .find('=')
        .ok_or_else(|| DoubaoWebError::protocol("豆包网页游客初始化数据格式无效，网页结构可能已改变"))?;
    let json_start = &after_marker[equals + 1..];
    let router = serde_json::Deserializer::from_str(json_start)
        .into_iter::<Value>()
        .next()
        .transpose()
        .map_err(|_| DoubaoWebError::protocol("无法解析豆包网页游客初始化数据，网页结构可能已改变"))?
        .ok_or_else(|| DoubaoWebError::protocol("无法解析豆包网页游客初始化数据，网页结构可能已改变"))?;
    let layout = router
        .pointer("/loaderData/chat_layout/chat_layout")
        .ok_or_else(|| DoubaoWebError::protocol("无法解析豆包网页游客初始化数据，网页结构可能已改变"))?;
    let device_id = scalar_string(layout.pointer("/launchCore/web_id"))
        .ok_or_else(|| DoubaoWebError::protocol("豆包网页首页未返回游客设备标识"))?;
    if device_id.is_empty()
        || !device_id.bytes().all(|byte| byte.is_ascii_digit())
        || device_id.parse::<u128>().ok().is_none_or(|value| value == 0)
    {
        return Err(DoubaoWebError::protocol("豆包网页首页返回的游客设备标识无效"));
    }
    let version = scalar_string(layout.get("pc_version"))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DoubaoWebError::protocol("豆包网页首页未返回版本信息，网页结构可能已改变"))?;
    let aid = scalar_string(layout.get("aid"))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DoubaoWebError::protocol("豆包网页首页未返回应用标识，网页结构可能已改变"))?;
    Ok(GuestContext::with_device(device_id, version, aid))
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn gateway_url(config: &DoubaoWebConfig, context: &GuestContext) -> Result<Url, DoubaoWebError> {
    let mut url = Url::parse(&config.gateway).map_err(|_| DoubaoWebError::protocol("豆包网页语音网关地址无效"))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in &context.pairs {
            query.append_pair(key, value);
        }
    }
    Ok(url)
}

async fn connect_gateway(config: &DoubaoWebConfig, context: &GuestContext) -> Result<WsStream, DoubaoWebError> {
    let url = gateway_url(config, context)?;
    let mut request =
        url.as_str().into_client_request().map_err(|_| DoubaoWebError::protocol("豆包网页语音 WebSocket 地址无效"))?;
    let headers = request.headers_mut();
    headers.insert("Origin", HeaderValue::from_static("https://www.doubao.com"));
    headers.insert(
        "User-Agent",
        HeaderValue::from_str(USER_AGENT).map_err(|_| DoubaoWebError::protocol("豆包网页语音 User-Agent 无效"))?,
    );
    match timeout(config.timeout, tokio_tungstenite::connect_async(request)).await {
        Err(_) => Err(DoubaoWebError::network("连接豆包网页语音服务超时，请稍后重试")),
        Ok(Ok((stream, _response))) => Ok(stream),
        Ok(Err(WsError::Http(response))) => {
            let code = response.status().as_u16();
            let kind = match code {
                404 | 410 => ErrorKind::Protocol,
                403 | 429 => ErrorKind::Quota,
                500..=599 => ErrorKind::Server,
                _ => ErrorKind::Network,
            };
            Err(DoubaoWebError::new(kind, format!("豆包网页语音连接失败（HTTP {code}），请稍后重试")))
        }
        Ok(Err(error)) => {
            tracing::debug!("豆包网页语音 WebSocket 握手失败: {error}");
            Err(DoubaoWebError::network("无法连接豆包网页语音服务，请检查网络后重试"))
        }
    }
}

struct SessionClient {
    writer: WsWriter,
    reader: WsReader,
    timeout: Duration,
    task_id: String,
    session_started: bool,
    session_finished: bool,
    finals: Vec<String>,
}

impl SessionClient {
    async fn connect(config: &DoubaoWebConfig, context: &GuestContext) -> Result<Self, DoubaoWebError> {
        let stream = connect_gateway(config, context).await?;
        let (writer, reader) = stream.split();
        Ok(Self {
            writer,
            reader,
            timeout: config.timeout,
            task_id: String::new(),
            session_started: false,
            session_finished: false,
            finals: Vec::new(),
        })
    }

    async fn send_event(
        &mut self,
        event: &str,
        payload: Option<&Value>,
        audio: Option<&[u8]>,
        sequence: Option<u64>,
    ) -> Result<(), DoubaoWebError> {
        let task_id = (!self.task_id.is_empty()).then_some(self.task_id.as_str());
        let packet = encode_request(event, task_id, payload, audio, sequence);
        self.writer.send(Message::binary(packet)).await.map_err(|error| {
            tracing::debug!("发送豆包网页语音请求失败: {error}");
            DoubaoWebError::network("发送豆包网页语音请求失败，请重试")
        })
    }

    async fn next_envelope(&mut self) -> Result<Envelope, DoubaoWebError> {
        loop {
            match self.reader.next().await {
                Some(Ok(Message::Binary(data))) => return decode_binary_response(&data),
                Some(Ok(Message::Text(text))) => return decode_text_response(&text),
                Some(Ok(Message::Close(_))) | None => {
                    return Err(DoubaoWebError::network("豆包网页语音连接在会话结束前关闭，请重试"));
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    tracing::debug!("读取豆包网页语音响应失败: {error}");
                    return Err(DoubaoWebError::network("读取豆包网页语音响应失败，请重试"));
                }
            }
        }
    }

    fn process_response(&mut self, response: &Envelope) -> Result<String, DoubaoWebError> {
        let event = response.event.clone().unwrap_or_default();
        let code = response.status_code;
        if code.is_some_and(|code| code != SUCCESS_STATUS) || matches!(event.as_str(), "TaskFailed" | "SessionFailed") {
            tracing::warn!(
                event = %event,
                status_code = ?code,
                "豆包网页语音返回失败"
            );
            let detail = code.map(|code| format!("，状态码 {code}")).unwrap_or_default();
            return Err(DoubaoWebError::server(format!(
                "豆包网页语音服务返回失败（{}{detail}）",
                if event.is_empty() { "未知事件" } else { event.as_str() }
            )));
        }
        if event == "ASRResponse" {
            self.collect_final_text(response)?;
        }
        Ok(event)
    }

    fn collect_final_text(&mut self, response: &Envelope) -> Result<(), DoubaoWebError> {
        let Some(payload) = response.payload.as_deref() else {
            return Ok(());
        };
        let value: Value =
            serde_json::from_str(payload).map_err(|_| DoubaoWebError::protocol("豆包网页语音识别结果格式无效"))?;
        let results = match value.get("results") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(results)) => results.as_slice(),
            Some(_) => return Err(DoubaoWebError::protocol("豆包网页语音识别结果格式无效")),
        };
        for result in results {
            if result.get("is_interim").and_then(Value::as_bool) != Some(false) {
                continue;
            }
            let text = result
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| DoubaoWebError::protocol("豆包网页语音识别结果缺少文本"))?;
            self.finals.push(text.to_string());
        }
        Ok(())
    }

    async fn wait_for(&mut self, expected: &str) -> Result<Envelope, DoubaoWebError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DoubaoWebError::network(format!("等待 {expected} 超时，请稍后重试")));
            }
            let response = match timeout(remaining, self.next_envelope()).await {
                Err(_) => return Err(DoubaoWebError::network(format!("等待 {expected} 超时，请稍后重试"))),
                Ok(result) => result?,
            };
            let event = self.process_response(&response)?;
            if event == expected {
                return Ok(response);
            }
        }
    }

    async fn pace_and_drain(&mut self, deadline: Instant) -> Result<(), DoubaoWebError> {
        loop {
            if Instant::now() >= deadline {
                return Ok(());
            }
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => return Ok(()),
                response = self.next_envelope() => {
                    let response = response?;
                    self.process_response(&response)?;
                }
            }
        }
    }

    async fn run(
        &mut self,
        samples: &[i16],
        on_push: &mut (dyn FnMut(usize) -> bool + Send),
    ) -> Result<String, DoubaoWebError> {
        self.send_event("StartTask", None, None, None).await?;
        let started = self.wait_for("TaskStarted").await?;
        self.task_id = started
            .task_id
            .filter(|task_id| !task_id.is_empty())
            .ok_or_else(|| DoubaoWebError::protocol("创建豆包网页语音任务后未收到 task_id"))?;

        self.send_event("StartSession", Some(&START_SESSION_PAYLOAD), None, None).await?;
        self.wait_for("SessionStarted").await?;
        self.session_started = true;

        let stream_started = Instant::now();
        for (index, chunk) in samples.chunks(CHUNK_SAMPLES).enumerate() {
            let pushed = index * CHUNK_SAMPLES;
            if !on_push(pushed) {
                return Err(DoubaoWebError::input(super::TRANSCRIBE_CANCELLED));
            }
            let audio = pcm16le_bytes(chunk);
            self.send_event("TaskRequest", None, Some(&audio), Some((index + 1) as u64)).await?;
            let sent_samples = ((index + 1) * CHUNK_SAMPLES).min(samples.len());
            let deadline = stream_started + Duration::from_secs_f64(sent_samples as f64 / SAMPLE_RATE as f64);
            self.pace_and_drain(deadline).await?;
        }
        if !on_push(samples.len()) {
            return Err(DoubaoWebError::input(super::TRANSCRIBE_CANCELLED));
        }

        self.send_event("EndASR", None, None, None).await?;
        self.wait_for("ASREnded").await?;
        self.send_event("FinishSession", None, None, None).await?;
        self.wait_for("SessionFinished").await?;
        self.session_finished = true;

        if self.finals.is_empty() {
            return Err(DoubaoWebError::server("豆包网页语音会话已结束，但没有最终识别结果"));
        }
        Ok(self.finals.concat().trim().to_string())
    }

    async fn close(&mut self) {
        let _ = timeout(
            Duration::from_secs(1),
            self.writer.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() }))),
        )
        .await;
    }
}

fn pcm16le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

async fn transcribe_session(
    config: &DoubaoWebConfig,
    context: &GuestContext,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoWebError> {
    if samples.is_empty() {
        return Err(DoubaoWebError::input("音频为空，无法转写"));
    }
    let mut client = SessionClient::connect(config, context).await?;
    let outcome = client.run(samples, on_push).await;
    if client.session_started && !client.session_finished {
        let _ = client.send_event("CancelSession", None, None, None).await;
    }
    client.close().await;
    outcome
}

async fn transcribe_pcm_with_progress_impl(
    config: &DoubaoWebConfig,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoWebError> {
    config.ensure()?;
    if samples.is_empty() {
        return Err(DoubaoWebError::input("音频为空，无法转写"));
    }
    let mut parts = Vec::new();
    for (index, chunk) in samples.chunks(MAX_SESSION_SAMPLES).enumerate() {
        let base = index * MAX_SESSION_SAMPLES;
        if !on_push(base) {
            return Err(DoubaoWebError::input(super::TRANSCRIBE_CANCELLED));
        }
        let context = bootstrap_guest(config.timeout).await?;
        let mut chunk_progress = |pushed: usize| on_push(base + pushed);
        let text = transcribe_session(config, &context, chunk, &mut chunk_progress).await?;
        if !text.is_empty() {
            parts.push(text);
        }
    }
    Ok(parts.join(" ").trim().to_string())
}

/// 整段转写：输入已经是 16k 单声道 PCM16。超过参考实现单次上限时自动拆成多个游客会话。
pub async fn transcribe_pcm(config: &DoubaoWebConfig, samples: &[i16]) -> Result<String, String> {
    transcribe_pcm_with_progress(config, samples, &mut |_| true).await.map_err(|error| error.to_string())
}

/// 同 [`transcribe_pcm`]，按已推送样本数回调进度；返回 false 时关闭当前会话并取消。
pub async fn transcribe_pcm_with_progress(
    config: &DoubaoWebConfig,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoWebError> {
    transcribe_pcm_with_progress_impl(config, samples, on_push).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::collections::BTreeMap;
    use tokio::sync::oneshot;

    const ROUTER_HTML: &str = r#"
        <html><script>
        window._ROUTER_DATA = {"loaderData":{"chat_layout":{"chat_layout":{
            "launchCore":{"web_id":731245678901},
            "pc_version":"1.2.3",
            "aid":497858
        }}}};
        </script></html>
    "#;

    #[test]
    fn 首页游客初始化_解析设备与版本() {
        let context = parse_guest_context(ROUTER_HTML).unwrap();
        assert_eq!(context.pairs.iter().find(|(key, _)| *key == "device_id").unwrap().1, "731245678901");
        assert_eq!(context.pairs.iter().find(|(key, _)| *key == "pc_version").unwrap().1, "1.2.3");
        assert_eq!(context.pairs.iter().find(|(key, _)| *key == "aid").unwrap().1, "497858");
        assert_eq!(context.pairs.iter().find(|(key, _)| *key == "real_aid").unwrap().1, "497858");
        assert_eq!(context.pairs.iter().find(|(key, _)| *key == "api_app_key").unwrap().1, APPKEY);
    }

    #[test]
    fn 首页游客初始化_结构变化与无效设备号给中文错误() {
        for html in [
            "<html>没有初始化数据</html>",
            r#"<script>window._ROUTER_DATA = {"loaderData":{}}</script>"#,
            r#"<script>window._ROUTER_DATA = {"loaderData":{"chat_layout":{"chat_layout":{
                "launchCore":{"web_id":0},"pc_version":"1.2.3","aid":1}}}}</script>"#,
            r#"<script>window._ROUTER_DATA = {"loaderData":{"chat_layout":{"chat_layout":{
                "launchCore":{"web_id":"abc"},"pc_version":"1.2.3","aid":1}}}}</script>"#,
        ] {
            let error = parse_guest_context(html).unwrap_err();
            assert!(error.message.contains("豆包网页"), "{error}");
        }
    }

    #[test]
    fn 协议编码_固定字节与参考实现一致() {
        let packet = encode_request("StartTask", None, None, None, None);
        assert_eq!(
            packet,
            vec![
                18, 16, 71, 79, 113, 81, 112, 102, 111, 49, 102, 79, 55, 115, 108, 72, 118, 56, 26, 10, 86, 111, 105,
                99, 101, 71, 101, 110, 105, 101, 42, 9, 83, 116, 97, 114, 116, 84, 97, 115, 107, 50, 2, 123, 125
            ]
        );
    }

    #[test]
    fn 协议解码_固定响应字段映射() {
        let packet = vec![
            10, 6, 116, 97, 115, 107, 45, 49, 26, 10, 86, 111, 105, 99, 101, 71, 101, 110, 105, 101, 34, 11, 84, 97,
            115, 107, 83, 116, 97, 114, 116, 101, 100, 40, 128, 218, 196, 9, 58, 11, 123, 34, 111, 107, 34, 58, 116,
            114, 117, 101, 125,
        ];
        let response = decode_binary_response(&packet).unwrap();
        assert_eq!(response.task_id.as_deref(), Some("task-1"));
        assert_eq!(response.event.as_deref(), Some("TaskStarted"));
        assert_eq!(response.status_code, Some(SUCCESS_STATUS));
        assert_eq!(response.payload.as_deref(), Some(r#"{"ok":true}"#));
    }

    #[test]
    fn 协议解码_拒绝截断与非法字段类型() {
        let truncated = decode_binary_response(&[10, 6, b'a']).unwrap_err();
        assert!(truncated.message.contains("截断"), "{truncated}");
        let wrong_wire = decode_binary_response(&[42, 1, b'A']).unwrap_err();
        assert!(wrong_wire.message.contains("类型"), "{wrong_wire}");
    }

    #[test]
    fn 开始会话载荷不带_language() {
        assert!(START_SESSION_PAYLOAD.get("language").is_none());
        assert_eq!(START_SESSION_PAYLOAD["asr"]["audio_info"]["sample_rate"], json!(16000));
        assert_eq!(START_SESSION_PAYLOAD["asr"]["audio_info"]["format"], json!("pcm"));
    }

    #[derive(Default, Clone, Copy)]
    struct StubBehavior {
        fail_task: bool,
        fail_audio: bool,
        no_finals: bool,
        silent: bool,
    }

    fn response_frame(
        event: &str,
        status_code: Option<u64>,
        task_id: Option<&str>,
        payload: Option<&Value>,
    ) -> Vec<u8> {
        let mut packet = Vec::new();
        if let Some(task_id) = task_id {
            push_bytes_field(&mut packet, 1, task_id.as_bytes());
        }
        push_bytes_field(&mut packet, 3, b"VoiceGenie");
        push_bytes_field(&mut packet, 4, event.as_bytes());
        if let Some(status_code) = status_code {
            push_varint_field(&mut packet, 5, status_code);
        }
        if let Some(payload) = payload {
            push_bytes_field(&mut packet, 7, payload.to_string().as_bytes());
        }
        packet
    }

    async fn stub_server(behavior: StubBehavior) -> (String, oneshot::Receiver<Vec<(String, Value)>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (events_tx, events_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut events = Vec::new();
            if behavior.silent {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = events_tx.send(events);
                return;
            }
            while let Some(message) = ws.next().await {
                let Ok(Message::Binary(data)) = message else {
                    break;
                };
                let fields = test_fields(&data);
                let event = fields.get(&5).and_then(Field::text).unwrap_or("").to_string();
                let payload = fields
                    .get(&6)
                    .and_then(Field::text)
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    .unwrap_or_else(|| json!({}));
                events.push((event.clone(), payload));
                match event.as_str() {
                    "StartTask" if behavior.fail_task => {
                        let _ = ws.send(Message::binary(response_frame("TaskFailed", Some(123), None, None))).await;
                        break;
                    }
                    "StartTask" => {
                        let _ = ws
                            .send(Message::binary(response_frame(
                                "TaskStarted",
                                Some(SUCCESS_STATUS),
                                Some("task-1"),
                                None,
                            )))
                            .await;
                    }
                    "StartSession" => {
                        let _ = ws
                            .send(Message::binary(response_frame("SessionStarted", Some(SUCCESS_STATUS), None, None)))
                            .await;
                    }
                    "TaskRequest" if behavior.fail_audio => {
                        let _ = ws.send(Message::binary(response_frame("TaskFailed", Some(456), None, None))).await;
                    }
                    "TaskRequest" if !behavior.no_finals => {
                        let payload = json!({"results": [
                            {"text": "你好，", "is_interim": false},
                            {"text": "中间", "is_interim": true},
                            {"text": "世界", "is_interim": false}
                        ]});
                        let _ = ws
                            .send(Message::binary(response_frame(
                                "ASRResponse",
                                Some(SUCCESS_STATUS),
                                None,
                                Some(&payload),
                            )))
                            .await;
                    }
                    "EndASR" => {
                        let _ = ws
                            .send(Message::binary(response_frame("ASREnded", Some(SUCCESS_STATUS), None, None)))
                            .await;
                    }
                    "FinishSession" => {
                        let _ = ws
                            .send(Message::binary(response_frame("SessionFinished", Some(SUCCESS_STATUS), None, None)))
                            .await;
                    }
                    "CancelSession" => break,
                    _ => {}
                }
            }
            let _ = events_tx.send(events);
        });
        (format!("ws://{address}"), events_rx)
    }

    #[derive(Debug, Clone)]
    enum Field {
        Varint,
        Bytes(Vec<u8>),
    }

    impl Field {
        fn text(&self) -> Option<&str> {
            match self {
                Self::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
                Self::Varint => None,
            }
        }
    }

    fn test_fields(data: &[u8]) -> BTreeMap<u32, Field> {
        let mut fields = BTreeMap::new();
        let mut offset = 0;
        while offset < data.len() {
            let tag = read_varint(data, &mut offset).unwrap();
            let number = (tag >> 3) as u32;
            let wire = (tag & 7) as u8;
            match wire {
                0 => {
                    read_varint(data, &mut offset).unwrap();
                    fields.insert(number, Field::Varint);
                }
                2 => {
                    let length = usize::try_from(read_varint(data, &mut offset).unwrap()).unwrap();
                    let end = offset + length;
                    fields.insert(number, Field::Bytes(data[offset..end].to_vec()));
                    offset = end;
                }
                other => panic!("测试桩不支持 wire type {other}"),
            }
        }
        fields
    }

    fn test_config(gateway: String, timeout: Duration) -> DoubaoWebConfig {
        DoubaoWebConfig { timeout, gateway }
    }

    fn test_context() -> GuestContext {
        GuestContext::with_device("123".into(), "1.2.3".into(), "497858".into())
    }

    #[tokio::test]
    async fn 事件流_成功只拼接最终结果_请求不带语言() {
        let (gateway, events_rx) = stub_server(StubBehavior::default()).await;
        let config = test_config(gateway, Duration::from_secs(2));
        let samples = vec![0i16; CHUNK_SAMPLES];
        let text = transcribe_session(&config, &test_context(), &samples, &mut |_| true).await.unwrap();
        assert_eq!(text, "你好，世界");

        let events = timeout(Duration::from_secs(2), events_rx).await.unwrap().unwrap();
        let names: Vec<&str> = events.iter().map(|(event, _)| event.as_str()).collect();
        assert_eq!(names, ["StartTask", "StartSession", "TaskRequest", "EndASR", "FinishSession"]);
        let start_session = events.iter().find(|(event, _)| event == "StartSession").unwrap();
        assert!(start_session.1.get("language").is_none());
    }

    #[tokio::test]
    async fn 事件流_任务失败_返回中文错误() {
        let (gateway, events_rx) = stub_server(StubBehavior { fail_task: true, ..StubBehavior::default() }).await;
        let config = test_config(gateway, Duration::from_secs(2));
        let error =
            transcribe_session(&config, &test_context(), &vec![0i16; CHUNK_SAMPLES], &mut |_| true).await.unwrap_err();
        assert!(error.message.contains("豆包网页语音服务返回失败"), "{error}");
        let _ = timeout(Duration::from_secs(2), events_rx).await;
    }

    #[tokio::test]
    async fn 事件流_会话开始后失败_发送取消() {
        let (gateway, events_rx) = stub_server(StubBehavior { fail_audio: true, ..StubBehavior::default() }).await;
        let config = test_config(gateway, Duration::from_secs(2));
        let error =
            transcribe_session(&config, &test_context(), &vec![0i16; CHUNK_SAMPLES], &mut |_| true).await.unwrap_err();
        assert!(error.message.contains("TaskFailed"), "{error}");
        let events = timeout(Duration::from_secs(2), events_rx).await.unwrap().unwrap();
        assert_eq!(events.last().unwrap().0, "CancelSession");
    }

    #[tokio::test]
    async fn 事件流_缺少最终结果_返回错误() {
        let (gateway, _events_rx) = stub_server(StubBehavior { no_finals: true, ..StubBehavior::default() }).await;
        let config = test_config(gateway, Duration::from_secs(2));
        let error =
            transcribe_session(&config, &test_context(), &vec![0i16; CHUNK_SAMPLES], &mut |_| true).await.unwrap_err();
        assert!(error.message.contains("没有最终识别结果"), "{error}");
    }

    #[tokio::test]
    async fn 事件流_阶段超时_返回可执行提示() {
        let (gateway, _events_rx) = stub_server(StubBehavior { silent: true, ..StubBehavior::default() }).await;
        let config = test_config(gateway, Duration::from_millis(80));
        let error =
            transcribe_session(&config, &test_context(), &vec![0i16; CHUNK_SAMPLES], &mut |_| true).await.unwrap_err();
        assert!(error.message.contains("等待 TaskStarted 超时"), "{error}");
    }

    /// 真实网页匿名链路测试（默认忽略）：
    /// `ATT_TEST_WAV=/abs/path/16k-mono.wav cargo test -p att-core 真实豆包网页 -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn 真实豆包网页_已知中文音频返回文本() {
        let path = std::env::var("ATT_TEST_WAV").expect("需要 ATT_TEST_WAV 指向 16k 单声道音频");
        let samples = crate::audio::read_audio_16k_mono(std::path::Path::new(&path)).expect("读取测试音频失败");
        let config = DoubaoWebConfig::from_seconds(DEFAULT_TIMEOUT_SECONDS);
        let text = transcribe_pcm(&config, &samples).await.expect("豆包网页真实识别失败");
        println!("豆包网页识别结果：{text}");
        assert!(!text.trim().is_empty(), "识别结果为空");
    }
}
