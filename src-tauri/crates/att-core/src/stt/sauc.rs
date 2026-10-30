//! 火山引擎豆包双向流式 ASR（SAUC 二进制协议）编解码层。
//!
//! 逐项移植自 Python `app/stt/providers/volcengine_sauc.py`（纯函数，便于单测）：
//! 1. 请求体构造（`build_bigmodel_request` / `build_dialogue_context_json`）；
//! 2. 帧编解码（`build_full_client_request` / `build_audio_request` / `parse_server_frame`）；
//! 3. 结果推进与错误归一化（`advance_definite_prefix` / `classify_*`）。
//!
//! 协议要点：
//! - 4 字节头：byte0 = 版本(高 4 位)|头长度(低 4 位)，byte1 = 消息类型|flags，
//!   byte2 = 序列化|压缩，byte3 保留；
//! - 客户端帧体：`[payload_size u32 BE][payload]`；
//! - 服务端结果帧体：`[sequence i32?][payload_size u32 BE][payload(JSON)]`；
//! - 服务端错误帧体：`[错误码 u32 BE][信息长度 u32 BE][信息 UTF-8]`（兼容头后多一个字节的形态）。

use serde_json::{json, Value};
use std::io::{Read, Write};

pub const DEFAULT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
pub const DEFAULT_RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
pub const MODEL_NAME: &str = "bigmodel";

pub const SAMPLE_RATE: u32 = 16_000;
/// 200ms / 16kHz / 单声道 = 3200 采样 = 6400 字节
pub const CHUNK_SAMPLES: usize = 3200;
/// corpus.context 上限（按码点）
pub const ASR_CONTEXT_MAX_CHARS: usize = 800;

const PROTOCOL_VERSION: u8 = 0b0001;
const HEADER_SIZE_UNITS: u8 = 0b0001;
const HEADER_BYTES: usize = 4;

const MSG_FULL_CLIENT_REQUEST: u8 = 0b0001;
const MSG_AUDIO_ONLY_REQUEST: u8 = 0b0010;
const MSG_FULL_SERVER_RESPONSE: u8 = 0b1001;
const MSG_AUDIO_ONLY_RESPONSE: u8 = 0b1011;
const MSG_SERVER_ERROR_RESPONSE: u8 = 0b1111;

const FLAG_NO_SEQUENCE: u8 = 0b0000;
const FLAG_POS_SEQUENCE: u8 = 0b0001;
const FLAG_LAST_NO_SEQUENCE: u8 = 0b0010;
const FLAG_LAST_NEG_SEQUENCE: u8 = 0b0011;
/// flags 的 bit1 表示「末包」
const FLAG_LAST_MASK: u8 = 0b0010;

const SERIALIZATION_RAW: u8 = 0b0000;
const SERIALIZATION_JSON: u8 = 0b0001;
const COMPRESSION_NONE: u8 = 0b0000;
const COMPRESSION_GZIP: u8 = 0b0001;

