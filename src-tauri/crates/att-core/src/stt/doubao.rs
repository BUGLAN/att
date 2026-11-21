//! 豆包双向流式 ASR 会话（tokio-tungstenite 实现，移植自 Python `doubao_asr_provider.py`）。
//!
//! 一条会话的生命周期：
//! 1. [`DoubaoSession::connect`]：握手（三个鉴权头）→ 发送全量客户端请求；
//! 2. [`DoubaoSession::push`]：16k 单声道 i16 PCM 切帧推流（实时 100ms，批量 200ms）；
//!    后台读任务持续解析服务端帧，按 definite 前缀推进文本并回调中间结果；
//! 3. [`DoubaoSession::finish`]：发末包 → 等末包结果/关闭/静默窗口 → 返回整场文字稿；
//! 4. [`DoubaoSession::cancel`]：用户取消，不算失败。
//!
//! 预热：流式录音结束后可以先建好下一条连接（[`DoubaoSession::connect`] 不带回调），按下时用
//! [`WarmKey`] 确认参数没变、[`DoubaoSession::check_alive`] 确认没断，再 [`DoubaoSession::set_partial_callback`]
//! 接上本次会话的回调直接推流，省掉握手。服务端对不发音频的连接有空闲上限（见 [`WARM_KEEP`]）。
//!
//! 约定：密钥只进握手头，不落日志、不进错误文案；连接直连（tungstenite 不读系统代理，
//! 与 Python 版显式 `proxy=None` 语义一致——境内服务不应绕代理）。

use super::sauc::{self, ErrorInfo, ErrorKind, ServerFrame};
use futures_util::stream::{SplitSink, StreamExt};
use futures_util::SinkExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 建连超时：超时按 network/可重试处理
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 流式收尾：硬超时与「收到结果后静默多久算收完」
pub const FINISH_TIMEOUT: Duration = Duration::from_secs(6);
pub const FINISH_QUIET: Duration = Duration::from_millis(1200);
/// 预热连接最多保留多久：取实测服务端空闲断开时长的一半，上限 30 秒。
///
/// 2026-11-21 实测（`examples/doubao_probe.rs` 的 idle 模式，`bigmodel_async` + `*.duration`，3 次一致）：
/// 发出全量请求后不发音频，8.4 秒时服务端回错误帧「等包超时」并断开 → 取 4 秒。
pub const WARM_KEEP: Duration = Duration::from_secs(4);

/// 豆包会话错误：带归一化分类（上层据此决定「去修复」还是「立即重试」）。
#[derive(Debug, Clone)]
pub struct SttError {
    pub info: ErrorInfo,
}

impl SttError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, retryable: bool, next_action: &str) -> Self {
        Self { info: ErrorInfo::new(kind, message, retryable, next_action) }
    }
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.info.to_message())
    }
}

impl std::error::Error for SttError {}

/// 接入配置（密钥为运行期解密后的明文，只活在内存里）。
#[derive(Clone)]
pub struct DoubaoConfig {
    pub api_key: String,
    pub resource_id: String,
    pub endpoint: String,
    pub end_window_size: i64,
    /// 个人词表（已归一化）；发送时按 [`sauc::HOTWORDS_MAX_TOKENS`] 截取前缀
    pub hotwords: Vec<String>,
}

impl std::fmt::Debug for DoubaoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不输出密钥
        f.debug_struct("DoubaoConfig")
            .field("api_key_set", &!self.api_key.is_empty())
            .field("resource_id", &self.resource_id)
            .field("endpoint", &self.endpoint)
            .field("end_window_size", &self.end_window_size)
            .field("hotwords", &self.hotwords.len())
            .finish()
    }
}

