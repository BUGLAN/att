//! 豆包输入法语音识别（独立实现自开源项目 doubaoime-asr 观察到的协议行为，MIT 参考；不复制其代码）。
//!
//! 这条链路是纯云端协议，不注入、不依赖本机安装豆包输入法：
//! 1. 模拟 Android 客户端向字节注册一个虚拟设备，取 device_id，再取 ASR token；
//! 2. 连输入法语音 WebSocket，按 StartTask -> StartSession -> TaskRequest(20ms PCM 帧) -> FinishSession 发送；
//! 3. 服务端在 result_json 里按段返回中间稿（累计）与最终稿；客户端自己按段累积，避免只拿到最后一段。
//!
//! 实测（2026-11-23）：服务端接受 audio_info.format = "pcm"（16k/单声道/PCM16），所以不需要 libopus。
//! 非官方链路，协议/版本可能变化；错误文案只给可执行结论，不回显上游响应体。

use super::sauc::ErrorKind;
use crate::storage::crypto::Crypto;
use crate::storage::json_store;
use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use futures_util::FutureExt;
use futures_util::SinkExt;
use serde_json::{json, Value};
#[cfg(test)]
use std::future::Future;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

pub const LABEL: &str = "豆包输入法语音";

pub const REGISTER_URL: &str = "https://log.snssdk.com/service/2/device_register/";
pub const SETTINGS_URL: &str = "https://is.snssdk.com/service/settings/v3/";
pub const WS_URL: &str = "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws";
pub const AID: &str = "401734";
pub const USER_AGENT: &str = "com.bytedance.android.doubaoime/100102018 (Linux; U; Android 16; en_US; \
     Pixel 7 Pro; Build/BP2A.250605.031.A2; Cronet/TTNetVersion:94cf429a 2025-11-17 QuicVersion:1f89f732 2025-05-08)";
/// MD5("body=null") 的大写十六进制，Settings 接口的 x-ss-stub 固定值。
const X_SS_STUB: &str = "46C03B52742B3F2615A3ABDF1636B754";

pub const DEFAULT_TIMEOUT_SECONDS: i64 = 30;
pub const MIN_TIMEOUT_SECONDS: i64 = 10;
pub const MAX_TIMEOUT_SECONDS: i64 = 300;

pub const SAMPLE_RATE: usize = 16_000;
/// 20ms 一帧：与服务端/参考实现一致。
const FRAME_SAMPLES: usize = 320;
/// 单次音频上限：按 13 分钟切会话，避免一次会话过长。
pub const MAX_SESSION_SECONDS: usize = 780;
const MAX_SESSION_SAMPLES: usize = MAX_SESSION_SECONDS * SAMPLE_RATE;
const FRAME_FIRST: u32 = 1;
const FRAME_MIDDLE: u32 = 3;
const FRAME_LAST: u32 = 9;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 运行期配置：无用户可见凭据；设备凭据缓存到 credential_path。
#[derive(Clone)]
pub struct DoubaoImeConfig {
    pub timeout: Duration,
    pub(crate) endpoint: String,
    pub(crate) credential_path: Option<PathBuf>,
    pub(crate) crypto: Option<Arc<Crypto>>,
    /// 测试或显式注入时直接使用，跳过注册。
    pub(crate) credentials: Option<Credentials>,
    #[cfg(test)]
    credential_refresher: Option<Arc<dyn Fn() -> RefreshFuture + Send + Sync>>,
}

#[cfg(test)]
type RefreshFuture = Pin<Box<dyn Future<Output = Result<Credentials, DoubaoImeError>> + Send>>;

impl std::fmt::Debug for DoubaoImeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoubaoImeConfig")
            .field("timeout", &self.timeout)
            .field("endpoint", &self.endpoint)
            .field("credential_path", &self.credential_path)
            .field("has_credentials", &self.credentials.is_some())
            .finish()
    }
}

impl Default for DoubaoImeConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS as u64),
            endpoint: WS_URL.into(),
            credential_path: None,
            crypto: None,
            credentials: None,
            #[cfg(test)]
            credential_refresher: None,
        }
    }
}

impl DoubaoImeConfig {
    pub fn from_seconds(seconds: i64) -> Self {
        Self {
            timeout: Duration::from_secs(seconds.clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS) as u64),
            ..Self::default()
        }
    }

    pub fn with_credential_path(mut self, path: PathBuf) -> Self {
        self.credential_path = Some(path);
        self
    }

    pub fn with_crypto(mut self, crypto: Arc<Crypto>) -> Self {
        self.crypto = Some(crypto);
        self
    }

    pub fn ensure(&self) -> Result<(), DoubaoImeError> {
        if self.timeout.is_zero() {
            return Err(DoubaoImeError::protocol("豆包输入法语音超时时间无效，请在设置中改为 10-300 秒"));
        }
        if self.endpoint.trim().is_empty() {
            return Err(DoubaoImeError::protocol("豆包输入法语音地址配置无效"));
        }
        Ok(())
    }
}

/// 虚拟设备凭据：token 是设备级匿名凭证，不是账号密钥。
#[derive(Clone, Default)]
pub struct Credentials {
    pub device_id: String,
    pub install_id: String,
    pub cdid: String,
    pub token: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("device_id", &self.device_id)
            .field("install_id", &self.install_id)
            .field("cdid", &self.cdid)
            .field("token_set", &!self.token.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct DoubaoImeError {
    pub kind: ErrorKind,
    pub message: String,
    fresh_device_retry: bool,
}

impl DoubaoImeError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), fresh_device_retry: false }
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

impl std::fmt::Display for DoubaoImeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for DoubaoImeError {}

impl DoubaoImeError {
    fn retry_with_fresh_device(mut self) -> Self {
        self.fresh_device_retry = true;
        self
    }

    /// 只有启动任务 / 会话阶段收到服务端失败时，才允许换一台新注册设备重试。
    fn needs_fresh_device(&self) -> bool {
        self.fresh_device_retry
    }

    fn user_facing(mut self) -> Self {
        if self.message.contains("service discovery failure") {
            self.message = "豆包输入法语音服务暂时不可用（后端服务发现失败），请稍后重试".into();
        }
        self
    }
}

// ---------------- protobuf（手写，字段见 doubaoime-asr 的 asr.proto） ----------------

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value > 127 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
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
fn read_varint(data: &[u8], offset: &mut usize) -> Result<u64, DoubaoImeError> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = *data.get(*offset).ok_or_else(|| DoubaoImeError::protocol("豆包输入法响应被截断"))?;
        *offset += 1;
        if index == 9 && byte > 1 {
            return Err(DoubaoImeError::protocol("豆包输入法响应整数超出范围"));
        }
        value |= u64::from(byte & 127) << (index * 7);
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(DoubaoImeError::protocol("豆包输入法响应 varint 无效"))
}

/// 组装 AsrRequest。StartTask/StartSession/FinishSession 带 token；TaskRequest 不带。
fn encode_request(
    token: Option<&str>,
    method: &str,
    payload: Option<&str>,
    audio: Option<&[u8]>,
    request_id: &str,
    frame_state: Option<u32>,
) -> Vec<u8> {
    let mut packet = Vec::new();
    if let Some(token) = token.filter(|value| !value.is_empty()) {
        push_bytes_field(&mut packet, 2, token.as_bytes());
    }
    push_bytes_field(&mut packet, 3, b"ASR");
    push_bytes_field(&mut packet, 5, method.as_bytes());
    if let Some(payload) = payload {
        push_bytes_field(&mut packet, 6, payload.as_bytes());
    }
    if let Some(audio) = audio {
        push_bytes_field(&mut packet, 7, audio);
    }
    push_bytes_field(&mut packet, 8, request_id.as_bytes());
    if let Some(frame_state) = frame_state {
        push_varint_field(&mut packet, 9, u64::from(frame_state));
    }
    packet
}

#[derive(Default, Debug)]
struct AsrResponseMsg {
    message_type: String,
    status_message: String,
    result_json: String,
}

