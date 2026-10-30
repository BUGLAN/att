//! 录音会话 Actor（架构文档 §5 / §7.1）：在线引擎豆包（流式 + 批量）与 ChatGPT 听写（批量）。
//!
//! 单个 tokio 任务串行消费命令（UI 按钮 / 全局热键 / 会话任务回报），状态机：
//!
//! ```text
//! Idle ─start→ Connecting(流式建连中，采集已在收数) ─connected→ Recording
//!      ─start(单段/多段)→ Recording
//! Connecting/Recording ─stop→ Finalizing ─done→ Idle（completed / error）
//!                                         └─(auto_optimize)→ Optimizing ─optimized→ Idle（completed）
//! 任意非 Idle ─cancel→ Idle（cancelled；Optimizing 下取消的是优化，保留原文）
//! ```
//!
//! 自动优化（设置 `auto_optimize` 且已配置默认 LLM 模型）：转写落库后进入 Optimizing，流式增量经
//! `recording:optimization` 推给前端与悬浮窗；开启自动输出时同时边生成边打进焦点输入框（typing.rs），
//! 收尾与最终清洗结果对齐。还没打出任何字就失败时退回粘贴原文，保证总有输出。
//! 与旧版语义一致：单段模式优化期间不接受新录音；流式模式允许直接开始下一段，
//! 此时上一段的优化转入后台，结果只入库、不粘贴（避免粘进新输入框）。
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
use att_core::llm::cleaning;
use att_core::llm::optimizer::{self, OptimizeEvent, OptimizeOutcome};
use att_core::stt::{self, doubao};
use att_hotkey::KeyPhase;
use crate::typing::StreamTyping;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};

/// 低于该时长视为误触（不建记录、不计费）。
const MIN_DURATION_SECONDS: f64 = 0.3;
/// 累计这么久的音频高于静音阈值才算「说过话」：按键的咔哒声、麦克风上电的爆音只有一两块（20ms/块），
/// 单块过阈值就算的话，敲一下热键也会被当成说话、进入「轉」。
const HEARD_MIN_VOICED: Duration = Duration::from_millis(100);
const SAMPLE_RATE: f64 = 16_000.0;
/// 音量事件节流（≈25Hz，足够驱动波形条）。
const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Connecting,
    Recording,
    Finalizing,
    /// 转写已落库，LLM 自动优化进行中
    Optimizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 真流式：边录边推，实时出字
    Streaming,
    /// 单段：录完整段后一次性识别
    Batch,
    /// 多段：静音切段，每段录完即送识别，实时拼接（整场仍落一条记录、一个音频）
    Multi,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::Streaming => "streaming",
            Mode::Batch => "single",
            Mode::Multi => "multi",
        }
    }
}

/// 多段模式：全部分段识别结束的等待上限（每段自身另有建连/收尾超时）。
const MULTI_DRAIN_TIMEOUT: Duration = Duration::from_secs(120);
/// 收尾看门狗在各引擎自身超时之外再留的余量：正常情况下永远轮不到它，只兜「卡死」。
const WATCHDOG_SLACK: Duration = Duration::from_secs(15);
/// 看门狗取消会话后再等这么久；会话任务仍无回应（卡死 / 已崩溃）就不再等它，直接收尾。
const WATCHDOG_GRACE: Duration = Duration::from_secs(3);

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
    /// 自动优化结束（会话 id, 记录 id, 结果, 流式输入会话）
    Optimized(u64, String, OptimizeOutcome, Option<StreamTyping>),
    /// 收尾看门狗到点（会话 id, 是否为宽限后的强制收尾）
    FinalizeTimeout(u64, bool),
}

/// 进行中的自动优化（转写记录已落库）。
struct PendingOptimization {
    session_id: u64,
    mode: Mode,
    record_id: String,
    raw_text: String,
    /// 转写阶段的降级原因（识别中断但有部分文本）
    transcribe_error: Option<String>,
    /// 已被新一段录音取代：结果只入库、不粘贴、不再驱动界面
    detached: Arc<AtomicBool>,
    /// 置位后流式输入立即停手（取消优化 / 被新录音取代）
    typing_halt: Arc<AtomicBool>,
}

