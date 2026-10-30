//! 录音会话 Actor（架构文档 §5 / §7.1，当前接入豆包在线引擎）。
//!
//! 单个 tokio 任务串行消费命令（UI 按钮 / 全局热键 / 会话任务回报），状态机：
//!
//! ```text
//! Idle ─start→ Connecting(流式建连中，采集已在收数) ─connected→ Recording
//!      ─start(单段/多段)→ Recording
//! Connecting/Recording ─stop→ Finalizing ─done→ Idle（completed / error）
//! 任意非 Idle ─cancel→ Idle（cancelled）
//! ```
//!
//! 三防御语义在这里天然成立：
//! - 不丢开头：采集先于建连启动，建连期间的样本缓存在会话任务里，就绪后统一冲刷；
//! - 不丢尾：停止 = 停采集并等采集线程把环里尾部样本全部交付，之后才发末包；
//! - 启动窗口排队停止：命令串行处理，Connecting 期间收到 stop 照常收尾，不会出现幽灵录音。
//!
//! 前端事件：`recording:state`（相位 + 文本 + 记录）、`recording:partial`（实时文本）、
//! `recording:level`（音量 0..1）。

use att_core::records::{Record, RecordStatus};
use att_core::settings::hotkey::{HotkeyAction, TriggerMode};
use att_core::state::CoreState;
use att_core::stt::{self, doubao};
use att_hotkey::KeyPhase;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

/// 低于该时长视为误触（不建记录、不计费）。
const MIN_DURATION_SECONDS: f64 = 0.3;
const SAMPLE_RATE: f64 = 16_000.0;
/// 音量事件节流（≈25Hz，足够驱动波形条）。
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Connecting,
    Recording,
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 真流式：边录边推，实时出字
    Streaming,
    /// 单段/多段：录完整段后一次性识别（多段的 VAD 分段尚未移植，按单段处理）
    Batch,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::Streaming => "streaming",
            Mode::Batch => "single",
        }
    }
}

/// 会话任务的最终产出。
struct SessionOutcome {
    samples: Vec<i16>,
    text: String,
    /// 识别失败原因（有部分文本时为降级成功）
    error: Option<String>,
    cancelled: bool,
    processing_time: f64,
}

enum Cmd {
    Start,
    Stop,
    Cancel,
    Hotkey(HotkeyAction, KeyPhase),
    /// 流式会话建连成功
    Connected(u64),
    /// 录音途中识别失败：立即停采集进入收尾（已收文本兜底建记录）
    Failed(u64),
    Done(u64, SessionOutcome),
}

struct Active {
    id: u64,
    mode: Mode,
    capture: Option<att_audio::CaptureHandle>,
    cancel: Option<oneshot::Sender<()>>,
    /// 本次录音由我们静音了系统输出，结束时需要恢复
    muted_by_us: bool,
}

#[derive(Clone)]
pub struct RecordingController {
    tx: mpsc::UnboundedSender<Cmd>,
    snapshot: Arc<Mutex<Value>>,
}

impl RecordingController {
    pub fn spawn(app: AppHandle, core: Arc<CoreState>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let snapshot = Arc::new(Mutex::new(json!({ "phase": "idle" })));
        let actor = Actor {
            app,
            core,
            tx: tx.clone(),
            phase: Phase::Idle,
            active: None,
            next_id: 0,
            snapshot: snapshot.clone(),
        };
        tauri::async_runtime::spawn(actor.run(rx));
        Self { tx, snapshot }
    }

