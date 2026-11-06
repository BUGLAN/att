//! 录音会话 Actor：在线引擎豆包（流式 + 批量）与 ChatGPT 听写（批量）。
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
//! 两种录音快捷键（可分键，同键时只能启用一种）：
//! - 按住录音 [`HotkeyAction::Record`]：按下开始、松开结束；
//! - 点按录音 [`HotkeyAction::RecordToggle`]：按一下开始、再按一下结束，长段口述不用一直按着。
//!   点按开始的录音进行中，按一下任一录音键都结束它；按住键的松开只收尾按住开始的会话。
//!
//! 按住模式的松开先进 [`HOLD_RELEASE_GRACE`] 缓冲（继续收音），缓冲内重新按下就当没松开、继续本段；
//! 到点才 stop。修饰键组合偶尔会被系统报告「抬起一下又按回」，零容忍的话说到一半就被打断。
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
//! 优化选中文本（快捷键 [`HotkeyAction::RewriteSelection`]）不录音：取前台应用的选中文字建一条文本记录，
//! 直接进入同一个 Optimizing 相位——同一个悬浮窗、同一套流式输入与收尾，区别只在 [`OptimizeSource`]：
//! 用「选中文本优化」预设、总是输回原处（不看自动粘贴开关）、失败时不动选区。
//!
//! 前端事件：`recording:state`（相位 + 文本 + 记录）、`recording:partial`（实时文本）、
//! `recording:level`（音量 0..1）。

use att_core::records::{Record, RecordStatus};
use att_core::settings::hotkey::HotkeyAction;
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
/// 按住模式的松开缓冲：松开后继续收音这么久，期间重新按下就当没松开、继续本段录音。
/// 实测按住 ⌘⌥ 时系统偶尔报告 ⌘ 抬起约 200ms 又按回（⌥ 一直按着），立即停止的话一句话被截成两段。
/// 代价是正常松开后晚这么久才开始收尾（多收一小段尾音，不影响识别）。
const HOLD_RELEASE_GRACE: Duration = Duration::from_millis(250);

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
    /// 按住模式的松开缓冲到点（会话 id, 缓冲序号）：期间没重新按下才真正收尾
    ReleaseGrace(u64, u64),
}

/// 一次优化从哪来：决定用哪个预设、结果要不要强制输出、收尾文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptimizeSource {
    /// 录音转写后的自动优化（默认预设，输出跟随「自动复制并粘贴」）
    Recording,
    /// 快捷键优化选中文本（「选中文本优化」预设，总是输回原处替换选区）
    Selection,
}

impl OptimizeSource {
    fn as_str(self) -> &'static str {
        match self {
            OptimizeSource::Recording => "recording",
            OptimizeSource::Selection => "selection",
        }
    }
}

/// 进行中的自动优化（转写记录已落库）。
struct PendingOptimization {
    session_id: u64,
    mode: Mode,
    source: OptimizeSource,
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
    /// 用户点了取消：看门狗强制收尾时仍按取消处理（丢弃），不算识别超时
    user_cancelled: bool,
    /// 按住模式已松开、正处在松开缓冲里：(缓冲序号, 松开时刻)。重新按下时清掉，到点仍在才收尾
    pending_release: Option<(u64, Instant)>,
    /// 由「按住录音」键开始：只有这种会话在松开时收尾；点按录音 / 界面按钮开始的会话不理会松开
    held: bool,
    started: Instant,
}

/// 录音热键在当前相位下该做什么（纯逻辑，见 [`record_hotkey_step`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordHotkeyStep {
    /// 开始新录音；`held` = 由按住录音键开始（松开时收尾）
    Start { held: bool },
    /// 按住键按下：在松开缓冲里就接着录，否则当普通按下（开始 / 录音中的自动重复被吞）
    ResumeOrStart,
    Stop,
    /// 按住键松开：进松开缓冲，到点收尾
    Release,
    /// 收尾中按下：摇一下印章表示这下没生效
    Nudge,
    Ignore,
}