impl DoubaoConfig {
    /// 校验接入配置；缺失时给出可执行的中文提示。
    pub fn ensure(&self) -> Result<(), SttError> {
        if self.api_key.trim().is_empty() {
            return Err(SttError::new(
                ErrorKind::Auth,
                "未配置豆包语音 Access Token，请在「设置 → 识别引擎」中填写",
                false,
                "",
            ));
        }
        if self.endpoint.trim().is_empty() {
            return Err(SttError::new(ErrorKind::Protocol, "未配置豆包语音 WebSocket 接入点地址", false, ""));
        }
        if self.resource_id.trim().is_empty() {
            return Err(SttError::new(ErrorKind::Protocol, "未配置豆包语音 Resource ID", false, ""));
        }
        Ok(())
    }
}

/// 中间结果：整场当前文本，及其末尾还没确认（definite）的字数——服务端断句后才确认，
/// 界面据此把正在说的这句与已确认的句子区分显示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    pub text: String,
    /// `text` 末尾未确认部分的字数（按 Unicode 标量计）
    pub pending_chars: usize,
}

/// 中间结果回调。
pub type PartialCallback = Arc<dyn Fn(Partial) + Send + Sync>;

#[derive(Default)]
struct TranscriptState {
    committed: Vec<String>,
    pending: String,
    server_text: String,
    last_text: String,
    audio_duration_ms: i64,
    server_final: bool,
    closed: bool,
    close_code: Option<u16>,
    close_reason: String,
    error: Option<SttError>,
    frame_count: u64,
    last_frame_at: Option<Instant>,
}

impl TranscriptState {
    fn final_text(&self) -> String {
        let text = if self.last_text.is_empty() { &self.server_text } else { &self.last_text };
        text.trim().to_string()
    }

    fn partial(&self) -> Partial {
        let text = self.final_text();
        // 首尾空白已被 final_text 剥掉：未确认部分同样去掉尾部空白，且不超过全文
        let pending_chars = self.pending.trim_end().chars().count().min(text.chars().count());
        Partial { text, pending_chars }
    }

    fn closed_error(&self) -> SttError {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if matches!(self.close_code, None | Some(1000)) {
            // 1000 常见于服务端 VAD 判定长时间静音后关闭；但刚建连就断同样走到这里，
            // 文案必须带上 code/reason，别把真实原因藏进一句「静音」里
            let mut detail = format!("（code={:?}", self.close_code);
            if !self.close_reason.is_empty() {
                detail.push_str(&format!("，reason={}", self.close_reason));
            }
            detail.push('）');
            return SttError::new(
                ErrorKind::Network,
                format!(
                    "流式连接被服务端关闭{detail}；录音较久可能是静音超时，刚建连就断请检查「强制断句静音时长」等设置"
                ),
                true,
                "重新开始录音",
            );
        }
        SttError { info: sauc::classify_close_code(self.close_code, &self.close_reason) }
    }

    fn apply_result(&mut self, result: sauc::ResultFrame) {
        if result.audio_duration_ms > 0 {
            self.audio_duration_ms = self.audio_duration_ms.max(result.audio_duration_ms);
        }
        if !result.text.is_empty() {
            self.server_text = result.text.clone();
        }
        if result.utterances.is_empty() {
            self.pending = sauc::pending_from_cumulative_text(&self.committed, &result.text);
        } else {
            let advance = sauc::advance_definite_prefix(&self.committed, &result.utterances);
            self.committed = advance.committed;
            self.pending = advance.pending;
        }
        self.last_text = format!("{}{}", self.committed.concat(), self.pending);
    }
}

struct Shared {
    state: Mutex<TranscriptState>,
    notify: Notify,
    /// 中间结果回调：预热连接建连时还没有会话，按下复用时再接上
    on_partial: Mutex<Option<PartialCallback>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, TranscriptState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 收尾结果。
#[derive(Debug, Clone)]
pub struct FinishOutcome {
    pub text: String,
    /// 服务端回报的计费时长（毫秒）
    pub audio_duration_ms: i64,
}

/// 预热连接的匹配键：建连时写进全量请求的参数（接入点、凭据、语言、断句时长、热词）任何一项变了，
/// 预热连接就不能用。凭据只存摘要，不多留一份明文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmKey {
    endpoint: String,
    resource_id: String,
    api_key_digest: String,
    language: String,
    end_window_size: i64,
    hotwords: Vec<String>,
}