    pub fn start(&self) {
        let _ = self.tx.send(Cmd::Start);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    pub fn hotkey(&self, action: HotkeyAction, phase: KeyPhase) {
        let _ = self.tx.send(Cmd::Hotkey(action, phase));
    }

    /// 最近一次发出的状态（前端挂载时同步用）。
    pub fn state(&self) -> Value {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_else(|_| json!({ "phase": "idle" }))
    }
}

struct Actor {
    app: AppHandle,
    core: Arc<CoreState>,
    tx: mpsc::UnboundedSender<Cmd>,
    phase: Phase,
    active: Option<Active>,
    next_id: u64,
    snapshot: Arc<Mutex<Value>>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Start => self.start().await,
                Cmd::Stop => self.stop("正在完成转写"),
                Cmd::Cancel => self.cancel(),
                Cmd::Hotkey(action, key_phase) => self.on_hotkey(action, key_phase).await,
                Cmd::Connected(id) => {
                    if self.is_current(id) && self.phase == Phase::Connecting {
                        self.phase = Phase::Recording;
                        self.emit_state("recording", json!({ "message": "豆包语音转写中" }));
                    }
                }
                Cmd::Failed(id) => {
                    if self.is_current(id) && matches!(self.phase, Phase::Connecting | Phase::Recording) {
                        self.stop("识别连接中断，正在保存已识别内容");
                    }
                }
                Cmd::Done(id, outcome) => {
                    if self.is_current(id) {
                        self.finish(outcome).await;
                    }
                }
            }
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.active.as_ref().map(|a| a.id == id).unwrap_or(false)
    }

    fn emit_state(&self, phase: &str, extra: Value) {
        let mode = self.active.as_ref().map(|a| a.mode.as_str()).unwrap_or("streaming");
        let mut payload = json!({ "phase": phase, "mode": mode });
        if let (Some(target), Some(source)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        if let Ok(mut snapshot) = self.snapshot.lock() {
            *snapshot = payload.clone();
        }
        match crate::floating::from_recording_state(&payload) {
            Some((state, show)) => crate::floating::update(&self.app, state, show),
            None => crate::floating::hide(&self.app),
        }
        let _ = self.app.emit("recording:state", payload);
    }

    async fn on_hotkey(&mut self, action: HotkeyAction, key_phase: KeyPhase) {
        if action != HotkeyAction::Record {
            return;
        }
        let trigger = self
            .core
            .settings()
            .binding_of(HotkeyAction::Record)
            .map(|b| b.trigger)
            .unwrap_or(TriggerMode::Hold);
        tracing::debug!("录音热键 {key_phase:?}（{trigger:?}），当前 {:?}", self.phase);
        match (trigger, key_phase) {
            (TriggerMode::Hold, KeyPhase::Pressed) => self.start().await,
            (TriggerMode::Hold, KeyPhase::Released) => self.stop("正在完成转写"),
            (TriggerMode::Toggle, KeyPhase::Pressed) => match self.phase {
                Phase::Idle => self.start().await,
                Phase::Connecting | Phase::Recording => self.stop("正在完成转写"),
                Phase::Finalizing => {}
            },
            (TriggerMode::Toggle, KeyPhase::Released) => {}
        }
    }

    fn fail_start(&mut self, message: String) {
        tracing::warn!("录音无法开始: {message}");
        self.phase = Phase::Idle;
        self.active = None;
        self.emit_state("error", json!({ "message": message }));
    }

    async fn start(&mut self) {
        if self.phase != Phase::Idle {
            // 收尾中的按下与重复按下一律吞掉（与旧版语义一致）
            return;
        }
        let settings = self.core.settings();
        if let Some(message) = stt::unsupported_provider_message(&settings.asr_provider) {
            return self.fail_start(message);
        }
        let config = stt::doubao_config(&self.core);
        if let Err(error) = config.ensure() {
            return self.fail_start(error.to_string());
        }
        let mode = if settings.recording_mode == "streaming" { Mode::Streaming } else { Mode::Batch };
        let language = settings.asr_language.clone();
        self.next_id += 1;
        let id = self.next_id;

        // 采集最先启动（按下即收数），建连与静音并行进行
        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let level_app = self.app.clone();
        let mut last_level_emit = Instant::now() - LEVEL_EMIT_INTERVAL;
        let on_chunk: att_audio::ChunkCallback = Box::new(move |pcm, level| {
            let _ = audio_tx.send(pcm.to_vec());
            if last_level_emit.elapsed() >= LEVEL_EMIT_INTERVAL {
                last_level_emit = Instant::now();
                let _ = level_app.emit("recording:level", json!({ "level": level }));
            }
        });
        let device = settings.audio_input_device.clone();
        let opened = tokio::task::spawn_blocking(move || att_audio::start_capture(device, on_chunk)).await;
        let capture = match opened {
            Ok(Ok(capture)) => capture,
            Ok(Err(message)) => return self.fail_start(message),
            Err(error) => return self.fail_start(format!("打开麦克风失败: {error}")),
        };

        let (cancel_tx, cancel_rx) = oneshot::channel();
        self.active = Some(Active {
            id,
            mode,
            capture: Some(capture),
            cancel: Some(cancel_tx),
            muted_by_us: false,
        });

        let tx = self.tx.clone();
        match mode {
            Mode::Streaming => {
                self.phase = Phase::Connecting;
                self.emit_state("connecting", json!({ "message": "正在连接豆包语音服务" }));
                let app = self.app.clone();
                tauri::async_runtime::spawn(streaming_task(id, config, language, audio_rx, cancel_rx, app, tx));
            }
            Mode::Batch => {
                self.phase = Phase::Recording;
                self.emit_state("recording", json!({ "message": "正在录音" }));
                tauri::async_runtime::spawn(batch_task(id, config, language, audio_rx, cancel_rx, tx));
            }
        }

        // 静音放在建连之后（osascript 两次调用约 300ms，不能挡在建连前面）
        if settings.pause_audio_on_recording {
            let muted = mute_output().await;
            if let Some(active) = self.active.as_mut().filter(|a| a.id == id) {
                active.muted_by_us = muted;
            } else if muted {
                // 极短录音：静音完成前会话已结束，立即恢复
                tokio::task::spawn_blocking(|| crate::desktop::set_output_muted(false));
            }
        }
    }

    /// 停止采集进入收尾：采集线程冲刷完尾部样本后关闭音频通道，会话任务据此发末包。
    fn stop(&mut self, message: &str) {
        if !matches!(self.phase, Phase::Connecting | Phase::Recording) {
            return;
        }
        self.phase = Phase::Finalizing;
        if let Some(capture) = self.active.as_mut().and_then(|a| a.capture.take()) {
            std::thread::spawn(move || capture.stop());
        }
        self.emit_state("finalizing", json!({ "message": message }));
    }

    fn cancel(&mut self) {
        if self.phase == Phase::Idle {
            return;
        }
        if let Some(active) = self.active.as_mut() {
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
            if let Some(capture) = active.capture.take() {
                std::thread::spawn(move || capture.stop());
            }
        }
        self.phase = Phase::Finalizing;
    }

    async fn finish(&mut self, outcome: SessionOutcome) {
        let Some(mut active) = self.active.take() else { return };
        if let Some(capture) = active.capture.take() {
            std::thread::spawn(move || capture.stop());
        }
        if active.muted_by_us {
            tokio::task::spawn_blocking(|| crate::desktop::set_output_muted(false));
        }
        self.phase = Phase::Idle;
        // 让 emit_state 仍能带上本次模式
        let mode = active.mode;
        self.active = Some(Active { capture: None, cancel: None, ..active });
        let duration = outcome.samples.len() as f64 / SAMPLE_RATE;

        if outcome.cancelled {
            self.emit_state("cancelled", json!({ "message": "已取消本次录音" }));
        } else if duration < MIN_DURATION_SECONDS && outcome.text.is_empty() {
            self.emit_state("completed", json!({ "message": "录音时间过短", "text": "" }));
        } else if outcome.text.is_empty() {
            match outcome.error {
                // 识别失败且没有任何文本：音频落盘并建失败记录，避免录音丢失
                Some(error) => {
                    let record = self.save_record(&outcome.samples, duration, None, Some(error.clone()), outcome.processing_time).await;
                    self.emit_state("error", json!({ "message": error, "record": record }));
                }
                None => self.emit_state("completed", json!({ "message": "没有识别到语音内容", "text": "" })),
            }
        } else {
            let record = self
                .save_record(&outcome.samples, duration, Some(outcome.text.clone()), outcome.error.clone(), outcome.processing_time)
                .await;
            let (copied, pasted, copy_error) = self.auto_copy(&outcome.text).await;
            let message = match &outcome.error {
                Some(error) => format!("识别中断，已保存已识别内容：{error}"),
                None => "录音已完成".to_string(),
            };
            self.emit_state(
                "completed",
                json!({
                    "message": message,
                    "text": outcome.text,
                    "record": record,
                    "copied": copied,
                    "pasted": pasted,
                    "copy_error": copy_error,
                    "degraded": outcome.error.is_some(),
                }),
            );
        }
        tracing::info!(
            "录音会话结束（{}，{:.2}s，识别 {} 字）",
            mode.as_str(),
            duration,
            outcome.text.chars().count()
        );
        self.active = None;
    }

    /// 音频落盘 + 建记录；返回记录 JSON（失败时返回 null，不阻断提示）。
    async fn save_record(
        &self,
        samples: &[i16],
        duration: f64,
        text: Option<String>,
        error: Option<String>,
        processing_time: f64,
    ) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        let audio_name = format!("{id}.wav");
        let audio_path = self.core.records.audio_dir().join(&audio_name);
        let samples = samples.to_vec();
        let wrote = tokio::task::spawn_blocking(move || att_core::audio::write_wav_16k(&audio_path, &samples)).await;
        let audio_ok = matches!(wrote, Ok(Ok(())));
        if !audio_ok {
            tracing::error!("录音落盘失败: {wrote:?}");
        }
        let status = if text.is_some() { RecordStatus::Transcribed } else { RecordStatus::Failed };
        let record = Record {
            id: id.clone(),
            audio_path: if audio_ok { audio_name } else { String::new() },
            duration: (duration * 100.0).round() / 100.0,
            raw_text: text,
            status,
            processing_time: Some((processing_time * 100.0).round() / 100.0),
            error,
            ..Default::default()
        };
        if let Err(error) = self.core.records.insert(record) {
            tracing::error!("保存记录失败: {error}");
            return Value::Null;
        }
        self.core
            .records
            .get(&id)
            .ok()
            .and_then(|r| serde_json::to_value(r).ok())
            .unwrap_or(Value::Null)
    }

    /// 自动复制并粘贴到焦点窗口（设置 auto_copy_to_clipboard）。
    async fn auto_copy(&self, text: &str) -> (bool, bool, Option<String>) {
        if !self.core.settings().auto_copy_to_clipboard {
            return (false, false, None);
        }
        let text = text.to_string();
        let result = tokio::task::spawn_blocking(move || {
            crate::desktop::write_clipboard(&text)?;
            crate::desktop::paste_active().map_err(|e| format!("已复制，{e}"))
        })
        .await;
        match result {
            Ok(Ok(())) => (true, true, None),
            Ok(Err(error)) => {
                tracing::warn!("自动粘贴失败: {error}");
                (error.starts_with("已复制"), false, Some(error))
            }
            Err(error) => (false, false, Some(error.to_string())),
        }
    }
}