struct Active {
    id: u64,
    mode: Mode,
    capture: Option<att_audio::CaptureHandle>,
    cancel: Option<oneshot::Sender<()>>,
    /// 本次录音由我们静音了系统输出，结束时需要恢复
    muted_by_us: bool,
    /// 听到过说话（电平高于静音阈值，或流式已出字）；松开时据此决定要不要进入转写
    heard: Arc<AtomicBool>,
    /// 仍在录音：松开 / 取消后置否。识别结果会在松开后继续到达（豆包末包、多段的尾段），
    /// 那时悬浮窗已进入「轉」，中间结果不能再把它推回「聽」
    live: Arc<AtomicBool>,
    /// 松开时判定为误触或整场没说话：会话任务被直接取消，按这里的原因安静结束（不进「轉」）
    quiet_end: Option<&'static str>,
    /// 收尾看门狗已触发：会话任务被强制取消，按「识别超时」收尾并保留已识别内容
    timed_out: bool,
    started: Instant,
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
            optimizing: None,
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
    optimizing: Option<PendingOptimization>,
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
                        // 识别失败要让用户看到原因：不走「没说话安静结束」
                        self.stop_with("识别连接中断，正在保存已识别内容", false);
                    }
                }
                Cmd::Done(id, outcome) => {
                    if self.is_current(id) {
                        self.finish(outcome).await;
                    }
                }
                Cmd::Optimized(id, record_id, outcome, typing) => self.on_optimized(id, record_id, outcome, typing).await,
                Cmd::FinalizeTimeout(id, hard) => self.on_finalize_timeout(id, hard).await,
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
                Phase::Idle | Phase::Optimizing => self.start().await,
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
        if self.phase == Phase::Optimizing {
            // 流式：上一段优化转入后台继续跑，直接开始新录音；单段：优化期间保持「处理中」，吞掉
            match self.optimizing.as_ref() {
                Some(pending) if pending.mode == Mode::Streaming => {
                    pending.detached.store(true, Ordering::SeqCst);
                    // 新录音的文字要进新的输入框：上一段别再往里打
                    pending.typing_halt.store(true, Ordering::SeqCst);
                    self.phase = Phase::Idle;
                }
                _ => return,
            }
        }
        if self.phase != Phase::Idle {
            // 收尾中的按下与重复按下一律吞掉（与旧版语义一致）
            return;
        }
        let settings = self.core.settings();
        let engine = match stt::Engine::from_settings(&self.core) {
            Ok(engine) => engine,
            Err(message) => return self.fail_start(message),
        };
        let mode = match settings.recording_mode.as_str() {
            "streaming" => Mode::Streaming,
            "multi" => Mode::Multi,
            _ => Mode::Batch,
        };
        // 流式只有豆包支持（ChatGPT 听写是批量接口）
        let streaming_config = match (&engine, mode) {
            (stt::Engine::Doubao(config), Mode::Streaming) => Some(config.clone()),
            (_, Mode::Streaming) => {
                return self.fail_start(format!("{}暂不支持流式录音，请在设置中改用单段或多段录音模式", engine.label()));
            }
            _ => None,
        };
        let language = settings.asr_language.clone();
        self.next_id += 1;
        let id = self.next_id;

        // 采集最先启动（按下即收数），建连与静音并行进行
        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let level_app = self.app.clone();
        let mut last_level_emit = Instant::now() - LEVEL_EMIT_INTERVAL;
        // 与多段切段同一个阈值判定「有没有说话」（多段模式本就据此丢弃无声分片）
        let heard = Arc::new(AtomicBool::new(false));
        let heard_by_level = heard.clone();
        let silence_db = settings.silence_level as f64;
        let mut voiced_samples = 0usize;
        let on_chunk: att_audio::ChunkCallback = Box::new(move |pcm, level| {
            if !heard_by_level.load(Ordering::Relaxed) && att_core::audio::chunk_db(pcm) > silence_db {
                voiced_samples += pcm.len();
                if voiced_samples as f64 >= HEARD_MIN_VOICED.as_secs_f64() * SAMPLE_RATE {
                    heard_by_level.store(true, Ordering::Relaxed);
                }
            }
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
        let live = Arc::new(AtomicBool::new(true));
        self.active = Some(Active {
            id,
            mode,
            capture: Some(capture),
            cancel: Some(cancel_tx),
            muted_by_us: false,
            heard: heard.clone(),
            live: live.clone(),
            quiet_end: None,
            timed_out: false,
            started: Instant::now(),
        });

        let tx = self.tx.clone();
        match mode {
            Mode::Streaming => {
                self.phase = Phase::Connecting;
                self.emit_state("connecting", json!({ "message": "正在连接豆包语音服务" }));
                let app = self.app.clone();
                let config = streaming_config.expect("流式模式已校验为豆包");
                tauri::async_runtime::spawn(streaming_task(id, config, language, audio_rx, cancel_rx, heard, live, app, tx));
            }
            Mode::Batch => {
                self.phase = Phase::Recording;
                self.emit_state("recording", json!({ "message": "正在录音" }));
                tauri::async_runtime::spawn(batch_task(id, engine, language, audio_rx, cancel_rx, tx));
            }
            Mode::Multi => {
                self.phase = Phase::Recording;
                self.emit_state("recording", json!({ "message": "静音切段监听中" }));
                let segmenter = att_core::audio::Segmenter::new(settings.silence_level as f64, settings.silence_threshold);
                let app = self.app.clone();
                tauri::async_runtime::spawn(multi_task(id, engine, language, audio_rx, cancel_rx, segmenter, live, app, tx));
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
        self.stop_with(message, true);
    }

    /// `allow_silent`：误触或整场没听到说话时，不进入转写、不调识别（不计费），直接安静结束。
    fn stop_with(&mut self, message: &str, allow_silent: bool) {
        if !matches!(self.phase, Phase::Connecting | Phase::Recording) {
            return;
        }
        self.phase = Phase::Finalizing;
        let Some(active) = self.active.as_mut() else { return };
        active.live.store(false, Ordering::SeqCst);
        if let Some(capture) = active.capture.take() {
            std::thread::spawn(move || capture.stop());
        }
        let quiet_end = quiet_end_reason(active.started.elapsed(), active.heard.load(Ordering::Relaxed));
        if let (true, Some(reason)) = (allow_silent, quiet_end) {
            active.quiet_end = Some(reason);
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
            return;
        }
        let (id, mode, recorded) = (active.id, active.mode, active.started.elapsed());
        let budget = self.finalize_budget(mode, recorded);
        self.emit_state("finalizing", json!({ "message": message }));
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(budget).await;
            let _ = tx.send(Cmd::FinalizeTimeout(id, false));
            tokio::time::sleep(WATCHDOG_GRACE).await;
            let _ = tx.send(Cmd::FinalizeTimeout(id, true));
        });
    }

    fn finalize_budget(&self, mode: Mode, recorded: Duration) -> Duration {
        finalize_budget(mode, recorded, self.core.settings().chatgpt_asr_timeout_seconds)
    }

    /// 看门狗到点仍在收尾：先取消会话任务（它会带着已收到的音频与文字回来）；
    /// 宽限后仍无回应，就不再等它，直接按超时收尾。
    async fn on_finalize_timeout(&mut self, id: u64, hard: bool) {
        if !self.is_current(id) || self.phase != Phase::Finalizing {
            return;
        }
        tracing::warn!("录音收尾超时（会话 {id}，{}）", if hard { "会话任务无回应，强制结束" } else { "取消会话任务" });
        let Some(active) = self.active.as_mut() else { return };
        active.timed_out = true;
        if !hard {
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
            return;
        }
        self.finish(SessionOutcome { samples: Vec::new(), text: String::new(), error: None, cancelled: true, processing_time: 0.0 })
            .await;
    }

    fn cancel(&mut self) {
        if self.phase == Phase::Optimizing {
            // 取消的是优化：结果按「已取消优化，保留原始文本」收尾（见 on_optimized）
            if let Some(pending) = self.optimizing.as_ref() {
                pending.typing_halt.store(true, Ordering::SeqCst);
                self.core.optimizer.cancel(&pending.record_id);
            }
            return;
        }
        if self.phase == Phase::Idle {
            return;
        }
        if let Some(active) = self.active.as_mut() {
            active.live.store(false, Ordering::SeqCst);
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
            if let Some(capture) = active.capture.take() {
                std::thread::spawn(move || capture.stop());
            }
        }
        self.phase = Phase::Finalizing;
    }

    async fn finish(&mut self, mut outcome: SessionOutcome) {
        let Some(mut active) = self.active.take() else { return };
        if let Some(capture) = active.capture.take() {
            // 麦克风中途断开：采集线程自行结束 → 音频通道关闭 → 会话按正常收尾走到这里。
            // 把原因并入结果，界面才会提示「已保存断开前的内容」，而不是当作一次正常完成
            if let Some(device_error) = capture.device_error() {
                outcome.error.get_or_insert(device_error);
            }
            std::thread::spawn(move || capture.stop());
        }
        if active.muted_by_us {
            tokio::task::spawn_blocking(|| crate::desktop::set_output_muted(false));
        }
        self.phase = Phase::Idle;
        // 让 emit_state 仍能带上本次模式
        let mode = active.mode;
        let (quiet_end, timed_out) = (active.quiet_end, active.timed_out);
        self.active = Some(Active { capture: None, cancel: None, ..active });
        let duration = outcome.samples.len() as f64 / SAMPLE_RATE;
        if outcome.cancelled && timed_out {
            // 看门狗取消的不算用户取消：带回来的音频与文字照常保存，原因记为超时
            outcome.cancelled = false;
            outcome.error = Some("识别超时，未等到识别结果".to_string());
        }

        if let (true, Some(reason)) = (outcome.cancelled, quiet_end) {
            // 松开时是误触或整场没说话：不进转写、不建记录，悬浮窗直接收起
            self.emit_state("completed", json!({ "message": reason, "text": "", "silent": true }));
        } else if outcome.cancelled {
            self.emit_state("cancelled", json!({ "message": "已取消本次录音" }));
        } else if outcome.samples.is_empty() && outcome.text.is_empty() && outcome.error.is_some() {
            // 会话任务卡死被强制结束：手里什么都没有，只报原因
            self.emit_state("error", json!({ "message": outcome.error }));
        } else if duration < MIN_DURATION_SECONDS && outcome.text.is_empty() {
            // 误触：同样安静收起
            self.emit_state("completed", json!({ "message": "录音时间过短", "text": "", "silent": true }));
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
            let record_id = record.get("id").and_then(Value::as_str).map(str::to_string);
            let auto_optimize = self.core.settings().auto_optimize && optimizer::has_default_model(&self.core);
            if let (true, Some(record_id)) = (auto_optimize, record_id) {
                // 自动优化：本次不在转写阶段粘贴，优化结果流式打进输入框（否则原文、优化文各出一遍）
                self.begin_optimization(mode, record_id, outcome.text.clone(), outcome.error.clone(), record);
                self.active = None;
                return;
            }
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

    /// 进入 Optimizing：优化在独立任务里跑（不阻塞 Actor 处理新命令），增量直接推给前端与悬浮窗，
    /// 开启自动输出时同时流式打进焦点输入框。
    fn begin_optimization(&mut self, mode: Mode, record_id: String, raw_text: String, transcribe_error: Option<String>, record: Value) {
        let session_id = self.next_id;
        let detached = Arc::new(AtomicBool::new(false));
        let typing_halt = Arc::new(AtomicBool::new(false));
        let mut typing = self.core.settings().auto_copy_to_clipboard.then(|| StreamTyping::spawn(typing_halt.clone()));
        self.phase = Phase::Optimizing;
        self.optimizing = Some(PendingOptimization {
            session_id,
            mode,
            record_id: record_id.clone(),
            raw_text: raw_text.clone(),
            transcribe_error,
            detached,
            typing_halt,
        });
        self.emit_state(
            "optimizing",
            json!({ "mode": mode.as_str(), "message": "转写完成，正在优化", "text": raw_text, "record": record }),
        );

        let core = self.core.clone();
        let app = self.app.clone();
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut accumulated = String::new();
            let delta_record = record_id.clone();
            let outcome = optimizer::optimize_record(&core, &record_id, &raw_text, |event| {
                let OptimizeEvent::Delta(chunk) = event else { return };
                accumulated.push_str(&chunk);
                // 输入框只出「收尾清洗不会剥掉」的部分（套话 / 思考块不上屏）。
                // 悬浮窗不回填优化稿：它停留很短，一闪而过反而突兀，「轉」期间一直显示淡色识别稿
                let stable = cleaning::stable_prefix(&accumulated);
                if let Some(typing) = typing.as_mut() {
                    typing.advance(&stable);
                }
                let _ = app.emit("recording:optimization", json!({ "record_id": &delta_record, "text": chunk }));
            })
            .await;
            let _ = tx.send(Cmd::Optimized(session_id, delta_record, outcome, typing));
        });
    }

    /// 自动优化结束：补齐流式输入（失败且没打出字时粘贴原文）；已转后台的只同步记录、不输出。
    async fn on_optimized(&mut self, session_id: u64, record_id: String, outcome: OptimizeOutcome, typing: Option<StreamTyping>) {
        let record = self.core.records.get(&record_id).ok().and_then(|r| serde_json::to_value(r).ok()).unwrap_or(Value::Null);
        let Some(pending) = self.optimizing.take_if(|p| p.session_id == session_id) else {
            // 已被更新的一段录音（及其优化）取代：只同步记录，不粘贴、不动当前会话
            abort_typing(typing).await;
            crate::tray::refresh(&self.app);
            self.emit_background_optimization(record, &outcome);
            return;
        };
        crate::tray::refresh(&self.app);
        let (final_text, optimized, optimize_error, base_message) = match &outcome {
            OptimizeOutcome::Completed(success) => (success.optimized_text.clone(), true, None, "录音已完成，已优化".to_string()),
            OptimizeOutcome::Failed(error) => (pending.raw_text.clone(), false, Some(error.clone()), format!("优化失败，已保留转写原文：{error}")),
            OptimizeOutcome::Cancelled => (pending.raw_text.clone(), false, None, "已取消优化，保留原始文本".to_string()),
        };
        let truncated = matches!(&outcome, OptimizeOutcome::Completed(s) if s.truncated);
        let cancelled = matches!(outcome, OptimizeOutcome::Cancelled);

        if pending.detached.load(Ordering::SeqCst) {
            // 用户已开始下一段录音：上一段结果只入库，不粘贴、不抢占界面状态
            abort_typing(typing).await;
            self.emit_background_optimization(record, &outcome);
            return;
        }

        self.phase = Phase::Idle;
        // 用户主动取消优化：与旧版一致只保留原文，不粘贴；已打出的字保留，不再补打
        let (copied, pasted, copy_error) = if cancelled {
            abort_typing(typing).await;
            (false, false, None)
        } else {
            self.deliver(&final_text, optimized, typing).await
        };
        let message = match &pending.transcribe_error {
            Some(error) => format!("识别中断，已保存已识别内容：{error}"),
            None if truncated => "优化完成，但输出已达 Max Tokens 上限被截断，可在设置 → LLM 服务里调优".to_string(),
            None => base_message,
        };
        self.emit_state(
            "completed",
            json!({
                "mode": pending.mode.as_str(),
                "message": message,
                "text": final_text,
                "raw_text": pending.raw_text,
                "record": record,
                "copied": copied,
                "pasted": pasted,
                "copy_error": copy_error,
                "degraded": pending.transcribe_error.is_some() || truncated,
                "optimized": optimized,
                "optimization_cancelled": cancelled,
                "optimize_error": optimize_error,
            }),
        );
    }

    fn emit_background_optimization(&self, record: Value, outcome: &OptimizeOutcome) {
        let optimized = matches!(outcome, OptimizeOutcome::Completed(_));
        let _ = self.app.emit(
            "recording:background-optimization",
            json!({
                "record": record,
                "optimized": optimized,
                "cancelled": matches!(outcome, OptimizeOutcome::Cancelled),
                "error": match outcome { OptimizeOutcome::Failed(error) => Some(error.clone()), _ => None },
                "message": if optimized { "上一段录音优化完成，文本已保存到记录集" } else { "上一段录音优化未完成，原文已保留在记录集中" },
            }),
        );
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
        crate::tray::refresh(&self.app);
        self.core
            .records
            .get(&id)
            .ok()
            .and_then(|r| serde_json::to_value(r).ok())
            .unwrap_or(Value::Null)
    }

    /// 输出优化收尾的文本。流式输入打出过内容：成功则补齐到最终文本，失败则停在已打出的部分；
    /// 两种情况都把最终文本（失败时为原文）放进剪贴板。一个字都没打出（未授权 / 失败在首字前）：退回整段粘贴。
    async fn deliver(&self, final_text: &str, optimized: bool, typing: Option<StreamTyping>) -> (bool, bool, Option<String>) {
        let Some(typing) = typing else { return self.auto_copy(final_text).await };
        let typed_any = !typing.typed().is_empty();
        if !typed_any && !optimized {
            abort_typing(Some(typing)).await;
            return self.auto_copy(final_text).await;
        }
        let text = final_text.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let typed = if optimized { typing.complete(&text) } else { typing.stop() };
            (typed, crate::desktop::write_clipboard(&text).is_ok())
        })
        .await;
        match result {
            Ok((Ok(()), copied)) if optimized => (copied, true, None),
            Ok((Ok(()), copied)) => (copied, false, Some("优化中断：输入框里是部分优化结果，原文已复制到剪贴板".into())),
            Ok((Err(error), _)) if !typed_any => {
                tracing::warn!("流式输入不可用，改为整段粘贴: {error}");
                self.auto_copy(final_text).await
            }
            Ok((Err(error), copied)) => {
                tracing::warn!("流式输入中断: {error}");
                (copied, false, Some(error))
            }
            Err(error) => (false, false, Some(error.to_string())),
        }
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