impl WarmKey {
    pub fn new(config: &DoubaoConfig, language: &str) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            endpoint: config.endpoint.trim().to_string(),
            resource_id: config.resource_id.trim().to_string(),
            api_key_digest: crate::clipboard::hex(&Sha256::digest(config.api_key.trim().as_bytes())),
            language: language.to_string(),
            end_window_size: config.end_window_size,
            hotwords: config.hotwords.clone(),
        }
    }
}

/// 只有按时长计费的资源（`*.duration`）才预热：按并发计费（`*.concurrent`）时预热连接会多占一路并发。
pub fn warm_allowed(config: &DoubaoConfig) -> bool {
    config.resource_id.trim().ends_with(".duration")
}

pub struct DoubaoSession {
    writer: SplitSink<WsStream, Message>,
    shared: Arc<Shared>,
    reader: tokio::task::JoinHandle<()>,
    buffer: Vec<i16>,
    /// 每帧采样数：实时推流 100ms，批量识别 200ms
    frame_samples: usize,
    pub connect_id: String,
    /// 服务端 X-Tt-Logid：排查时提供给火山侧定位，不含凭据
    pub log_id: String,
    /// 建连完成（全量请求已发出）的时刻：预热连接据此判断是否过期
    pub opened_at: Instant,
}

impl DoubaoSession {
    /// 握手 + 发送全量请求。`on_partial` 在每个结果帧后以整场当前文本回调。
    pub async fn connect(
        config: &DoubaoConfig,
        language: &str,
        context: &str,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, SttError> {
        config.ensure()?;
        let connect_id = uuid::Uuid::new_v4().to_string();
        let mut request = config.endpoint.as_str().into_client_request().map_err(|e| {
            SttError::new(ErrorKind::Protocol, format!("接入点地址无效（{e}）"), false, "检查 WebSocket 接入点地址")
        })?;
        let headers = request.headers_mut();
        let header = |value: &str| {
            HeaderValue::from_str(value.trim()).map_err(|_| {
                SttError::new(ErrorKind::Auth, "Access Token / Resource ID 含非法字符", false, "在设置中重新填写")
            })
        };
        headers.insert("X-Api-Key", header(&config.api_key)?);
        headers.insert("X-Api-Resource-Id", header(&config.resource_id)?);
        headers.insert("X-Api-Connect-Id", header(&connect_id)?);

        tracing::info!(
            endpoint = %config.endpoint,
            resource_id = %config.resource_id,
            connect_id = %connect_id,
            hotwords = config.hotwords.len(),
            "连接豆包双向流式 ASR"
        );
        let handshake_started = Instant::now();
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await;
        let (stream, response) = match connected {
            Err(_) => {
                return Err(SttError::new(ErrorKind::Network, "连接豆包语音服务超时（10 秒）", true, "检查网络后重试"));
            }
            Ok(Err(WsError::Http(response))) => {
                let status = response.status();
                tracing::error!(status = status.as_u16(), connect_id = %connect_id, "豆包流式握手失败");
                return Err(SttError {
                    info: sauc::classify_handshake_status(status.as_u16(), status.canonical_reason().unwrap_or("")),
                });
            }
            Ok(Err(error)) => {
                return Err(SttError::new(
                    ErrorKind::Network,
                    format!("无法连接豆包语音服务（{error}）"),
                    true,
                    "检查网络后重试",
                ));
            }
            Ok(Ok(pair)) => pair,
        };
        let log_id = response.headers().get("x-tt-logid").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();

        tracing::info!(connect_id = %connect_id, "豆包流式握手完成（{:?}）", handshake_started.elapsed());
        let (mut writer, mut read) = stream.split();
        let uid = format!("att-desktop-{}", &connect_id[..8]);
        let payload = sauc::build_bigmodel_request(&uid, language, context, &config.hotwords, config.end_window_size);
        if let Err(error) = writer.send(Message::binary(sauc::build_full_client_request(&payload))).await {
            return Err(SttError::new(ErrorKind::Network, format!("发送请求失败（{error}）"), true, "重新开始录音"));
        }

        let shared = Arc::new(Shared {
            state: Mutex::new(TranscriptState::default()),
            notify: Notify::new(),
            on_partial: Mutex::new(on_partial),
        });
        let reader_shared = shared.clone();
        let reader_connect_id = connect_id.clone();
        let reader = tokio::spawn(async move {
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Binary(data)) => {
                        let partial = handle_frame(&reader_shared, &data, &reader_connect_id);
                        reader_shared.notify.notify_waiters();
                        let callback = reader_shared.on_partial.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if let (Some(text), Some(callback)) = (partial, callback) {
                            callback(text);
                        }
                        if reader_shared.lock().closed {
                            break;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        let mut state = reader_shared.lock();
                        state.closed = true;
                        if let Some(frame) = frame {
                            state.close_code = Some(u16::from(frame.code));
                            state.close_reason = frame.reason.to_string();
                        }
                        tracing::debug!(
                            connect_id = %reader_connect_id,
                            code = ?state.close_code,
                            "豆包流式连接被服务端关闭"
                        );
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::debug!("豆包流式连接读取结束: {error}");
                        break;
                    }
                }
            }
            reader_shared.lock().closed = true;
            reader_shared.notify.notify_waiters();
        });