fn decode_response(data: &[u8]) -> Result<AsrResponseMsg, DoubaoImeError> {
    let mut msg = AsrResponseMsg::default();
    let mut offset = 0usize;
    while offset < data.len() {
        let tag = read_varint(data, &mut offset)?;
        let number = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        match wire {
            0 => {
                let _ = read_varint(data, &mut offset)?;
            }
            1 => {
                offset = offset
                    .checked_add(8)
                    .filter(|end| *end <= data.len())
                    .ok_or_else(|| DoubaoImeError::protocol("豆包输入法响应被截断"))?;
            }
            5 => {
                offset = offset
                    .checked_add(4)
                    .filter(|end| *end <= data.len())
                    .ok_or_else(|| DoubaoImeError::protocol("豆包输入法响应被截断"))?;
            }
            2 => {
                let length = usize::try_from(read_varint(data, &mut offset)?)
                    .map_err(|_| DoubaoImeError::protocol("豆包输入法响应字段过长"))?;
                let end = offset
                    .checked_add(length)
                    .filter(|end| *end <= data.len())
                    .ok_or_else(|| DoubaoImeError::protocol("豆包输入法响应被截断"))?;
                let bytes = &data[offset..end];
                offset = end;
                match number {
                    4 => msg.message_type = utf8(bytes)?,
                    6 => msg.status_message = utf8(bytes)?,
                    7 => msg.result_json = utf8(bytes)?,
                    _ => {}
                }
            }
            _ => return Err(DoubaoImeError::protocol("豆包输入法响应使用了不支持的编码")),
        }
    }
    Ok(msg)
}

fn utf8(bytes: &[u8]) -> Result<String, DoubaoImeError> {
    std::str::from_utf8(bytes).map(str::to_string).map_err(|_| DoubaoImeError::protocol("豆包输入法响应文本不是 UTF-8"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Parsed {
    TaskStarted,
    SessionStarted,
    SessionFinished,
    VadStart,
    Interim(String),
    Final(String),
    Heartbeat,
    Other(String),
    /// 只保留失败事件名；上游其它字段只进脱敏后的 debug 日志。
    Failed(String),
}

/// 去掉 URL / 文本里设备标识的参数值，日志只保留失败事件和其余可诊断信息。
fn redact_device_identifiers(input: &str) -> String {
    let mut redacted = input.to_string();
    for key in ["device_id=", "cdid="] {
        let mut offset = 0;
        while let Some(relative) = redacted[offset..].find(key) {
            let start = offset + relative;
            let value_end = redacted[start..]
                .find(|ch: char| ch == '&' || ch.is_whitespace() || ch == '"' || ch == '\'')
                .map(|end| start + end)
                .unwrap_or(redacted.len());
            redacted.replace_range(start..value_end, "[已隐藏]");
            offset = start + "[已隐藏]".len();
        }
    }
    redacted
}

fn parse_response(data: &[u8]) -> Result<Parsed, DoubaoImeError> {
    let msg = decode_response(data)?;
    match msg.message_type.as_str() {
        "TaskStarted" => Ok(Parsed::TaskStarted),
        "SessionStarted" => Ok(Parsed::SessionStarted),
        "SessionFinished" => Ok(Parsed::SessionFinished),
        "TaskFailed" | "SessionFailed" => {
            if !msg.status_message.is_empty() {
                tracing::debug!(
                    event = %msg.message_type,
                    detail = %redact_device_identifiers(&msg.status_message),
                    "豆包输入法语音服务返回失败"
                );
            }
            Ok(Parsed::Failed(msg.message_type))
        }
        other => {
            if msg.result_json.is_empty() {
                return Ok(Parsed::Other(other.to_string()));
            }
            let value: Value = serde_json::from_str(&msg.result_json)
                .map_err(|_| DoubaoImeError::protocol("豆包输入法识别结果格式无效"))?;
            let extra = value.get("extra").cloned().unwrap_or_else(|| json!({}));
            let Some(results) = value.get("results") else {
                return Ok(Parsed::Heartbeat);
            };
            if extra.get("vad_start").and_then(Value::as_bool) == Some(true) {
                return Ok(Parsed::VadStart);
            }
            let results = results.as_array().ok_or_else(|| DoubaoImeError::protocol("豆包输入法识别结果格式无效"))?;
            let mut text = String::new();
            let mut is_interim = true;
            let mut vad_finished = false;
            let mut nonstream = false;
            for result in results {
                if let Some(item) = result.get("text").and_then(Value::as_str) {
                    if !item.is_empty() {
                        text = item.to_string();
                    }
                }
                if result.get("is_interim").and_then(Value::as_bool) == Some(false) {
                    is_interim = false;
                }
                if result.get("is_vad_finished").and_then(Value::as_bool) == Some(true) {
                    vad_finished = true;
                }
                if result.get("extra").and_then(|item| item.get("nonstream_result")).and_then(Value::as_bool)
                    == Some(true)
                {
                    nonstream = true;
                }
            }
            if nonstream || (!is_interim && vad_finished) {
                Ok(Parsed::Final(text))
            } else {
                Ok(Parsed::Interim(text))
            }
        }
    }
}

/// 按段累积：中间稿用最新一条覆盖；新稿与旧稿没有公共前缀时视为新段，先提交旧稿。
/// 收到最终稿时若它属于另一段，就保留前面累计的中间稿，避免只拿到最后一句。
#[derive(Default)]
struct Accumulator {
    committed: String,
    pending: String,
}

impl Accumulator {
    fn interim(&mut self, text: String) {
        if !self.pending.is_empty() && !has_common_prefix(&self.pending, &text) {
            let pending = std::mem::take(&mut self.pending);
            self.committed.push_str(&pending);
        }
        self.pending = text;
    }

    fn final_result(&mut self, text: String) {
        let pending = std::mem::take(&mut self.pending);
        let segment = if text.is_empty() {
            pending
        } else if pending.is_empty() || has_common_prefix(&pending, &text) {
            if pending.starts_with(&text) {
                pending
            } else {
                text
            }
        } else {
            self.committed.push_str(&pending);
            text
        };
        self.committed.push_str(&segment);
    }

    fn vad_start(&mut self) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.committed.push_str(&pending);
        }
    }

    fn snapshot(&self) -> String {
        let mut text = self.committed.clone();
        text.push_str(&self.pending);
        text.trim().to_string()
    }

    fn finish(&mut self) -> String {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.committed.push_str(&pending);
        }
        self.committed.trim().to_string()
    }
}

fn has_common_prefix(left: &str, right: &str) -> bool {
    matches!((left.chars().next(), right.chars().next()), (Some(left), Some(right)) if left == right)
}

// ---------------- HTTP：注册设备 + 取 token ----------------

fn device_register_query(cdid: &str) -> Vec<(&'static str, String)> {
    vec![
        ("device_platform", "android".into()),
        ("os", "android".into()),
        ("ssmix", "a".into()),
        ("_rticket", now_ms().to_string()),
        ("cdid", cdid.to_string()),
        ("channel", "official".into()),
        ("aid", AID.into()),
        ("app_name", "oime".into()),
        ("version_code", "100102018".into()),
        ("version_name", "1.1.2".into()),
        ("manifest_version_code", "100102018".into()),
        ("update_version_code", "100102018".into()),
        ("resolution", "1080*2400".into()),
        ("dpi", "420".into()),
        ("device_type", "Pixel 7 Pro".into()),
        ("device_brand", "google".into()),
        ("language", "zh".into()),
        ("os_api", "34".into()),
        ("os_version", "16".into()),
        ("ac", "wifi".into()),
    ]
}

