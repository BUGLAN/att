//! 豆包双向流式 ASR 会话（tokio-tungstenite 实现，移植自 Python `doubao_asr_provider.py`）。
//!
//! 一条会话的生命周期：
//! 1. [`DoubaoSession::open`]：握手（三个鉴权头）；[`DoubaoSession::begin`]：发送全量客户端请求。
//!    [`DoubaoSession::connect`] 把两步连着做；
//! 2. [`DoubaoSession::push`]：16k 单声道 i16 PCM 切帧推流（实时 100ms，批量 200ms）；
//!    后台读任务持续解析服务端帧，按 definite 前缀推进文本并回调中间结果；
//! 3. [`DoubaoSession::finish`]：发末包 → 等末包结果/关闭/静默窗口 → 返回整场文字稿；
//! 4. [`DoubaoSession::cancel`]：用户取消，不算失败。
//!
//! 预热（待命连接）：握手要 1~3 秒，是按下到出字之间最大的一块。空闲时先 [`DoubaoSession::open`]
//! 握好手、不发全量请求，连接就能一直备着；按下时用 [`WarmKey`] 确认接入参数没变、
//! [`DoubaoSession::check_alive`] 与 [`DoubaoSession::idle_for`] 确认没断，再 [`DoubaoSession::begin`]
//! 发请求并直接推流。保活靠心跳，判活与取值见 [`WARM_STALE`]。
//!
//! 约定：密钥只进握手头，不落日志、不进错误文案；连接直连（tungstenite 不读系统代理，
//! 与 Python 版显式 `proxy=None` 语义一致——境内服务不应绕代理）。

use super::sauc::{self, ErrorInfo, ErrorKind, ServerFrame};
use futures_util::stream::{SplitSink, StreamExt};
use futures_util::SinkExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
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
/// 预热连接多久没收到任何帧就当它断了（判活用墙上时钟，系统睡眠的时间也算进去）。
///
/// 2026-11-22 实测（`examples/doubao_probe.rs`，`*.duration` 资源）：
/// - 发出全量请求后不发音频：8.4 秒时服务端回错误帧「等包超时」并断开，WebSocket Ping 续不了命
///   ——所以预热连接只握手、不发全量请求；
/// - 只握手不发请求：服务端每 15 秒发一个 Ping，连接留到约 15 分钟（实测 915 秒）才以
///   close 1000「connection expired」回收（保活方发现后重建一条即可）；期间随时发全量请求 + 音频
///   都照常识别（空闲 20 / 40 / 60 / 90 秒各测过），计费时长只算音频；
/// - 全量请求发出后约 0.35 秒服务端回一个空结果帧（`bigmodel` / `bigmodel_async` / `bigmodel_nostream`
///   都有），可当作「连接真的还通」的回执（见 [`DoubaoSession::acked`]）。
///
/// 取 3 个服务端心跳周期；我们自己也按 [`WARM_PING_INTERVAL`] 发 Ping，对端不主动发心跳时靠 Pong 判活。
pub const WARM_STALE: Duration = Duration::from_secs(45);
/// 预热连接的心跳间隔。
pub const WARM_PING_INTERVAL: Duration = Duration::from_secs(15);
/// 预热连接备到这么久就换一条新的：服务端约 15 分钟回收连接，别让一场识别开在快到期的连接上。
pub const WARM_MAX_AGE: Duration = Duration::from_secs(300);

/// 豆包会话错误：带归一化分类（上层据此决定「去修复」还是「立即重试」）。
#[derive(Debug, Clone)]
pub struct SttError {
    pub info: ErrorInfo,
}