        Ok(Self {
            writer,
            shared,
            reader,
            buffer: Vec::new(),
            frame_samples: sauc::STREAM_CHUNK_SAMPLES,
            connect_id,
            log_id,
            opened_at: Instant::now(),
        })
    }

    /// 接上（或换掉）中间结果回调：预热连接在按下复用时调用，之后的结果帧回调给本次会话。
    pub fn set_partial_callback(&self, on_partial: Option<PartialCallback>) {
        *self.shared.on_partial.lock().unwrap_or_else(|e| e.into_inner()) = on_partial;
    }

    /// 连接被服务端关闭时的 close code（预热实测与日志用）；还开着时为 None。
    pub fn close_code(&self) -> Option<u16> {
        let state = self.shared.lock();
        state.closed.then_some(state.close_code).flatten()
    }

    /// 等到连接被关闭（服务端关闭 / 出错 / 读取结束）为止。
    pub async fn wait_closed(&self) {
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.shared.lock().closed {
                return;
            }
            notified.await;
        }
    }

    /// 当前状态下是否已不可继续推流（错误帧/断连）。
    pub fn check_alive(&self) -> Result<(), SttError> {
        let state = self.shared.lock();
        if state.error.is_some() || state.closed {
            return Err(state.closed_error());
        }
        Ok(())
    }

    /// 当前整场文本（已确认 + 中间结果）。
    pub fn current_text(&self) -> String {
        self.shared.lock().final_text()
    }

    /// 追加 16k 单声道 PCM：按帧长切帧发送，不足一帧的留在缓冲里。
    pub async fn push(&mut self, pcm: &[i16]) -> Result<(), SttError> {
        self.check_alive()?;
        self.buffer.extend_from_slice(pcm);
        while self.buffer.len() >= self.frame_samples {
            let chunk: Vec<i16> = self.buffer.drain(..self.frame_samples).collect();
            self.send_audio(&chunk, false).await?;
        }
        Ok(())
    }

    async fn send_audio(&mut self, pcm: &[i16], last: bool) -> Result<(), SttError> {
        if let Err(error) = self.writer.send(Message::binary(sauc::build_audio_request(pcm, last))).await {
            let state = self.shared.lock();
            if state.closed || state.error.is_some() {
                return Err(state.closed_error());
            }
            return Err(SttError::new(ErrorKind::Network, format!("发送音频失败（{error}）"), true, "重新开始录音"));
        }
        Ok(())
    }

    /// 发送末包并收尾；中途已出错则返回归一化错误。
    pub async fn finish(mut self, timeout: Duration, quiet: Duration) -> Result<FinishOutcome, SttError> {
        let alive = self.check_alive();
        if alive.is_ok() {
            let tail = std::mem::take(&mut self.buffer);
            if let Err(error) = self.send_audio(&tail, true).await {
                self.close().await;
                return Err(error);
            }
            let baseline = self.shared.lock().frame_count;
            let deadline = Instant::now() + timeout;
            loop {
                let notified = self.shared.notify.notified();
                {
                    let state = self.shared.lock();
                    if state.server_final || state.closed || state.error.is_some() {
                        break;
                    }
                    if state.frame_count > baseline
                        && state.last_frame_at.map(|t| t.elapsed() >= quiet).unwrap_or(false)
                    {
                        break;
                    }
                }
                let now = Instant::now();
                if now >= deadline {
                    tracing::warn!(connect_id = %self.connect_id, "豆包流式收尾超时，按已收结果返回");
                    break;
                }
                let wait = (deadline - now).min(Duration::from_millis(100));
                let _ = tokio::time::timeout(wait, notified).await;
            }
        }
        self.close().await;
        let state = self.shared.lock();
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        Ok(FinishOutcome { text: state.final_text(), audio_duration_ms: state.audio_duration_ms })
    }

    /// 用户主动取消：不算失败。
    pub async fn cancel(mut self) {
        self.close().await;
    }

    async fn close(&mut self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.writer.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() }))),
        )
        .await;
        self.reader.abort();
    }
}