/// 两种录音快捷键的分派：
/// - 按住录音：按下开始、松开收尾；但点按开始的录音进行中按下它 = 结束（按任一录音键都能停）；
/// - 点按录音：空闲 / 优化中按下开始，录音中（不论哪种方式开始）按下结束，收尾中按下摇印章；
/// - 按住键的松开只收尾按住开始的会话，其余松开一律忽略。
fn record_hotkey_step(action: HotkeyAction, key_phase: KeyPhase, phase: Phase, held: bool) -> RecordHotkeyStep {
    let recording = matches!(phase, Phase::Connecting | Phase::Recording);
    match (action, key_phase) {
        (HotkeyAction::Record, KeyPhase::Pressed) if recording && !held => RecordHotkeyStep::Stop,
        (HotkeyAction::Record, KeyPhase::Pressed) => RecordHotkeyStep::ResumeOrStart,
        (HotkeyAction::Record, KeyPhase::Released) if held => RecordHotkeyStep::Release,
        (HotkeyAction::RecordToggle, KeyPhase::Pressed) => match phase {
            Phase::Idle | Phase::Optimizing => RecordHotkeyStep::Start { held: false },
            Phase::Connecting | Phase::Recording => RecordHotkeyStep::Stop,
            Phase::Finalizing => RecordHotkeyStep::Nudge,
        },
        _ => RecordHotkeyStep::Ignore,
    }
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
            next_release: 0,
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
    /// 松开缓冲序号：晚到的旧缓冲（已被重新按下作废）据此识别
    next_release: u64,
    snapshot: Arc<Mutex<Value>>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Start => self.start(false).await,
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
                        self.stop_with("识别连接中断，正在保存已识别内容", false, None);
                    }
                }
                Cmd::Done(id, outcome) => {
                    if self.is_current(id) {
                        self.finish(outcome).await;
                    }
                }
                Cmd::Optimized(id, record_id, outcome, typing) => self.on_optimized(id, record_id, outcome, typing).await,
                Cmd::FinalizeTimeout(id, hard) => self.on_finalize_timeout(id, hard).await,
                Cmd::ReleaseGrace(id, token) => self.on_release_grace(id, token),
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
        if action == HotkeyAction::RewriteSelection {
            if key_phase == KeyPhase::Pressed {
                self.rewrite_selection().await;
            }
            return;
        }
        let held = self.active.as_ref().is_some_and(|a| a.held);
        let step = record_hotkey_step(action, key_phase, self.phase, held);
        tracing::debug!("录音热键 {action:?} {key_phase:?}，当前 {:?}（按住开始: {held}）→ {step:?}", self.phase);
        match step {
            RecordHotkeyStep::Start { held } => self.start(held).await,
            RecordHotkeyStep::ResumeOrStart => {
                if !self.resume_released() {
                    self.start(true).await;
                }
            }
            RecordHotkeyStep::Stop => self.stop("正在完成转写"),
            RecordHotkeyStep::Release => self.release_after_grace(),
            RecordHotkeyStep::Nudge => crate::floating::nudge(&self.app),
            RecordHotkeyStep::Ignore => {}
        }
    }

    /// 按住模式松开：不立即收尾，先进松开缓冲（继续收音），到点仍没重新按下才 stop。
    fn release_after_grace(&mut self) {
        if !matches!(self.phase, Phase::Connecting | Phase::Recording) {
            return;
        }
        let Some(active) = self.active.as_mut() else { return };
        if active.pending_release.is_some() {
            return;
        }
        self.next_release += 1;
        let token = self.next_release;
        active.pending_release = Some((token, Instant::now()));
        let (tx, id) = (self.tx.clone(), active.id);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(HOLD_RELEASE_GRACE).await;
            let _ = tx.send(Cmd::ReleaseGrace(id, token));
        });
    }

    /// 松开缓冲里重新按下：当没松开过，继续本段录音。返回 false 表示不在缓冲里，按普通按下处理。
    fn resume_released(&mut self) -> bool {
        if !matches!(self.phase, Phase::Connecting | Phase::Recording) {
            return false;
        }
        let Some((_, released_at)) = self.active.as_mut().and_then(|a| a.pending_release.take()) else {
            return false;
        };
        tracing::info!("松开后 {}ms 内重新按下，继续本段录音", released_at.elapsed().as_millis());
        true
    }

    fn on_release_grace(&mut self, id: u64, token: u64) {
        let released_at = self
            .active
            .as_ref()
            .filter(|a| a.id == id)
            .and_then(|a| a.pending_release)
            .filter(|(pending, _)| *pending == token)
            .map(|(_, at)| at);
        if let Some(released_at) = released_at {
            self.stop_with("正在完成转写", true, Some(released_at));
        }
    }

    /// 优化选中文本：复制选区 → 建文本记录 → 进入与录音自动优化相同的 Optimizing 相位。
    async fn rewrite_selection(&mut self) {
        if self.phase != Phase::Idle {
            // 录音 / 转写 / 优化中：不抢占当前会话，摇一下印章表示这下没生效
            tracing::info!("{:?} 中按下优化选中文本，忽略", self.phase);
            return crate::floating::nudge(&self.app);
        }
        if !att_hotkey::listener::suppresses_keys() {
            // 只读监听（启动时没有「辅助功能」权限）：快捷键的主键已经送进前台应用、替换掉了选区，再复制只会拿到错的内容
            return self.emit_state(
                "error",
                json!({ "message": "优化选中文本需要「辅助功能」权限：授权后请重启应用（否则快捷键会在前台应用里打出字符）" }),
            );
        }
        if !optimizer::has_default_model(&self.core) {
            return self.emit_state("error", json!({ "message": "优化选中文本需要 LLM 模型：请先在 设置 → LLM 服务 中添加" }));
        }
        let selection = tokio::task::spawn_blocking(crate::desktop::copy_selection)
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        let text = match selection {
            Ok(Some(text)) => text,
            Ok(None) => return self.emit_state("error", json!({ "message": "没有读到选中的文字：先选中要优化的文本再按快捷键" })),
            Err(message) => {
                tracing::warn!("读取选中文本失败: {message}");
                return self.emit_state("error", json!({ "message": message }));
            }
        };
        tracing::info!("优化选中文本（{} 字）", text.chars().count());
        let record = self.save_record(&[], 0.0, Some(text.clone()), None, 0.0).await;
        let Some(record_id) = record.get("id").and_then(Value::as_str).map(str::to_string) else {
            return self.emit_state("error", json!({ "message": "保存记录失败，未开始优化" }));
        };
        self.begin_optimization(Mode::Batch, record_id, text, None, record, OptimizeSource::Selection);
    }

    fn fail_start(&mut self, message: String) {
        tracing::warn!("录音无法开始: {message}");
        self.phase = Phase::Idle;
        self.active = None;
        self.emit_state("error", json!({ "message": message }));
    }

    /// 开始一段录音；`held` = 由「按住录音」键按下开始（松开时收尾）。
    async fn start(&mut self, held: bool) {
        if self.phase == Phase::Optimizing {
            // 流式：上一段优化转入后台继续跑，直接开始新录音；单段：优化期间保持「处理中」，吞掉
            match self.optimizing.as_ref() {
                Some(pending) if pending.mode == Mode::Streaming => {
                    tracing::info!("流式优化中开始新录音：上一段优化转入后台，停止往输入框输出");
                    pending.detached.store(true, Ordering::SeqCst);
                    // 新录音的文字要进新的输入框：上一段别再往里打
                    pending.typing_halt.store(true, Ordering::SeqCst);
                    self.phase = Phase::Idle;
                }
                _ => {
                    tracing::info!("单段优化中按下录音键，忽略");
                    return crate::floating::nudge(&self.app);
                }
            }
        }
        if self.phase != Phase::Idle {
            // 收尾中的按下与重复按下一律吞掉（与旧版语义一致）。
            // 收尾中的按下多半是想开始下一段：摇一下印章表示这下没生效；录音中的是按住热键的自动重复，不提示
            if self.phase == Phase::Finalizing {
                tracing::info!("收尾中按下录音键，忽略");
                crate::floating::nudge(&self.app);
            }
            return;
        }
        let settings = self.core.settings();
        // 本地引擎不在这里等模型加载（只校验文件并在后台预加载），录音按下即开始，推理时再等加载完成
        let engine = match stt::Engine::resolve(&self.core) {
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
        // 按下即亮「聽」：打开麦克风可能要几百毫秒（蓝牙耳机切换通话模式更久），别让用户干等没有反馈。
        // 打不开时 fail_start 推「敗」接上；之后的 connecting / recording 映射成同一状态，胶囊不会重播
        crate::floating::update(&self.app, json!({ "phase": "recording", "mode": mode.as_str(), "text": "", "duration": 0 }), true);
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
            user_cancelled: false,
            pending_release: None,
            held,
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
        self.stop_with(message, true, None);
    }

    /// `allow_silent`：误触或整场没听到说话时，不进入转写、不调识别（不计费），直接安静结束。
    /// `released_at`：按住模式经松开缓冲收尾时的真实松开时刻——误触判定按它算时长，不把缓冲里多收的音算进去。
    fn stop_with(&mut self, message: &str, allow_silent: bool, released_at: Option<Instant>) {
        if !matches!(self.phase, Phase::Connecting | Phase::Recording) {
            return;
        }
        self.phase = Phase::Finalizing;
        let Some(active) = self.active.as_mut() else { return };
        active.live.store(false, Ordering::SeqCst);
        if let Some(capture) = active.capture.take() {
            std::thread::spawn(move || capture.stop());
        }
        active.pending_release = None;
        let recorded = recorded_until(active.started, released_at, Instant::now());
        let quiet_end = quiet_end_reason(recorded, active.heard.load(Ordering::Relaxed));
        if let (true, Some(reason)) = (allow_silent, quiet_end) {
            active.quiet_end = Some(reason);
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
            let id = active.id;
            // 不进「轉」：悬浮窗当场收起，不必等会话任务关掉连接回来
            crate::floating::hide(&self.app);
            self.arm_watchdog(id, None);
            return;
        }
        let (id, mode) = (active.id, active.mode);
        let budget = self.finalize_budget(mode, recorded);
        self.emit_state("finalizing", json!({ "message": message }));
        self.arm_watchdog(id, Some(budget));
    }

    /// 收尾看门狗：`budget` 到点先取消会话任务（它会带着已收内容回来），再过 [`WATCHDOG_GRACE`] 仍无回应就强制收尾。
    /// 已经发过取消（用户取消 / 安静结束）时传 None，只留强制收尾这一步——
    /// 否则会话任务卡在关连接上，Actor 会永远停在收尾中，之后的热键全被吞掉。
    fn arm_watchdog(&self, id: u64, budget: Option<Duration>) {
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(budget) = budget {
                tokio::time::sleep(budget).await;
                let _ = tx.send(Cmd::FinalizeTimeout(id, false));
            }
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
        active.timed_out = counts_as_timeout(active.quiet_end, active.user_cancelled);
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
        let Some(active) = self.active.as_mut() else { return };
        active.live.store(false, Ordering::SeqCst);
        active.user_cancelled = true;
        // 收尾看门狗可能已按「超时」标过（软取消后还在等宽限）：用户随后点了 ✕，就按取消丢弃，不再存超时记录
        active.timed_out = false;
        if let Some(cancel) = active.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(capture) = active.capture.take() {
            std::thread::spawn(move || capture.stop());
        }
        let id = active.id;
        self.phase = Phase::Finalizing;
        // 点了 ✕ 当场收起；会话任务回来后照常发 cancelled（主窗口据此复位）
        crate::floating::hide(&self.app);
        self.arm_watchdog(id, None);
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
                self.begin_optimization(mode, record_id, outcome.text.clone(), outcome.error.clone(), record, OptimizeSource::Recording);
                self.active = None;
                return;
            }
            let (copied, pasted, copy_error) = self.auto_copy(&outcome.text, false).await;
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
    /// 开启自动输出时（优化选中文本则总是）同时流式打进焦点输入框。
    fn begin_optimization(
        &mut self,
        mode: Mode,
        record_id: String,
        raw_text: String,
        transcribe_error: Option<String>,
        record: Value,
        source: OptimizeSource,
    ) {
        let session_id = self.next_id;
        let detached = Arc::new(AtomicBool::new(false));
        let typing_halt = Arc::new(AtomicBool::new(false));
        let settings = self.core.settings();
        let output = source == OptimizeSource::Selection || settings.auto_copy_to_clipboard;
        let mut typing = output.then(|| StreamTyping::spawn(typing_halt.clone()));
        let preset_id = match source {
            OptimizeSource::Recording => None,
            OptimizeSource::Selection => settings.rewrite_preset_id.clone(),
        };
        let message = match source {
            OptimizeSource::Recording => "转写完成，正在优化",
            OptimizeSource::Selection => "正在优化选中文本",
        };
        self.phase = Phase::Optimizing;
        self.optimizing = Some(PendingOptimization {
            session_id,
            mode,
            source,
            record_id: record_id.clone(),
            raw_text: raw_text.clone(),
            transcribe_error,
            detached,
            typing_halt,
        });
        self.emit_state(
            "optimizing",
            json!({ "mode": mode.as_str(), "source": source.as_str(), "message": message, "text": raw_text, "record": record }),
        );

        let core = self.core.clone();
        let app = self.app.clone();
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut accumulated = String::new();
            let delta_record = record_id.clone();
            let outcome = optimizer::optimize_record_with_preset(&core, &record_id, &raw_text, preset_id.as_deref(), |event| {
                let OptimizeEvent::Delta(chunk) = event else { return };
                accumulated.push_str(&chunk);
                // 输入框只出「收尾清洗不会剥掉」的部分（套话 / 思考块不上屏）。
                // 悬浮窗不回填优化稿：它停留很短，一闪而过反而突兀，「轉」期间一直显示识别稿
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
        let selection = pending.source == OptimizeSource::Selection;
        let (final_text, optimized, optimize_error, base_message) = match (&outcome, selection) {
            (OptimizeOutcome::Completed(success), false) => (success.optimized_text.clone(), true, None, "录音已完成，已优化".to_string()),
            (OptimizeOutcome::Completed(success), true) => (success.optimized_text.clone(), true, None, "选中文本已优化".to_string()),
            (OptimizeOutcome::Failed(error), false) => (pending.raw_text.clone(), false, Some(error.clone()), format!("优化失败，已保留转写原文：{error}")),
            (OptimizeOutcome::Failed(error), true) => (pending.raw_text.clone(), false, Some(error.clone()), format!("优化失败，选中的文字没有改动：{error}")),
            (OptimizeOutcome::Cancelled, _) => (pending.raw_text.clone(), false, None, "已取消优化，保留原始文本".to_string()),
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
        // 用户主动取消优化：与旧版一致只保留原文，不粘贴；已打出的字保留，不再补打。
        // 优化选中文本时已打出的字替换掉了选区：把原文放进剪贴板，用户能贴回去
        let (copied, pasted, copy_error) = if cancelled {
            let typed_any = typing.as_ref().is_some_and(|t| !t.typed().is_empty());
            abort_typing(typing).await;
            let copied = selection && typed_any && copy_to_clipboard(&pending.raw_text).await;
            (copied, false, None)
        } else {
            self.deliver(&final_text, optimized, typing, pending.source).await
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
                "source": pending.source.as_str(),
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
        let audio_name = format!("{id}.{}", att_core::audio::RECORDING_EXTENSION);
        // 没有音频（优化选中文本建的文本记录）：不落空音频文件，audio_path 留空
        let audio_ok = !samples.is_empty() && {
            let audio_path = self.core.records.audio_dir().join(&audio_name);
            let samples = samples.to_vec();
            let wrote = tokio::task::spawn_blocking(move || att_core::audio::write_recording_16k(&audio_path, &samples)).await;
            if !matches!(wrote, Ok(Ok(()))) {
                tracing::error!("录音落盘失败: {wrote:?}");
            }
            matches!(wrote, Ok(Ok(())))
        };
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
    /// 优化选中文本不看自动粘贴开关（输回原处就是这个功能本身）；失败且一个字没打出时什么都不做，选区原样留着。
    async fn deliver(&self, final_text: &str, optimized: bool, typing: Option<StreamTyping>, source: OptimizeSource) -> (bool, bool, Option<String>) {
        let force = source == OptimizeSource::Selection;
        let Some(typing) = typing else { return self.auto_copy(final_text, force).await };
        let typed_any = !typing.typed().is_empty();
        if !typed_any && !optimized {
            abort_typing(Some(typing)).await;
            if force {
                return (false, false, None);
            }
            return self.auto_copy(final_text, force).await;
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
                self.auto_copy(final_text, force).await
            }
            Ok((Err(error), copied)) => {
                tracing::warn!("流式输入中断: {error}");
                (copied, false, Some(error))
            }
            Err(error) => (false, false, Some(error.to_string())),
        }
    }

    /// 自动复制并粘贴到焦点窗口（设置 auto_copy_to_clipboard；`force` 时不看开关）。
    async fn auto_copy(&self, text: &str, force: bool) -> (bool, bool, Option<String>) {
        if !force && !self.core.settings().auto_copy_to_clipboard {
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

/// 看门狗强制结束会话时，算不算「识别超时」：用户取消与安静结束本就要丢弃，仍按原因收尾。
fn counts_as_timeout(quiet_end: Option<&'static str>, user_cancelled: bool) -> bool {
    quiet_end.is_none() && !user_cancelled
}

/// 松开时要不要跳过转写直接安静结束：按下不到 0.3 秒算误触（哪怕按键声过了阈值），整场没说话同样跳过。
/// 本段录了多久：经松开缓冲收尾时算到真实松开那一刻，缓冲里多收的尾音不算（否则短按会被当成说过话）。
fn recorded_until(started: Instant, released_at: Option<Instant>, now: Instant) -> Duration {
    released_at.unwrap_or(now).saturating_duration_since(started)
}

fn quiet_end_reason(recorded: Duration, heard: bool) -> Option<&'static str> {
    if recorded.as_secs_f64() < MIN_DURATION_SECONDS {
        Some("录音时间过短")
    } else if !heard {
        Some("没有检测到说话")
    } else {
        None
    }
}

/// 收尾看门狗的上限：各引擎自身超时之和再加余量。正常收尾远早于它，只兜卡死。
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
/// 只写剪贴板（不粘贴）；返回是否写入成功。
async fn copy_to_clipboard(text: &str) -> bool {
    let text = text.to_string();
    matches!(tokio::task::spawn_blocking(move || crate::desktop::write_clipboard(&text)).await, Ok(Ok(())))
}

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
            // pending：末尾还没确认的字数，悬浮窗把这截显示成淡白色。
            // live 只挡已结束的旧会话；与松开并发的那一刻由 update_partial 在锁内按「仍是聽」再判一次
            crate::floating::update_partial(
                &partial_app,
                json!({ "phase": "recording", "mode": "streaming", "text": text, "pending": pending_chars, "duration": 0 }),
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
            crate::floating::update_partial(&app, json!({ "phase": "recording", "mode": "multi", "text": text, "duration": 0 }));
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
    use super::{
        counts_as_timeout, finalize_budget, quiet_end_reason, record_hotkey_step, recorded_until, Mode, Phase,
        RecordHotkeyStep, HOLD_RELEASE_GRACE, MIN_DURATION_SECONDS,
    };
    use att_core::settings::hotkey::HotkeyAction::{ClipboardPanel, Record, RecordToggle};
    use att_hotkey::KeyPhase::{Pressed, Released};
    use std::time::{Duration, Instant};

    #[test]
    fn 按住录音_按下开始_松开收尾() {
        assert_eq!(record_hotkey_step(Record, Pressed, Phase::Idle, false), RecordHotkeyStep::ResumeOrStart);
        assert_eq!(record_hotkey_step(Record, Pressed, Phase::Recording, true), RecordHotkeyStep::ResumeOrStart, "缓冲里重新按下 / 自动重复");
        assert_eq!(record_hotkey_step(Record, Released, Phase::Recording, true), RecordHotkeyStep::Release);
        assert_eq!(record_hotkey_step(Record, Released, Phase::Connecting, true), RecordHotkeyStep::Release);
    }

    #[test]
    fn 点按录音_按一下开始_再按一下结束_松开不管() {
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Idle, false), RecordHotkeyStep::Start { held: false });
        assert_eq!(record_hotkey_step(RecordToggle, Released, Phase::Recording, false), RecordHotkeyStep::Ignore);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Recording, false), RecordHotkeyStep::Stop);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Connecting, false), RecordHotkeyStep::Stop);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Finalizing, false), RecordHotkeyStep::Nudge);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Optimizing, false), RecordHotkeyStep::Start { held: false });
    }

    #[test]
    fn 两种录音键混用_任一键都能结束点按录音_松开只收尾按住录音() {
        // 点按开始的录音里按一下按住键：按下即结束，随后的松开不再触发任何事
        assert_eq!(record_hotkey_step(Record, Pressed, Phase::Recording, false), RecordHotkeyStep::Stop);
        assert_eq!(record_hotkey_step(Record, Released, Phase::Finalizing, false), RecordHotkeyStep::Ignore);
        assert_eq!(record_hotkey_step(Record, Released, Phase::Recording, false), RecordHotkeyStep::Ignore);
        // 按住开始的录音里点一下点按键：同样结束
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Recording, true), RecordHotkeyStep::Stop);
        // 非录音动作不归这里管
        assert_eq!(record_hotkey_step(ClipboardPanel, Pressed, Phase::Idle, false), RecordHotkeyStep::Ignore);
    }

    #[test]
    fn 松开缓冲_吸收修饰键闪断_且不把缓冲算进录音时长() {
        // 实测按住 ⌘⌥ 时 ⌘ 闪断 213ms：缓冲要盖得住，又不能长到让正常松开明显变慢
        assert!(HOLD_RELEASE_GRACE > Duration::from_millis(213) && HOLD_RELEASE_GRACE <= Duration::from_millis(400));

        // 短按 144ms：缓冲到点（多收了 250ms）时仍按松开那一刻算，照样是误触
        let started = Instant::now();
        let released = started + Duration::from_millis(144);
        let recorded = recorded_until(started, Some(released), released + HOLD_RELEASE_GRACE);
        assert_eq!(recorded, Duration::from_millis(144));
        assert!(recorded.as_secs_f64() < MIN_DURATION_SECONDS);
        assert_eq!(quiet_end_reason(recorded, true), Some("录音时间过短"));

        // 不经缓冲（UI 停止、识别失败）按当前时刻算
        let now = started + Duration::from_secs(2);
        assert_eq!(recorded_until(started, None, now), Duration::from_secs(2));
    }

    #[test]
    fn 短按与没说话_松开即安静结束不进转写() {
        // 短按：按键声把电平顶过阈值也算误触
        assert_eq!(quiet_end_reason(Duration::from_millis(150), true), Some("录音时间过短"));
        assert_eq!(quiet_end_reason(Duration::from_millis(150), false), Some("录音时间过短"));
        assert_eq!(quiet_end_reason(Duration::from_secs(3), false), Some("没有检测到说话"));
        assert_eq!(quiet_end_reason(Duration::from_secs(3), true), None);
    }

    #[test]
    fn 看门狗强制收尾_取消与安静结束不算识别超时() {
        assert!(counts_as_timeout(None, false));
        assert!(!counts_as_timeout(None, true));
        assert!(!counts_as_timeout(Some("没有检测到说话"), false));
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