/// 收尾看门狗的上限：各引擎自身超时之和再加余量。正常收尾远早于它，只兜卡死。
/// 松开时要不要跳过转写直接安静结束：按下不到 0.3 秒算误触（哪怕按键声过了阈值），整场没说话同样跳过。
fn quiet_end_reason(recorded: Duration, heard: bool) -> Option<&'static str> {
    if recorded.as_secs_f64() < MIN_DURATION_SECONDS {
        Some("录音时间过短")
    } else if !heard {
        Some("没有检测到说话")
    } else {
        None
    }
}

fn finalize_budget(mode: Mode, recorded: Duration, chatgpt_timeout_seconds: i64) -> Duration {
    // 豆包：建连 + 末包等待（批量另按音频时长加时）+ 关闭
    let doubao = doubao::CONNECT_TIMEOUT + doubao::FINISH_TIMEOUT + Duration::from_secs(2);
    let engine = match mode {
        Mode::Streaming => doubao,
        Mode::Multi => MULTI_DRAIN_TIMEOUT,
        Mode::Batch => Duration::from_secs(chatgpt_timeout_seconds.max(0) as u64).max(doubao + recorded / 2),
    };
    engine + WATCHDOG_SLACK
}

/// 流式输入立即停手（丢弃未发出的按键）。
async fn abort_typing(typing: Option<StreamTyping>) {
    if let Some(typing) = typing {
        let _ = tokio::task::spawn_blocking(move || typing.abort()).await;
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
    heard: Arc<AtomicBool>,
    live: Arc<AtomicBool>,
    app: AppHandle,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let started = Instant::now();
    // 出字耗时埋点（距会话开始 ≈ 按下）：建连就绪 / 首字 / 松开后收尾，会话结束打一行汇总
    let mut ready_at: Option<Duration> = None;
    let first_text_at: Arc<OnceLock<Duration>> = Arc::new(OnceLock::new());
    let first_text_mark = first_text_at.clone();
    let partial_app = app.clone();
    let on_partial: doubao::PartialCallback = Arc::new(move |partial: doubao::Partial| {
        let doubao::Partial { text, pending_chars } = partial;
        // 已经出字就算说过话（小声说话电平可能压在阈值下，但识别出来了）
        if !text.trim().is_empty() {
            heard.store(true, Ordering::Relaxed);
            let _ = first_text_mark.set(started.elapsed());
        }
        if live.load(Ordering::SeqCst) {
            // pending：末尾还没确认的字数，悬浮窗把这截显示成淡白色
            crate::floating::update(
                &partial_app,
                json!({ "phase": "recording", "mode": "streaming", "text": text, "pending": pending_chars, "duration": 0 }),
                false,
            );
        }
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
                // 取消也带回已收的音频与文字：用户取消时由 Actor 丢弃，看门狗超时时据此保存
                let text = session.as_ref().map(doubao::DoubaoSession::current_text).unwrap_or_default();
                if let Some(session) = session.take() {
                    session.cancel().await;
                }
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true, processing_time: started.elapsed().as_secs_f64(),
                }));
                return;
            }
            result = &mut connect, if !connect_done => {
                connect_done = true;
                match result {
                    Ok(mut ready) => {
                        ready_at = Some(started.elapsed());
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
    let released_at = started.elapsed();
    let partial = session.as_ref().map(doubao::DoubaoSession::current_text).unwrap_or_default();
    let finalize = async move {
        match (session, failure) {
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
        }
    };
    // 等末包期间同样响应取消（悬浮窗 ✕ / 看门狗），不必干等到收尾超时
    let (text, error, cancelled) = tokio::select! {
        (text, error) = finalize => (text, error, false),
        _ = &mut cancel_rx => (partial, None, true),
    };
    let ms = |d: Option<Duration>| d.map(|d| format!("{}ms", d.as_millis())).unwrap_or_else(|| "-".into());
    tracing::info!(
        "流式出字耗时（距按下）：建连就绪 {}，首字 {}，松开 {}，收尾 {}",
        ms(ready_at),
        ms(first_text_at.get().copied()),
        ms(Some(released_at)),
        ms(Some(started.elapsed() - released_at)),
    );
    let _ = tx.send(Cmd::Done(id, SessionOutcome {
        samples: all,
        text,
        error,
        cancelled,
        processing_time: started.elapsed().as_secs_f64(),
    }));
}

/// 单段会话：录完整段后一次性批量识别。
async fn batch_task(
    id: u64,
    engine: stt::Engine,
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
                    samples: all, text: String::new(), error: None, cancelled: true, processing_time: 0.0,
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
    let (text, error, cancelled) = if (all.len() as f64 / SAMPLE_RATE) < MIN_DURATION_SECONDS {
        (String::new(), None, false)
    } else {
        // 识别期间同样响应取消（ChatGPT 听写默认可等 120s）
        tokio::select! {
            result = engine.transcribe_pcm(&language, &all) => match result {
                Ok(text) => (text, None, false),
                Err(error) => (String::new(), Some(error), false),
            },
            _ = &mut cancel_rx => (String::new(), None, true),
        }
    };
    let _ = tx.send(Cmd::Done(id, SessionOutcome {
        samples: all,
        text,
        error,
        cancelled,
        processing_time: started.elapsed().as_secs_f64(),
    }));
}

/// 多段会话：静音切段 → 每段立即并发送批量识别 → 按段序实时拼接推给前端与悬浮窗。
///
/// 与旧版「最后一片上传失败就永久卡在转写中」不同：收尾有总等待上限，单段失败只降级（保留其余段文本）。
#[allow(clippy::too_many_arguments)]
async fn multi_task(
    id: u64,
    engine: stt::Engine,
    language: String,
    mut audio_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    mut cancel_rx: oneshot::Receiver<()>,
    mut segmenter: att_core::audio::Segmenter,
    live: Arc<AtomicBool>,
    app: AppHandle,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let started = Instant::now();
    let mut all: Vec<i16> = Vec::new();
    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<(usize, Result<String, String>)>();
    let mut texts: Vec<Option<Result<String, String>>> = Vec::new();
    let spawn_segment = |index: usize, segment: Vec<i16>| {
        let (engine, language, result_tx) = (engine.clone(), language.clone(), result_tx.clone());
        tokio::spawn(async move {
            let result = engine.transcribe_pcm(&language, &segment).await;
            let _ = result_tx.send((index, result));
        });
    };
    let joined = |texts: &[Option<Result<String, String>>]| {
        texts.iter().filter_map(|t| t.as_ref().and_then(|r| r.as_ref().ok())).filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join("\n\n")
    };
    let emit_partial = |text: &str| {
        if live.load(Ordering::SeqCst) {
            crate::floating::update(&app, json!({ "phase": "recording", "mode": "multi", "text": text, "duration": 0 }), false);
        }
        let _ = app.emit("recording:partial", json!({ "text": text }));
    };

    let mut audio_open = true;
    while audio_open {
        tokio::select! {
            _ = &mut cancel_rx => {
                let text = joined(&texts);
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true, processing_time: started.elapsed().as_secs_f64(),
                }));
                return;
            }
            chunk = audio_rx.recv() => match chunk {
                Some(pcm) => {
                    all.extend_from_slice(&pcm);
                    if let Some(segment) = segmenter.push(&pcm) {
                        texts.push(None);
                        spawn_segment(texts.len() - 1, segment);
                    }
                }
                None => audio_open = false,
            },
            Some((index, result)) = result_rx.recv() => {
                texts[index] = Some(result);
                emit_partial(&joined(&texts));
            }
        }
    }
    if let Some(segment) = segmenter.finish() {
        texts.push(None);
        spawn_segment(texts.len() - 1, segment);
    }
    drop(result_tx);

    // 收尾：等全部分段识别结束（有总上限，不会永久卡在「正在转写」）
    let deadline = tokio::time::Instant::now() + MULTI_DRAIN_TIMEOUT;
    while texts.iter().any(Option::is_none) {
        tokio::select! {
            _ = &mut cancel_rx => {
                let text = joined(&texts);
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true, processing_time: started.elapsed().as_secs_f64(),
                }));
                return;
            }
            received = tokio::time::timeout_at(deadline, result_rx.recv()) => match received {
                Ok(Some((index, result))) => {
                    texts[index] = Some(result);
                    emit_partial(&joined(&texts));
                }
                _ => break,
            },
        }
    }
    let text = joined(&texts);
    let pending = texts.iter().filter(|t| t.is_none()).count();
    let error = texts
        .iter()
        .find_map(|t| t.as_ref().and_then(|r| r.as_ref().err()).cloned())
        .or_else(|| (pending > 0).then(|| format!("{pending} 个分段识别超时")));
    let _ = tx.send(Cmd::Done(id, SessionOutcome {
        samples: all,
        text,
        error,
        cancelled: false,
        processing_time: started.elapsed().as_secs_f64(),
    }));
}