fn device_register_body(openudid: &str, clientudid: &str, cdid: &str) -> Value {
    json!({
        "magic_tag": "ss_app_log",
        "header": {
            "device_id": 0,
            "install_id": 0,
            "aid": 401734,
            "app_name": "oime",
            "version_code": 100102018,
            "version_name": "1.1.2",
            "manifest_version_code": 100102018,
            "update_version_code": 100102018,
            "channel": "official",
            "package": "com.bytedance.android.doubaoime",
            "device_platform": "android",
            "os": "android",
            "os_api": "34",
            "os_version": "16",
            "device_type": "Pixel 7 Pro",
            "device_brand": "google",
            "device_model": "Pixel 7 Pro",
            "resolution": "1080*2400",
            "dpi": "420",
            "language": "zh",
            "timezone": 8,
            "access": "wifi",
            "rom": "UP1A.231005.007",
            "rom_version": "UP1A.231005.007",
            "openudid": openudid,
            "clientudid": clientudid,
            "cdid": cdid,
            "region": "CN",
            "tz_name": "Asia/Shanghai",
            "tz_offset": 28800,
            "sim_region": "cn",
            "carrier_region": "cn",
            "cpu_abi": "arm64-v8a",
            "build_serial": "unknown",
            "not_request_sender": 0,
            "sig_hash": "",
            "google_aid": "",
            "mc": "",
            "serial_number": "",
        },
        "_gen_time": now_ms(),
    })
}

fn with_query(mut url: reqwest::Url, params: &[(&'static str, String)]) -> reqwest::Url {
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    url
}

fn http_client(timeout: Duration) -> Result<reqwest::Client, DoubaoImeError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| DoubaoImeError::network("初始化豆包输入法语音网络客户端失败"))
}

async fn register_device(client: &reqwest::Client) -> Result<Credentials, DoubaoImeError> {
    let cdid = Uuid::new_v4().to_string();
    let openudid = Uuid::new_v4().simple().to_string()[..16].to_string();
    let clientudid = Uuid::new_v4().to_string();
    let body = device_register_body(&openudid, &clientudid, &cdid);
    let url = with_query(
        reqwest::Url::parse(REGISTER_URL).map_err(|_| DoubaoImeError::protocol("注册地址无效"))?,
        &device_register_query(&cdid),
    );
    let response = client.post(url).header("User-Agent", USER_AGENT).json(&body).send().await.map_err(|error| {
        tracing::debug!("豆包输入法设备注册失败: {}", error.without_url());
        DoubaoImeError::network("注册豆包输入法虚拟设备失败，请检查网络后重试")
    })?;
    if !response.status().is_success() {
        return Err(DoubaoImeError::network(format!(
            "注册豆包输入法虚拟设备失败（HTTP {}）",
            response.status().as_u16()
        )));
    }
    let value: Value =
        response.json().await.map_err(|_| DoubaoImeError::protocol("豆包输入法设备注册响应不是有效 JSON"))?;
    let device_id = scalar(&value, "device_id_str")
        .or_else(|| value.get("device_id").map(value_text))
        .filter(|text| !text.is_empty() && text != "0")
        .ok_or_else(|| DoubaoImeError::protocol("豆包输入法设备注册未返回 device_id"))?;
    let install_id =
        scalar(&value, "install_id_str").or_else(|| value.get("install_id").map(value_text)).unwrap_or_default();
    Ok(Credentials { device_id, install_id, cdid, token: String::new() })
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn scalar(value: &Value, key: &str) -> Option<String> {
    value.get(key).map(value_text).filter(|text| !text.is_empty())
}

fn settings_query(device_id: &str, cdid: &str) -> Vec<(&'static str, String)> {
    vec![
        ("device_platform", "android".into()),
        ("os", "android".into()),
        ("ssmix", "a".into()),
        ("_rticket", now_ms().to_string()),
        ("cdid", cdid.to_string()),
        ("channel", "official".into()),
        ("aid", AID.into()),
        ("app_name", "oime".into()),
        ("version_code", "100102018".into()),
        ("version_name", "1.1.2".into()),
        ("device_id", device_id.to_string()),
    ]
}

async fn fetch_token(client: &reqwest::Client, device_id: &str, cdid: &str) -> Result<String, DoubaoImeError> {
    let url = with_query(
        reqwest::Url::parse(SETTINGS_URL).map_err(|_| DoubaoImeError::protocol("设置地址无效"))?,
        &settings_query(device_id, cdid),
    );
    let response = client
        .post(url)
        .header("User-Agent", USER_AGENT)
        .header("x-ss-stub", X_SS_STUB)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("body=null")
        .send()
        .await
        .map_err(|error| {
            tracing::debug!("豆包输入法 token 请求失败: {}", error.without_url());
            DoubaoImeError::network("获取豆包输入法语音 token 失败，请检查网络后重试")
        })?;
    if !response.status().is_success() {
        return Err(DoubaoImeError::network(format!(
            "获取豆包输入法语音 token 失败（HTTP {}）",
            response.status().as_u16()
        )));
    }
    let value: Value =
        response.json().await.map_err(|_| DoubaoImeError::protocol("豆包输入法 token 响应不是有效 JSON"))?;
    value
        .pointer("/data/settings/asr_config/app_key")
        .or_else(|| value.pointer("/settings/asr_config/app_key"))
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DoubaoImeError::protocol("豆包输入法 token 响应缺少 app_key"))
}

// ---------------- 凭据缓存 ----------------

fn load_credentials(config: &DoubaoImeConfig, path: &Path) -> Option<Credentials> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let token_raw = value.get("token").and_then(Value::as_str)?.to_string();
    let encrypted = value.get("encrypted").and_then(Value::as_bool).unwrap_or(false);
    let token = if encrypted { config.crypto.as_ref()?.decrypt(&token_raw)? } else { token_raw };
    let device_id = value.get("device_id").and_then(Value::as_str).unwrap_or_default().to_string();
    if device_id.is_empty() || token.is_empty() {
        return None;
    }
    Some(Credentials {
        device_id,
        install_id: value.get("install_id").and_then(Value::as_str).unwrap_or_default().to_string(),
        cdid: value.get("cdid").and_then(Value::as_str).unwrap_or_default().to_string(),
        token,
    })
}

fn save_credentials(config: &DoubaoImeConfig, path: &Path, credentials: &Credentials) {
    let (token, encrypted) = match &config.crypto {
        Some(crypto) => (crypto.encrypt(&credentials.token), true),
        None => (credentials.token.clone(), false),
    };
    let value = json!({
        "device_id": credentials.device_id,
        "install_id": credentials.install_id,
        "cdid": credentials.cdid,
        "token": token,
        "encrypted": encrypted,
    });
    if let Err(error) = json_store::write_json_atomic(path, &value) {
        tracing::warn!("保存豆包输入法设备凭据失败：{error}");
    }
}

/// 忽略缓存，强制重新注册一台虚拟设备并取新 token（用于服务端偶发 device 会话失败后的重试）。
async fn refresh_credentials(config: &DoubaoImeConfig) -> Result<Credentials, DoubaoImeError> {
    remove_cached_credentials(config);
    #[cfg(test)]
    if let Some(refresher) = &config.credential_refresher {
        return refresher().await;
    }
    let client = http_client(config.timeout)?;
    let mut credentials = register_device(&client).await?;
    credentials.token = fetch_token(&client, &credentials.device_id, &credentials.cdid).await?;
    if let Some(path) = &config.credential_path {
        save_credentials(config, path, &credentials);
    }
    Ok(credentials)
}

fn remove_cached_credentials(config: &DoubaoImeConfig) {
    let Some(path) = &config.credential_path else { return };
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("删除失效的豆包输入法设备凭据失败：{error}"),
    }
}

static CREDENTIAL_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

async fn ensure_credentials(config: &DoubaoImeConfig) -> Result<Credentials, DoubaoImeError> {
    if let Some(credentials) = &config.credentials {
        return Ok(credentials.clone());
    }
    if let Some(path) = &config.credential_path {
        if let Some(credentials) = load_credentials(config, path) {
            return Ok(credentials);
        }
    }
    let lock = CREDENTIAL_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;
    if let Some(path) = &config.credential_path {
        if let Some(credentials) = load_credentials(config, path) {
            return Ok(credentials);
        }
    }
    let client = http_client(config.timeout)?;
    let mut credentials = register_device(&client).await?;
    credentials.token = fetch_token(&client, &credentials.device_id, &credentials.cdid).await?;
    if let Some(path) = &config.credential_path {
        save_credentials(config, path, &credentials);
    }
    Ok(credentials)
}

