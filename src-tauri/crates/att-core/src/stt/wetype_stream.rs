//! 微信输入法流式协议的纯逻辑：protobuf、Opus 帧、流式会话状态机。
//!
//! 这里不引用 Windows/Frida；生产环境的注入实现通过 [`StreamTransport`] 接进来，
//! Windows 真机测试与 macOS 上的单测共用同一套分片、累计文本和收尾语义。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) const OPUS_HEADER: &[u8; 16] = b"#!OPUS_RAW_V1\x02\x01\x00";
/// 原生编码器每帧接收的 16k/mono/PCM16 字节数（20ms）。
pub(super) const FRAME_BYTES: usize = 640;
/// 一帧对应的 i16 采样数（20ms）。
pub(super) const FRAME_SAMPLES: usize = FRAME_BYTES / 2;
pub(super) const STREAM_CHUNK_SAMPLES: usize = 16_000;
pub(super) const PARTIAL_DEBOUNCE: Duration = Duration::from_millis(400);

/// 每次 submit/轮询后的累计文本与结束状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamUpdate {
    pub text: String,
    pub end_flag: u64,
    pub interval_ms: u64,
}

/// 流式注入后端：生产实现走 Frida host 命令通道，测试实现只记录调用。
pub(super) trait StreamTransport: Send + Sync {
    fn start(&self, voice_id: &str, deadline: Instant) -> Result<(), String>;
    fn chunk(&self, voice_id: &str, samples: &[i16], deadline: Instant) -> Result<StreamUpdate, String>;
    fn end(&self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String>;
    fn poll(&self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String>;
    fn cancel(&self, voice_id: &str) -> Result<(), String>;
}

pub type PartialCallback = Arc<dyn Fn(String) + Send + Sync>;

/// 尾部修订防抖：前缀增长立即上屏，非前缀修订等 `PARTIAL_DEBOUNCE` 稳定后再覆盖。
#[derive(Debug, Default)]
struct PartialGate {
    visible: String,
    generation: u64,
    pending: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum PartialDecision {
    Ignore,
    Emit(String),
    Defer { generation: u64, text: String },
}

impl PartialGate {
    fn accept(&mut self, text: String) -> PartialDecision {
        self.generation = self.generation.wrapping_add(1);
        if text == self.visible {
            self.pending = None;
            return PartialDecision::Ignore;
        }
        if self.visible.is_empty() || text.starts_with(&self.visible) {
            self.pending = None;
            self.visible.clone_from(&text);
            return PartialDecision::Emit(text);
        }
        self.pending = Some(text.clone());
        PartialDecision::Defer { generation: self.generation, text }
    }

    fn commit(&mut self, generation: u64, text: String) -> bool {
        if generation != self.generation || self.pending.as_deref() != Some(text.as_str()) {
            return false;
        }
        self.pending = None;
        self.visible = text;
        true
    }

    fn finalize(&mut self, text: String) -> Option<String> {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        if self.visible == text {
            return None;
        }
        self.visible.clone_from(&text);
        Some(text)
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
    }
}

/// 流式会话：Rust 侧只做 1 秒分片缓冲、累计文本覆盖与结束标志判定。
pub struct WetypeSession {
    transport: Arc<dyn StreamTransport>,
    voice_id: String,
    buffer: Vec<i16>,
    current_text: String,
    request_timeout: Duration,
    on_partial: Option<PartialCallback>,
    gate: Arc<Mutex<PartialGate>>,
    last_end_flag: u64,
    finished: bool,
}

impl WetypeSession {
    pub(super) async fn from_transport(
        transport: Arc<dyn StreamTransport>,
        voice_id: String,
        request_timeout: Duration,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, String> {
        let start_transport = transport.clone();
        let start_voice = voice_id.clone();
        let deadline = Instant::now() + request_timeout;
        blocking(move || start_transport.start(&start_voice, deadline)).await?;
        Ok(Self {
            transport,
            voice_id,
            buffer: Vec::new(),
            current_text: String::new(),
            request_timeout,
            on_partial,
            gate: Arc::new(Mutex::new(PartialGate::default())),
            last_end_flag: 0,
            finished: false,
        })
    }

    pub fn current_text(&self) -> String {
        self.current_text.clone()
    }

    /// 追加 16k/mono/PCM16；每凑满 1 秒就编码并提交一包。
    pub async fn push(&mut self, samples: &[i16]) -> Result<(), String> {
        if self.finished {
            return Err("微信输入法流式会话已经结束".into());
        }
        self.buffer.extend_from_slice(samples);
        while self.buffer.len() >= STREAM_CHUNK_SAMPLES {
            let chunk: Vec<i16> = self.buffer.drain(..STREAM_CHUNK_SAMPLES).collect();
            let update = self.send_chunk(chunk, None).await?;
            if update.end_flag != 0 {
                return Err("微信输入法在收尾前提前结束了识别".into());
            }
            self.apply_update(update);
        }
        Ok(())
    }

    /// 补足最后一帧、提交尾包，再轮询 seq=0 直到明确收到 end_flag。
    pub async fn finish(mut self, timeout: Duration) -> Result<String, String> {
        match self.finish_inner(timeout).await {
            Ok(text) => Ok(text),
            Err(error) => {
                // 收尾超时/提前结束也要把输入法侧会话清掉再返回，否则下一段录音可能
                // 抢在 Drop 的后台 cancel 之前启动，撞上「已有正在进行的流式会话」。
                self.cancel().await;
                Err(error)
            }
        }
    }

    async fn finish_inner(&mut self, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        if !self.buffer.is_empty() {
            let frames = self.buffer.len().div_ceil(FRAME_SAMPLES);
            self.buffer.resize(frames * FRAME_SAMPLES, 0);
            let tail = std::mem::take(&mut self.buffer);
            let update = self.send_chunk(tail, Some(deadline)).await?;
            if update.end_flag != 0 {
                return Err("微信输入法在发尾包时提前结束了识别".into());
            }
            self.apply_update(update);
        }

        let transport = self.transport.clone();
        let voice_id = self.voice_id.clone();
        let update = blocking(move || transport.end(&voice_id, deadline)).await?;
        self.apply_update(update);
        let mut current = self.current_text.clone();
        if self.last_end_flag != 0 {
            self.finished = true;
            self.emit_final(current.clone());
            return Ok(current);
        }

        loop {
            let transport = self.transport.clone();
            let voice_id = self.voice_id.clone();
            let update = blocking(move || transport.poll(&voice_id, deadline)).await?;
            let interval_ms = update.interval_ms;
            self.apply_update(update);
            current.clone_from(&self.current_text);
            let reply_end = self.last_end_flag;
            if reply_end != 0 {
                self.finished = true;
                self.emit_final(current.clone());
                return Ok(current);
            }
            if Instant::now() >= deadline {
                return Err("微信输入法未确认识别结束，未交付可能不完整的文字".into());
            }
            let wait = Duration::from_millis(interval_ms.clamp(100, 1000));
            tokio::time::sleep(wait.min(deadline.saturating_duration_since(Instant::now()))).await;
        }
    }

    /// 用户取消：结束本请求，不算失败。
    pub async fn cancel(mut self) {
        self.finished = true;
        self.gate.lock().unwrap_or_else(|error| error.into_inner()).cancel();
        let transport = self.transport.clone();
        let voice_id = self.voice_id.clone();
        if let Err(error) = blocking(move || transport.cancel(&voice_id)).await {
            tracing::warn!(%error, "取消微信输入法流式会话时清理未完成");
        }
    }

    async fn send_chunk(&mut self, samples: Vec<i16>, deadline: Option<Instant>) -> Result<StreamUpdate, String> {
        let transport = self.transport.clone();
        let voice_id = self.voice_id.clone();
        let deadline = deadline.unwrap_or_else(|| Instant::now() + self.request_timeout);
        blocking(move || transport.chunk(&voice_id, &samples, deadline)).await
    }

    fn apply_update(&mut self, update: StreamUpdate) {
        self.last_end_flag = update.end_flag;
        if update.text == self.current_text {
            return;
        }
        self.current_text.clone_from(&update.text);
        self.emit_latest(update.text);
    }

    fn emit_latest(&self, text: String) {
        let Some(callback) = self.on_partial.clone() else { return };
        let decision = self.gate.lock().unwrap_or_else(|error| error.into_inner()).accept(text);
        match decision {
            PartialDecision::Ignore => {}
            PartialDecision::Emit(text) => callback(text),
            PartialDecision::Defer { generation, text } => {
                let gate = self.gate.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(PARTIAL_DEBOUNCE).await;
                    let ready = gate.lock().unwrap_or_else(|error| error.into_inner()).commit(generation, text.clone());
                    if ready {
                        callback(text);
                    }
                });
            }
        }
    }

    fn emit_final(&self, text: String) {
        let Some(callback) = self.on_partial.clone() else { return };
        let final_text = self.gate.lock().unwrap_or_else(|error| error.into_inner()).finalize(text);
        if let Some(text) = final_text {
            callback(text);
        }
    }
}

impl Drop for WetypeSession {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.gate.lock().unwrap_or_else(|error| error.into_inner()).cancel();
        let transport = self.transport.clone();
        let voice_id = self.voice_id.clone();
        // 只排队，不在这里等清理；host 会在当前请求返回后处理。
        let _ = std::thread::Builder::new().name("att-wetype-cancel".into()).spawn(move || {
            let _ = transport.cancel(&voice_id);
        });
    }
}

async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|error| format!("微信输入法任务失败：{error}"))?
}