impl SttError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, next_action: &str) -> Self {
        Self { info: ErrorInfo::new(kind, message, next_action) }
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
                "",
            ));
        }
        if self.endpoint.trim().is_empty() {
            return Err(SttError::new(ErrorKind::Protocol, "未配置豆包语音 WebSocket 接入点地址", ""));
        }
        if self.resource_id.trim().is_empty() {
            return Err(SttError::new(ErrorKind::Protocol, "未配置豆包语音 Resource ID", ""));
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
    /// 最近一次收到对端任何消息（含 Ping / Pong）的墙上时刻；建连时初始化
    last_inbound_at: Option<SystemTime>,
    /// 发出全量请求后服务端是否回过二进制帧
    acked: bool,
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
        if let Some(code) = self.close_code.filter(|code| *code != 1000) {
            return SttError { info: sauc::classify_close_code(code, &self.close_reason) };
        }
        // 1000 常见于服务端 VAD 判定长时间静音后关闭；但刚建连就断同样走到这里，
        // 文案必须带上 code/reason，别把真实原因藏进一句「静音」里
        let mut detail = format!("（code={:?}", self.close_code);
        if !self.close_reason.is_empty() {
            detail.push_str(&format!("，reason={}", self.close_reason));
        }
        detail.push('）');
        SttError::new(
            ErrorKind::Network,
            format!("流式连接被服务端关闭{detail}；录音较久可能是静音超时，刚建连就断请检查「强制断句静音时长」等设置"),
            "重新开始录音",
        )
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
            // result_type=full 的 text 是整场累积文本，是最终输出的权威来源。
            // 句级列表偶尔会重叠时，只按已确认前缀重算待定尾巴，不能让重叠部分污染全文。
            let committed_text = self.committed.concat();
            if !result.text.is_empty() && result.text.starts_with(&committed_text) {
                self.pending = result.text[committed_text.len()..].to_string();
            }
        }
        self.last_text =
            if result.text.is_empty() { format!("{}{}", self.committed.concat(), self.pending) } else { result.text };
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
    /// 是否收到服务端末包（`is_last`）。
    pub complete: bool,
    /// 没收到末包时的降级原因；调用方应保留正文并展示或落库。
    pub warning: Option<String>,
}

pub const INCOMPLETE_WARNING: &str = "识别可能不完整：未收到服务端末包";

/// 预热连接的匹配键：握手时用到的参数（接入点、资源、凭据）任何一项变了，预热连接就不能用。
/// 语言、断句时长、热词写在全量请求里，按下时才发，不影响复用。凭据只存摘要，不多留一份明文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmKey {
    endpoint: String,
    resource_id: String,
    api_key_digest: String,
}

impl WarmKey {
    pub fn new(config: &DoubaoConfig) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            endpoint: config.endpoint.trim().to_string(),
            resource_id: config.resource_id.trim().to_string(),
            api_key_digest: crate::clipboard::hex(&Sha256::digest(config.api_key.trim().as_bytes())),
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
    /// 握手完成的时刻（日志里看预热连接备了多久）
    pub opened_at: Instant,
    /// 全量请求是否已发出：发出之后才能推音频
    begun: bool,
}