// ==================== 数据结构 ====================

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Utterance {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub definite: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultFrame {
    pub text: String,
    pub utterances: Vec<Utterance>,
    pub audio_duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    Result { is_last: bool, result: ResultFrame },
    Error { code: u32, message: String },
    /// 结构异常或空载荷：调用方必须静默忽略，不能因此断流
    Unknown,
}

/// 错误归一化的类别（与前端「去修复 / 立即重试」两类出口对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    None,
    Auth,
    Quota,
    Server,
    Network,
    Protocol,
    Input,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorInfo {
    pub kind: ErrorKind,
    pub message: String,
    pub retryable: bool,
    pub next_action: String,
}

impl ErrorInfo {
    pub fn new(kind: ErrorKind, message: impl Into<String>, retryable: bool, next_action: &str) -> Self {
        Self { kind, message: message.into(), retryable, next_action: next_action.to_string() }
    }

    /// 拼成可直接展示给用户的一句话。
    pub fn to_message(&self) -> String {
        if self.next_action.is_empty() {
            self.message.clone()
        } else {
            format!("{}（{}）", self.message, self.next_action)
        }
    }
}

// ==================== 请求体构造 ====================

/// 按码点截尾：越靠近当前句的上下文偏置越强。
pub fn truncate_context_text(context: &str, max_chars: usize) -> String {
    let count = context.chars().count();
    if count <= max_chars {
        return context.to_string();
    }
    context.chars().skip(count - max_chars).collect()
}

/// `corpus.context` 的值是 **JSON 字符串**；直接塞纯文本会被服务端判 45000001 断连。
pub fn build_dialogue_context_json(context: &str) -> String {
    let trimmed = truncate_context_text(context, ASR_CONTEXT_MAX_CHARS);
    if trimmed.is_empty() {
        return String::new();
    }
    json!({ "context_type": "dialog_ctx", "context_data": [{ "text": trimmed }] }).to_string()
}

/// 应用内短语言码 → 协议取值（BCP-47）；空串表示不传（自动识别）。
pub fn normalize_language(language: &str) -> String {
    let normalized = language.trim();
    if normalized.is_empty() {
        return "zh-CN".into();
    }
    match normalized.to_lowercase().as_str() {
        "zh" | "zh-cn" | "cn" => "zh-CN".into(),
        "en" | "en-us" => "en-US".into(),
        "ja" => "ja-JP".into(),
        "ko" => "ko-KR".into(),
        "yue" => "yue-CN".into(),
        "auto" => String::new(),
        _ => normalized.to_string(),
    }
}

/// 全量客户端请求体。`result_type=full` + `show_utterances=true` 必须同时满足
/// （definite 判据依赖句级信息，single 模式会静默丢句）。
pub fn build_bigmodel_request(uid: &str, language: &str, context: &str, end_window_size: i64) -> Value {
    let mut request = json!({
        "model_name": MODEL_NAME,
        "enable_itn": true,
        "enable_punc": true,
        "enable_ddc": false,
        "result_type": "full",
        "show_utterances": true,
    });
    if end_window_size > 0 {
        request["end_window_size"] = json!(end_window_size);
        request["force_to_speech_time"] = json!(0);
    }
    let context_json = build_dialogue_context_json(context);
    if !context_json.is_empty() {
        request["corpus"] = json!({ "context": context_json });
    }
    let mut audio = json!({ "format": "pcm", "rate": SAMPLE_RATE, "bits": 16, "channel": 1 });
    let language = normalize_language(language);
    if !language.is_empty() {
        audio["language"] = json!(language);
    }
    let mut payload = json!({ "audio": audio, "request": request });
    if !uid.is_empty() {
        payload["user"] = json!({ "uid": uid });
    }
    payload
}

// ==================== 帧编解码 ====================

fn build_header(message_type: u8, flags: u8, serialization: u8, compression: u8) -> [u8; 4] {
    [
        (PROTOCOL_VERSION << 4) | HEADER_SIZE_UNITS,
        ((message_type & 0x0F) << 4) | (flags & 0x0F),
        ((serialization & 0x0F) << 4) | (compression & 0x0F),
        0x00,
    ]
}

fn frame(header: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + 4 + body.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// 全量客户端请求帧：JSON 序列化，不压缩。
pub fn build_full_client_request(payload: &Value) -> Vec<u8> {
    let body = payload.to_string().into_bytes();
    frame(
        build_header(MSG_FULL_CLIENT_REQUEST, FLAG_NO_SEQUENCE, SERIALIZATION_JSON, COMPRESSION_NONE),
        &body,
    )
}

/// 音频帧：RAW 序列化；末包 flags=0b0010（末包无序号）。
pub fn build_audio_request(pcm: &[i16], last: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(pcm.len() * 2);
    for sample in pcm {
        body.extend_from_slice(&sample.to_le_bytes());
    }
    let flags = if last { FLAG_LAST_NO_SEQUENCE } else { FLAG_NO_SEQUENCE };
    frame(build_header(MSG_AUDIO_ONLY_REQUEST, flags, SERIALIZATION_RAW, COMPRESSION_NONE), &body)
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// 按头的压缩标志尝试解压，失败按原文处理（错误帧的压缩约定官方未明确）。
fn gunzip_tolerant(payload: &[u8], compression: u8) -> Vec<u8> {
    if compression != COMPRESSION_GZIP || payload.is_empty() {
        return payload.to_vec();
    }
    let mut decoder = flate2::read::GzDecoder::new(payload);
    let mut out = Vec::new();
    match decoder.read_to_end(&mut out) {
        Ok(_) => out,
        Err(_) => payload.to_vec(),
    }
}

fn read_error_at(data: &[u8], offset: usize) -> Option<(u32, String)> {
    let code = read_u32(data, offset)?;
    let size = read_u32(data, offset + 4)? as usize;
    if size > data.len().saturating_sub(offset + 8) {
        return None;
    }
    // 火山错误码是 8 位十进制（如 45000001），用它做合理性校验
    if !(10_000_000..=99_999_999).contains(&code) {
        return None;
    }
    let message = String::from_utf8_lossy(&data[offset + 8..offset + 8 + size]).into_owned();
    Some((code, message))
}

fn safe_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn parse_result_payload(payload: &[u8]) -> Option<ResultFrame> {
    let decoded: Value = serde_json::from_slice(payload).ok()?;
    let object = decoded.as_object()?;
    let audio_duration_ms = safe_i64(object.get("audio_info").and_then(|a| a.get("duration")));
    let mut frame = ResultFrame { audio_duration_ms, ..Default::default() };
    if let Some(result) = object.get("result").and_then(Value::as_object) {
        frame.text = result.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(items) = result.get("utterances").and_then(Value::as_array) {
            for item in items.iter().filter_map(Value::as_object) {
                frame.utterances.push(Utterance {
                    text: item.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                    start_ms: safe_i64(item.get("start_time")),
                    end_ms: safe_i64(item.get("end_time")),
                    definite: item.get("definite").and_then(Value::as_bool).unwrap_or(false),
                });
            }
        }
    }
    Some(frame)
}

/// 解析服务端帧。结构异常一律返回 Unknown（调用方静默忽略，不断流）。
pub fn parse_server_frame(data: &[u8]) -> ServerFrame {
    if data.len() < HEADER_BYTES {
        return ServerFrame::Unknown;
    }
    let message_type = (data[1] >> 4) & 0x0F;
    let flags = data[1] & 0x0F;
    let serialization = (data[2] >> 4) & 0x0F;
    let compression = data[2] & 0x0F;

    if message_type == MSG_SERVER_ERROR_RESPONSE {
        let parsed = read_error_at(data, HEADER_BYTES).or_else(|| read_error_at(data, HEADER_BYTES + 1));
        let (code, message) = parsed.unwrap_or_else(|| (read_u32(data, HEADER_BYTES).unwrap_or(0), String::new()));
        return ServerFrame::Error { code, message };
    }
    if message_type != MSG_FULL_SERVER_RESPONSE && message_type != MSG_AUDIO_ONLY_RESPONSE {
        return ServerFrame::Unknown;
    }

    let mut position = HEADER_BYTES;
    if matches!(flags & 0b0011, FLAG_POS_SEQUENCE | FLAG_LAST_NEG_SEQUENCE) {
        if position + 4 > data.len() {
            return ServerFrame::Unknown;
        }
        position += 4;
    }
    let Some(payload_size) = read_u32(data, position) else {
        return ServerFrame::Unknown;
    };
    position += 4;
    let payload_size = payload_size as usize;
    if payload_size == 0 || position + payload_size > data.len() {
        return ServerFrame::Unknown;
    }
    if serialization != SERIALIZATION_JSON {
        return ServerFrame::Unknown;
    }
    let raw = gunzip_tolerant(&data[position..position + payload_size], compression);
    match parse_result_payload(&raw) {
        Some(result) => ServerFrame::Result { is_last: flags & FLAG_LAST_MASK != 0, result },
        None => ServerFrame::Unknown,
    }
}

/// 测试辅助：构造服务端结果帧（可选 gzip）。
pub fn build_server_result_frame(payload: &Value, last: bool, gzip: bool) -> Vec<u8> {
    let mut body = payload.to_string().into_bytes();
    if gzip {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = encoder.write_all(&body);
        body = encoder.finish().unwrap_or_default();
    }
    let flags = if last { FLAG_LAST_NEG_SEQUENCE } else { FLAG_POS_SEQUENCE };
    let mut out = Vec::new();
    out.extend_from_slice(&build_header(
        MSG_FULL_SERVER_RESPONSE,
        flags,
        SERIALIZATION_JSON,
        if gzip { COMPRESSION_GZIP } else { COMPRESSION_NONE },
    ));
    out.extend_from_slice(&(if last { -1i32 } else { 1 }).to_be_bytes());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

// ==================== 结果推进 ====================

/// 已确认前缀推进的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Advance {
    pub committed: Vec<String>,
    pub pending: String,
}

/// 按 definite 连续前缀推进已确认文本（三条硬规则见 Python 版注释）：
/// 1. 每帧都推进，不做「文本未变」短路（interim→definite 可在文本不变时发生）；
/// 2. 遇到第一个未定稿句就停，其后作为中间结果；
/// 3. 服务端回退时收缩已确认列表；utterances 为空只代表本帧不推进。
pub fn advance_definite_prefix(committed: &[String], utterances: &[Utterance]) -> Advance {
    if utterances.is_empty() {
        return Advance { committed: committed.to_vec(), pending: String::new() };
    }
    let prefix = utterances.iter().take_while(|u| u.definite).count();
    let mut kept: Vec<String> = if prefix < committed.len() {
        committed[..prefix].to_vec()
    } else {
        committed.to_vec()
    };
    for utterance in &utterances[kept.len().min(prefix)..prefix] {
        if !utterance.text.is_empty() {
            kept.push(utterance.text.clone());
        }
    }
    let pending = utterances[prefix..].iter().map(|u| u.text.as_str()).collect();
    Advance { committed: kept, pending }
}

/// 拿不到句级信息时的兜底：从累积文本里减掉已确认前缀。
pub fn pending_from_cumulative_text(committed: &[String], text: &str) -> String {
    let joined: String = committed.concat();
    if !joined.is_empty() && text.starts_with(&joined) {
        return text[joined.len()..].to_string();
    }
    text.to_string()
}

// ==================== 错误归一化 ====================

/// 服务端错误码 → 归一化错误。
pub fn classify_sauc_error(code: u32, message: &str) -> ErrorInfo {
    let detail = if message.is_empty() { String::new() } else { format!("：{message}") };
    match code {
        20_000_000 => ErrorInfo::new(ErrorKind::None, "成功", false, ""),
        45_000_001 => ErrorInfo::new(ErrorKind::Protocol, format!("请求参数无效{detail}"), false, "检查服务商参数后重新测试连接"),
        45_000_002 => ErrorInfo::new(ErrorKind::Input, "音频为空", false, "检查音频格式（16kHz 单声道 16bit）"),
        45_000_081 => ErrorInfo::new(ErrorKind::Network, "服务端等包超时", true, "立即重试"),
        45_000_151 => ErrorInfo::new(ErrorKind::Input, "音频格式不正确", false, "检查音频格式（16kHz 单声道 16bit）"),
        55_000_031 => ErrorInfo::new(ErrorKind::Quota, "服务端过载", true, "稍后重试，或检查并发配额"),
        55_000_000..=55_999_999 => ErrorInfo::new(ErrorKind::Server, format!("服务端内部错误（{code}）{detail}"), true, "立即重试"),
        _ => ErrorInfo::new(ErrorKind::Protocol, format!("服务端返回未知错误（{code}）{detail}"), false, "切换服务商或稍后重试"),
    }
}

/// WebSocket 关闭码 → 归一化错误（「请求被拒」不能当「网络抖动」反复重试）。
pub fn classify_close_code(code: Option<u16>, reason: &str) -> ErrorInfo {
    let Some(code) = code else {
        return ErrorInfo::new(ErrorKind::Network, "连接已断开（无关闭码）", true, "立即重试");
    };
    if code == 1000 {
        return ErrorInfo::new(ErrorKind::None, "连接正常关闭", false, "");
    }
    let detail = if reason.is_empty() { String::new() } else { format!(" {reason}") };
    let message = format!("连接被服务端关闭（code {code}{detail}）");
    match code {
        1002 | 1003 | 1007 | 1008 | 4400..=4499 => {
            ErrorInfo::new(ErrorKind::Protocol, message, false, "检查服务商参数后重新测试连接")
        }
        1011..=1013 => ErrorInfo::new(ErrorKind::Server, message, true, "立即重试"),
        _ => ErrorInfo::new(ErrorKind::Network, message, true, "立即重试"),
    }
}

/// 握手 HTTP 状态码 → 归一化错误（403 典型原因是 Resource ID 与开通产品不匹配）。
pub fn classify_handshake_status(status: u16, reason: &str) -> ErrorInfo {
    let detail = if reason.is_empty() { String::new() } else { format!(" {reason}") };
    match status {
        401 => ErrorInfo::new(
            ErrorKind::Auth,
            format!("鉴权失败：Access Token 无效或已过期（401{detail}）"),
            false,
            "在设置中重新填写火山语音技术 Access Token",
        ),
        403 => ErrorInfo::new(
            ErrorKind::Auth,
            format!("鉴权失败：Resource ID 与账号开通的产品不匹配（403{detail}）"),
            false,
            "ASR 1.0 用 volc.bigasr.sauc.*，ASR 2.0 用 volc.seedasr.sauc.*，逐个确认",
        ),
        404 => ErrorInfo::new(ErrorKind::Protocol, format!("接入点不存在（404{detail}）"), false, "检查 WebSocket 接入点地址"),
        429 => ErrorInfo::new(ErrorKind::Quota, format!("请求过于频繁（429{detail}）"), true, "稍后重试"),
        500..=599 => ErrorInfo::new(ErrorKind::Server, format!("服务端异常（{status}{detail}）"), true, "立即重试"),
        _ => ErrorInfo::new(ErrorKind::Network, format!("握手失败（{status}{detail}）"), true, "立即重试"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utt(text: &str, definite: bool) -> Utterance {
        Utterance { text: text.into(), definite, ..Default::default() }
    }

    #[test]
    fn 音频帧_头与长度() {
        let frame = build_audio_request(&[1, -1], false);
        assert_eq!(&frame[..4], &[0x11, 0x20, 0x00, 0x00]);
        assert_eq!(&frame[4..8], &4u32.to_be_bytes());
        assert_eq!(&frame[8..], &[1, 0, 0xFF, 0xFF]);
        let last = build_audio_request(&[], true);
        assert_eq!(last[1], 0x22, "末包 flags=0b0010");
        assert_eq!(&last[4..8], &0u32.to_be_bytes());
    }

    #[test]
    fn 全量请求_上下文是json字符串() {
        let payload = build_bigmodel_request("u1", "zh", "之前的话", 0);
        assert_eq!(payload["audio"]["language"], json!("zh-CN"));
        assert_eq!(payload["request"]["result_type"], json!("full"));
        let context = payload["request"]["corpus"]["context"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(context).unwrap();
        assert_eq!(parsed["context_data"][0]["text"], json!("之前的话"));
        assert!(build_bigmodel_request("", "auto", "", 0)["audio"].get("language").is_none());
        let frame = build_full_client_request(&payload);
        assert_eq!(&frame[..4], &[0x11, 0x10, 0x10, 0x00]);
    }

    #[test]
    fn 解析结果帧_含gzip与末包标记() {
        let payload = json!({
            "audio_info": {"duration": 1200},
            "result": {"text": "你好世界", "utterances": [
                {"text": "你好", "definite": true, "start_time": 0, "end_time": 500},
                {"text": "世界", "definite": false}
            ]}
        });
        for gzip in [false, true] {
            match parse_server_frame(&build_server_result_frame(&payload, true, gzip)) {
                ServerFrame::Result { is_last, result } => {
                    assert!(is_last);
                    assert_eq!(result.text, "你好世界");
                    assert_eq!(result.audio_duration_ms, 1200);
                    assert_eq!(result.utterances.len(), 2);
                    assert!(result.utterances[0].definite);
                }
                other => panic!("期望结果帧，得到 {other:?}"),
            }
        }
        assert_eq!(parse_server_frame(&[0x11]), ServerFrame::Unknown);
    }

    #[test]
    fn 解析错误帧_两种布局() {
        let message = "bad request";
        let mut frame = vec![0x11, 0xF0, 0x10, 0x00];
        frame.extend_from_slice(&45_000_001u32.to_be_bytes());
        frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
        frame.extend_from_slice(message.as_bytes());
        assert_eq!(parse_server_frame(&frame), ServerFrame::Error { code: 45_000_001, message: message.into() });

        let mut shifted = vec![0x11, 0xF0, 0x10, 0x00, 0x00];
        shifted.extend_from_slice(&frame[4..]);
        assert_eq!(parse_server_frame(&shifted), ServerFrame::Error { code: 45_000_001, message: message.into() });
    }

    #[test]
    fn 定稿前缀推进_回退与空句() {
        let advance = advance_definite_prefix(&[], &[utt("你好", true), utt("世", false)]);
        assert_eq!(advance.committed, vec!["你好".to_string()]);
        assert_eq!(advance.pending, "世");

        // interim→definite 在文本不变时发生也要推进
        let advance = advance_definite_prefix(&advance.committed, &[utt("你好", true), utt("世界", true)]);
        assert_eq!(advance.committed, vec!["你好".to_string(), "世界".to_string()]);
        assert_eq!(advance.pending, "");

        // 服务端回退：收缩
        let advance = advance_definite_prefix(&advance.committed, &[utt("你好", true), utt("世界啊", false)]);
        assert_eq!(advance.committed, vec!["你好".to_string()]);
        assert_eq!(advance.pending, "世界啊");

        // utterances 为空不收缩
        let advance = advance_definite_prefix(&advance.committed, &[]);
        assert_eq!(advance.committed, vec!["你好".to_string()]);

        assert_eq!(pending_from_cumulative_text(&["你好".into()], "你好世界"), "世界");
    }

    #[test]
    fn 错误归一化() {
        assert_eq!(classify_handshake_status(401, "").kind, ErrorKind::Auth);
        assert!(classify_handshake_status(403, "").to_message().contains("Resource ID"));
        assert!(!classify_close_code(Some(1008), "").retryable);
        assert!(classify_close_code(Some(1011), "").retryable);
        assert_eq!(classify_sauc_error(55_000_031, "").kind, ErrorKind::Quota);
        assert_eq!(truncate_context_text("一二三四", 2), "三四");
    }
}