pub(super) enum WireValue {
    Varint(u64),
    Bytes(Vec<u8>),
}

pub(super) type Fields = HashMap<u32, Vec<(u8, WireValue)>>;

pub(super) fn varint(mut value: u64) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            break;
        }
        output.push(byte | 0x80);
    }
    output
}

pub(super) fn field_int(field: u32, value: u64) -> Vec<u8> {
    let mut output = varint(u64::from(field) << 3);
    output.extend(varint(value));
    output
}

pub(super) fn field_blob(field: u32, value: &[u8]) -> Vec<u8> {
    let mut output = varint((u64::from(field) << 3) | 2);
    output.extend(varint(value.len() as u64));
    output.extend_from_slice(value);
    output
}

pub(super) fn parse_fields(data: &[u8]) -> Result<Fields, String> {
    if data.len() > 4 * 1024 * 1024 {
        return Err("响应过大".into());
    }
    let mut fields = Fields::new();
    let mut position = 0usize;
    while position < data.len() {
        let key = read_varint(data, &mut position)?;
        let field = u32::try_from(key >> 3).map_err(|_| "protobuf 字段号溢出")?;
        let wire = (key & 7) as u8;
        if field == 0 || field >= 1 << 29 {
            return Err("无效的字段号".into());
        }
        let value = match wire {
            0 => WireValue::Varint(read_varint(data, &mut position)?),
            1 | 2 | 5 => {
                let length = match wire {
                    2 => usize::try_from(read_varint(data, &mut position)?).map_err(|_| "protobuf 字节长度溢出")?,
                    1 => 8,
                    5 => 4,
                    _ => unreachable!(),
                };
                let end = position.checked_add(length).ok_or("protobuf 字节长度溢出")?;
                if end > data.len() {
                    return Err("protobuf 字节字段被截断".into());
                }
                let bytes = data[position..end].to_vec();
                position = end;
                WireValue::Bytes(bytes)
            }
            other => return Err(format!("不支持的 protobuf wire type {other}")),
        };
        fields.entry(field).or_default().push((wire, value));
    }
    Ok(fields)
}