impl DoubaoSession {
    /// 握手 + 发送全量请求。`on_partial` 在每个结果帧后以整场当前文本回调。
    pub async fn connect(
        config: &DoubaoConfig,
        language: &str,
        context: &str,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, SttError> {
        let mut session = Self::open(config).await?;
        session.begin(config, language, context, on_partial).await?;
        Ok(session)
    }

    /// 只握手、不发全量请求：连接可以一直备着（预热），用之前再 [`Self::begin`]。
    pub async fn open(config: &DoubaoConfig) -> Result<Self, SttError> {
        config.ensure()?;
        let connect_id = uuid::Uuid::new_v4().to_string();
        let mut request = config.endpoint.as_str().into_client_request().map_err(|e| {
            SttError::new(ErrorKind::Protocol, format!("接入点地址无效（{e}）"), "检查 WebSocket 接入点地址")
        })?;
        let headers = request.headers_mut();
        let header = |value: &str| {
            HeaderValue::from_str(value.trim()).map_err(|_| {
                SttError::new(ErrorKind::Auth, "Access Token / Resource ID 含非法字符", "在设置中重新填写")
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
        // 关掉 Nagle：音频是 100ms 一个的小包，攒包只会让它晚到
        let connected =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async_with_config(request, None, true))
                .await;
        let (stream, response) = match connected {
            Err(_) => {
                return Err(SttError::new(ErrorKind::Network, "连接豆包语音服务超时（10 秒）", "检查网络后重试"));
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
                    "检查网络后重试",
                ));
            }
            Ok(Ok(pair)) => pair,
        };
        let log_id = response.headers().get("x-tt-logid").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();

        tracing::info!(
            connect_id = %connect_id,
            log_id = %log_id,
            "豆包流式握手完成（{:?}）",
            handshake_started.elapsed()
        );
        let (writer, mut read) = stream.split();

        let shared = Arc::new(Shared {
            state: Mutex::new(TranscriptState { last_inbound_at: Some(SystemTime::now()), ..Default::default() }),
            notify: Notify::new(),
            on_partial: Mutex::new(None),
        });
        let reader_shared = shared.clone();
        let reader_connect_id = connect_id.clone();
        let reader = tokio::spawn(async move {
            while let Some(message) = read.next().await {
                if message.is_ok() {
                    // 任何消息（含服务端心跳 Ping、我们心跳的 Pong）都说明连接还通
                    reader_shared.lock().last_inbound_at = Some(SystemTime::now());
                }
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
            begun: false,
        })
    }

    /// 发送全量请求，开始一场识别；之后才能 [`Self::push`]。`on_partial` 在每个结果帧后以整场当前文本回调。
    pub async fn begin(
        &mut self,
        config: &DoubaoConfig,
        language: &str,
        context: &str,
        on_partial: Option<PartialCallback>,
    ) -> Result<(), SttError> {
        self.check_alive()?;
        *self.shared.on_partial.lock().unwrap_or_else(|e| e.into_inner()) = on_partial;
        let uid = format!("att-desktop-{}", &self.connect_id[..8]);
        let payload = sauc::build_bigmodel_request(&uid, language, context, &config.hotwords, config.end_window_size);
        if let Err(error) = self.writer.send(Message::binary(sauc::build_full_client_request(&payload))).await {
            return Err(SttError::new(ErrorKind::Network, format!("发送请求失败（{error}）"), "重新开始录音"));
        }
        self.begun = true;
        Ok(())
    }

    /// 发一个 WebSocket Ping（预热连接的心跳）：对端回的 Pong 会刷新 [`Self::idle_for`]。
    pub async fn ping(&mut self) -> Result<(), SttError> {
        self.check_alive()?;
        self.writer
            .send(Message::Ping(Vec::new().into()))
            .await
            .map_err(|error| SttError::new(ErrorKind::Network, format!("心跳发送失败（{error}）"), ""))
    }

    /// 距上次收到对端任何消息过了多久（墙上时钟：系统睡眠的时间也算，醒来后据此发现连接早断了）。
    /// 系统时钟被往回拨时按「刚收到」算，宁可多信一次、由回执超时兜底。
    pub fn idle_for(&self) -> Duration {
        let last = self.shared.lock().last_inbound_at;
        last.and_then(|at| at.elapsed().ok()).unwrap_or_default()
    }

    /// 全量请求发出后服务端是否回过帧（正常约 0.35 秒内回一个空结果帧）：
    /// 复用预热连接时据此确认它不是一条「写得进去、其实早断了」的死连接。
    pub fn acked(&self) -> bool {
        self.shared.lock().acked
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
        if !self.begun {
            return Err(SttError::new(ErrorKind::Protocol, "识别请求还没发出，不能推送音频", ""));
        }
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
            return Err(SttError::new(ErrorKind::Network, format!("发送音频失败（{error}）"), "重新开始录音"));
        }
        Ok(())
    }

    /// 发送末包并收尾；中途已出错则返回归一化错误。
    ///
    /// `quiet` 只适用于实时录音。批量识别传 `None`，一直等到服务端末包或 `timeout`，
    /// 避免音频里的长停顿被误判为结果已收完。
    pub async fn finish(mut self, timeout: Duration, quiet: Option<Duration>) -> Result<FinishOutcome, SttError> {
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
                    if quiet.is_some_and(|quiet| {
                        state.frame_count > baseline
                            && state.last_frame_at.map(|t| t.elapsed() >= quiet).unwrap_or(false)
                    }) {
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
        let complete = state.server_final;
        Ok(FinishOutcome {
            text: state.final_text(),
            audio_duration_ms: state.audio_duration_ms,
            complete,
            warning: (!complete).then(|| INCOMPLETE_WARNING.to_string()),
        })
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
    state.acked = true;
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
        return Err(SttError::new(ErrorKind::Input, "音频为空，无法转写", ""));
    }
    let mut session = DoubaoSession::connect(config, language, context, None).await?;
    session.frame_samples = sauc::CHUNK_SAMPLES;
    for (index, chunk) in samples.chunks(sauc::CHUNK_SAMPLES).enumerate() {
        if !on_push(index * sauc::CHUNK_SAMPLES) {
            session.close().await;
            return Err(SttError::new(ErrorKind::Input, super::TRANSCRIBE_CANCELLED, ""));
        }
        session.push(chunk).await?;
    }
    on_push(samples.len());
    // 批量推流比实时快得多，服务端需要更长时间吐完结果
    let audio_seconds = samples.len() as u64 / sauc::SAMPLE_RATE as u64;
    let timeout = FINISH_TIMEOUT + Duration::from_secs(audio_seconds / 2);
    session.finish(timeout, None).await
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

    /// 本地 WebSocket 桩，行为照着实测的服务端：第一个二进制帧是全量请求，回一个空结果帧当回执；
    /// 之后 `on_audio` 为真时每收到一个音频包回一个结果帧。`close_after` 这么久没收到任何消息就主动关闭
    /// （模拟服务端断开）；Ping 由 tungstenite 自动回 Pong。
    async fn stub_server(on_audio: bool, close_after: Option<Duration>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut requested = false;
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
                    Some(Ok(Message::Binary(_))) if !requested => {
                        requested = true;
                        let frame = sauc::build_server_result_frame(&json!({ "result": { "text": "" } }), false, false);
                        let _ = ws.send(Message::binary(frame)).await;
                    }
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

    /// 批量桩：收到音频后先回正文，再延迟发末包；验证批量收尾不会按静默窗口提前结束。
    async fn delayed_final_stub() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut requested = false;
            while let Some(Ok(message)) = ws.next().await {
                if !matches!(message, Message::Binary(_)) {
                    continue;
                }
                if !requested {
                    requested = true;
                    let ack = sauc::build_server_result_frame(&json!({ "result": { "text": "" } }), false, false);
                    let _ = ws.send(Message::binary(ack)).await;
                    continue;
                }
                let partial =
                    sauc::build_server_result_frame(&json!({ "result": { "text": "半截结果" } }), false, false);
                let _ = ws.send(Message::binary(partial)).await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let last = sauc::build_server_result_frame(&json!({ "result": { "text": "完整结果" } }), true, true);
                let _ = ws.send(Message::binary(last)).await;
                return;
            }
        });
        format!("ws://{address}")
    }