/// 会话被中途丢弃（如收尾阶段被取消）时停掉读帧任务，不让它挂到服务端断开为止。
impl Drop for DoubaoSession {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// 解析一帧并更新状态；文本或确认进度有变化时返回新的中间结果（供回调）。
fn handle_frame(shared: &Shared, data: &[u8], connect_id: &str) -> Option<Partial> {
    let mut state = shared.lock();
    match sauc::parse_server_frame(data) {
        ServerFrame::Error { code, message } => {
            let info = sauc::classify_sauc_error(code, &message);
            // 只记录错误码与分类，不打印 payload（避免把用户语音文本写进日志）
            tracing::warn!(code, kind = ?info.kind, connect_id, "豆包流式返回错误帧");
            state.error = Some(SttError { info });
            state.closed = true;
            None
        }
        ServerFrame::Unknown => None,
        ServerFrame::Result { is_last, result } => {
            state.frame_count += 1;
            state.last_frame_at = Some(Instant::now());
            if is_last {
                state.server_final = true;
            }
            // 文本不变、只是这句被确认了，也要回调（界面要把它从「未确认」改成「已确认」）
            let before = state.partial();
            state.apply_result(result);
            let after = state.partial();
            (after != before).then_some(after)
        }
    }
}

/// 批量转写：整段 PCM 按 200ms 推给同一条 SAUC 会话（单段/多段录音、文件转写共用）。
pub async fn transcribe_pcm(
    config: &DoubaoConfig,
    language: &str,
    context: &str,
    samples: &[i16],
) -> Result<FinishOutcome, SttError> {
    transcribe_pcm_with_progress(config, language, context, samples, &mut |_| true).await
}

/// 同 [`transcribe_pcm`]，每推一帧前回调已推送的样本数；回调返回 false 时停止推流、关闭会话，
/// 返回 [`super::TRANSCRIBE_CANCELLED`]（文件转写取消用）。
pub async fn transcribe_pcm_with_progress(
    config: &DoubaoConfig,
    language: &str,
    context: &str,
    samples: &[i16],
    on_push: &mut (dyn FnMut(usize) -> bool + Send),
) -> Result<FinishOutcome, SttError> {
    if samples.is_empty() {
        return Err(SttError::new(ErrorKind::Input, "音频为空，无法转写", false, ""));
    }
    let mut session = DoubaoSession::connect(config, language, context, None).await?;
    session.frame_samples = sauc::CHUNK_SAMPLES;
    for (index, chunk) in samples.chunks(sauc::CHUNK_SAMPLES).enumerate() {
        if !on_push(index * sauc::CHUNK_SAMPLES) {
            session.close().await;
            return Err(SttError::new(ErrorKind::Input, super::TRANSCRIBE_CANCELLED, false, ""));
        }
        session.push(chunk).await?;
    }
    on_push(samples.len());
    // 批量推流比实时快得多，服务端需要更长时间吐完结果
    let audio_seconds = samples.len() as u64 / sauc::SAMPLE_RATE as u64;
    let timeout = FINISH_TIMEOUT + Duration::from_secs(audio_seconds / 2);
    session.finish(timeout, FINISH_QUIET * 2).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 配置缺密钥给出中文提示() {
        let config = DoubaoConfig {
            api_key: " ".into(),
            resource_id: sauc::DEFAULT_RESOURCE_ID.into(),
            endpoint: sauc::DEFAULT_ENDPOINT.into(),
            end_window_size: 0,
            hotwords: Vec::new(),
        };
        assert!(config.ensure().unwrap_err().to_string().contains("Access Token"));
        assert!(!format!("{config:?}").contains("api_key:"));
    }

    fn local_config(endpoint: String) -> DoubaoConfig {
        DoubaoConfig {
            api_key: "test-key".into(),
            resource_id: "volc.seedasr.sauc.duration".into(),
            endpoint,
            end_window_size: 800,
            hotwords: vec!["落墨".into()],
        }
    }

    /// 本地 WebSocket 桩：收下全量请求后，`on_audio` 为真时每收到一个音频包回一个结果帧，
    /// 否则一直不回、直到 `close_after` 到点主动关闭（模拟服务端空闲断开）。
    async fn stub_server(on_audio: bool, close_after: Option<Duration>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _request = ws.next().await;
            let mut sent = 0;
            loop {
                let next = match close_after {
                    Some(after) => match tokio::time::timeout(after, ws.next()).await {
                        Ok(next) => next,
                        Err(_) => {
                            let _ = ws.close(None).await;
                            return;
                        }
                    },
                    None => ws.next().await,
                };
                match next {
                    Some(Ok(Message::Binary(_))) if on_audio => {
                        sent += 1;
                        let text = if sent == 1 { "你好" } else { "你好世界" };
                        let frame =
                            sauc::build_server_result_frame(&json!({ "result": { "text": text } }), false, false);
                        let _ = ws.send(Message::binary(frame)).await;
                    }
                    Some(Ok(_)) => {}
                    _ => return,
                }
            }
        });
        format!("ws://{address}")
    }

    #[test]
    fn 预热匹配键_参数变了就不复用_只预热按时长计费() {
        let config = local_config("ws://127.0.0.1:1".into());
        let key = WarmKey::new(&config, "zh");
        assert_eq!(key, WarmKey::new(&config.clone(), "zh"));
        assert_ne!(key, WarmKey::new(&config, "en"), "语言变了");
        for change in [
            DoubaoConfig { api_key: "other".into(), ..config.clone() },
            DoubaoConfig { resource_id: "volc.seedasr.sauc.concurrent".into(), ..config.clone() },
            DoubaoConfig { end_window_size: 1200, ..config.clone() },
            DoubaoConfig { hotwords: Vec::new(), ..config.clone() },
        ] {
            assert_ne!(key, WarmKey::new(&change, "zh"), "{change:?}");
        }
        assert!(!format!("{key:?}").contains("test-key"), "匹配键里不留明文凭据");
        assert!(warm_allowed(&config));
        assert!(!warm_allowed(&DoubaoConfig { resource_id: "volc.seedasr.sauc.concurrent".into(), ..config }));
        assert!(WARM_KEEP <= Duration::from_secs(30));
    }

    #[tokio::test]
    async fn 预热连接_按下时接上回调_推流收到中间结果() {
        let endpoint = stub_server(true, None).await;
        // 预热：建连时还没有会话回调
        let mut session = DoubaoSession::connect(&local_config(endpoint), "zh", "", None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(session.check_alive().is_ok());

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        session.set_partial_callback(Some(Arc::new(move |partial: Partial| sink.lock().unwrap().push(partial.text))));
        session.push(&vec![0i16; sauc::STREAM_CHUNK_SAMPLES * 2]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while seen.lock().unwrap().len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("接上的回调应收到中间结果");
        assert_eq!(*seen.lock().unwrap(), ["你好", "你好世界"]);
        session.cancel().await;
    }

    #[tokio::test]
    async fn 预热连接_服务端空闲断开后_check_alive_报错() {
        let endpoint = stub_server(false, Some(Duration::from_millis(100))).await;
        let session = DoubaoSession::connect(&local_config(endpoint), "zh", "", None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), session.wait_closed()).await.expect("应等到断开");
        assert!(session.check_alive().is_err(), "断开的预热连接不能再用");
    }

    #[test]
    fn 结果帧推进文本与末包() {
        let shared = Shared {
            state: Mutex::new(TranscriptState::default()),
            notify: Notify::new(),
            on_partial: Mutex::new(None),
        };
        let frame = sauc::build_server_result_frame(
            &json!({"result": {"text": "你好世", "utterances": [
                {"text": "你好", "definite": true}, {"text": "世", "definite": false}
            ]}}),
            false,
            false,
        );
        let partial = |text: &str, pending_chars| Some(Partial { text: text.into(), pending_chars });
        assert_eq!(handle_frame(&shared, &frame, "t"), partial("你好世", 1));
        // 文本与确认进度都不变：不重复回调
        assert_eq!(handle_frame(&shared, &frame, "t"), None);
        let last = sauc::build_server_result_frame(
            &json!({"audio_info": {"duration": 900}, "result": {"text": "你好世界", "utterances": [
                {"text": "你好", "definite": true}, {"text": "世界", "definite": true}
            ]}}),
            true,
            true,
        );
        assert_eq!(handle_frame(&shared, &last, "t"), partial("你好世界", 0));
        let state = shared.lock();
        assert!(state.server_final);
        assert_eq!(state.audio_duration_ms, 900);
        assert_eq!(state.frame_count, 3);
    }

    #[test]
    fn 文本不变只是被确认_照样回调() {
        let shared = Shared {
            state: Mutex::new(TranscriptState::default()),
            notify: Notify::new(),
            on_partial: Mutex::new(None),
        };
        let frame = |definite: bool| {
            sauc::build_server_result_frame(
                &json!({"result": {"text": "你好世界", "utterances": [
                    {"text": "你好", "definite": true}, {"text": "世界", "definite": definite}
                ]}}),
                false,
                false,
            )
        };
        assert_eq!(
            handle_frame(&shared, &frame(false), "t"),
            Some(Partial { text: "你好世界".into(), pending_chars: 2 })
        );
        assert_eq!(
            handle_frame(&shared, &frame(true), "t"),
            Some(Partial { text: "你好世界".into(), pending_chars: 0 })
        );
    }

    #[test]
    fn 错误帧标记关闭并保留错误() {
        let shared = Shared {
            state: Mutex::new(TranscriptState::default()),
            notify: Notify::new(),
            on_partial: Mutex::new(None),
        };
        let mut frame = vec![0x11, 0xF0, 0x10, 0x00];
        frame.extend_from_slice(&45_000_081u32.to_be_bytes());
        frame.extend_from_slice(&0u32.to_be_bytes());
        handle_frame(&shared, &frame, "t");
        let state = shared.lock();
        assert!(state.closed);
        assert!(state.closed_error().info.retryable);
    }
}