// ---------------- WebSocket 会话 ----------------

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = SplitSink<WsStream, Message>;
type WsReader = SplitStream<WsStream>;

fn ws_url(config: &DoubaoImeConfig, credentials: &Credentials) -> Result<reqwest::Url, DoubaoImeError> {
    let mut url =
        reqwest::Url::parse(&config.endpoint).map_err(|_| DoubaoImeError::protocol("豆包输入法语音地址无效"))?;
    url.query_pairs_mut().append_pair("aid", AID).append_pair("device_id", &credentials.device_id);
    Ok(url)
}

async fn connect(config: &DoubaoImeConfig, credentials: &Credentials) -> Result<WsStream, DoubaoImeError> {
    let url = ws_url(config, credentials)?;
    let mut request =
        url.as_str().into_client_request().map_err(|_| DoubaoImeError::protocol("豆包输入法 WebSocket 地址无效"))?;
    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        HeaderValue::from_str(USER_AGENT).map_err(|_| DoubaoImeError::protocol("User-Agent 无效"))?,
    );
    headers.insert("proto-version", HeaderValue::from_static("v2"));
    headers.insert("x-custom-keepalive", HeaderValue::from_static("true"));
    match timeout(config.timeout, tokio_tungstenite::connect_async(request)).await {
        Err(_) => Err(DoubaoImeError::network("连接豆包输入法语音服务超时，请稍后重试")),
        Ok(Ok((stream, _))) => Ok(stream),
        Ok(Err(WsError::Http(response))) => Err(DoubaoImeError::network(format!(
            "豆包输入法语音连接失败（HTTP {}），请稍后重试",
            response.status().as_u16()
        ))),
        Ok(Err(error)) => {
            tracing::debug!("豆包输入法 WebSocket 握手失败: {error}");
            Err(DoubaoImeError::network("无法连接豆包输入法语音服务，请检查网络后重试"))
        }
    }
}

fn session_payload(device_id: &str) -> String {
    json!({
        "audio_info": { "channel": 1, "format": "pcm", "sample_rate": SAMPLE_RATE as u32 },
        "enable_punctuation": true,
        "enable_speech_rejection": false,
        "extra": {
            "app_name": "com.android.chrome",
            "cell_compress_rate": 8,
            "did": device_id,
            "enable_asr_threepass": true,
            "enable_asr_twopass": true,
            "input_mode": "tool"
        }
    })
    .to_string()
}

fn pcm16le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