#[cfg(test)]
mod tests {
    use super::{finalize_budget, quiet_end_reason, Mode};
    use std::time::Duration;

    #[test]
    fn 短按与没说话_松开即安静结束不进转写() {
        // 短按：按键声把电平顶过阈值也算误触
        assert_eq!(quiet_end_reason(Duration::from_millis(150), true), Some("录音时间过短"));
        assert_eq!(quiet_end_reason(Duration::from_millis(150), false), Some("录音时间过短"));
        assert_eq!(quiet_end_reason(Duration::from_secs(3), false), Some("没有检测到说话"));
        assert_eq!(quiet_end_reason(Duration::from_secs(3), true), None);
    }

    #[test]
    fn 收尾看门狗上限_覆盖各引擎自身超时() {
        // 流式：建连 10 + 末包 6 + 关闭 2 + 余量 15
        assert_eq!(finalize_budget(Mode::Streaming, Duration::from_secs(30), 120), Duration::from_secs(33));
        // 多段：分段总等待 120 + 余量
        assert_eq!(finalize_budget(Mode::Multi, Duration::from_secs(30), 120), Duration::from_secs(135));
        // 单段：ChatGPT 的超时设置与豆包按时长加时取大
        assert_eq!(finalize_budget(Mode::Batch, Duration::from_secs(30), 120), Duration::from_secs(135));
        assert_eq!(finalize_budget(Mode::Batch, Duration::from_secs(600), 120), Duration::from_secs(333));
    }
}