/// 静音系统输出（已静音则不动）；返回是否由本次录音静音。
async fn mute_output() -> bool {
    tokio::task::spawn_blocking(|| match crate::desktop::output_muted() {
        Some(false) => crate::desktop::set_output_muted(true),
        _ => false,
    })
    .await
    .unwrap_or(false)
}

/// 流式会话：建连与采集并行，建连期间样本先缓存，就绪后冲刷；音频通道关闭即发末包收尾。
async fn streaming_task(
    id: u64,
    config: doubao::DoubaoConfig,
    language: String,
    mut audio_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    mut cancel_rx: oneshot::Receiver<()>,
    app: AppHandle,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let started = Instant::now();
    let partial_app = app.clone();
    let on_partial: doubao::PartialCallback = Arc::new(move |text: String| {
        crate::floating::update(
            &partial_app,
            json!({ "phase": "recording", "mode": "streaming", "text": text, "duration": 0 }),
            false,
        );
        let _ = partial_app.emit("recording:partial", json!({ "text": text }));
    });
    let connect = doubao::DoubaoSession::connect(&config, &language, "", Some(on_partial));
    tokio::pin!(connect);

    let mut session: Option<doubao::DoubaoSession> = None;
    let mut connect_done = false;
    let mut audio_open = true;
    let mut pending: Vec<i16> = Vec::new();
    let mut all: Vec<i16> = Vec::new();
    let mut failure: Option<String> = None;

    while audio_open || !connect_done {
        tokio::select! {
            _ = &mut cancel_rx => {
                if let Some(session) = session.take() {
                    session.cancel().await;
                }
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: Vec::new(), text: String::new(), error: None, cancelled: true, processing_time: 0.0,
                }));
                return;
            }
            result = &mut connect, if !connect_done => {
                connect_done = true;
                match result {
                    Ok(mut ready) => {
                        tracing::info!("豆包流式就绪（{:?}，缓冲 {} 采样）", started.elapsed(), pending.len());
                        if !pending.is_empty() {
                            if let Err(error) = ready.push(&pending).await {
                                failure = Some(error.to_string());
                                let _ = tx.send(Cmd::Failed(id));
                            }
                            pending = Vec::new();
                        }
                        session = Some(ready);
                        let _ = tx.send(Cmd::Connected(id));
                    }
                    Err(error) => {
                        failure = Some(error.to_string());
                        let _ = tx.send(Cmd::Failed(id));
                    }
                }
            }
            chunk = audio_rx.recv(), if audio_open => match chunk {
                Some(pcm) => {
                    all.extend_from_slice(&pcm);
                    if failure.is_none() {
                        match session.as_mut() {
                            Some(active) => {
                                if let Err(error) = active.push(&pcm).await {
                                    failure = Some(error.to_string());
                                    let _ = tx.send(Cmd::Failed(id));
                                }
                            }
                            None => pending.extend_from_slice(&pcm),
                        }
                    }
                }
                None => audio_open = false,
            },
        }
    }

    let duration = all.len() as f64 / SAMPLE_RATE;
    let (text, error) = match (session, failure) {
        (Some(active), Some(error)) => {
            let text = active.current_text();
            active.cancel().await;
            (text, Some(error))
        }
        (None, error) => (String::new(), error),
        (Some(active), None) if duration < MIN_DURATION_SECONDS => {
            active.cancel().await;
            (String::new(), None)
        }
        (Some(active), None) => {
            let partial = active.current_text();
            match active.finish(doubao::FINISH_TIMEOUT, doubao::FINISH_QUIET).await {
                Ok(outcome) => (outcome.text, None),
                Err(error) => (partial, Some(error.to_string())),
            }
        }
    };
    let _ = tx.send(Cmd::Done(id, SessionOutcome {
        samples: all,
        text,
        error,
        cancelled: false,
        processing_time: started.elapsed().as_secs_f64(),
    }));
}

/// 单段会话：录完整段后一次性批量识别。
async fn batch_task(
    id: u64,
    config: doubao::DoubaoConfig,
    language: String,
    mut audio_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    mut cancel_rx: oneshot::Receiver<()>,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let mut all: Vec<i16> = Vec::new();
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: Vec::new(), text: String::new(), error: None, cancelled: true, processing_time: 0.0,
                }));
                return;
            }
            chunk = audio_rx.recv() => match chunk {
                Some(pcm) => all.extend_from_slice(&pcm),
                None => break,
            },
        }
    }
    let started = Instant::now();
    let (text, error) = if (all.len() as f64 / SAMPLE_RATE) < MIN_DURATION_SECONDS {
        (String::new(), None)
    } else {
        match doubao::transcribe_pcm(&config, &language, "", &all).await {
            Ok(outcome) => (outcome.text, None),
            Err(error) => (String::new(), Some(error.to_string())),
        }
    };
    let _ = tx.send(Cmd::Done(id, SessionOutcome {
        samples: all,
        text,
        error,
        cancelled: false,
        processing_time: started.elapsed().as_secs_f64(),
    }));
}