async fn transcribe_session(
    config: &DoubaoImeConfig,
    credentials: &Credentials,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoImeError> {
    if samples.is_empty() {
        return Err(DoubaoImeError::input("音频为空，无法转写"));
    }
    let mut session = DoubaoImeSession::connect_with(config, credentials, None).await?;
    for (index, frame) in samples.chunks(FRAME_SAMPLES).enumerate() {
        session.push_samples(frame).await?;
        let pushed = ((index + 1) * FRAME_SAMPLES).min(samples.len());
        if !on_push(pushed) {
            session.cancel().await;
            return Err(DoubaoImeError::input(super::TRANSCRIBE_CANCELLED));
        }
    }
    session.finish_result(config.timeout).await
}

async fn transcribe_pcm_with_progress_impl(
    config: &DoubaoImeConfig,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoImeError> {
    config.ensure()?;
    if samples.is_empty() {
        return Err(DoubaoImeError::input("音频为空，无法转写"));
    }
    let mut credentials = ensure_credentials(config).await?;
    let mut refreshed = false;
    loop {
        match run_sessions(config, &credentials, samples, &mut *on_push).await {
            Ok(text) => return Ok(text),
            Err(error) if !refreshed && error.needs_fresh_device() => {
                tracing::warn!("豆包输入法语音会话失败（{}），换一台新虚拟设备重试", error.message);
                refreshed = true;
                credentials = refresh_credentials(config).await?;
            }
            Err(error) => return Err(error.user_facing()),
        }
    }
}

async fn run_sessions(
    config: &DoubaoImeConfig,
    credentials: &Credentials,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoImeError> {
    let mut parts = Vec::new();
    for (index, chunk) in samples.chunks(MAX_SESSION_SAMPLES).enumerate() {
        let base = index * MAX_SESSION_SAMPLES;
        if !on_push(base) {
            return Err(DoubaoImeError::input(super::TRANSCRIBE_CANCELLED));
        }
        let mut chunk_progress = |pushed: usize| on_push(base + pushed);
        let text = transcribe_session(config, credentials, chunk, &mut chunk_progress).await?;
        if !text.is_empty() {
            parts.push(text);
        }
    }
    Ok(parts.join(" ").trim().to_string())
}

/// 整段转写：输入 16k 单声道 PCM16。超长音频按 13 分钟切会话。
pub async fn transcribe_pcm(config: &DoubaoImeConfig, samples: &[i16]) -> Result<String, String> {
    transcribe_pcm_with_progress(config, samples, &mut |_| true).await.map_err(|error| error.to_string())
}

pub async fn transcribe_pcm_with_progress(
    config: &DoubaoImeConfig,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<String, DoubaoImeError> {
    transcribe_pcm_with_progress_impl(config, samples, on_push).await
}

/// 轻量探活：注册/取 token（如果还没缓存）并完成 WebSocket 握手，不发送识别请求。
pub async fn check_connection(config: &DoubaoImeConfig) -> Result<(), DoubaoImeError> {
    config.ensure()?;
    let credentials = ensure_credentials(config).await?;
    let mut stream = connect(config, &credentials).await?;
    let _ =
        timeout(Duration::from_secs(1), stream.close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() })))
            .await;
    Ok(())
}

/// 流式中间结果回调：参数是当前的累计文本。
pub type PartialCallback = Arc<dyn Fn(String) + Send + Sync>;

/// 流式会话：边录边发 20ms PCM 帧，服务端返回的中间稿实时回调给界面。
pub struct DoubaoImeSession {
    writer: WsWriter,
    reader: WsReader,
    timeout: Duration,
    request_id: String,
    token: String,
    device_id: String,
    accumulator: Accumulator,
    frame_index: u32,
    started_at: i64,
    session_finished: bool,
    acked: bool,
    on_partial: Option<PartialCallback>,
}

impl DoubaoImeSession {
    /// 连接并初始化一场流式会话；on_partial 收到的始终是当前累计文本。
    pub async fn connect(
        config: &DoubaoImeConfig,
        _language: &str,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, String> {
        config.ensure().map_err(|error| error.message)?;
        let mut credentials = ensure_credentials(config).await.map_err(|error| error.message)?;
        match Self::connect_with(config, &credentials, on_partial.clone()).await {
            Ok(session) => Ok(session),
            Err(error) if error.needs_fresh_device() => {
                tracing::warn!("豆包输入法流式会话失败（{}），换一台新虚拟设备重试", error.message);
                credentials = refresh_credentials(config).await.map_err(|error| error.message)?;
                Self::connect_with(config, &credentials, on_partial).await.map_err(|error| error.user_facing().message)
            }
            Err(error) => Err(error.user_facing().message),
        }
    }

    async fn connect_with(
        config: &DoubaoImeConfig,
        credentials: &Credentials,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, DoubaoImeError> {
        let stream = connect(config, credentials).await?;
        let (writer, reader) = stream.split();
        let mut session = Self {
            writer,
            reader,
            timeout: config.timeout,
            request_id: Uuid::new_v4().to_string(),
            token: credentials.token.clone(),
            device_id: credentials.device_id.clone(),
            accumulator: Accumulator::default(),
            frame_index: 0,
            started_at: now_ms(),
            session_finished: false,
            acked: false,
            on_partial,
        };
        session.send("StartTask", None, None, None, true).await?;
        session.wait_for("TaskStarted").await?;
        let payload = session_payload(&session.device_id);
        session.send("StartSession", Some(&payload), None, None, true).await?;
        session.wait_for("SessionStarted").await?;
        Ok(session)
    }

    pub fn current_text(&self) -> String {
        self.accumulator.snapshot()
    }

    pub fn acked(&self) -> bool {
        self.acked
    }

    fn early_finish_error(&self) -> DoubaoImeError {
        tracing::warn!(
            pushed_frames = self.frame_index,
            elapsed_ms = now_ms().saturating_sub(self.started_at),
            "豆包输入法流式服务端提前结束了会话"
        );
        DoubaoImeError::server("豆包输入法服务端提前结束了本次会话，已保存之前的识别内容")
    }

    async fn send(
        &mut self,
        method: &str,
        payload: Option<&str>,
        audio: Option<&[u8]>,
        frame_state: Option<u32>,
        with_token: bool,
    ) -> Result<(), DoubaoImeError> {
        let token = if with_token { Some(self.token.as_str()) } else { None };
        let packet = encode_request(token, method, payload, audio, &self.request_id, frame_state);
        match timeout(self.timeout, self.writer.send(Message::binary(packet))).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                tracing::debug!("发送豆包输入法流式请求失败: {error}");
                Err(DoubaoImeError::network("发送豆包输入法语音请求失败，请重试"))
            }
            Err(_) => Err(DoubaoImeError::network("发送豆包输入法语音请求超时，请稍后重试")),
        }
    }

    /// 发送音频帧时同时读取已经到达的服务端响应；写缓冲反压期间仍持续排空读缓冲。
    async fn send_task_request(
        &mut self,
        method: &str,
        payload: Option<&str>,
        audio: &[u8],
        frame_state: u32,
    ) -> Result<(), DoubaoImeError> {
        let packet = encode_request(None, method, payload, Some(audio), &self.request_id, Some(frame_state));
        let send = self.writer.send(Message::binary(packet));
        tokio::pin!(send);
        let mut responses = Vec::new();
        let outcome = timeout(self.timeout, async {
            loop {
                tokio::select! {
                    result = &mut send => {
                        result.map_err(|error| {
                            tracing::debug!("发送豆包输入法流式请求失败: {error}");
                            DoubaoImeError::network("发送豆包输入法语音请求失败，请重试")
                        })?;
                        break;
                    }
                    message = self.reader.next() => match message {
                        None => {
                            return Err(DoubaoImeError::network(
                                "豆包输入法语音连接在会话结束前关闭，请重试",
                            ));
                        }
                        Some(Ok(Message::Binary(data))) => responses.push(parse_response(&data)?),
                        Some(Ok(Message::Close(_))) => {
                            return Err(DoubaoImeError::network(
                                "豆包输入法语音连接在会话结束前关闭，请重试",
                            ));
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            tracing::debug!("读取豆包输入法流式响应失败: {error}");
                            return Err(DoubaoImeError::network("读取豆包输入法语音响应失败，请重试"));
                        }
                    },
                }
            }
            Ok::<(), DoubaoImeError>(())
        })
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(DoubaoImeError::network("发送豆包输入法语音请求超时，请稍后重试")),
        }
        for parsed in responses {
            let _ = self.handle(parsed)?;
        }
        Ok(())
    }

    fn emit(&self) {
        if let Some(callback) = &self.on_partial {
            callback(self.accumulator.snapshot());
        }
    }

    fn handle(&mut self, parsed: Parsed) -> Result<Option<&'static str>, DoubaoImeError> {
        match parsed {
            Parsed::TaskStarted => {
                self.acked = true;
                Ok(Some("TaskStarted"))
            }
            Parsed::SessionStarted => {
                self.acked = true;
                Ok(Some("SessionStarted"))
            }
            Parsed::SessionFinished => {
                self.acked = true;
                self.session_finished = true;
                Ok(Some("SessionFinished"))
            }
            Parsed::VadStart => {
                self.accumulator.vad_start();
                self.emit();
                Ok(None)
            }
            Parsed::Interim(text) => {
                self.acked = true;
                self.accumulator.interim(text);
                self.emit();
                Ok(None)
            }
            Parsed::Final(text) => {
                self.acked = true;
                self.accumulator.final_result(text);
                self.emit();
                Ok(None)
            }
            Parsed::Heartbeat | Parsed::Other(_) => {
                self.acked = true;
                Ok(None)
            }
            Parsed::Failed(_) => {
                self.acked = true;
                Err(DoubaoImeError::server("豆包输入法语音服务返回失败，请稍后重试"))
            }
        }
    }

    async fn next_parsed(&mut self, deadline: Instant) -> Result<Parsed, DoubaoImeError> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DoubaoImeError::network("等待豆包输入法语音响应超时，请稍后重试"));
            }
            match timeout(remaining, self.reader.next()).await {
                Err(_) => return Err(DoubaoImeError::network("等待豆包输入法语音响应超时，请稍后重试")),
                Ok(None) if self.session_finished => return Ok(Parsed::SessionFinished),
                Ok(None) => return Err(DoubaoImeError::network("豆包输入法语音连接在会话结束前关闭，请重试")),
                Ok(Some(Ok(Message::Binary(data)))) => return parse_response(&data),
                Ok(Some(Ok(Message::Close(_)))) if self.session_finished => {
                    return Ok(Parsed::SessionFinished);
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    return Err(DoubaoImeError::network("豆包输入法语音连接在会话结束前关闭，请重试"));
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(_))) if self.session_finished => return Ok(Parsed::SessionFinished),
                Ok(Some(Err(error))) => {
                    tracing::debug!("读取豆包输入法流式响应失败: {error}");
                    return Err(DoubaoImeError::network("读取豆包输入法语音响应失败，请重试"));
                }
            }
        }
    }

    async fn wait_for(&mut self, expected: &str) -> Result<(), DoubaoImeError> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let parsed = self.next_parsed(deadline).await?;
            let processed = self.handle(parsed);
            let tag = match processed {
                Ok(tag) => tag,
                Err(error) => return Err(error.retry_with_fresh_device()),
            };
            if let Some(tag) = tag {
                if tag == expected {
                    return Ok(());
                }
            }
        }
    }

    /// 把服务端已经到达的响应全部非阻塞读完，实时更新累计文本。
    fn drain_ready(&mut self) -> Result<(), DoubaoImeError> {
        loop {
            match self.reader.next().now_or_never() {
                None => return Ok(()),
                Some(None) if self.session_finished => return Ok(()),
                Some(None) => {
                    return Err(DoubaoImeError::network("豆包输入法语音连接在会话结束前关闭，请重试"));
                }
                Some(Some(Ok(Message::Binary(data)))) => {
                    let parsed = parse_response(&data)?;
                    let _ = self.handle(parsed)?;
                }
                Some(Some(Ok(Message::Close(_)))) if self.session_finished => return Ok(()),
                Some(Some(Ok(Message::Close(_)))) => {
                    return Err(DoubaoImeError::network("豆包输入法语音连接在会话结束前关闭，请重试"));
                }
                Some(Some(Ok(_))) => continue,
                Some(Some(Err(_))) if self.session_finished => return Ok(()),
                Some(Some(Err(error))) => {
                    tracing::debug!("读取豆包输入法流式响应失败: {error}");
                    return Err(DoubaoImeError::network("读取豆包输入法语音响应失败，请重试"));
                }
            }
        }
    }

    /// 边录边推一段 PCM（16k/mono/PCM16），并即时把中间稿回调出去。
    pub async fn push(&mut self, samples: &[i16]) -> Result<(), String> {
        self.push_samples(samples).await.map_err(|error| error.user_facing().message)
    }

    async fn push_samples(&mut self, samples: &[i16]) -> Result<(), DoubaoImeError> {
        if samples.is_empty() {
            return Ok(());
        }
        self.drain_ready()?;
        if self.session_finished {
            return Err(self.early_finish_error());
        }
        let total = samples.len().div_ceil(FRAME_SAMPLES);
        for index in 0..total {
            let begin = index * FRAME_SAMPLES;
            let end = (begin + FRAME_SAMPLES).min(samples.len());
            let mut owned;
            let chunk = if end - begin < FRAME_SAMPLES {
                owned = samples[begin..end].to_vec();
                owned.resize(FRAME_SAMPLES, 0);
                owned.as_slice()
            } else {
                &samples[begin..end]
            };
            let frame_state = if self.frame_index == 0 { FRAME_FIRST } else { FRAME_MIDDLE };
            let metadata =
                json!({ "extra": {}, "timestamp_ms": self.started_at + (self.frame_index as i64) * 20 }).to_string();
            let bytes = pcm16le_bytes(chunk);
            self.send_task_request("TaskRequest", Some(&metadata), &bytes, frame_state).await?;
            self.frame_index += 1;
        }
        self.drain_ready()?;
        if self.session_finished {
            return Err(self.early_finish_error());
        }
        Ok(())
    }

    /// 结束会话：补一个 LAST 帧 → FinishSession → 读到底，返回整段文本。
    pub async fn finish(self, timeout: Duration) -> Result<String, String> {
        self.finish_result(timeout).await.map_err(|error| error.user_facing().message)
    }

    async fn finish_result(mut self, timeout: Duration) -> Result<String, DoubaoImeError> {
        if self.session_finished {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                self.writer.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() }))),
            )
            .await;
            return self.finish_accumulator();
        }
        let deadline = Instant::now() + timeout + Duration::from_secs(3);
        let silent = vec![0i16; FRAME_SAMPLES];
        let bytes = pcm16le_bytes(&silent);
        let metadata =
            json!({ "extra": {}, "timestamp_ms": self.started_at + (self.frame_index as i64) * 20 }).to_string();
        let _ = self.send("TaskRequest", Some(&metadata), Some(&bytes), Some(FRAME_LAST), false).await;
        self.send("FinishSession", None, None, None, true).await?;
        loop {
            let parsed = self.next_parsed(deadline).await?;
            if let Some(tag) = self.handle(parsed)? {
                if tag == "SessionFinished" {
                    break;
                }
            }
        }
        self.finish_accumulator()
    }

    fn finish_accumulator(mut self) -> Result<String, DoubaoImeError> {
        let text = self.accumulator.finish();
        if text.is_empty() {
            return Err(DoubaoImeError::server("豆包输入法语音会话已结束，但没有识别结果"));
        }
        Ok(text)
    }

    pub async fn cancel(mut self) {
        let _ = self.send("FinishSession", None, None, None, true).await;
        let _ = timeout(
            Duration::from_secs(1),
            self.writer.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() }))),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::collections::BTreeMap;
    use tokio::sync::oneshot;

    fn text_response(message_type: &str, status_message: &str, result_json: Option<&str>) -> Vec<u8> {
        let mut packet = Vec::new();
        push_bytes_field(&mut packet, 4, message_type.as_bytes());
        if !status_message.is_empty() {
            push_bytes_field(&mut packet, 6, status_message.as_bytes());
        }
        if let Some(result) = result_json {
            push_bytes_field(&mut packet, 7, result.as_bytes());
        }
        packet
    }

    fn decode_fields(data: &[u8]) -> BTreeMap<u32, Vec<u8>> {
        let mut fields = BTreeMap::new();
        let mut offset = 0;
        while offset < data.len() {
            let tag = read_varint(data, &mut offset).unwrap();
            let number = (tag >> 3) as u32;
            let wire = (tag & 7) as u8;
            match wire {
                0 => {
                    let value = read_varint(data, &mut offset).unwrap();
                    fields.insert(number, value.to_le_bytes().to_vec());
                }
                2 => {
                    let length = usize::try_from(read_varint(data, &mut offset).unwrap()).unwrap();
                    let end = offset + length;
                    fields.insert(number, data[offset..end].to_vec());
                    offset = end;
                }
                _ => panic!("unsupported wire"),
            }
        }
        fields
    }

    #[test]
    fn 请求编码_方法_token_与帧状态() {
        let start = encode_request(Some("tk"), "StartTask", None, None, "rid", None);
        let fields = decode_fields(&start);
        assert_eq!(fields.get(&2).map(Vec::as_slice), Some(&b"tk"[..]));
        assert_eq!(fields.get(&3).map(Vec::as_slice), Some(&b"ASR"[..]));
        assert_eq!(fields.get(&5).map(Vec::as_slice), Some(&b"StartTask"[..]));
        assert_eq!(fields.get(&8).map(Vec::as_slice), Some(&b"rid"[..]));
        assert!(!fields.contains_key(&9));

        let frame = encode_request(None, "TaskRequest", Some("{}"), Some(b"pcm"), "rid", Some(FRAME_LAST));
        let fields = decode_fields(&frame);
        assert!(!fields.contains_key(&2), "TaskRequest 不带 token");
        assert_eq!(fields.get(&7).map(Vec::as_slice), Some(&b"pcm"[..]));
        assert_eq!(fields.get(&9).map(Vec::as_slice), Some((FRAME_LAST as u64).to_le_bytes().as_slice()));
    }

    #[test]
    fn 响应解析_中间稿_最终稿_失败() {
        let interim =
            text_response("ASRResponse", "", Some(r#"{"results":[{"text":"你好","is_interim":true}],"extra":{}}"#));
        assert_eq!(parse_response(&interim).unwrap(), Parsed::Interim("你好".into()));
        let final_frame = text_response(
            "ASRResponse",
            "",
            Some(r#"{"results":[{"text":"你好世界。","is_interim":false,"is_vad_finished":true}],"extra":{}}"#),
        );
        assert_eq!(parse_response(&final_frame).unwrap(), Parsed::Final("你好世界。".into()));
        let failed = text_response("SessionFailed", "鉴权失败", None);
        assert_eq!(parse_response(&failed).unwrap(), Parsed::Failed("SessionFailed".into()));
        let heartbeat = text_response("ASRResponse", "", Some(r#"{"extra":{"packet_number":1}}"#));
        assert_eq!(parse_response(&heartbeat).unwrap(), Parsed::Heartbeat);
    }

    #[test]
    fn 累积_最终稿延续中间稿时只取最终稿() {
        let mut accumulator = Accumulator::default();
        accumulator.interim("今天天气不错".into());
        accumulator.interim("今天天气不错，我们一起去公园散步".into());
        accumulator.final_result("今天天气不错，我们一起去公园散步吧。".into());
        assert_eq!(accumulator.finish(), "今天天气不错，我们一起去公园散步吧。");
    }

    #[test]
    fn 累积_新段中间稿无公共前缀时保留上一段() {
        let mut accumulator = Accumulator::default();
        accumulator.interim("好的。".into());
        accumulator.interim("我们明天下午三点开会".into());
        accumulator.final_result("我们明天下午三点开会。".into());
        assert_eq!(accumulator.finish(), "好的。我们明天下午三点开会。");
    }

    #[test]
    fn 累积_同长度修订中间稿时使用最新内容() {
        let mut accumulator = Accumulator::default();
        accumulator.interim("今天天汽不错".into());
        accumulator.interim("今天天气不错".into());
        accumulator.final_result("今天天气不错。".into());
        assert_eq!(accumulator.finish(), "今天天气不错。");
    }

    #[test]
    fn 累积_最终稿只是末段短句时补上前面中间稿() {
        let mut accumulator = Accumulator::default();
        let long = "第一句话，今天天气不错。第二句话，明天开会。第五句话，谢谢大家的配合，我们。";
        accumulator.interim(long.to_string());
        accumulator.interim("明天".into());
        accumulator.final_result("明天见。".into());
        assert_eq!(accumulator.finish(), format!("{long}明天见。"));
    }

    fn response_with_interim(text: &str) -> Vec<u8> {
        let payload = json!({ "results": [{ "text": text, "is_interim": true }], "extra": {} }).to_string();
        text_response("ASRResponse", "", Some(&payload))
    }

    fn response_with_final(text: &str) -> Vec<u8> {
        let payload =
            json!({ "results": [{ "text": text, "is_interim": false, "is_vad_finished": true }], "extra": {} })
                .to_string();
        text_response("ASRResponse", "", Some(&payload))
    }

    #[derive(Default, Clone, Copy)]
    struct StubBehavior {
        final_extends: bool,
        fail_midstream: bool,
        large_interim: bool,
        silent: bool,
        finish_after_frames: Option<usize>,
    }

    async fn stub_server(behavior: StubBehavior) -> (String, oneshot::Receiver<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut methods = Vec::new();
            let large_interim = format!("{}。", "测".repeat(1300));
            if behavior.silent {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = tx.send(methods);
                return;
            }
            let mut frames = 0;
            while let Some(message) = ws.next().await {
                let Ok(Message::Binary(data)) = message else { break };
                let fields = decode_fields(&data);
                let method = String::from_utf8(fields.get(&5).cloned().unwrap_or_default()).unwrap_or_default();
                methods.push(method.clone());
                match method.as_str() {
                    "StartTask" => {
                        let _ = ws.send(Message::binary(text_response("TaskStarted", "", None))).await;
                    }
                    "StartSession" => {
                        let _ = ws.send(Message::binary(text_response("SessionStarted", "", None))).await;
                    }
                    "TaskRequest" => {
                        if behavior.fail_midstream {
                            let _ = ws
                                .send(Message::binary(text_response(
                                    "SessionFailed",
                                    "upstream-secret device_id=fake&cdid=also-fake",
                                    None,
                                )))
                                .await;
                            break;
                        }
                        frames += 1;
                        let text = if behavior.large_interim {
                            large_interim.clone()
                        } else if frames == 1 {
                            "你好".into()
                        } else {
                            "你好世界".into()
                        };
                        let _ = ws.send(Message::binary(response_with_interim(&text))).await;
                        if behavior.finish_after_frames == Some(frames) {
                            let _ = ws.send(Message::binary(text_response("SessionFinished", "", None))).await;
                            break;
                        }
                    }
                    "FinishSession" => {
                        if behavior.large_interim {
                            let _ = ws.send(Message::binary(response_with_final("结束。"))).await;
                        } else if behavior.final_extends {
                            let _ = ws.send(Message::binary(response_with_final("你好世界。"))).await;
                        } else {
                            let _ = ws.send(Message::binary(response_with_interim("很长的前面内容。"))).await;
                            let _ = ws.send(Message::binary(response_with_final("明天见。"))).await;
                        }
                        let _ = ws.send(Message::binary(text_response("SessionFinished", "", None))).await;
                        break;
                    }
                    _ => {}
                }
            }
            let _ = tx.send(methods);
        });
        (format!("ws://{address}"), rx)
    }

    /// 前一场在 StartTask / StartSession 失败，第二场正常结束；用于验证启动阶段自动换设备。
    async fn startup_failure_stub(
        fail_event: &'static str,
        succeed_after_retry: bool,
    ) -> (String, oneshot::Receiver<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut methods = Vec::new();
            for attempt in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(Message::Binary(data))) = ws.next().await {
                    let fields = decode_fields(&data);
                    let method = String::from_utf8(fields.get(&5).cloned().unwrap_or_default()).unwrap_or_default();
                    methods.push(method.clone());
                    match method.as_str() {
                        "StartTask" if (attempt == 0 || !succeed_after_retry) && fail_event == "TaskFailed" => {
                            let _ = ws
                                .send(Message::binary(text_response(
                                    "TaskFailed",
                                    "upstream-secret device_id=fake&cdid=also-fake",
                                    None,
                                )))
                                .await;
                            break;
                        }
                        "StartTask" => {
                            let _ = ws.send(Message::binary(text_response("TaskStarted", "", None))).await;
                        }
                        "StartSession" if attempt == 0 || !succeed_after_retry => {
                            let _ = ws
                                .send(Message::binary(text_response(
                                    "SessionFailed",
                                    "upstream-secret device_id=fake&cdid=also-fake",
                                    None,
                                )))
                                .await;
                            break;
                        }
                        "StartSession" => {
                            let _ = ws.send(Message::binary(text_response("SessionStarted", "", None))).await;
                        }
                        "TaskRequest" => {
                            let _ = ws.send(Message::binary(response_with_interim("你好世界"))).await;
                        }
                        "FinishSession" => {
                            let _ = ws.send(Message::binary(response_with_final("你好世界。"))).await;
                            let _ = ws.send(Message::binary(text_response("SessionFinished", "", None))).await;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            let _ = tx.send(methods);
        });
        (format!("ws://{address}"), rx)
    }

    fn test_refresher(calls: Arc<std::sync::atomic::AtomicUsize>) -> Arc<dyn Fn() -> RefreshFuture + Send + Sync> {
        Arc::new(move || {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(Credentials {
                    device_id: "fresh-device".into(),
                    install_id: "fresh-install".into(),
                    cdid: "fresh-cdid".into(),
                    token: "fresh-token".into(),
                })
            })
        })
    }

    fn test_config(endpoint: String) -> DoubaoImeConfig {
        DoubaoImeConfig {
            timeout: Duration::from_secs(5),
            endpoint,
            credential_path: None,
            crypto: None,
            credentials: Some(Credentials {
                device_id: "123456".into(),
                install_id: "1".into(),
                cdid: "c".into(),
                token: "tk".into(),
            }),
            credential_refresher: None,
        }
    }

    #[tokio::test]
    async fn 会话_中间稿与最终稿_成功拼接() {
        let (endpoint, methods_rx) = stub_server(StubBehavior { final_extends: true, ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let samples = vec![0i16; FRAME_SAMPLES * 3];
        let text = transcribe_pcm(&config, &samples).await.unwrap();
        assert_eq!(text, "你好世界。");
        let methods = timeout(Duration::from_secs(2), methods_rx).await.unwrap().unwrap();
        assert_eq!(methods.first().map(String::as_str), Some("StartTask"));
        assert_eq!(methods.last().map(String::as_str), Some("FinishSession"));
    }

    #[tokio::test]
    async fn 会话_末段短_final_时保留前面中间稿() {
        let (endpoint, _methods_rx) =
            stub_server(StubBehavior { final_extends: false, ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let samples = vec![0i16; FRAME_SAMPLES * 2];
        let text = transcribe_pcm(&config, &samples).await.unwrap();
        assert_eq!(text, "你好世界很长的前面内容。明天见。");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 批量_长静音_逐帧大响应_边推边读不会互等() {
        let (endpoint, _methods_rx) =
            stub_server(StubBehavior { large_interim: true, ..StubBehavior::default() }).await;
        let mut config = test_config(endpoint);
        config.timeout = Duration::from_secs(30);
        let samples = vec![0i16; MAX_SESSION_SAMPLES];
        let text = timeout(Duration::from_secs(120), transcribe_pcm(&config, &samples))
            .await
            .expect("批量识别在收发互等时挂起")
            .expect("批量识别失败");
        assert!(text.ends_with("结束。"), "结果长度={}", text.len());
    }

    #[tokio::test]
    async fn 启动会话失败_删除旧凭据并换设备重试一次() {
        let (endpoint, methods_rx) = startup_failure_stub("SessionFailed", true).await;
        let mut config = test_config(endpoint);
        let dir = std::env::temp_dir().join(format!("att-doubao-ime-refresh-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let credential_path = dir.join("credentials.json");
        std::fs::write(&credential_path, "{}").unwrap();
        config.credential_path = Some(credential_path.clone());
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        config.credential_refresher = Some(test_refresher(refreshes.clone()));

        let text = transcribe_pcm(&config, &vec![0i16; FRAME_SAMPLES]).await.unwrap();
        assert_eq!(text, "你好世界。");
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!credential_path.exists(), "换设备前必须先删掉失效缓存");
        let methods = timeout(Duration::from_secs(2), methods_rx).await.unwrap().unwrap();
        assert_eq!(methods.iter().filter(|method| *method == "StartTask").count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 启动任务失败_同样换设备并重试一次() {
        let (endpoint, _methods_rx) = startup_failure_stub("TaskFailed", true).await;
        let mut config = test_config(endpoint);
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        config.credential_refresher = Some(test_refresher(refreshes.clone()));
        let text = transcribe_pcm(&config, &vec![0i16; FRAME_SAMPLES]).await.unwrap();
        assert_eq!(text, "你好世界。");
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn 换设备后仍失败_只重试一次且返回脱敏错误() {
        let (endpoint, methods_rx) = startup_failure_stub("SessionFailed", false).await;
        let mut config = test_config(endpoint);
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        config.credential_refresher = Some(test_refresher(refreshes.clone()));
        let error = transcribe_pcm(&config, &vec![0i16; FRAME_SAMPLES]).await.unwrap_err();
        assert!(error.contains("豆包输入法语音服务返回失败"), "{error}");
        assert!(!error.contains("upstream-secret"), "{error}");
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let methods = timeout(Duration::from_secs(2), methods_rx).await.unwrap().unwrap();
        assert_eq!(methods.iter().filter(|method| *method == "StartTask").count(), 2);
    }

    #[tokio::test]
    async fn 流式中途失败_返回文案不含上游原文() {
        let (endpoint, _methods_rx) =
            stub_server(StubBehavior { fail_midstream: true, ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let mut session = DoubaoImeSession::connect(&config, "zh", None).await.unwrap();
        session.push(&vec![0i16; FRAME_SAMPLES]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let error = session.drain_ready().unwrap_err().user_facing().message;
        assert!(error.contains("豆包输入法语音服务返回失败"), "{error}");
        assert!(!error.contains("upstream-secret"), "{error}");
        assert!(!error.contains("device_id"), "{error}");
    }

    #[test]
    fn 日志脱敏_移除设备参数值() {
        let redacted = redact_device_identifiers("request failed: ws://host/ws?device_id=secret-1&cdid=secret-2 other");
        assert!(!redacted.contains("device_id="), "{redacted}");
        assert!(!redacted.contains("cdid="), "{redacted}");
        assert!(!redacted.contains("secret-1"), "{redacted}");
        assert!(!redacted.contains("secret-2"), "{redacted}");
    }

    #[tokio::test]
    async fn 会话_静默超时_返回可执行提示() {
        let (endpoint, _methods_rx) = stub_server(StubBehavior { silent: true, ..StubBehavior::default() }).await;
        let mut config = test_config(endpoint);
        config.timeout = Duration::from_millis(150);
        let error = transcribe_pcm(&config, &vec![0i16; FRAME_SAMPLES]).await.unwrap_err();
        assert!(error.contains("超时"), "{error}");
    }

    #[test]
    fn 服务发现失败_判定为可换设备重试并给友好文案() {
        let error = DoubaoImeError::server(
            "豆包输入法语音服务返回失败：read backend response: rpc error: code = 2 desc = service discovery failure",
        )
        .retry_with_fresh_device();
        assert!(error.needs_fresh_device());
        assert!(error.user_facing().message.contains("暂时不可用"));
    }

    #[tokio::test]
    async fn 流式会话_推流中间稿与最终文本() {
        let (endpoint, _methods_rx) =
            stub_server(StubBehavior { final_extends: true, ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let partials = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = partials.clone();
        let callback: PartialCallback = Arc::new(move |text| {
            sink.lock().unwrap().push(text);
        });
        let mut session = DoubaoImeSession::connect(&config, "zh", Some(callback)).await.expect("连接流式会话失败");
        session.push(&vec![0i16; FRAME_SAMPLES * 2]).await.expect("推流失败");
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = session.drain_ready();
        assert!(session.current_text().contains("你好"), "current={}", session.current_text());
        let text = session.finish(Duration::from_secs(2)).await.expect("结束流式会话失败");
        assert_eq!(text, "你好世界。");
        assert!(!partials.lock().unwrap().is_empty(), "应至少回调一次中间稿");
    }

    #[tokio::test]
    async fn 流式会话_推流中途服务端结束_立即报错并保留中间稿() {
        let (endpoint, methods_rx) =
            stub_server(StubBehavior { finish_after_frames: Some(1), ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let mut session = DoubaoImeSession::connect(&config, "zh", None).await.unwrap();

        session.push(&vec![0i16; FRAME_SAMPLES]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let error = session.push(&vec![0i16; FRAME_SAMPLES]).await.expect_err("服务端提前结束时应停止推流");

        assert!(error.contains("服务端提前结束了本次会话"), "{error}");
        assert_eq!(session.current_text(), "你好");
        let methods = timeout(Duration::from_secs(2), methods_rx).await.unwrap().unwrap();
        assert_eq!(methods.iter().filter(|method| *method == "TaskRequest").count(), 1);
        assert!(!methods.iter().any(|method| method == "FinishSession"));
    }

    #[tokio::test]
    async fn 流式会话_收尾前服务端已结束_直接返回累积文本() {
        let (endpoint, methods_rx) =
            stub_server(StubBehavior { finish_after_frames: Some(1), ..StubBehavior::default() }).await;
        let config = test_config(endpoint);
        let mut session = DoubaoImeSession::connect(&config, "zh", None).await.unwrap();
        let bytes = pcm16le_bytes(&vec![0i16; FRAME_SAMPLES]);
        let metadata = json!({ "extra": {}, "timestamp_ms": session.started_at }).to_string();
        session.send_task_request("TaskRequest", Some(&metadata), &bytes, FRAME_MIDDLE).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        session.drain_ready().unwrap();
        assert!(session.session_finished);
        assert_eq!(session.current_text(), "你好");

        let started = Instant::now();
        let text = session.finish(Duration::from_secs(2)).await.unwrap();

        assert_eq!(text, "你好");
        assert!(started.elapsed() < Duration::from_secs(1), "收尾不应等待超时");
        let methods = timeout(Duration::from_secs(2), methods_rx).await.unwrap().unwrap();
        assert!(!methods.iter().any(|method| method == "FinishSession"));
    }

    /// 真实流式链路（默认 ignore）：ATT_TEST_WAV=/abs/16k-mono.wav cargo test -p att-core 真实豆包输入法流式 -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn 真实豆包输入法流式_已知音频() {
        let path = std::env::var("ATT_TEST_WAV").expect("需要 ATT_TEST_WAV 指向 16k 单声道音频");
        let samples = crate::audio::read_audio_16k_mono(std::path::Path::new(&path)).expect("读取测试音频失败");
        let dir = std::env::temp_dir().join(format!("att-doubao-ime-stream-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config =
            DoubaoImeConfig::from_seconds(DEFAULT_TIMEOUT_SECONDS).with_credential_path(dir.join("credentials.json"));
        let partials = Arc::new(std::sync::Mutex::new(0usize));
        let sink = partials.clone();
        let callback: PartialCallback = Arc::new(move |text| {
            if !text.is_empty() {
                *sink.lock().unwrap() += 1;
            }
        });
        let mut session = DoubaoImeSession::connect(&config, "zh", Some(callback)).await.expect("流式建连失败");
        let chunk = SAMPLE_RATE / 10;
        for piece in samples.chunks(chunk) {
            session.push(piece).await.expect("流式推流失败");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let text = session.finish(Duration::from_secs(15)).await.expect("流式收尾失败");
        println!("豆包输入法流式结果：{text}");
        assert!(!text.trim().is_empty(), "流式结果为空");
        assert!(*partials.lock().unwrap() > 0, "应至少回调一次中间稿");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实链路测试（默认 ignore）：ATT_TEST_WAV=/abs/16k-mono.wav cargo test -p att-core 真实豆包输入法 -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn 真实豆包输入法_已知中文音频返回文本() {
        let path = std::env::var("ATT_TEST_WAV").expect("需要 ATT_TEST_WAV 指向 16k 单声道音频");
        let samples = crate::audio::read_audio_16k_mono(std::path::Path::new(&path)).expect("读取测试音频失败");
        let dir = std::env::temp_dir().join(format!("att-doubao-ime-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config =
            DoubaoImeConfig::from_seconds(DEFAULT_TIMEOUT_SECONDS).with_credential_path(dir.join("credentials.json"));
        let text = transcribe_pcm(&config, &samples).await.expect("豆包输入法真实识别失败");
        println!("豆包输入法识别结果：{text}");
        assert!(!text.trim().is_empty(), "识别结果为空");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