    #[test]
    fn 预热匹配键_只看握手参数_只预热按时长计费() {
        let config = local_config("ws://127.0.0.1:1".into());
        let key = WarmKey::new(&config);
        // 断句时长、热词写在全量请求里，按下时才发：变了照样复用
        for same in [
            DoubaoConfig { end_window_size: 1200, ..config.clone() },
            DoubaoConfig { hotwords: Vec::new(), ..config.clone() },
        ] {
            assert_eq!(key, WarmKey::new(&same), "{same:?}");
        }
        for change in [
            DoubaoConfig { api_key: "other".into(), ..config.clone() },
            DoubaoConfig { resource_id: "volc.seedasr.sauc.concurrent".into(), ..config.clone() },
            DoubaoConfig { endpoint: "ws://127.0.0.1:2".into(), ..config.clone() },
        ] {
            assert_ne!(key, WarmKey::new(&change), "{change:?}");
        }
        assert!(!format!("{key:?}").contains("test-key"), "匹配键里不留明文凭据");
        assert!(warm_allowed(&config));
        assert!(!warm_allowed(&DoubaoConfig { resource_id: "volc.seedasr.sauc.concurrent".into(), ..config }));
        assert!(WARM_PING_INTERVAL * 2 < WARM_STALE, "漏一次心跳不该被当成断开");
        assert!(WARM_MAX_AGE < Duration::from_secs(900), "要赶在服务端回收之前换新");
    }