fn read_varint(data: &[u8], position: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*position).ok_or("protobuf varint 被截断")?;
        *position += 1;
        if shift == 63 && byte > 1 {
            return Err("protobuf varint 溢出".into());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("protobuf varint 过长".into())
}

fn field_value(fields: &Fields, field: u32, wire: u8) -> Result<Option<&WireValue>, String> {
    let Some(entries) = fields.get(&field) else { return Ok(None) };
    if entries.len() != 1 || entries[0].0 != wire {
        return Err(format!("字段 {field} 的类型或数量不符"));
    }
    Ok(Some(&entries[0].1))
}

pub(super) fn bytes_value(fields: &Fields, field: u32) -> Result<Option<&[u8]>, String> {
    Ok(match field_value(fields, field, 2)? {
        Some(WireValue::Bytes(bytes)) => Some(bytes),
        _ => None,
    })
}

pub(super) fn int_value(fields: &Fields, field: u32, default: u64) -> Result<u64, String> {
    Ok(match field_value(fields, field, 0)? {
        Some(WireValue::Varint(value)) => *value,
        _ => default,
    })
}

pub(super) fn build_request(voice_id: &str, audio: &[u8], sequence: u64, total_bytes: usize, end: bool) -> Vec<u8> {
    let mut inner = Vec::new();
    inner.extend(field_blob(2, voice_id.as_bytes()));
    inner.extend(field_blob(4, audio));
    inner.extend(field_int(5, 5));
    inner.extend(field_int(6, end as u64));
    inner.extend(field_int(7, sequence));
    inner.extend(field_int(11, total_bytes as u64));
    inner.extend(field_blob(25, &field_blob(1, b"WeType")));
    field_blob(1, &inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VoiceResponse {
    pub text: String,
    pub end_flag: u64,
    pub interval_ms: u64,
}

impl From<VoiceResponse> for StreamUpdate {
    fn from(value: VoiceResponse) -> Self {
        Self { text: value.text, end_flag: value.end_flag, interval_ms: value.interval_ms }
    }
}

pub(super) fn parse_response(data: &[u8], expected_voice_id: &str) -> Result<VoiceResponse, String> {
    let outer = parse_fields(data)?;
    let payload = bytes_value(&outer, 1)?.ok_or("缺少业务响应")?;
    let inner = parse_fields(payload)?;
    let base_payload = bytes_value(&inner, 1)?.ok_or("缺少业务状态")?;
    let base = parse_fields(base_payload)?;
    let ret = int_value(&base, 1, 0)?;
    if ret != 0 {
        return Err(format!("输入法业务返回错误 {ret}"));
    }
    let voice_id = bytes_value(&inner, 2)?.unwrap_or_default();
    if voice_id != expected_voice_id.as_bytes() {
        return Err("响应会话标识不匹配".into());
    }
    let text =
        String::from_utf8(bytes_value(&inner, 4)?.unwrap_or_default().to_vec()).map_err(|_| "响应文字编码不正确")?;
    Ok(VoiceResponse { text, end_flag: int_value(&inner, 5, 0)?, interval_ms: int_value(&inner, 6, 100)? })
}

#[derive(Debug)]
struct FrameRanges {
    ranges: Vec<(usize, usize)>,
}

fn frame_ranges(data: &[u8], has_header: bool) -> Result<FrameRanges, String> {
    let mut position = if has_header {
        if data.len() < OPUS_HEADER.len() || &data[..OPUS_HEADER.len()] != OPUS_HEADER {
            return Err("不支持的 Opus 流头".into());
        }
        OPUS_HEADER.len()
    } else {
        0
    };
    let start = position;
    let mut ranges = Vec::new();
    while position < data.len() {
        if data.len() - position < 2 {
            return Err("Opus 帧长度不完整".into());
        }
        let size = u16::from_le_bytes([data[position], data[position + 1]]) as usize;
        position += 2;
        if size == 0 || size > 1500 || position + size > data.len() {
            return Err("Opus 帧不完整".into());
        }
        ranges.push((position - 2, position + size));
        position += size;
    }
    if ranges.is_empty() && data.len() > start {
        return Err("Opus 流没有音频帧".into());
    }
    Ok(FrameRanges { ranges })
}

pub(super) fn validate_stream_frames(data: &[u8], expected_frames: usize) -> Result<(), String> {
    let ranges = frame_ranges(data, false)?;
    if ranges.ranges.len() != expected_frames {
        return Err(format!("Opus 帧数与新增音频不一致：期望 {expected_frames}，实际 {}", ranges.ranges.len()));
    }
    Ok(())
}

pub(super) fn stream_packet(frames: &[u8], first: bool) -> Vec<u8> {
    let mut packet = Vec::with_capacity(frames.len() + if first { OPUS_HEADER.len() } else { 0 });
    if first {
        packet.extend_from_slice(OPUS_HEADER);
    }
    packet.extend_from_slice(frames);
    packet
}

#[derive(Debug)]
pub(super) struct OpusPackets<'a> {
    pub packets: Vec<&'a [u8]>,
    pub frames: usize,
}

pub(super) fn opus_packets(encoded: &[u8], frames_per_packet: usize) -> Result<OpusPackets<'_>, String> {
    if frames_per_packet == 0 {
        return Err("无效的分片帧数".into());
    }
    let ranges = frame_ranges(encoded, true)?;
    let frames = ranges.ranges.len();
    if frames == 0 {
        return Err("Opus 流没有音频帧".into());
    }
    let packets = (0..frames)
        .step_by(frames_per_packet)
        .map(|start| {
            let end_frame = (start + frames_per_packet).min(frames);
            let from = if start == 0 { 0 } else { ranges.ranges[start].0 };
            &encoded[from..ranges.ranges[end_frame - 1].1]
        })
        .collect();
    Ok(OpusPackets { packets, frames })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn test_response(voice_id: &str, text: &str, end: u64) -> Vec<u8> {
        let base = field_int(1, 0);
        let inner = [
            field_blob(1, &base),
            field_blob(2, voice_id.as_bytes()),
            field_int(3, 1),
            field_blob(4, text.as_bytes()),
            field_int(5, end),
            field_int(6, 100),
        ]
        .concat();
        field_blob(1, &inner)
    }

    fn test_frames(payloads: &[&[u8]]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for payload in payloads {
            encoded.extend((payload.len() as u16).to_le_bytes());
            encoded.extend_from_slice(payload);
        }
        encoded
    }

    #[test]
    fn 一秒音频对应五十个二十毫秒帧() {
        assert_eq!(FRAME_BYTES, 640);
        assert_eq!(FRAME_SAMPLES, 320);
        assert_eq!(STREAM_CHUNK_SAMPLES * 2 / FRAME_BYTES, 50);
        assert_eq!(STREAM_CHUNK_SAMPLES / FRAME_SAMPLES, 50);
    }

    #[test]
    fn 请求字段与参考协议一致() {
        let request = build_request("wt-test", b"audio", 3, 1234, true);
        let outer = parse_fields(&request).unwrap();
        let inner = parse_fields(bytes_value(&outer, 1).unwrap().unwrap()).unwrap();
        assert_eq!(bytes_value(&inner, 2).unwrap(), Some(b"wt-test".as_slice()));
        assert_eq!(bytes_value(&inner, 4).unwrap(), Some(b"audio".as_slice()));
        assert_eq!(int_value(&inner, 5, 0).unwrap(), 5);
        assert_eq!(int_value(&inner, 6, 0).unwrap(), 1);
        assert_eq!(int_value(&inner, 7, 0).unwrap(), 3);
        assert_eq!(int_value(&inner, 11, 0).unwrap(), 1234);
        let app = parse_fields(bytes_value(&inner, 25).unwrap().unwrap()).unwrap();
        assert_eq!(bytes_value(&app, 1).unwrap(), Some(b"WeType".as_slice()));
    }

    #[test]
    fn 响应必须校验会话并取得明确结束标志() {
        let response = test_response("voice-1", "今天天气不错", 1);
        let parsed = parse_response(&response, "voice-1").unwrap();
        assert_eq!(parsed.text, "今天天气不错");
        assert_eq!(parsed.end_flag, 1);
        assert!(parse_response(&response, "voice-2").unwrap_err().contains("会话标识不匹配"));
    }

    #[test]
    fn 响应业务错误不交付文字() {
        let base = field_int(1, 7);
        let inner = [field_blob(1, &base), field_blob(2, b"voice-1"), field_blob(4, "半截".as_bytes())].concat();
        let response = field_blob(1, &inner);
        assert!(parse_response(&response, "voice-1").unwrap_err().contains("业务返回错误 7"));
    }

    #[test]
    fn opus_按帧边界切包且只在首包带流头() {
        let frames = test_frames(&[b"one".as_slice(), b"two".as_slice(), b"three".as_slice()]);
        let encoded = stream_packet(&frames, true);
        let packets = opus_packets(&encoded, 2).unwrap();
        assert_eq!(packets.frames, 3);
        assert_eq!(packets.packets.len(), 2);
        assert!(packets.packets[0].starts_with(OPUS_HEADER));
        assert!(!packets.packets[1].starts_with(OPUS_HEADER));
        assert_eq!(packets.packets.concat().len(), encoded.len());
    }

    #[test]
    fn 流式每包只带新增帧_首包补流头() {
        let frames = test_frames(&[b"aaa".as_slice(), b"bb".as_slice()]);
        validate_stream_frames(&frames, 2).unwrap();
        let first = stream_packet(&frames, true);
        let next = stream_packet(&frames, false);
        assert_eq!(&first[..OPUS_HEADER.len()], OPUS_HEADER);
        assert_eq!(&first[OPUS_HEADER.len()..], frames.as_slice());
        assert_eq!(next, frames);
        assert!(validate_stream_frames(&frames, 3).unwrap_err().contains("期望 3"));
    }

    #[test]
    fn opus_拒绝截断帧() {
        let encoded = stream_packet(&[10, 0, 1, 2], true);
        assert!(opus_packets(&encoded, 250).unwrap_err().contains("帧不完整"));
    }

    #[test]
    fn 累计文本只在前缀增长时立即上屏_非前缀修订要防抖() {
        let mut gate = PartialGate::default();
        assert_eq!(gate.accept("你好".into()), PartialDecision::Emit("你好".into()));
        assert_eq!(gate.accept("你好世".into()), PartialDecision::Emit("你好世".into()));
        assert_eq!(gate.accept("你好世界".into()), PartialDecision::Emit("你好世界".into()));
        assert_eq!(
            gate.accept("你好，世界".into()),
            PartialDecision::Defer { generation: 4, text: "你好，世界".into() }
        );
        // 文本每次都覆盖成最新累计值，而不是在旧值后面追加 delta。
        assert!(gate.commit(4, "你好，世界".into()));
        assert_eq!(gate.visible, "你好，世界");
        assert!(!gate.commit(2, "过期".into()));
        assert_eq!(gate.finalize("你好，世界".into()), None);
        assert_eq!(gate.finalize("你好，世界！".into()), Some("你好，世界！".into()));
    }

    struct StubTransport {
        calls: Mutex<Vec<String>>,
        updates: Mutex<VecDeque<StreamUpdate>>,
    }

    impl StubTransport {
        fn new(updates: Vec<StreamUpdate>) -> Self {
            Self { calls: Mutex::new(Vec::new()), updates: Mutex::new(updates.into()) }
        }

        fn record(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }

        fn next(&self) -> Result<StreamUpdate, String> {
            self.updates.lock().unwrap().pop_front().ok_or_else(|| "桩没有更多响应".to_string())
        }
    }

    impl StreamTransport for StubTransport {
        fn start(&self, voice_id: &str, _deadline: Instant) -> Result<(), String> {
            self.record(format!("start:{voice_id}"));
            Ok(())
        }

        fn chunk(&self, _voice_id: &str, samples: &[i16], _deadline: Instant) -> Result<StreamUpdate, String> {
            self.record(format!("chunk:{}", samples.len()));
            self.next()
        }

        fn end(&self, voice_id: &str, _deadline: Instant) -> Result<StreamUpdate, String> {
            self.record(format!("end:{voice_id}"));
            self.next()
        }

        fn poll(&self, voice_id: &str, _deadline: Instant) -> Result<StreamUpdate, String> {
            self.record(format!("poll:{voice_id}"));
            self.next()
        }

        fn cancel(&self, voice_id: &str) -> Result<(), String> {
            self.record(format!("cancel:{voice_id}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn 会话状态机_一秒分片_尾包补帧_轮询到结束标志() {
        let stub = Arc::new(StubTransport::new(vec![
            StreamUpdate { text: "你好".into(), end_flag: 0, interval_ms: 100 },
            StreamUpdate { text: "你好世界".into(), end_flag: 0, interval_ms: 100 },
            StreamUpdate { text: "你好世界".into(), end_flag: 0, interval_ms: 100 },
            StreamUpdate { text: "你好世界。".into(), end_flag: 1, interval_ms: 100 },
        ]));
        let mut session =
            WetypeSession::from_transport(stub.clone(), "voice-test".into(), Duration::from_secs(2), None)
                .await
                .unwrap();
        session.push(&vec![1i16; STREAM_CHUNK_SAMPLES + 123]).await.unwrap();
        assert_eq!(session.current_text(), "你好");
        let final_text = session.finish(Duration::from_secs(2)).await.unwrap();
        assert_eq!(final_text, "你好世界。");
        let calls = stub.calls.lock().unwrap().clone();
        assert_eq!(calls[0], "start:voice-test");
        assert_eq!(calls[1], format!("chunk:{STREAM_CHUNK_SAMPLES}"));
        assert_eq!(calls[2], format!("chunk:{FRAME_SAMPLES}"));
        assert_eq!(calls[3], "end:voice-test");
        assert_eq!(calls[4], "poll:voice-test");
    }

    #[tokio::test]
    async fn 会话取消不发尾包且只取消一次() {
        let stub = Arc::new(StubTransport::new(Vec::new()));
        let session = WetypeSession::from_transport(stub.clone(), "voice-test".into(), Duration::from_secs(2), None)
            .await
            .unwrap();
        session.cancel().await;
        let calls = stub.calls.lock().unwrap().clone();
        assert_eq!(calls, ["start:voice-test", "cancel:voice-test"]);
    }

    #[tokio::test]
    async fn 收尾超时会同步清理会话再返回错误() {
        let stub = Arc::new(StubTransport::new(vec![
            StreamUpdate { text: "半截".into(), end_flag: 0, interval_ms: 100 },
            StreamUpdate { text: "半截".into(), end_flag: 0, interval_ms: 100 },
        ]));
        let session = WetypeSession::from_transport(stub.clone(), "voice-test".into(), Duration::from_secs(2), None)
            .await
            .unwrap();
        let error = session.finish(Duration::ZERO).await.unwrap_err();
        assert!(error.contains("未确认识别结束"), "{error}");
        let calls = stub.calls.lock().unwrap().clone();
        assert_eq!(calls, ["start:voice-test", "end:voice-test", "poll:voice-test", "cancel:voice-test"]);
    }
}