    #[tokio::test]
    async fn 预热连接_按下时才发请求_推流收到中间结果() {
        let endpoint = stub_server(true, None).await;
        let config = local_config(endpoint);
        // 预热：只握手，不发全量请求
        let mut session = DoubaoSession::open(&config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(session.check_alive().is_ok());
        assert!(!session.acked());
        assert!(session.push(&[0i16; 16]).await.is_err(), "请求没发出前不能推音频");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let callback: PartialCallback = Arc::new(move |partial: Partial| sink.lock().unwrap().push(partial.text));
        session.begin(&config, "zh", "", Some(callback)).await.unwrap();
        session.push(&vec![0i16; sauc::STREAM_CHUNK_SAMPLES * 2]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while seen.lock().unwrap().len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("接上的回调应收到中间结果");
        assert_eq!(*seen.lock().unwrap(), ["你好", "你好世界"]);
        assert!(session.acked(), "服务端回过帧");
        session.cancel().await;
    }

    #[tokio::test]
    async fn 预热连接_心跳的回应刷新空闲时长() {
        let endpoint = stub_server(false, None).await;
        let mut session = DoubaoSession::open(&local_config(endpoint)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(session.idle_for() >= Duration::from_millis(100), "没有任何来帧，空闲时长在涨");
        session.ping().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while session.idle_for() >= Duration::from_millis(100) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("收到 Pong 后空闲时长应归零");
        assert!(!session.acked(), "心跳不算识别请求的回执");
        session.cancel().await;
    }

    #[tokio::test]
    async fn 预热连接_服务端断开后_不能再用() {
        let endpoint = stub_server(false, Some(Duration::from_millis(100))).await;
        let config = local_config(endpoint);
        let mut session = DoubaoSession::open(&config).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), session.wait_closed()).await.expect("应等到断开");
        assert!(session.check_alive().is_err(), "断开的预热连接不能再用");
        assert!(session.begin(&config, "zh", "", None).await.is_err());
        assert!(session.ping().await.is_err());
    }

    #[tokio::test]
    async fn 批量收尾忽略静默窗口并等待末包() {
        let endpoint = delayed_final_stub().await;
        let mut session = DoubaoSession::connect(&local_config(endpoint), "zh", "", None).await.unwrap();
        session.push(&vec![0i16; sauc::CHUNK_SAMPLES]).await.unwrap();
        let outcome = session.finish(Duration::from_secs(2), None).await.unwrap();
        assert_eq!(outcome.text, "完整结果");
        assert!(outcome.complete);
        assert!(outcome.warning.is_none());
    }

    #[tokio::test]
    async fn 未收到末包时保留正文并标记不完整() {
        let endpoint = stub_server(true, Some(Duration::from_millis(200))).await;
        let mut session = DoubaoSession::connect(&local_config(endpoint), "zh", "", None).await.unwrap();
        // 只送入不足一帧的尾音频，让 finish 发送的唯一音频包得到一次结果帧。
        session.push(&[0i16; 16]).await.unwrap();
        let outcome = session.finish(Duration::from_secs(2), Some(Duration::from_millis(30))).await.unwrap();
        assert_eq!(outcome.text, "你好");
        assert!(!outcome.complete);
        assert_eq!(outcome.warning.as_deref(), Some(INCOMPLETE_WARNING));
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
    fn 句级结果重叠时全文以累积文本为准且待定尾巴正确() {
        let mut state = TranscriptState::default();
        let text = "你知道我是谁吗？";
        state.apply_result(sauc::ResultFrame {
            text: text.into(),
            utterances: vec![
                sauc::Utterance { text: text.into(), definite: true, ..Default::default() },
                sauc::Utterance { text: text.into(), definite: false, ..Default::default() },
            ],
            audio_duration_ms: 900,
        });
        assert_eq!(state.final_text(), text);
        assert_eq!(state.partial().pending_chars, 0);
    }

    #[test]
    fn 连续说两遍同句时仍输出两遍() {
        let mut state = TranscriptState::default();
        let text = "你好。你好。";
        state.apply_result(sauc::ResultFrame {
            text: text.into(),
            utterances: vec![
                sauc::Utterance { text: "你好。".into(), definite: true, ..Default::default() },
                sauc::Utterance { text: "你好。".into(), definite: true, ..Default::default() },
            ],
            audio_duration_ms: 1100,
        });
        assert_eq!(state.final_text(), text);
        assert_eq!(state.partial().pending_chars, 0);
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
        assert_eq!(state.closed_error().info.kind, ErrorKind::Network);
    }
}
