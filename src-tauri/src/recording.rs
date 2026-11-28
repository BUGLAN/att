//! 录音会话 Actor：在线引擎豆包（流式 + 批量）、Windows 微信输入法（流式 + 批量）、
//! 豆包网页、ChatGPT 听写与 DeepSeek（批量）。
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
//! 录音 / 收尾 / 优化中的 Esc 走同一条取消路径（键盘监听里的内部绑定，不占设置项；生效时吞掉这次 Esc）。
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
//! `recording:optimization` 推给前端与悬浮窗；开启自动输出时同时边生成边打进录音开始时的目标输入框
//! （typing.rs），收尾与最终清洗结果对齐。目标窗口中途切走时停手，只把全文留在剪贴板。
//! 还没打出任何字就失败时退回粘贴原文，保证总有输出。
//! 与旧版语义一致：单段模式优化期间不接受新录音；流式模式允许直接开始下一段，
//! 此时上一段的优化转入后台，结果只入库、不粘贴（避免粘进新输入框）。
//!
//! 没听到人声：识别出了字，但按短帧能量估计录音里几乎没有人声（[`att_core::audio::speech_activity`]，
//! 典型是按住十几秒没说话、底噪被识别成一个词），三种模式一律不送优化、不复制也不输入，
//! 悬浮窗提示检查麦克风；记录照常保存（error 字段注明），便于回听排查。
//!
//! 豆包流式预热（待命连接）：设置是「流式 + 豆包 + 按时长计费」时，空闲期间一直备着一条握好手、
//! 还没发识别请求的连接（握手 1~3 秒，是按下到出字之间最大的一块）。按下时接入参数没变就直接在它上面
//! 发请求推流，实测首字从按下后 2.6~3.8s 降到 1.6s 左右（基本只剩开口说话本身的时间）。
//! - 何时建：启动后、每段录音结束后、连接断开后；[`WARM_TICK`] 定时巡检兜底（改了设置、建连失败后退避重试）；
//! - 保活与判活在 [`keep_warm`]：定时心跳，[`doubao::WARM_STALE`] 没有任何来帧就重建，
//!   备满 [`doubao::WARM_MAX_AGE`] 也换一条新的（服务端约 15 分钟回收连接）；
//! - 复用的连接可能是「写得进去、其实早断了」的死连接（刚从睡眠醒来、网络切换）：发出请求后
//!   [`WARM_ACK_TIMEOUT`] 内没有回执、或出字前推流失败，会话任务静默改为新建并重发已收的音频。
//!
//! 预热槽只在 Actor 里推进。微信输入法没有预热：每次流式录音都由输入法内的常驻编码器新建一条 voice_id。
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
//! 语音改写选中文本（快捷键 [`HotkeyAction::VoiceRewriteSelection`]）与上一条共用「输回选区」的收尾，
//! 但按下时同时开始录音；松开后把识别出的话当作改写要求，和之前复制的选中文字一起送固定模板，
//! 不用设置里的任何预设。没有选区、没有识别出要求、或录音里没人声时都不调用 LLM，原文保持不动。
//!
//! 前端事件：`recording:state`（相位 + 文本 + 记录）、`recording:partial`（实时文本）、
//! `recording:level`（音量 0..1）。

use crate::desktop::FocusTarget;
use crate::typing::StreamTyping;
use att_core::llm::cleaning;
use att_core::llm::optimizer::{self, OptimizeEvent, OptimizeOutcome};
use att_core::records::{Record, RecordStatus};
use att_core::settings::hotkey::{HotkeyAction, ModSet};
use att_core::state::CoreState;
use att_core::stt::{self, deepseek, doubao, doubao_web};
use att_hotkey::listener::CancelBinding;
use att_hotkey::KeyPhase;
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, watch};

/// 低于该时长视为误触（不建记录、不计费）。
const MIN_DURATION_SECONDS: f64 = 0.3;
/// 识别结果为空时，超过该时长的录音仍保存成失败记录，便于换引擎重新识别。
const EMPTY_TRANSCRIPT_RECORD_SECONDS: f64 = 3.0;
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
/// 识别出了字、录音里却几乎没有人声时的提示（悬浮窗「敗」旁与主窗口共用）。
const NO_VOICE_MESSAGE: &str = "没有听到声音，检查麦克风";
/// 同上情况在记录里的标记：写进 error 字段（记录格式不变，旧数据照常可读）。
const NO_VOICE_NOTE: &str = "没有听到人声，识别结果未优化、未输入";

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

/// 录音会话内部 Esc 取消绑定当前应允许的修饰键；`None` 表示回到 Idle、绑定停用。
fn cancel_binding_modifiers(phase: Phase, held_modifiers: ModSet) -> Option<ModSet> {
    match phase {
        Phase::Idle => None,
        // 收尾（按住录音松开后经过松开缓冲才进来，修饰键已经全松开）与之后的优化都不再按着录音快捷键：
        // Esc 用无修饰键绑定，单按 Esc 就能取消，也不会吞掉带修饰键的系统快捷键。
        Phase::Finalizing | Phase::Optimizing => Some(ModSet::default()),
        Phase::Connecting | Phase::Recording => Some(held_modifiers),
    }
}

/// 录音启动失败后回到的相位：已有优化仍在进行时不能吞掉它的后续输出。
fn start_failure_phase(has_optimization: bool) -> Phase {
    if has_optimization {
        Phase::Optimizing
    } else {
        Phase::Idle
    }
}

/// 从独立计数器分配下一个非零会话号。
fn next_session_id(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}

/// Actor 侧留一份采集音频。会话任务卡死时拿不到它自己的 `all`，仍可用这里落盘。
fn append_captured_samples(samples: &Mutex<Vec<i16>>, pcm: &[i16]) {
    let mut samples = samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    samples.extend_from_slice(pcm);
}

fn snapshot_captured_samples(samples: &Mutex<Vec<i16>>) -> Vec<i16> {
    samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// 识别为空是否要保存失败记录：短录音仍按误触/空操作处理。
fn should_record_empty_transcript(duration: f64) -> bool {
    duration > EMPTY_TRANSCRIPT_RECORD_SECONDS
}

/// 批量识别的请求形态：决定收尾预算怎么算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchPlan {
    /// 一次请求识别整段（ChatGPT 听写 / 豆包批量）。
    Single,
    /// DeepSeek：超过 115 秒按块串行请求，每块各受一次单块超时约束。
    DeepSeek { chunks: usize },
    /// 豆包网页：音频按实时速率推送，长录音再按 13 分钟游客会话分段。
    DoubaoWeb { sessions: usize },
}

/// 多段模式的预算参数：每个在途任务的最长运行时间，以及配置错误的兜底下限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MultiDrainPolicy {
    segment_timeout: Duration,
    fallback: Duration,
}

impl MultiDrainPolicy {
    fn for_engine(engine: &stt::Engine, fallback: Duration) -> Self {
        let segment_timeout = match engine {
            // 单段的 FINISH_TIMEOUT 会再按音频时长加时；分段最长 10 秒，这里覆盖建连、推流和末包。
            stt::Engine::Doubao(_) => {
                doubao::CONNECT_TIMEOUT
                    + doubao::FINISH_TIMEOUT
                    + Duration::from_secs_f64(att_core::audio::MAX_SEGMENT_SECONDS / 2.0)
                    + WATCHDOG_SLACK
            }
            stt::Engine::ChatGpt(config) => config.timeout,
            stt::Engine::DoubaoWeb(config) => {
                doubao_web::estimated_timeout(config, Duration::from_secs_f64(att_core::audio::MAX_SEGMENT_SECONDS))
                    + WATCHDOG_SLACK
            }
            stt::Engine::DoubaoIme(config) => config.timeout,
            // DeepSeek 在单个 10 秒分段内仍会按暂时性错误重试三次。
            stt::Engine::DeepSeek(config) => {
                config.timeout.saturating_mul(deepseek::MAX_ATTEMPTS_PER_CHUNK as u32) + Duration::from_secs(3)
            }
            #[cfg(windows)]
            stt::Engine::WeType(config) => config.timeout,
            // 本地推理没有上游请求超时；由运行时可配置值统一限制单段，避免模型卡死拖住整场录音。
            stt::Engine::Local(_) => fallback,
        };
        Self { segment_timeout: segment_timeout.max(Duration::from_secs(1)), fallback }
    }

    fn budget(self, pending_segments: usize) -> Duration {
        let rounds = pending_segments.div_ceil(MULTI_TRANSCRIBE_CONCURRENCY);
        let derived = self
            .segment_timeout
            .saturating_mul(u32::try_from(rounds).unwrap_or(u32::MAX))
            .saturating_add(MULTI_DRAIN_SCHEDULING_SLACK);
        derived.max(self.fallback)
    }
}

/// 解析多段收尾兜底：默认 120 秒，允许通过环境变量提高到最多 7 天；非法值继续使用默认值。
fn parse_multi_drain_fallback(value: Option<&str>) -> Duration {
    let seconds = value.and_then(|value| value.trim().parse::<u64>().ok()).filter(|seconds| *seconds > 0);
    seconds.map(|seconds| Duration::from_secs(seconds.min(7 * 24 * 60 * 60))).unwrap_or(MULTI_DRAIN_DEFAULT_FALLBACK)
}

fn multi_drain_fallback_timeout() -> Duration {
    parse_multi_drain_fallback(std::env::var("ATT_MULTI_DRAIN_FALLBACK_SECONDS").ok().as_deref())
}

/// 多段模式同时在途的识别请求上限：长录音分段很多时，避免瞬间打满在线接口或争抢本地推理锁。
const MULTI_TRANSCRIBE_CONCURRENCY: usize = 2;
/// 多段收尾的默认保底时间。实际等待会按剩余段数、并发上限和单段超时继续放大；
/// 可用 `ATT_MULTI_DRAIN_FALLBACK_SECONDS` 覆盖，避免旧版本固定 120 秒在慢网络下截断尾段。
const MULTI_DRAIN_DEFAULT_FALLBACK: Duration = Duration::from_secs(120);
/// 多段收尾调度的少量余量，确保最后一批正好在单段超时边界返回时仍有提交结果的窗口。
const MULTI_DRAIN_SCHEDULING_SLACK: Duration = Duration::from_secs(1);
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
    /// 豆包预热连接没了（预热序号, 握手后活了多久；建连失败为 0）：断开 / 无响应 / 建连失败
    WarmEnded(u64, Duration),
    /// 预热巡检：该有预热连接而没有时建一条，设置变了就关掉
    WarmTick,
}

/// 预热连接的交接：按下时会话任务从这里拿到握好手的连接（或建连失败的原因）。
type WarmReceiver = oneshot::Receiver<Result<doubao::DoubaoSession, doubao::SttError>>;
type WarmSender = oneshot::Sender<Result<doubao::DoubaoSession, doubao::SttError>>;

/// 豆包预热槽（只在 Actor 里读写）。连接本身由 [`keep_warm`] 任务拿着保活，这里只留认领它的口子。
struct WarmSlot {
    generation: u64,
    /// 握手时的参数：按下时参数变了就不能用
    key: doubao::WarmKey,
    /// 认领：把交接通道发给保活任务，它握好手就把连接送过去；这个口子被丢掉时保活任务关掉连接
    claim: oneshot::Sender<WarmSender>,
}

/// 预热巡检间隔：启动后第一条预热连接、改设置后的跟进、建连失败后的重试都靠它。
const WARM_TICK: Duration = Duration::from_secs(15);
/// 启动后多久做第一次巡检（避开启动高峰）。
const WARM_FIRST_TICK: Duration = Duration::from_secs(3);
/// 预热连接活过这么久才算「建成过」：之后断开立刻重建；没活到就断的按失败退避，免得对着坏网络反复握手。
const WARM_HEALTHY: Duration = Duration::from_secs(30);
/// 预热连续失败时的最长重试间隔。
const WARM_RETRY_MAX: Duration = Duration::from_secs(300);
/// 复用预热连接发出识别请求后，等服务端回执的上限（正常约 0.35 秒）；超过就当死连接、改为新建。
const WARM_ACK_TIMEOUT: Duration = Duration::from_millis(1500);
/// 等回执期间的轮询间隔（音频已经停了、没有别的事件唤醒循环时用）。
const WARM_ACK_POLL: Duration = Duration::from_millis(50);

/// 预热第 `failures` 次连续失败后隔多久再试：15s、30s、60s… 封顶 [`WARM_RETRY_MAX`]。
fn warm_backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(8);
    (WARM_TICK * 2u32.pow(doublings)).min(WARM_RETRY_MAX)
}

/// 预热连接的保活任务：握手 → 定时心跳 → 被认领时交出去。返回握手后活了多久；
/// `None` 表示连接已交出或槽已被丢弃（Actor 自己知道，不用回报）。
async fn keep_warm(config: doubao::DoubaoConfig, mut claim: oneshot::Receiver<WarmSender>) -> Option<Duration> {
    let started = Instant::now();
    let mut session = match doubao::DoubaoSession::open(&config).await {
        Ok(session) => session,
        Err(error) => {
            tracing::info!("豆包预热建连失败，按下时照常新建：{error}");
            // 已经有人在等这条连接：把失败告诉它，让它马上新建
            if let Ok(reply) = claim.try_recv() {
                let _ = reply.send(Err(error));
            }
            return Some(Duration::ZERO);
        }
    };
    tracing::info!(connect_id = %session.connect_id, "豆包预热连接已建好（{:?}）", started.elapsed());
    let mut heartbeat = tokio::time::interval(doubao::WARM_PING_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            biased;
            claimed = &mut claim => {
                match claimed {
                    Ok(reply) => {
                        if let Err(Ok(session)) = reply.send(Ok(session)) {
                            // 会话任务已经不在了（刚按下就取消）
                            session.cancel().await;
                        }
                    }
                    Err(_) => {
                        tracing::debug!(connect_id = %session.connect_id, "豆包预热连接关闭（不再需要）");
                        session.cancel().await;
                    }
                }
                return None;
            }
            _ = session.wait_closed() => break "服务端断开".to_string(),
            _ = heartbeat.tick() => {
                if session.opened_at.elapsed() >= doubao::WARM_MAX_AGE {
                    break "到龄换新".to_string();
                }
                let idle = session.idle_for();
                if idle >= doubao::WARM_STALE {
                    break format!("{}s 没有任何来帧", idle.as_secs());
                }
                if let Err(error) = session.ping().await {
                    break error.to_string();
                }
            }
        }
    };
    let lived = session.opened_at.elapsed();
    tracing::info!(connect_id = %session.connect_id, "豆包预热连接结束（{reason}，已待命 {lived:?}）");
    session.cancel().await;
    Some(lived)
}

/// 一次优化从哪来：决定提示词、结果要不要强制输出、收尾文案。
#[derive(Debug, Clone, PartialEq, Eq)]
enum OptimizeSource {
    /// 录音转写后的自动优化（默认预设，输出跟随「自动复制并粘贴」）
    Recording,
    /// 快捷键优化选中文本（「选中文本优化」预设，总是输回原处替换选区）
    Selection,
    /// 语音指令改写选中文本（固定模板，总是输回原处替换选区）
    VoiceRewriteSelection { instruction: String, selection: String },
}

impl OptimizeSource {
    fn as_str(&self) -> &'static str {
        match self {
            OptimizeSource::Recording => "recording",
            OptimizeSource::Selection => "selection",
            OptimizeSource::VoiceRewriteSelection { .. } => "voice_selection",
        }
    }

    /// 结果是否总是替换原选区（不看自动粘贴开关）。
    fn replaces_selection(&self) -> bool {
        matches!(self, OptimizeSource::Selection | OptimizeSource::VoiceRewriteSelection { .. })
    }
}

/// 进行中的自动优化（转写记录已落库）。
struct PendingOptimization {
    session_id: u64,
    mode: Mode,
    source: OptimizeSource,
    /// 开始录音时的前台窗口；优化输出只在它仍是前台时打进输入框
    focus_target: Option<FocusTarget>,
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
    /// 多段任务关闭音频后回报的实际收尾预算；看门狗据此与 `multi_task` 使用同一个截止时间
    multi_drain_budget: Option<watch::Receiver<Option<Duration>>>,
    /// 多段任务迟迟没有回报预算时的兜底，同时用于推导本地推理的单段上限
    multi_drain_policy: Option<MultiDrainPolicy>,
    /// 按住模式已松开、正处在松开缓冲里：(缓冲序号, 松开时刻)。重新按下时清掉，到点仍在才收尾
    pending_release: Option<(u64, Instant)>,
    /// 由「按住录音」键开始：只有这种会话在松开时收尾；点按录音 / 界面按钮开始的会话不理会松开
    held: bool,
    /// 本次录音是「语音改写选中文本」且已复制到的选中文字；转写完成后走固定模板
    voice_rewrite_selection: Option<String>,
    started: Instant,
    /// 停止录音的时刻（按住模式取真实松开那一刻）：记录里的「处理耗时」从这里算起，不含录音本身
    stopped_at: Option<Instant>,
    /// 本次用的识别引擎与语言（落库写进记录）
    asr: AsrInfo,
    /// 采集回调同步归档的整段 PCM；会话任务无回应时看门狗据此保住录音。
    captured_samples: Arc<Mutex<Vec<i16>>>,
    /// 开始录音时的前台窗口：结果输出前核对，避免焦点切换后打进别的输入框
    focus_target: Option<FocusTarget>,
    /// 本地引擎录音会话持有的模型使用闸门，防止录音途中模型被删除。
    _model_gate: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
}

/// 识别引擎与语言：写进记录，换引擎重试、排查识别质量时对照。
#[derive(Debug, Clone)]
struct AsrInfo {
    /// provider 标识（与设置 `asr_provider` 取值一致）
    provider: &'static str,
    language: String,
}

/// 录音热键在当前相位下该做什么（纯逻辑，见 [`record_hotkey_step`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordHotkeyStep {
    /// 开始新录音；`held` = 由按住录音键开始（松开时收尾）
    Start {
        held: bool,
    },
    /// 按住键按下：在松开缓冲里就接着录，否则当普通按下（开始 / 录音中的自动重复被吞）
    ResumeOrStart,
    Stop,
    /// 按住键松开：进松开缓冲，到点收尾
    Release,
    /// 收尾中按下：摇一下印章表示这下没生效
    Nudge,
    Ignore,
}

/// 语音改写按住录音的再次按下：松开缓冲仍在有效期内时继续本段，不能重新复制选区或摇提示。
fn voice_rewrite_can_resume(phase: Phase, held: bool, is_voice_rewrite: bool, released_at: Option<Instant>) -> bool {
    matches!(phase, Phase::Connecting | Phase::Recording)
        && held
        && is_voice_rewrite
        && released_at.is_some_and(|at| at.elapsed() <= HOLD_RELEASE_GRACE)
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

fn recording_state_uses_asr(state: &Value) -> bool {
    matches!(state.get("phase").and_then(Value::as_str), Some("connecting" | "recording" | "finalizing"))
}

#[derive(Clone)]
pub struct RecordingController {
    tx: mpsc::UnboundedSender<Cmd>,
    snapshot: Arc<Mutex<Value>>,
    cancel_binding: CancelBinding,
    core: Arc<CoreState>,
}

impl RecordingController {
    pub fn spawn(app: AppHandle, core: Arc<CoreState>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let snapshot = Arc::new(Mutex::new(json!({ "phase": "idle" })));
        let cancel_binding = CancelBinding::default();
        let controller_core = core.clone();
        let actor = Actor {
            app,
            core,
            tx: tx.clone(),
            phase: Phase::Idle,
            cancel_binding: cancel_binding.clone(),
            cancel_modifiers: ModSet::default(),
            active: None,
            optimizing: None,
            next_id: 0,
            next_optimization_id: 0,
            next_release: 0,
            warm: None,
            next_warm: 0,
            warm_failures: 0,
            warm_retry_at: None,
            snapshot: snapshot.clone(),
        };
        tauri::async_runtime::spawn(actor.run(rx));
        let tick_tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(WARM_FIRST_TICK).await;
            while tick_tx.send(Cmd::WarmTick).is_ok() {
                tokio::time::sleep(WARM_TICK).await;
            }
        });
        Self { tx, snapshot, cancel_binding, core: controller_core }
    }

    pub fn start(&self) {
        if self.core.backup_maintenance_active() {
            return;
        }
        let _ = self.tx.send(Cmd::Start);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }

    pub fn hotkey(&self, action: HotkeyAction, phase: KeyPhase) {
        if self.core.backup_maintenance_active() && action != HotkeyAction::CancelRecording {
            return;
        }
        let _ = self.tx.send(Cmd::Hotkey(action, phase));
    }

    /// 录音会话内部 Esc 取消绑定的开关（由 Actor 的相位变化推进）。
    pub fn cancel_binding(&self) -> CancelBinding {
        self.cancel_binding.clone()
    }

    /// 最近一次发出的状态（前端挂载时同步用）。
    pub fn state(&self) -> Value {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_else(|_| json!({ "phase": "idle" }))
    }

    /// 录音会话是否仍需要或正在使用 ASR（采集、收尾转写阶段）。
    ///
    /// LLM 自动优化阶段已经拿到转写正文，不再占用本地 ASR 模型，因此允许空闲回收。
    pub fn is_using_asr(&self) -> bool {
        recording_state_uses_asr(&self.state())
    }
}

struct Actor {
    app: AppHandle,
    core: Arc<CoreState>,
    tx: mpsc::UnboundedSender<Cmd>,
    phase: Phase,
    cancel_binding: CancelBinding,
    /// 当前会话允许 Esc 带着的修饰键：按住录音时是录音快捷键的修饰键
    cancel_modifiers: ModSet,
    active: Option<Active>,
    optimizing: Option<PendingOptimization>,
    next_id: u64,
    /// 优化会话独立编号：优化选中文本不启动录音，不能借用录音会话号。
    next_optimization_id: u64,
    /// 松开缓冲序号：晚到的旧缓冲（已被重新按下作废）据此识别
    next_release: u64,
    /// 豆包流式预热连接（见模块说明）
    warm: Option<WarmSlot>,
    next_warm: u64,
    /// 预热连续失败次数与下次允许重试的时刻（退避，见 [`warm_backoff`]）
    warm_failures: u32,
    warm_retry_at: Option<Instant>,
    snapshot: Arc<Mutex<Value>>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Start => {
                    if !self.core.backup_maintenance_active() {
                        self.start(false).await;
                    }
                }
                Cmd::Stop => self.stop("正在完成转写"),
                Cmd::Cancel => self.cancel(),
                Cmd::Hotkey(action, key_phase) => {
                    if !self.core.backup_maintenance_active() || action == HotkeyAction::CancelRecording {
                        self.on_hotkey(action, key_phase).await;
                    }
                }
                Cmd::Connected(id) => {
                    if self.is_current(id) && self.phase == Phase::Connecting {
                        self.set_phase(Phase::Recording);
                        let message = self
                            .active
                            .as_ref()
                            .map(|active| streaming_message(active.asr.provider, false))
                            .unwrap_or("流式转写中");
                        self.emit_state("recording", json!({ "message": message }));
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
                    // 会话任务结束（含被取消的）说明刚用掉一条连接：马上备好下一条，不等巡检与退避
                    self.warm_retry_at = None;
                    self.maintain_warm();
                }
                Cmd::Optimized(id, record_id, outcome, typing) => {
                    self.on_optimized(id, record_id, outcome, typing).await
                }
                Cmd::FinalizeTimeout(id, hard) => self.on_finalize_timeout(id, hard).await,
                Cmd::ReleaseGrace(id, token) => self.on_release_grace(id, token),
                Cmd::WarmEnded(generation, lived) => {
                    if self.warm.as_ref().is_some_and(|slot| slot.generation == generation) {
                        self.warm = None;
                        if lived >= WARM_HEALTHY {
                            // 用了一阵才断（服务端回收、网络切换）：立刻重建
                            self.warm_failures = 0;
                            self.warm_retry_at = None;
                            self.maintain_warm();
                        } else {
                            self.warm_failures += 1;
                            self.warm_retry_at = Some(Instant::now() + warm_backoff(self.warm_failures));
                        }
                    }
                }
                Cmd::WarmTick => self.maintain_warm(),
            }
        }
    }

    /// 当前设置下该不该备着预热连接：流式 + 豆包 + 凭据齐全 + 按时长计费。该备时返回接入配置。
    fn warm_target(&self) -> Option<doubao::DoubaoConfig> {
        let settings = self.core.settings();
        // 只看设置、不走 Engine::resolve（它对本地引擎有预加载的副作用）
        if settings.recording_mode != "streaming" || settings.asr_provider != "doubao_asr" {
            return None;
        }
        let config = stt::doubao_config(&self.core);
        (config.ensure().is_ok() && doubao::warm_allowed(&config)).then_some(config)
    }

    /// 让预热槽跟上当前设置：不该有（或接入参数变了）就关掉，该有而没有就建一条。
    /// 流式录音进行中不建（那条连接正在用，结束时会再来一次）；连续失败后在退避期内不建。
    fn maintain_warm(&mut self) {
        let target = self.warm_target();
        if let Some(slot) = &self.warm {
            if target.as_ref().is_some_and(|config| doubao::WarmKey::new(config) == slot.key) {
                return;
            }
            self.drop_warm("设置已变");
        }
        let Some(config) = target else { return };
        if self.active.as_ref().is_some_and(|active| active.mode == Mode::Streaming) {
            return;
        }
        if self.warm_retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.next_warm += 1;
        let generation = self.next_warm;
        let (claim_tx, claim_rx) = oneshot::channel();
        self.warm = Some(WarmSlot { generation, key: doubao::WarmKey::new(&config), claim: claim_tx });
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(lived) = keep_warm(config, claim_rx).await {
                let _ = tx.send(Cmd::WarmEnded(generation, lived));
            }
        });
    }

    /// 关掉预热连接：丢掉认领口子，保活任务随之关闭连接（还在握手的握完就关）。
    fn drop_warm(&mut self, reason: &str) {
        if self.warm.take().is_some() {
            tracing::debug!("豆包预热连接不再保留（{reason}）");
        }
    }

    /// 按下流式录音时认领预热连接；接入参数变了（换了接入点 / 资源 / 凭据）就关掉。
    /// 返回的通道里稍后会收到握好手的连接：还在握手时由会话任务等它。
    fn take_warm(&mut self, config: &doubao::DoubaoConfig) -> Option<WarmReceiver> {
        let slot = self.warm.take()?;
        if slot.key != doubao::WarmKey::new(config) {
            tracing::debug!("豆包预热连接不再保留（接入参数已变）");
            return None;
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        // 保活任务刚好结束（连接断了，回报还在路上）时认领不到，照常新建
        slot.claim.send(reply_tx).ok()?;
        self.warm_failures = 0;
        Some(reply_rx)
    }

    fn is_current(&self, id: u64) -> bool {
        self.active.as_ref().map(|a| a.id == id).unwrap_or(false)
    }

    /// 相位唯一写入口：同步录音会话内部的 Esc 取消绑定。
    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.cancel_binding.set_modifiers(cancel_binding_modifiers(phase, self.cancel_modifiers));
    }

    fn emit_state(&self, phase: &str, extra: Value) {
        // 删除模型要拒绝正在录音的场景（建连、录音、收尾都算），由数据面读取
        self.core.set_recording_active(matches!(phase, "connecting" | "recording" | "finalizing"));
        let mode = self.active.as_ref().map(|a| a.mode.as_str()).unwrap_or("streaming");
        let mut payload = json!({ "phase": phase, "mode": mode });
        if let Some(provider) = self.active.as_ref().map(|active| active.asr.provider) {
            payload["provider"] = json!(provider);
        }
        if let Some(name) = self.preset_tag(phase) {
            payload["preset"] = json!(name);
        }
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
        // 托盘菜单顶部的「开始 / 停止录音」跟随相位切换
        crate::tray::refresh(&self.app);
        let _ = self.app.emit("recording:state", payload);
    }

    /// 悬浮窗上的预设名：只在这段录音转写后会用默认优化预设自动优化时给（录音 / 收尾 / 优化中）；
    /// 优化选中文本、语音改写用的不是默认预设，不显示。
    fn preset_tag(&self, phase: &str) -> Option<String> {
        let recording = match phase {
            "connecting" | "recording" | "finalizing" => {
                self.active.as_ref().is_some_and(|active| active.voice_rewrite_selection.is_none())
            }
            "optimizing" => self.optimizing.as_ref().is_some_and(|pending| pending.source == OptimizeSource::Recording),
            _ => false,
        };
        if !recording || !(self.core.settings().auto_optimize && optimizer::has_default_model(&self.core)) {
            return None;
        }
        self.core.default_preset().map(|preset| preset.name)
    }

    async fn on_hotkey(&mut self, action: HotkeyAction, key_phase: KeyPhase) {
        if action == HotkeyAction::RewriteSelection {
            if key_phase == KeyPhase::Pressed {
                self.rewrite_selection().await;
            }
            return;
        }
        if action == HotkeyAction::VoiceRewriteSelection {
            match key_phase {
                KeyPhase::Pressed
                    if self.active.as_ref().is_some_and(|active| {
                        voice_rewrite_can_resume(
                            self.phase,
                            active.held,
                            active.voice_rewrite_selection.is_some(),
                            active.pending_release.map(|(_, released_at)| released_at),
                        )
                    }) =>
                {
                    self.resume_released();
                }
                KeyPhase::Pressed => self.begin_voice_rewrite().await,
                KeyPhase::Released
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.held && active.voice_rewrite_selection.is_some()) =>
                {
                    self.release_after_grace();
                }
                KeyPhase::Released => {}
            }
            return;
        }
        if self.active.as_ref().is_some_and(|active| active.voice_rewrite_selection.is_some()) {
            // 语音改写按住期间只认自己的松开；别的录音键不能把它当普通录音停掉
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
        // 选区所在的原始应用：优化流式输出时若焦点已经切走，不能再把改写结果打进新窗口。
        let focus_target = crate::desktop::foreground_target();
        if !att_hotkey::listener::suppresses_keys() {
            // 只读监听（启动时没有「辅助功能」权限）：快捷键的主键已经送进前台应用、替换掉了选区，再复制只会拿到错的内容
            return self.emit_state(
                "error",
                json!({ "message": "优化选中文本需要「辅助功能」权限：授权后请重启应用（否则快捷键会在前台应用里打出字符）" }),
            );
        }
        if !optimizer::has_default_model(&self.core) {
            return self
                .emit_state("error", json!({ "message": "优化选中文本需要 LLM 模型：请先在 设置 → LLM 服务 中添加" }));
        }
        let selection = tokio::task::spawn_blocking(crate::desktop::copy_selection)
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        let text = match selection {
            Ok(Some(text)) => text,
            Ok(None) => {
                return self
                    .emit_state("error", json!({ "message": "没有读到选中的文字：先选中要优化的文本再按快捷键" }))
            }
            Err(message) => {
                tracing::warn!("读取选中文本失败: {message}");
                return self.emit_state("error", json!({ "message": message }));
            }
        };
        tracing::info!("优化选中文本（{} 字）", text.chars().count());
        let record = self.save_record(&[], 0.0, Some(text.clone()), None, 0.0, None).await;
        let Some(record_id) = record.get("id").and_then(Value::as_str).map(str::to_string) else {
            return self.emit_state("error", json!({ "message": "保存记录失败，未开始优化" }));
        };
        self.begin_optimization(Mode::Batch, record_id, text, None, record, OptimizeSource::Selection, focus_target);
    }

    /// 语音改写选中文本：先复制选区（不等待用户松开按键），再开始按住录音。
    /// 松开后识别出的话作为改写要求；没有选区时只提示，不打开麦克风。
    async fn begin_voice_rewrite(&mut self) {
        if self.phase != Phase::Idle {
            tracing::info!("{:?} 中按下语音改写选中文本，忽略", self.phase);
            return crate::floating::nudge(&self.app);
        }
        if !att_hotkey::listener::suppresses_keys() {
            return self.emit_state(
                "error",
                json!({ "message": "语音改写选中文本需要「辅助功能」权限：授权后请重启应用（否则快捷键会在前台应用里打出字符）" }),
            );
        }
        if !optimizer::has_default_model(&self.core) {
            return self.emit_state(
                "error",
                json!({ "message": "语音改写选中文本需要 LLM 模型：请先在 设置 → LLM 服务 中添加" }),
            );
        }
        let selection = tokio::task::spawn_blocking(crate::desktop::copy_selection_while_shortcut_held)
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        let text = match selection {
            Ok(Some(text)) => text,
            Ok(None) => {
                return self.emit_state("error", json!({ "message": "请先选中要改写的文字，再按住快捷键说出改写要求" }))
            }
            Err(message) => {
                tracing::warn!("读取选中文本失败: {message}");
                return self.emit_state("error", json!({ "message": message }));
            }
        };
        tracing::info!("语音改写选中文本（{} 字），开始收音", text.chars().count());
        self.start_with(true, Some(text)).await;
    }

    fn fail_start(&mut self, message: String) {
        tracing::warn!("录音无法开始: {message}");
        // 流式优化中尝试开始新录音时，只有新录音真正打开麦克风后才让它转入后台。
        // 前置检查或打开麦克风失败时，上一段优化仍应继续正常输出。
        let phase = start_failure_phase(self.optimizing.is_some());
        self.set_phase(phase);
        self.active = None;
        self.emit_state("error", json!({ "message": message }));
    }

    /// 开始一段录音；`held` = 由「按住录音」键按下开始（松开时收尾）。
    async fn start(&mut self, held: bool) {
        self.start_with(held, None).await;
    }

    /// 录音统一入口；`voice_rewrite_selection` 有值时强制单段识别，并在转写后走语音改写模板。
    async fn start_with(&mut self, held: bool, voice_rewrite_selection: Option<String>) {
        if self.phase == Phase::Optimizing {
            // 流式：允许开始新录音；单段：优化期间保持「处理中」，吞掉。
            // 此时先不接管上一段优化，等新录音确认打开麦克风后再切到 Idle。
            match self.optimizing.as_ref() {
                Some(pending) if pending.mode == Mode::Streaming => {
                    tracing::info!("流式优化中准备开始新录音：先检查新录音能否启动");
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
        let voice_rewrite = voice_rewrite_selection.is_some();
        let model_gate = if stt::local_kind(&settings.asr_provider).is_some() {
            match self.core.model_gate.clone().try_read_owned() {
                Ok(guard) => Some(guard),
                Err(_) => {
                    return self.fail_start("本地模型正在维护，请稍后再开始录音".to_string());
                }
            }
        } else {
            None
        };
        self.cancel_modifiers = if held { start_cancel_modifiers(&settings, voice_rewrite) } else { ModSet::default() };
        // 本地引擎不在这里等模型加载（只校验文件并在后台预加载），录音按下即开始，推理时再等加载完成
        let engine = match stt::Engine::resolve(&self.core) {
            Ok(engine) => engine,
            Err(message) => return self.fail_start(message),
        };
        let mode = if voice_rewrite {
            // 改写要求是一句短话，等松开后一次性识别最稳，也避免流式中间结果影响提示词
            Mode::Batch
        } else {
            match settings.recording_mode.as_str() {
                "streaming" => Mode::Streaming,
                "multi" => Mode::Multi,
                _ => Mode::Batch,
            }
        };
        let streaming_config = match streaming_config_for(&engine, mode) {
            Ok(config) => config,
            Err(message) => return self.fail_start(message),
        };
        let language = settings.asr_language.clone();
        // 预热连接只给豆包流式用；别的录音不动它（设置真变了由巡检关掉）
        let warm = match streaming_config.as_ref() {
            Some(StreamingConfig::Doubao(config)) => self.take_warm(config),
            None => None,
            Some(StreamingConfig::DoubaoIme(_)) => None,
            #[cfg(windows)]
            Some(StreamingConfig::WeType(_)) => None,
        };
        let asr = AsrInfo {
            provider: engine.provider(),
            language: att_core::stt::effective_language(engine.provider(), &language),
        };
        let id = next_session_id(&mut self.next_id);
        // 输出目标：录音开始时的前台窗口。多段收尾 / 优化可能持续数秒，期间用户切走焦点，
        // 结果不能跟着打进新窗口（尤其别打进设置页或别的输入框）。
        let focus_target = crate::desktop::foreground_target();

        // 采集最先启动（按下即收数），建连与静音并行进行
        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let level_app = self.app.clone();
        let mut last_level_emit = Instant::now() - LEVEL_EMIT_INTERVAL;
        // 与多段切段同一个阈值判定「有没有说话」（多段模式本就据此丢弃无声分片）
        let heard = Arc::new(AtomicBool::new(false));
        let heard_by_level = heard.clone();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_by_chunk = captured.clone();
        let silence_db = settings.silence_level as f64;
        let mut voiced_samples = 0usize;
        let on_chunk: att_audio::ChunkCallback = Box::new(move |pcm, level| {
            if !heard_by_level.load(Ordering::Relaxed) && att_core::audio::chunk_db(pcm) > silence_db {
                voiced_samples += pcm.len();
                if voiced_samples as f64 >= HEARD_MIN_VOICED.as_secs_f64() * SAMPLE_RATE {
                    heard_by_level.store(true, Ordering::Relaxed);
                }
            }
            append_captured_samples(&captured_by_chunk, pcm);
            let _ = audio_tx.send(pcm.to_vec());
            if last_level_emit.elapsed() >= LEVEL_EMIT_INTERVAL {
                last_level_emit = Instant::now();
                let _ = level_app.emit("recording:level", json!({ "level": level }));
            }
        });
        let device = settings.audio_input_device.clone();
        // 按下即亮「聽」：打开麦克风可能要几百毫秒（蓝牙耳机切换通话模式更久），别让用户干等没有反馈。
        // 打不开时 fail_start 推「敗」接上；之后的 connecting / recording 映射成同一状态，胶囊不会重播
        crate::floating::update(
            &self.app,
            json!({ "phase": "recording", "mode": mode.as_str(), "text": "", "duration": 0 }),
            true,
        );
        let opened = tokio::task::spawn_blocking(move || att_audio::start_capture(device, on_chunk)).await;
        let capture = match opened {
            Ok(Ok(capture)) => capture,
            Ok(Err(message)) => return self.fail_start(message),
            Err(error) => return self.fail_start(format!("打开麦克风失败: {error}")),
        };

        if let Some(pending) = self.optimizing.as_ref().filter(|pending| pending.mode == Mode::Streaming) {
            tracing::info!("新录音麦克风已打开：上一段流式优化转入后台，停止往输入框输出");
            pending.detached.store(true, Ordering::SeqCst);
            // 新录音的文字要进新的输入框：上一段别再往里打
            pending.typing_halt.store(true, Ordering::SeqCst);
            self.set_phase(Phase::Idle);
        }

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
            multi_drain_budget: None,
            multi_drain_policy: None,
            pending_release: None,
            held,
            voice_rewrite_selection,
            started: Instant::now(),
            stopped_at: None,
            asr,
            captured_samples: captured,
            focus_target,
            _model_gate: model_gate,
        });

        let tx = self.tx.clone();
        match mode {
            Mode::Streaming => {
                self.set_phase(Phase::Connecting);
                let message = streaming_message(engine.provider(), true);
                self.emit_state("connecting", json!({ "message": message }));
                let app = self.app.clone();
                let config = streaming_config.expect("流式模式已校验为支持的引擎");
                tauri::async_runtime::spawn(streaming_task(
                    id, config, language, warm, audio_rx, cancel_rx, heard, live, app, tx,
                ));
            }
            Mode::Batch => {
                self.set_phase(Phase::Recording);
                let message = if voice_rewrite { "请说出改写要求，松开结束" } else { "正在录音" };
                self.emit_state("recording", json!({ "message": message }));
                tauri::async_runtime::spawn(batch_task(id, engine, language, audio_rx, cancel_rx, tx));
            }
            Mode::Multi => {
                self.set_phase(Phase::Recording);
                self.emit_state("recording", json!({ "message": "静音切段监听中" }));
                let segmenter =
                    att_core::audio::Segmenter::new(settings.silence_level as f64, settings.silence_threshold);
                let app = self.app.clone();
                let policy = MultiDrainPolicy::for_engine(&engine, multi_drain_fallback_timeout());
                let (budget_tx, budget_rx) = watch::channel::<Option<Duration>>(None);
                if let Some(active) = self.active.as_mut().filter(|active| active.id == id) {
                    active.multi_drain_budget = Some(budget_rx);
                    active.multi_drain_policy = Some(policy);
                }
                tauri::async_runtime::spawn(multi_task(
                    id, engine, language, audio_rx, cancel_rx, segmenter, live, app, tx, policy, budget_tx,
                ));
            }
        }

        // 静音放在建连之后（osascript 两次调用约 300ms，不能挡在建连前面）
        if settings.pause_audio_on_recording {
            let muted = mute_output().await;
            if let Some(active) = self.active.as_mut().filter(|a| a.id == id) {
                active.muted_by_us = muted;
            } else if muted {
                // 极短录音：静音完成前会话已结束，立即恢复
                restore_output().await;
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
        self.set_phase(Phase::Finalizing);
        let Some(active) = self.active.as_mut() else { return };
        active.live.store(false, Ordering::SeqCst);
        if let Some(capture) = active.capture.take() {
            std::thread::spawn(move || capture.stop());
        }
        active.pending_release = None;
        let now = Instant::now();
        active.stopped_at = Some(released_at.unwrap_or(now));
        let recorded = recorded_until(active.started, released_at, now);
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
        let multi_budget = active.multi_drain_budget.take();
        let multi_policy = active.multi_drain_policy;
        self.emit_state("finalizing", json!({ "message": message }));
        if let (Mode::Multi, Some(budget), Some(policy)) = (mode, multi_budget, multi_policy) {
            self.arm_multi_watchdog(id, budget, policy.fallback);
        } else {
            let budget = self.finalize_budget(mode, recorded);
            self.arm_watchdog(id, Some(budget));
        }
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

    /// 多段收尾先等会话任务在音频关闭后回报实际段数预算，再启动同额看门狗。
    /// 音频迟迟不能关闭或会话任务提前退出时，用可配置兜底超时取消，避免 Actor 永久卡在收尾中。
    fn arm_multi_watchdog(&self, id: u64, mut budget_rx: watch::Receiver<Option<Duration>>, fallback: Duration) {
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            let announced = async {
                loop {
                    if let Some(budget) = *budget_rx.borrow_and_update() {
                        break Some(budget);
                    }
                    if budget_rx.changed().await.is_err() {
                        break None;
                    }
                }
            };
            let budget = tokio::time::timeout(fallback, announced).await.ok().flatten().unwrap_or(fallback);
            tokio::time::sleep(budget).await;
            let _ = tx.send(Cmd::FinalizeTimeout(id, false));
            tokio::time::sleep(WATCHDOG_GRACE).await;
            let _ = tx.send(Cmd::FinalizeTimeout(id, true));
        });
    }

    fn finalize_budget(&self, mode: Mode, recorded: Duration) -> Duration {
        let settings = self.core.settings();
        let (batch_timeout, plan) = batch_plan(
            &settings.asr_provider,
            recorded,
            settings.deepseek_asr_timeout_seconds,
            settings.doubao_web_timeout_seconds,
            settings.chatgpt_asr_timeout_seconds,
            settings.wetype_asr_timeout_seconds,
        );
        finalize_budget_for_provider(mode, &settings.asr_provider, recorded, batch_timeout, plan)
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
        let samples = snapshot_captured_samples(&active.captured_samples);
        self.finish(SessionOutcome { samples, text: String::new(), error: None, cancelled: true }).await;
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
        self.set_phase(Phase::Finalizing);
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
            // 等恢复做完再进 Idle：紧接着的下一段要先读到「未静音」才会再静音
            restore_output().await;
        }
        self.set_phase(Phase::Idle);
        // 让 emit_state 仍能带上本次模式
        let mode = active.mode;
        let (quiet_end, timed_out) = (active.quiet_end, active.timed_out);
        let voice_rewrite_selection = active.voice_rewrite_selection.take();
        let processing_time = processing_seconds(active.stopped_at, Instant::now());
        let asr = active.asr.clone();
        let focus_target = active.focus_target;
        self.active = Some(Active { capture: None, cancel: None, ..active });
        let duration = outcome.samples.len() as f64 / SAMPLE_RATE;
        if outcome.cancelled && timed_out {
            // 看门狗取消的不算用户取消：带回来的音频与文字照常保存，原因记为超时
            outcome.cancelled = false;
            outcome.error = Some("识别超时，未等到识别结果".to_string());
        }

        if let (true, Some(reason)) = (outcome.cancelled, quiet_end) {
            // 松开时是误触或整场没说话：不进转写、不建记录，悬浮窗直接收起
            let message = quiet_end_message(reason, voice_rewrite_selection.is_some());
            self.emit_state("completed", json!({ "message": message, "text": "", "silent": true }));
        } else if outcome.cancelled {
            self.emit_state("cancelled", json!({ "message": "已取消本次录音" }));
        } else if outcome.samples.is_empty() && outcome.text.is_empty() && outcome.error.is_some() {
            // 会话任务卡死被强制结束：手里什么都没有，只报原因
            self.emit_state("error", json!({ "message": outcome.error }));
        } else if duration < MIN_DURATION_SECONDS && outcome.text.is_empty() {
            // 误触：同样安静收起
            let message = quiet_end_message("录音时间过短", voice_rewrite_selection.is_some());
            self.emit_state("completed", json!({ "message": message, "text": "", "silent": true }));
        } else if outcome.text.is_empty() {
            match outcome.error {
                // 识别失败且没有任何文本：音频落盘并建失败记录，避免录音丢失
                Some(error) => {
                    let record = self
                        .save_record(&outcome.samples, duration, None, Some(error.clone()), processing_time, Some(&asr))
                        .await;
                    self.emit_state("error", json!({ "message": error, "record": record }));
                }
                None => {
                    if should_record_empty_transcript(duration) {
                        let error = "未识别到文字".to_string();
                        let record = self
                            .save_record(
                                &outcome.samples,
                                duration,
                                None,
                                Some(error.clone()),
                                processing_time,
                                Some(&asr),
                            )
                            .await;
                        let message = if voice_rewrite_selection.is_some() {
                            format!("{error}，选中文字未改动")
                        } else {
                            error
                        };
                        self.emit_state("error", json!({ "message": message, "record": record }));
                    } else {
                        let message = if voice_rewrite_selection.is_some() {
                            "没有识别到语音内容，选中文字未改动"
                        } else {
                            "没有识别到语音内容"
                        };
                        self.emit_state("completed", json!({ "message": message, "text": "" }));
                    }
                }
            }
        } else if let Some(activity) = missing_voice(&outcome.samples) {
            // 识别出了字，但录音里几乎没有人声：多半是底噪或一下杂音被识别成了词（实测按住 16s 没说话，识别出「Clean.」）。
            // 不送优化、不复制也不输入；记录照常保存并在 error 里注明，便于回听排查
            tracing::info!(
                "没有听到人声，识别结果不输出（{}，共 {:.1}s，底噪 {:.0} dBFS，清楚 {:.2}s，隐约 {:.2}s，识别 {} 字）",
                mode.as_str(),
                activity.total_seconds,
                activity.noise_floor_db,
                activity.clear_seconds,
                activity.faint_seconds,
                outcome.text.chars().count()
            );
            let note = match &outcome.error {
                Some(error) => format!("{NO_VOICE_NOTE}；{error}"),
                None => NO_VOICE_NOTE.to_string(),
            };
            let raw_text = match &voice_rewrite_selection {
                Some(selection) => compose_voice_rewrite_raw_text(&outcome.text, selection),
                None => outcome.text.clone(),
            };
            let record = self
                .save_record(
                    &outcome.samples,
                    duration,
                    Some(raw_text.clone()),
                    Some(note),
                    processing_time,
                    Some(&asr),
                )
                .await;
            let mut state = no_voice_state(&raw_text, record);
            if voice_rewrite_selection.is_some() {
                state["source"] = json!("voice_selection");
                state["message"] = json!("没有听到改写要求，检查麦克风；选中文字未改动");
            }
            self.emit_state("completed", state);
        } else {
            if let Some(selection) = voice_rewrite_selection {
                // 语音要求与选中文字分开送模型；记录仍沿用 Selection 的形状：raw_text = 要求 + 原文
                let raw_text = compose_voice_rewrite_raw_text(&outcome.text, &selection);
                let record = self
                    .save_record(
                        &outcome.samples,
                        duration,
                        Some(raw_text.clone()),
                        outcome.error.clone(),
                        processing_time,
                        Some(&asr),
                    )
                    .await;
                let Some(record_id) = record.get("id").and_then(Value::as_str).map(str::to_string) else {
                    self.emit_state("error", json!({ "message": "保存记录失败，选中文字未改写" }));
                    self.active = None;
                    return;
                };
                self.begin_optimization(
                    mode,
                    record_id,
                    raw_text,
                    outcome.error.clone(),
                    record,
                    OptimizeSource::VoiceRewriteSelection { instruction: outcome.text.clone(), selection },
                    focus_target,
                );
                self.active = None;
                return;
            }
            let record = self
                .save_record(
                    &outcome.samples,
                    duration,
                    Some(outcome.text.clone()),
                    outcome.error.clone(),
                    processing_time,
                    Some(&asr),
                )
                .await;
            let record_id = record.get("id").and_then(Value::as_str).map(str::to_string);
            // 微信输入法流式超时/断连时只有半截累计文本：记录照常保存，但不自动输出或优化，
            // 避免把未收到 end_flag 的内容打进用户输入框。
            let withhold_partial = withhold_partial_stream_output(mode, asr.provider, &outcome.error);
            let auto_optimize = self.core.settings().auto_optimize && optimizer::has_default_model(&self.core);
            if let (true, Some(record_id)) = (auto_optimize && !withhold_partial, record_id) {
                // 自动优化：本次不在转写阶段粘贴，优化结果流式打进输入框（否则原文、优化文各出一遍）
                self.begin_optimization(
                    mode,
                    record_id,
                    outcome.text.clone(),
                    outcome.error.clone(),
                    record,
                    OptimizeSource::Recording,
                    focus_target,
                );
                self.active = None;
                return;
            }
            let (copied, pasted, copy_error) = if withhold_partial {
                (false, false, None)
            } else {
                self.auto_copy(&outcome.text, false, focus_target).await
            };
            let message = match (&outcome.error, withhold_partial) {
                (Some(error), true) => format!("微信输入法流式识别中断，已保存已识别内容但未自动输出：{error}"),
                (Some(error), false) => format!("识别中断，已保存已识别内容：{error}"),
                (None, _) => "录音已完成".to_string(),
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
        tracing::info!("录音会话结束（{}，{:.2}s，识别 {} 字）", mode.as_str(), duration, outcome.text.chars().count());
        self.active = None;
    }

    /// 进入 Optimizing：优化在独立任务里跑（不阻塞 Actor 处理新命令），增量直接推给前端与悬浮窗，
    /// 开启自动输出时（优化选中文本则总是）同时流式打进录音开始时的目标输入框。
    #[allow(clippy::too_many_arguments)]
    fn begin_optimization(
        &mut self,
        mode: Mode,
        record_id: String,
        raw_text: String,
        transcribe_error: Option<String>,
        record: Value,
        source: OptimizeSource,
        focus_target: Option<FocusTarget>,
    ) {
        let session_id = next_session_id(&mut self.next_optimization_id);
        let detached = Arc::new(AtomicBool::new(false));
        let typing_halt = Arc::new(AtomicBool::new(false));
        let settings = self.core.settings();
        let output = source.replaces_selection() || settings.auto_copy_to_clipboard;
        let mut typing = output.then(|| StreamTyping::spawn(typing_halt.clone(), focus_target));
        let preset_id = match &source {
            OptimizeSource::Recording => None,
            OptimizeSource::Selection => settings.rewrite_preset_id.clone(),
            OptimizeSource::VoiceRewriteSelection { .. } => None,
        };
        let message = match &source {
            OptimizeSource::Recording => "转写完成，正在优化",
            OptimizeSource::Selection => "正在优化选中文本",
            OptimizeSource::VoiceRewriteSelection { .. } => "正在按语音要求改写选中文本",
        };
        let voice_rewrite = match &source {
            OptimizeSource::VoiceRewriteSelection { instruction, selection } => {
                Some((instruction.clone(), selection.clone()))
            }
            _ => None,
        };
        let source_label = source.as_str();
        self.set_phase(Phase::Optimizing);
        self.optimizing = Some(PendingOptimization {
            session_id,
            mode,
            source,
            focus_target,
            record_id: record_id.clone(),
            raw_text: raw_text.clone(),
            transcribe_error,
            detached,
            typing_halt,
        });
        self.emit_state(
            "optimizing",
            json!({ "mode": mode.as_str(), "source": source_label, "message": message, "text": raw_text, "record": record }),
        );

        let core = self.core.clone();
        let app = self.app.clone();
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            let mut accumulated = String::new();
            let delta_record = record_id.clone();
            let on_event = |event| {
                let OptimizeEvent::Delta(chunk) = event else { return };
                accumulated.push_str(&chunk);
                // 输入框只出「收尾清洗不会剥掉」的部分（套话 / 思考块不上屏）。
                // 悬浮窗不回填优化稿：它停留很短，一闪而过反而突兀，「轉」期间一直显示识别稿
                let stable = cleaning::stable_prefix(&accumulated);
                if let Some(typing) = typing.as_mut() {
                    typing.advance(&stable);
                }
                let _ = app.emit("recording:optimization", json!({ "record_id": &delta_record, "text": chunk }));
            };
            let outcome = match voice_rewrite {
                Some((instruction, selection)) => {
                    optimizer::optimize_record_with_voice_rewrite(&core, &record_id, &instruction, &selection, on_event)
                        .await
                }
                None => {
                    optimizer::optimize_record_with_preset(&core, &record_id, &raw_text, preset_id.as_deref(), on_event)
                        .await
                }
            };
            let _ = tx.send(Cmd::Optimized(session_id, delta_record, outcome, typing));
        });
    }

    /// 自动优化结束：补齐流式输入（失败且没打出字时粘贴原文）；已转后台的只同步记录、不输出。
    async fn on_optimized(
        &mut self,
        session_id: u64,
        record_id: String,
        outcome: OptimizeOutcome,
        typing: Option<StreamTyping>,
    ) {
        let record =
            self.core.records.get(&record_id).ok().and_then(|r| serde_json::to_value(r).ok()).unwrap_or(Value::Null);
        let Some(pending) = self.optimizing.take_if(|p| p.session_id == session_id) else {
            // 已被更新的一段录音（及其优化）取代：只同步记录，不粘贴、不动当前会话
            abort_typing(typing).await;
            crate::tray::refresh(&self.app);
            self.emit_background_optimization(record, &outcome);
            return;
        };
        crate::tray::refresh(&self.app);
        let replace_selection = pending.source.replaces_selection();
        let voice_rewrite = matches!(pending.source, OptimizeSource::VoiceRewriteSelection { .. });
        let original_text = match &pending.source {
            OptimizeSource::VoiceRewriteSelection { selection, .. } => selection.clone(),
            _ => pending.raw_text.clone(),
        };
        let (final_text, optimized, optimize_error, base_message) = match (&outcome, replace_selection) {
            (OptimizeOutcome::Completed(success), false) => {
                (success.optimized_text.clone(), true, None, "录音已完成，已优化".to_string())
            }
            (OptimizeOutcome::Completed(success), true) => (
                success.optimized_text.clone(),
                true,
                None,
                if voice_rewrite { "选中文本已按语音要求改写" } else { "选中文本已优化" }.to_string(),
            ),
            (OptimizeOutcome::Failed(error), false) => {
                (pending.raw_text.clone(), false, Some(error.clone()), format!("优化失败，已保留转写原文：{error}"))
            }
            (OptimizeOutcome::Failed(error), true) => {
                let prefix = if voice_rewrite { "改写失败" } else { "优化失败" };
                (original_text.clone(), false, Some(error.clone()), format!("{prefix}，选中的文字没有改动：{error}"))
            }
            (OptimizeOutcome::Cancelled, _) => {
                (original_text.clone(), false, None, "已取消优化，保留原始文本".to_string())
            }
        };
        let truncated = matches!(&outcome, OptimizeOutcome::Completed(s) if s.truncated);
        let cancelled = matches!(outcome, OptimizeOutcome::Cancelled);

        if pending.detached.load(Ordering::SeqCst) {
            // 用户已开始下一段录音：上一段结果只入库，不粘贴、不抢占界面状态
            abort_typing(typing).await;
            self.emit_background_optimization(record, &outcome);
            return;
        }

        self.set_phase(Phase::Idle);
        // 用户主动取消优化：与旧版一致只保留原文，不粘贴；已打出的字保留，不再补打。
        // 优化选中文本时已打出的字替换掉了选区：把原文放进剪贴板，用户能贴回去
        let (copied, pasted, copy_error) = if cancelled {
            let typed_any = typing.as_ref().is_some_and(|t| !t.typed().is_empty());
            abort_typing(typing).await;
            let copied = replace_selection && typed_any && copy_to_clipboard(&original_text).await;
            (copied, false, None)
        } else {
            self.deliver(&final_text, optimized, typing, &pending.source, pending.focus_target).await
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
    /// `asr`：识别用的引擎与语言；优化选中文本建的文本记录没有识别，传 None。
    async fn save_record(
        &self,
        samples: &[i16],
        duration: f64,
        text: Option<String>,
        error: Option<String>,
        processing_time: f64,
        asr: Option<&AsrInfo>,
    ) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        let audio_name = format!("{id}.{}", att_core::audio::RECORDING_EXTENSION);
        // 没有音频（优化选中文本建的文本记录）：不落空音频文件，audio_path 留空
        let audio_ok = !samples.is_empty() && {
            let audio_path = self.core.records.audio_dir().join(&audio_name);
            let samples = samples.to_vec();
            let wrote =
                tokio::task::spawn_blocking(move || att_core::audio::write_recording_16k(&audio_path, &samples)).await;
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
            engine: asr.map(|asr| asr.provider.to_string()),
            language: asr.map(|asr| asr.language.clone()),
            ..Default::default()
        };
        if let Err(error) = self.core.records.insert(record) {
            tracing::error!("保存记录失败: {error}");
            return Value::Null;
        }
        crate::tray::refresh(&self.app);
        self.core.records.get(&id).ok().and_then(|r| serde_json::to_value(r).ok()).unwrap_or(Value::Null)
    }

    /// 输出优化收尾的文本。流式输入打出过内容：成功则补齐到最终文本，失败则停在已打出的部分；
    /// 两种情况都把最终文本（失败时为原文）放进剪贴板。一个字都没打出（未授权 / 失败在首字前）：退回整段粘贴。
    /// 优化选中文本不看自动粘贴开关（输回原处就是这个功能本身）；失败且一个字没打出时什么都不做，选区原样留着。
    async fn deliver(
        &self,
        final_text: &str,
        optimized: bool,
        typing: Option<StreamTyping>,
        source: &OptimizeSource,
        focus_target: Option<FocusTarget>,
    ) -> (bool, bool, Option<String>) {
        let force = source.replaces_selection();
        let Some(typing) = typing else { return self.auto_copy(final_text, force, focus_target).await };
        let typed_any = !typing.typed().is_empty();
        if !typed_any && !optimized {
            abort_typing(Some(typing)).await;
            if force {
                return (false, false, None);
            }
            return self.auto_copy(final_text, force, focus_target).await;
        }
        let text = final_text.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let typed = if optimized { typing.complete(&text) } else { typing.stop() };
            (typed, crate::desktop::write_clipboard(&text).is_ok())
        })
        .await;
        match result {
            Ok((Ok(()), copied)) if optimized => (copied, true, None),
            Ok((Ok(()), copied)) => {
                (copied, false, Some("优化中断：输入框里是部分优化结果，原文已复制到剪贴板".into()))
            }
            Ok((Err(error), _)) if !typed_any => {
                tracing::warn!("流式输入不可用，改为整段粘贴: {error}");
                self.auto_copy(final_text, force, focus_target).await
            }
            Ok((Err(error), copied)) => {
                tracing::warn!("流式输入中断: {error}");
                let error = if error == crate::typing::FOCUS_CHANGED {
                    format!("{}，文本已复制到剪贴板", crate::typing::FOCUS_CHANGED)
                } else {
                    error
                };
                (copied, false, Some(error))
            }
            Err(error) => (false, false, Some(error.to_string())),
        }
    }

    /// 自动复制并粘贴到录音开始时的目标窗口（设置 auto_copy_to_clipboard；`force` 时不看开关）。
    /// 目标窗口已不是前台时只写剪贴板，不发送粘贴，避免把文本打进用户后来切到的输入框。
    async fn auto_copy(
        &self,
        text: &str,
        force: bool,
        focus_target: Option<FocusTarget>,
    ) -> (bool, bool, Option<String>) {
        if !force && !self.core.settings().auto_copy_to_clipboard {
            return (false, false, None);
        }
        let text = text.to_string();
        let result = tokio::task::spawn_blocking(move || {
            crate::desktop::write_clipboard(&text)?;
            crate::desktop::paste_active_to(focus_target).map_err(|error| {
                if error == "目标窗口已切换" {
                    format!("已复制，{}，文本已复制到剪贴板", crate::typing::FOCUS_CHANGED)
                } else {
                    format!("已复制，{error}")
                }
            })
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

/// 识别出了字但录音里几乎没有人声时，返回人声估计（用于日志）；没有音频（强制收尾带回的空结果）时不判定。
fn missing_voice(samples: &[i16]) -> Option<att_core::audio::SpeechActivity> {
    if samples.is_empty() {
        return None;
    }
    let activity = att_core::audio::speech_activity(samples);
    (!activity.heard_voice()).then_some(activity)
}

/// 没听到人声时的收尾载荷：text 留空——主窗口据此只弹提示、不当成一次成功的转写，
/// 悬浮窗按「敗 + 提示」收起（见 floating::from_recording_state）。识别出的字在 raw_text 与记录里。
fn no_voice_state(raw_text: &str, record: Value) -> Value {
    json!({ "message": NO_VOICE_MESSAGE, "text": "", "raw_text": raw_text, "record": record, "no_voice": true })
}

/// 记录里的「处理耗时」：从停止录音（按住模式是真实松开那一刻，松开缓冲算在处理里；多段模式最后一段也在这时送出）
/// 到拿到识别结果，三种模式同一口径，不含录音本身。自动 / 手动优化只写 optimize_time，不重新计算这个值。
/// 没经过停止就结束的（麦克风中途断开）记 0，界面不显示。
fn processing_seconds(stopped_at: Option<Instant>, done_at: Instant) -> f64 {
    stopped_at.map(|at| done_at.saturating_duration_since(at).as_secs_f64()).unwrap_or(0.0)
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

/// 按住期间 Esc 取消绑定的修饰键：普通录音跟随「按住录音」，语音改写跟随自己的热键。
/// 两者可能是不同的组合；取错会让录音中的 Esc 匹配不上，取消失效。
fn start_cancel_modifiers(settings: &att_core::settings::UserSettings, voice_rewrite: bool) -> ModSet {
    let action = if voice_rewrite { HotkeyAction::VoiceRewriteSelection } else { HotkeyAction::Record };
    settings.binding_of(action).filter(|binding| binding.enabled).map(|binding| binding.modifiers).unwrap_or_default()
}

/// 语音改写的安静结束提示：原文没动，文案要说清楚没有发生改写。
fn quiet_end_message(reason: &'static str, voice_rewrite: bool) -> &'static str {
    if !voice_rewrite {
        return reason;
    }
    match reason {
        "没有检测到说话" => "没有听到改写要求，选中文字未改动",
        "录音时间过短" => "按键时间过短，选中文字未改动",
        _ => reason,
    }
}

/// 选中文本记录的形状：raw_text = 语音要求 + 原文；optimized_text 由优化落盘补齐。
fn compose_voice_rewrite_raw_text(instruction: &str, selection: &str) -> String {
    format!("改写要求：{}\n\n原文：\n{}", instruction.trim(), selection)
}

/// 批量引擎的收尾预算参数：单次请求超时 + 请求形态。
fn batch_plan(
    provider: &str,
    recorded: Duration,
    deepseek_timeout_seconds: i64,
    doubao_web_timeout_seconds: i64,
    chatgpt_timeout_seconds: i64,
    wetype_timeout_seconds: i64,
) -> (Duration, BatchPlan) {
    if provider == "doubao_web_asr" {
        let config = doubao_web::DoubaoWebConfig::from_seconds(doubao_web_timeout_seconds);
        let sessions = doubao_web::session_count_for_duration(recorded + HOLD_RELEASE_GRACE);
        (config.timeout, BatchPlan::DoubaoWeb { sessions })
    } else if provider == "doubao_ime_asr" {
        (Duration::from_secs(120), BatchPlan::Single)
    } else if provider == "deepseek_asr" {
        // 按住模式松开后还会多收一小段尾音（不算进 recorded），估算块数时补上，避免边界上少算一块
        let chunks = deepseek::chunk_count_for_duration(recorded + HOLD_RELEASE_GRACE);
        let timeout = Duration::from_secs(deepseek::request_timeout_seconds(deepseek_timeout_seconds));
        (timeout, BatchPlan::DeepSeek { chunks })
    } else if provider == "wetype_asr" {
        (Duration::from_secs(wetype_timeout_seconds.clamp(10, 600) as u64), BatchPlan::Single)
    } else {
        (Duration::from_secs(chatgpt_timeout_seconds.max(0) as u64), BatchPlan::Single)
    }
}

/// 收尾看门狗的上限：各引擎自身超时之和再加余量。正常收尾远早于它，只兜卡死。
///
/// DeepSeek 长录音是「115 秒一块、顺序 await」：每块最多尝试 [`deepseek::MAX_ATTEMPTS_PER_CHUNK`] 次，
/// 预算必须按「块数 × 尝试次数」放大，不能沿用 `recorded / 2`——600 秒录音有 6 块，
/// 最坏 6 × 3 × 120 秒，旧算法只给 318 秒。
#[cfg(test)]
fn finalize_budget(mode: Mode, recorded: Duration, batch_timeout: Duration, plan: BatchPlan) -> Duration {
    finalize_budget_for_provider(mode, "doubao_asr", recorded, batch_timeout, plan)
}

fn finalize_budget_for_provider(
    mode: Mode,
    provider: &str,
    recorded: Duration,
    batch_timeout: Duration,
    plan: BatchPlan,
) -> Duration {
    // 豆包：建连 + 末包等待（批量另按音频时长加时）+ 关闭
    let doubao = doubao::CONNECT_TIMEOUT + doubao::FINISH_TIMEOUT + Duration::from_secs(2);
    let engine = match mode {
        Mode::Streaming if provider == "wetype_asr" => {
            batch_timeout.max(Duration::from_secs(30)) + Duration::from_secs(2)
        }
        Mode::Streaming => doubao,
        // 正常情况下多段走 `arm_multi_watchdog` 的动态预算；这里只兜内部状态异常时仍保证能收尾。
        Mode::Multi => MULTI_DRAIN_DEFAULT_FALLBACK,
        Mode::Batch => match plan {
            BatchPlan::Single => batch_timeout.max(doubao + recorded / 2),
            BatchPlan::DeepSeek { chunks } => batch_timeout * (chunks.max(1) * deepseek::MAX_ATTEMPTS_PER_CHUNK) as u32,
            BatchPlan::DoubaoWeb { sessions } => {
                let control_per_session = batch_timeout.saturating_mul(6).saturating_add(Duration::from_secs(2));
                recorded.saturating_add(
                    control_per_session.saturating_mul(u32::try_from(sessions.max(1)).unwrap_or(u32::MAX)),
                )
            }
        },
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
    tokio::task::spawn_blocking(|| {
        OUTPUT_MUTE.mute_with(crate::desktop::output_muted, crate::desktop::set_output_muted)
    })
    .await
    .unwrap_or(false)
}

/// 恢复本应用静音的系统输出，等它做完。
async fn restore_output() {
    let _ = tokio::task::spawn_blocking(|| OUTPUT_MUTE.restore_with(false, crate::desktop::set_output_muted)).await;
}

/// 退出、重启、安装更新之前调用（同步）：本应用静音过系统输出就立即恢复，之后不再静音。
///
/// 这些路径不经过录音收尾（`finish`），不在这里恢复的话系统输出会一直静音。
/// 正在进行的静音会先做完再恢复；可以重复调用。
pub fn restore_output_before_exit() {
    OUTPUT_MUTE.restore_with(true, crate::desktop::set_output_muted);
}

/// 安装更新失败、进程继续运行：重新允许录音时暂停外放。
pub fn allow_output_mute_again() {
    OUTPUT_MUTE.resume();
}

static OUTPUT_MUTE: OutputMute = OutputMute::new();

/// 「系统输出由本应用静音、尚未恢复」的全局状态。
///
/// 静音与恢复都在锁内读改系统音量，退出时的恢复不会和录音开始时的静音交错（先恢复、后被静音）。
struct OutputMute {
    state: Mutex<OutputMuteState>,
}

struct OutputMuteState {
    /// 本应用把系统输出静音了，还没恢复
    muted_by_us: bool,
    /// 正在退出 / 重启 / 安装更新：不再静音
    exiting: bool,
}

impl OutputMute {
    const fn new() -> Self {
        Self { state: Mutex::new(OutputMuteState { muted_by_us: false, exiting: false }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutputMuteState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 系统输出当前未静音时静音它；返回是否由这次调用静音。
    fn mute_with(&self, is_muted: impl FnOnce() -> Option<bool>, set_muted: impl FnOnce(bool) -> bool) -> bool {
        let mut state = self.lock();
        if state.exiting {
            return false;
        }
        let muted = matches!(is_muted(), Some(false)) && set_muted(true);
        if muted {
            state.muted_by_us = true;
        }
        muted
    }

    /// 本应用静音过就恢复；`exiting` 为真时之后不再静音。返回是否执行了恢复。
    fn restore_with(&self, exiting: bool, set_muted: impl FnOnce(bool) -> bool) -> bool {
        let mut state = self.lock();
        if exiting {
            state.exiting = true;
        }
        if !std::mem::take(&mut state.muted_by_us) {
            return false;
        }
        if !set_muted(false) {
            tracing::warn!("恢复系统输出静音失败");
        }
        true
    }

    fn resume(&self) {
        self.lock().exiting = false;
    }
}

/// 按下时预热连接还在握手，最多等它这么久（正常 1~1.5 秒握好，它比现在新建起步早）；再久就当网络不好、
/// 直接新建，两段加起来不至于超出收尾看门狗给建连留的预算太多。
const WARM_CONNECT_WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
enum StreamingConfig {
    Doubao(doubao::DoubaoConfig),
    DoubaoIme(stt::doubao_ime::DoubaoImeConfig),
    #[cfg(windows)]
    WeType(stt::wetype::WetypeConfig),
}

fn streaming_config_for(engine: &stt::Engine, mode: Mode) -> Result<Option<StreamingConfig>, String> {
    if mode != Mode::Streaming {
        return Ok(None);
    }
    match engine {
        stt::Engine::Doubao(config) => Ok(Some(StreamingConfig::Doubao(config.clone()))),
        stt::Engine::DoubaoIme(config) => Ok(Some(StreamingConfig::DoubaoIme(config.clone()))),
        #[cfg(windows)]
        stt::Engine::WeType(config) => Ok(Some(StreamingConfig::WeType(config.clone()))),
        _ => Err(format!("{}暂不支持流式录音，请在设置中改用单段或多段录音模式", engine.label())),
    }
}

impl StreamingConfig {
    fn label(&self) -> &'static str {
        match self {
            StreamingConfig::Doubao(_) => "豆包语音",
            StreamingConfig::DoubaoIme(_) => stt::doubao_ime::LABEL,
            #[cfg(windows)]
            StreamingConfig::WeType(_) => stt::wetype::LABEL,
        }
    }

    fn finish_timeout(&self) -> Duration {
        match self {
            StreamingConfig::Doubao(_) => doubao::FINISH_TIMEOUT,
            StreamingConfig::DoubaoIme(config) => config.timeout,
            #[cfg(windows)]
            StreamingConfig::WeType(config) => config.timeout,
        }
    }
}

/// 流式录音给前端的阶段文案；前端 nativeSession 有同一套缺省文案兜底。
fn streaming_message(provider: &str, connecting: bool) -> &'static str {
    match (provider, connecting) {
        ("wetype_asr", true) => "正在连接微信输入法语音",
        ("wetype_asr", false) => "微信输入法转写中",
        ("doubao_ime_asr", true) => "正在连接豆包输入法服务",
        ("doubao_ime_asr", false) => "豆包输入法转写中",
        (_, true) => "正在连接豆包语音服务",
        (_, false) => "豆包语音转写中",
    }
}

/// 流式识别中断时是否只留记录、不把半截文字自动送进输入框。
fn withhold_partial_stream_output(mode: Mode, provider: &str, error: &Option<String>) -> bool {
    mode == Mode::Streaming && (provider == "wetype_asr" || provider == "doubao_ime_asr") && error.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamingPartial {
    text: String,
    pending_chars: usize,
}

type StreamingCallback = Arc<dyn Fn(StreamingPartial) + Send + Sync>;

enum StreamingSession {
    Doubao(doubao::DoubaoSession),
    DoubaoIme(stt::doubao_ime::DoubaoImeSession),
    #[cfg(windows)]
    WeType(stt::wetype::WetypeSession),
}

impl StreamingSession {
    fn current_text(&self) -> String {
        match self {
            StreamingSession::Doubao(session) => session.current_text(),
            StreamingSession::DoubaoIme(session) => session.current_text(),
            #[cfg(windows)]
            StreamingSession::WeType(session) => session.current_text(),
        }
    }

    /// 识别请求发出后服务端是否回过帧（复用预热连接时确认它没死）。
    fn acked(&self) -> bool {
        match self {
            StreamingSession::Doubao(session) => session.acked(),
            StreamingSession::DoubaoIme(_) => true,
            #[cfg(windows)]
            StreamingSession::WeType(_) => true,
        }
    }

    async fn push(&mut self, samples: &[i16]) -> Result<(), String> {
        match self {
            StreamingSession::Doubao(session) => session.push(samples).await.map_err(|error| error.to_string()),
            StreamingSession::DoubaoIme(session) => session.push(samples).await,
            #[cfg(windows)]
            StreamingSession::WeType(session) => session.push(samples).await,
        }
    }

    async fn finish(self, timeout: Duration, quiet: Option<Duration>) -> Result<(String, Option<String>), String> {
        match self {
            StreamingSession::Doubao(session) => session
                .finish(timeout, quiet)
                .await
                .map(|outcome| (outcome.text, outcome.warning))
                .map_err(|error| error.to_string()),
            StreamingSession::DoubaoIme(session) => session.finish(timeout).await.map(|text| (text, None)),
            #[cfg(windows)]
            StreamingSession::WeType(session) => session.finish(timeout).await.map(|text| (text, None)),
        }
    }

    async fn cancel(self) {
        match self {
            StreamingSession::Doubao(session) => session.cancel().await,
            StreamingSession::DoubaoIme(session) => session.cancel().await,
            #[cfg(windows)]
            StreamingSession::WeType(session) => session.cancel().await,
        }
    }
}

/// 建连结果：会话 + 是否复用了预热连接。
type ConnectFuture = Pin<Box<dyn Future<Output = (Result<StreamingSession, String>, bool)> + Send>>;

/// 新建一条流式连接。
fn connect_fresh(config: &StreamingConfig, language: &str, on_partial: StreamingCallback) -> ConnectFuture {
    let config = config.clone();
    let language = language.to_string();
    Box::pin(async move {
        let result = match config {
            StreamingConfig::Doubao(config) => {
                let callback = on_partial.clone();
                let adapter: doubao::PartialCallback = Arc::new(move |partial: doubao::Partial| {
                    callback(StreamingPartial { text: partial.text, pending_chars: partial.pending_chars });
                });
                doubao::DoubaoSession::connect(&config, &language, "", Some(adapter))
                    .await
                    .map(StreamingSession::Doubao)
                    .map_err(|error| error.to_string())
            }
            StreamingConfig::DoubaoIme(config) => {
                let callback = on_partial.clone();
                let adapter: stt::doubao_ime::PartialCallback = Arc::new(move |text: String| {
                    callback(StreamingPartial { text, pending_chars: 0 });
                });
                stt::doubao_ime::DoubaoImeSession::connect(&config, &language, Some(adapter))
                    .await
                    .map(StreamingSession::DoubaoIme)
            }
            #[cfg(windows)]
            StreamingConfig::WeType(config) => {
                let callback = on_partial.clone();
                let adapter: stt::wetype::PartialCallback = Arc::new(move |text: String| {
                    callback(StreamingPartial { text, pending_chars: 0 });
                });
                stt::wetype::WetypeSession::connect(&config, &language, Some(adapter))
                    .await
                    .map(StreamingSession::WeType)
            }
        };
        (result, false)
    })
}

/// 优先用预热连接：等它握好手（最多 [`WARM_CONNECT_WAIT`]）、确认没断就发识别请求直接用；否则照旧新建。
fn connect_or_reuse(
    config: &doubao::DoubaoConfig,
    language: &str,
    on_partial: StreamingCallback,
    warm: Option<WarmReceiver>,
) -> ConnectFuture {
    let streaming_config = StreamingConfig::Doubao(config.clone());
    let Some(warm) = warm else { return connect_fresh(&streaming_config, language, on_partial) };
    let fresh = connect_fresh(&streaming_config, language, on_partial.clone());
    let config = config.clone();
    let language = language.to_string();
    Box::pin(async move {
        let reason = match tokio::time::timeout(WARM_CONNECT_WAIT, warm).await {
            Ok(Ok(Ok(mut session))) => {
                let idle = session.idle_for();
                let begun = match session.check_alive() {
                    Err(error) => Err(format!("已断开：{error}")),
                    // 心跳早该到了却没到（典型是刚从睡眠醒来）：别在死连接上浪费回执超时
                    Ok(()) if idle >= doubao::WARM_STALE => Err(format!("{}s 没有任何来帧", idle.as_secs())),
                    Ok(()) => {
                        let callback = on_partial.clone();
                        let adapter: doubao::PartialCallback = Arc::new(move |partial: doubao::Partial| {
                            callback(StreamingPartial { text: partial.text, pending_chars: partial.pending_chars });
                        });
                        session
                            .begin(&config, &language, "", Some(adapter))
                            .await
                            .map_err(|error| format!("发送识别请求失败：{error}"))
                    }
                };
                match begun {
                    Ok(()) => {
                        tracing::info!(
                            connect_id = %session.connect_id,
                            "复用豆包预热连接（已待命 {:?}）",
                            session.opened_at.elapsed()
                        );
                        return (Ok(StreamingSession::Doubao(session)), true);
                    }
                    Err(reason) => {
                        session.cancel().await;
                        reason
                    }
                }
            }
            Ok(Ok(Err(error))) => format!("预热建连失败：{error}"),
            Ok(Err(_)) => "预热任务已结束".to_string(),
            Err(_) => "预热建连超时".to_string(),
        };
        tracing::info!("豆包预热连接不可用（{reason}），改为新建");
        fresh.await
    })
}

fn connect_stream(
    config: &StreamingConfig,
    language: &str,
    on_partial: StreamingCallback,
    warm: Option<WarmReceiver>,
) -> ConnectFuture {
    match config {
        StreamingConfig::Doubao(config) => connect_or_reuse(config, language, on_partial, warm),
        StreamingConfig::DoubaoIme(_) => connect_fresh(config, language, on_partial),
        #[cfg(windows)]
        StreamingConfig::WeType(_) => connect_fresh(config, language, on_partial),
    }
}

/// 流式会话任务：把中间结果接到界面（悬浮窗 + 前端事件），识别本身在 [`streaming_session`]。
#[allow(clippy::too_many_arguments)]
async fn streaming_task(
    id: u64,
    config: StreamingConfig,
    language: String,
    warm: Option<WarmReceiver>,
    audio_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    cancel_rx: oneshot::Receiver<()>,
    heard: Arc<AtomicBool>,
    live: Arc<AtomicBool>,
    app: AppHandle,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let on_text: StreamingCallback = Arc::new(move |partial: StreamingPartial| {
        let StreamingPartial { text, pending_chars } = partial;
        // 已经出字就算说过话（小声说话电平可能压在阈值下，但识别出来了）
        if !text.trim().is_empty() {
            heard.store(true, Ordering::Relaxed);
        }
        if live.load(Ordering::SeqCst) {
            // pending：末尾还没确认的字数，悬浮窗把这截显示成淡白色。
            // live 只挡已结束的旧会话；与松开并发的那一刻由 update_partial 在锁内按「仍是聽」再判一次
            crate::floating::update_partial(
                &app,
                json!({ "phase": "recording", "mode": "streaming", "text": text, "pending": pending_chars, "duration": 0 }),
            );
        }
        let _ = app.emit("recording:partial", json!({ "text": text }));
    });
    streaming_session(id, config, language, warm, audio_rx, cancel_rx, on_text, tx).await;
}

/// 流式会话：建连与采集并行，建连期间样本先缓存，就绪后冲刷；音频通道关闭即发末包收尾。
///
/// 有预热连接时先试它；复用的连接在出字前推流失败、或发出识别请求后迟迟没有回执（死连接）
/// 就改为新建、把已收的音频重发一遍，用户无感。
#[allow(clippy::too_many_arguments)]
async fn streaming_session(
    id: u64,
    config: StreamingConfig,
    language: String,
    warm: Option<WarmReceiver>,
    mut audio_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    mut cancel_rx: oneshot::Receiver<()>,
    on_text: StreamingCallback,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let started = Instant::now();
    // 出字耗时埋点（距会话开始 ≈ 按下）：建连就绪 / 首字 / 松开后收尾，会话结束打一行汇总
    let mut ready_at: Option<Duration> = None;
    let first_text_at: Arc<OnceLock<Duration>> = Arc::new(OnceLock::new());
    let first_text_mark = first_text_at.clone();
    let on_partial: StreamingCallback = Arc::new(move |partial: StreamingPartial| {
        if !partial.text.trim().is_empty() {
            let _ = first_text_mark.set(started.elapsed());
        }
        on_text(partial);
    });
    let mut connect = connect_stream(&config, &language, on_partial.clone(), warm);
    // 当前会话是否复用的预热连接（出字前出错时据此改为新建）
    let mut reused = false;
    let mut reused_any = false;

    let mut session: Option<StreamingSession> = None;
    let mut connect_done = false;
    let mut audio_open = true;
    let mut pending: Vec<i16> = Vec::new();
    let mut all: Vec<i16> = Vec::new();
    let mut failure: Option<String> = None;
    // 复用预热连接后等服务端回执的截止时刻；等到之前即使音频已停也不收尾（死连接上收尾只会白等到超时）
    let mut ack_deadline: Option<Instant> = None;

    while audio_open || !connect_done || ack_deadline.is_some() {
        if let Some(deadline) = ack_deadline {
            if failure.is_some() || session.as_ref().is_none_or(StreamingSession::acked) {
                ack_deadline = None;
            } else if Instant::now() >= deadline {
                tracing::info!(
                    "预热连接 {WARM_ACK_TIMEOUT:?} 内没有回执，按死连接处理：改为新建并重发 {} 采样",
                    all.len()
                );
                if let Some(stale) = session.take() {
                    // 死连接上发关闭帧可能卡住，别挡着新建
                    tokio::spawn(stale.cancel());
                }
                pending = all.clone();
                connect = connect_stream(&config, &language, on_partial.clone(), None);
                connect_done = false;
                reused = false;
                ack_deadline = None;
            }
            if !(audio_open || !connect_done || ack_deadline.is_some()) {
                break;
            }
        }
        tokio::select! {
            // 等回执期间保证循环被唤醒（音频已停时没有别的事件）
            _ = tokio::time::sleep(WARM_ACK_POLL), if ack_deadline.is_some() => {}
            _ = &mut cancel_rx => {
                // 取消也带回已收的音频与文字：用户取消时由 Actor 丢弃，看门狗超时时据此保存
                let text = session.as_ref().map(StreamingSession::current_text).unwrap_or_default();
                if let Some(session) = session.take() {
                    session.cancel().await;
                }
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true,
                }));
                return;
            }
            (result, from_warm) = &mut connect, if !connect_done => {
                connect_done = true;
                match result {
                    Ok(mut ready) => {
                        reused = from_warm;
                        reused_any |= from_warm;
                        if !pending.is_empty() {
                            if let Err(error) = ready.push(&pending).await {
                                if reused {
                                    tracing::info!("预热连接推流失败（{error}），改为新建并重发 {} 采样", pending.len());
                                    ready.cancel().await;
                                    connect = connect_stream(&config, &language, on_partial.clone(), None);
                                    connect_done = false;
                                    reused = false;
                                    continue;
                                }
                                failure = Some(error.to_string());
                                let _ = tx.send(Cmd::Failed(id));
                            }
                            pending = Vec::new();
                        }
                        if reused && failure.is_none() {
                            ack_deadline = Some(Instant::now() + WARM_ACK_TIMEOUT);
                        }
                        if ready_at.is_none() {
                            ready_at = Some(started.elapsed());
                            let _ = tx.send(Cmd::Connected(id));
                        }
                        tracing::info!(
                            "流式就绪（{}；{:?}，{}，缓冲 {} 采样）",
                            config.label(),
                            started.elapsed(),
                            if reused { "复用预热连接" } else { "新建连接" },
                            all.len()
                        );
                        session = Some(ready);
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
                                    if reused && first_text_at.get().is_none() {
                                        // 复用的预热连接还没出字就断了：改为新建，已收音频整段重发
                                        tracing::info!("预热连接推流失败（{error}），改为新建并重发 {} 采样", all.len());
                                        if let Some(stale) = session.take() {
                                            stale.cancel().await;
                                        }
                                        pending = all.clone();
                                        connect = connect_stream(&config, &language, on_partial.clone(), None);
                                        connect_done = false;
                                        reused = false;
                                    } else {
                                        failure = Some(error.to_string());
                                        let _ = tx.send(Cmd::Failed(id));
                                    }
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
    let partial = session.as_ref().map(StreamingSession::current_text).unwrap_or_default();
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
                match active.finish(config.finish_timeout(), Some(doubao::FINISH_QUIET)).await {
                    Ok((text, warning)) => (text, warning),
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
        "流式出字耗时（距按下，{}）：建连就绪 {}，首字 {}，松开 {}，收尾 {}",
        match (reused_any, reused) {
            (true, true) => "复用预热连接",
            (true, false) => "预热连接中途失效、改为新建",
            _ => "新建连接",
        },
        ms(ready_at),
        ms(first_text_at.get().copied()),
        ms(Some(released_at)),
        ms(Some(started.elapsed() - released_at)),
    );
    let _ = tx.send(Cmd::Done(id, SessionOutcome { samples: all, text, error, cancelled }));
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
                    samples: all, text: String::new(), error: None, cancelled: true,
                }));
                return;
            }
            chunk = audio_rx.recv() => match chunk {
                Some(pcm) => all.extend_from_slice(&pcm),
                None => break,
            },
        }
    }
    let (text, error, cancelled) = if (all.len() as f64 / SAMPLE_RATE) < MIN_DURATION_SECONDS {
        (String::new(), None, false)
    } else {
        // 识别期间同样响应取消（ChatGPT 听写默认可等 120s）
        tokio::select! {
            result = engine.transcribe_pcm(&language, &all) => match result {
                Ok(transcript) => (transcript.text, transcript.warning, false),
                Err(error) => (String::new(), Some(error), false),
            },
            _ = &mut cancel_rx => (String::new(), None, true),
        }
    };
    let _ = tx.send(Cmd::Done(id, SessionOutcome { samples: all, text, error, cancelled }));
}

/// 启动一个分段识别任务；共享信号量限制同时进入引擎的分段数量。
fn spawn_multi_segment<F, Fut>(
    index: usize,
    semaphore: Arc<tokio::sync::Semaphore>,
    result_tx: mpsc::UnboundedSender<(usize, Result<stt::BatchTranscript, String>)>,
    timeout: Duration,
    transcribe: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<stt::BatchTranscript, String>> + Send + 'static,
{
    tokio::spawn(async move {
        let result = match semaphore.acquire_owned().await {
            Ok(_permit) => match tokio::time::timeout(timeout, transcribe()).await {
                Ok(result) => result,
                Err(_) => Err(format!("多段第 {} 段识别超时", index + 1)),
            },
            Err(_) => Err("多段识别并发控制已关闭".to_string()),
        };
        let _ = result_tx.send((index, result));
    })
}

/// 取消或收尾结束时中止所有排队/在途分段，并等句柄真正退出。
/// 排队任务还没拿到 permit 就会被 abort，因此不会在用户取消后继续发起 ASR 请求。
async fn abort_multi_segments(tasks: &mut Vec<tokio::task::JoinHandle<()>>) {
    for task in tasks.iter() {
        task.abort();
    }
    while let Some(task) = tasks.pop() {
        let _ = task.await;
    }
}

/// 按原始分段顺序拼接成功结果；失败和超时段不参与拼接。
fn join_multi_texts(texts: &[Option<Result<stt::BatchTranscript, String>>]) -> String {
    texts
        .iter()
        .filter_map(|t| t.as_ref().and_then(|r| r.as_ref().ok()))
        .filter(|transcript| !transcript.text.is_empty())
        .map(|transcript| transcript.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 多段会话：静音切段 → 每段受并发上限约束送批量识别 → 按段序实时拼接推给前端与悬浮窗。
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
    policy: MultiDrainPolicy,
    drain_budget_tx: watch::Sender<Option<Duration>>,
) {
    let mut all: Vec<i16> = Vec::new();
    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<(usize, Result<stt::BatchTranscript, String>)>();
    let mut texts: Vec<Option<Result<stt::BatchTranscript, String>>> = Vec::new();
    let mut segment_tasks = Vec::new();
    let segment_semaphore = Arc::new(tokio::sync::Semaphore::new(MULTI_TRANSCRIBE_CONCURRENCY));
    let mut spawn_segment = |index: usize, segment: Vec<i16>| {
        let (engine, language, result_tx) = (engine.clone(), language.clone(), result_tx.clone());
        segment_tasks.push(spawn_multi_segment(
            index,
            segment_semaphore.clone(),
            result_tx,
            policy.segment_timeout,
            move || async move { engine.transcribe_pcm(&language, &segment).await },
        ));
    };
    let emit_partial = |text: &str| {
        if live.load(Ordering::SeqCst) {
            crate::floating::update_partial(
                &app,
                json!({ "phase": "recording", "mode": "multi", "text": text, "duration": 0 }),
            );
        }
        let _ = app.emit("recording:partial", json!({ "text": text }));
    };

    let mut audio_open = true;
    while audio_open {
        tokio::select! {
            _ = &mut cancel_rx => {
                abort_multi_segments(&mut segment_tasks).await;
                let text = join_multi_texts(&texts);
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true,
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
                emit_partial(&join_multi_texts(&texts));
            }
        }
    }
    if let Some(segment) = segmenter.finish() {
        texts.push(None);
        spawn_segment(texts.len() - 1, segment);
    }
    drop(result_tx);

    // 收尾：按实际剩余段数排满并发轮次；外层的动态看门狗共享同一个预算。
    let pending = texts.iter().filter(|text| text.is_none()).count();
    let budget = policy.budget(pending);
    let _ = drain_budget_tx.send(Some(budget));
    let deadline = tokio::time::Instant::now() + budget;
    while texts.iter().any(Option::is_none) {
        tokio::select! {
            _ = &mut cancel_rx => {
                abort_multi_segments(&mut segment_tasks).await;
                let text = join_multi_texts(&texts);
                let _ = tx.send(Cmd::Done(id, SessionOutcome {
                    samples: all, text, error: None, cancelled: true,
                }));
                return;
            }
            received = tokio::time::timeout_at(deadline, result_rx.recv()) => match received {
                Ok(Some((index, result))) => {
                    texts[index] = Some(result);
                    emit_partial(&join_multi_texts(&texts));
                }
                Ok(None) | Err(_) => break,
            },
        }
    }
    abort_multi_segments(&mut segment_tasks).await;
    while let Ok((index, result)) = result_rx.try_recv() {
        texts[index] = Some(result);
    }
    let text = join_multi_texts(&texts);
    let pending = texts.iter().filter(|text| text.is_none()).count();
    let mut reasons = Vec::new();
    for result in &texts {
        match result {
            Some(Err(error)) => reasons.push(error.clone()),
            Some(Ok(transcript)) => {
                if let Some(warning) = &transcript.warning {
                    reasons.push(warning.clone());
                }
            }
            None => {}
        }
    }
    if pending > 0 {
        reasons.push(format!("{pending} 个分段识别超时"));
    }
    let error = (!reasons.is_empty()).then(|| reasons.join("；"));
    let _ = tx.send(Cmd::Done(id, SessionOutcome { samples: all, text, error, cancelled: false }));
}

#[cfg(test)]
mod tests {
    use super::recording_state_uses_asr;
    use super::voice_rewrite_can_resume;
    use super::OutputMute;
    use super::{
        abort_multi_segments, append_captured_samples, batch_plan, cancel_binding_modifiers,
        compose_voice_rewrite_raw_text, counts_as_timeout, finalize_budget, finalize_budget_for_provider,
        join_multi_texts, missing_voice, next_session_id, no_voice_state, parse_multi_drain_fallback,
        processing_seconds, quiet_end_message, quiet_end_reason, record_hotkey_step, recorded_until,
        should_record_empty_transcript, snapshot_captured_samples, spawn_multi_segment, start_cancel_modifiers,
        start_failure_phase, streaming_config_for, withhold_partial_stream_output, BatchPlan, Mode, MultiDrainPolicy,
        Phase, RecordHotkeyStep, StreamingConfig, HOLD_RELEASE_GRACE, MIN_DURATION_SECONDS,
        MULTI_DRAIN_DEFAULT_FALLBACK, MULTI_DRAIN_SCHEDULING_SLACK, MULTI_TRANSCRIBE_CONCURRENCY, NO_VOICE_MESSAGE,
        WATCHDOG_SLACK,
    };
    use super::{
        keep_warm, streaming_session, warm_backoff, Cmd, SessionOutcome, StreamingCallback, StreamingPartial,
        WarmReceiver, WarmSender, WARM_ACK_TIMEOUT, WARM_CONNECT_WAIT, WARM_HEALTHY, WARM_RETRY_MAX, WARM_TICK,
    };
    use att_core::settings::hotkey::HotkeyAction::{ClipboardPanel, Record, RecordToggle, VoiceRewriteSelection};
    use att_core::settings::hotkey::ModSet;
    use att_core::stt::{self, doubao};
    use att_hotkey::KeyPhase::{Pressed, Released};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::{mpsc, oneshot};

    /// 记录 set_muted 调用的桩，返回值固定为成功。
    fn record(calls: &Mutex<Vec<bool>>) -> impl FnOnce(bool) -> bool + '_ {
        move |muted| {
            calls.lock().unwrap().push(muted);
            true
        }
    }

    #[test]
    fn output_mute_restores_only_what_we_muted() {
        let mute = OutputMute::new();
        let calls = Mutex::new(Vec::new());
        // 系统本来就静音：不动它，恢复时也不取消静音
        assert!(!mute.mute_with(|| Some(true), record(&calls)));
        assert!(!mute.restore_with(false, record(&calls)));
        assert!(calls.lock().unwrap().is_empty());
        // 由我们静音：恢复一次，再恢复不重复调用
        assert!(mute.mute_with(|| Some(false), record(&calls)));
        assert!(mute.restore_with(false, record(&calls)));
        assert!(!mute.restore_with(false, record(&calls)));
        assert_eq!(*calls.lock().unwrap(), vec![true, false]);
        // 读不到状态或静音失败：不记为由我们静音
        assert!(!mute.mute_with(|| None, |_| true));
        assert!(!mute.mute_with(|| Some(false), |_| false));
        assert!(!mute.restore_with(false, |_| panic!("不应恢复")));
    }

    #[test]
    fn output_mute_exit_restores_and_blocks_further_mute() {
        let mute = OutputMute::new();
        assert!(mute.mute_with(|| Some(false), |_| true));
        // 退出：录音中被静音的输出立即恢复，之后（例如退出前还在排队的下一段）不再静音
        let calls = Mutex::new(Vec::new());
        assert!(mute.restore_with(true, record(&calls)));
        assert_eq!(*calls.lock().unwrap(), vec![false]);
        assert!(!mute.mute_with(|| Some(false), |_| panic!("退出后不应静音")));
        // 安装更新失败、进程继续：恢复静音能力
        mute.resume();
        assert!(mute.mute_with(|| Some(false), |_| true));
    }

    #[test]
    fn output_mute_exit_waits_for_in_flight_mute() {
        // 录音开始的静音正在进行（持锁读改系统音量）时触发退出：要等它做完再恢复，不能先恢复、后被静音
        let mute = Arc::new(OutputMute::new());
        let system_muted = Arc::new(Mutex::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let muter = {
            let (mute, system_muted) = (mute.clone(), system_muted.clone());
            std::thread::spawn(move || {
                mute.mute_with(
                    || Some(false),
                    |muted| {
                        entered_tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(50));
                        *system_muted.lock().unwrap() = muted;
                        true
                    },
                )
            })
        };
        entered_rx.recv().unwrap();
        let restored = mute.restore_with(true, |muted| {
            *system_muted.lock().unwrap() = muted;
            true
        });
        assert!(muter.join().unwrap());
        assert!(restored);
        assert!(!*system_muted.lock().unwrap());
    }

    /// 本地豆包桩里一条连接的行为。
    #[derive(Clone, Copy)]
    enum StubConn {
        /// 握手成功后什么都不回（「写得进去、其实早断了」的死连接）
        Dead,
        /// 全量请求回一个空结果帧当回执，音频包回带文字的结果帧，末包回末包结果
        Alive,
        /// 与 `Alive` 相同，但始终不回末包。
        NoFinal,
    }

    fn stub_result_frame(text: &str, last: bool) -> Vec<u8> {
        let body = json!({ "result": { "text": text } }).to_string().into_bytes();
        // 头：版本 1 / 头长 1，full server response + 序号标志（末包 0b0011），JSON 不压缩
        let mut frame = vec![0x11, if last { 0x93 } else { 0x91 }, 0x10, 0x00];
        frame.extend_from_slice(&(if last { -1i32 } else { 1 }).to_be_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    /// 本地豆包桩：按连接顺序套用 `scripts` 里的行为（用完后沿用最后一个）；返回接入点地址与已接受的连接数。
    async fn stub_doubao(scripts: Vec<StubConn>) -> (String, Arc<AtomicUsize>) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let script = scripts[index.min(scripts.len() - 1)];
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let mut requested = false;
                    while let Some(Ok(message)) = ws.next().await {
                        let Message::Binary(data) = message else { continue };
                        if matches!(script, StubConn::Dead) {
                            continue;
                        }
                        let reply = if !requested {
                            requested = true;
                            stub_result_frame("", false)
                        } else {
                            // 音频帧 flags 的 bit1 = 末包
                            let last = !matches!(script, StubConn::NoFinal) && data[1] & 0b0010 != 0;
                            stub_result_frame("你好世界", last)
                        };
                        let _ = ws.send(Message::binary(reply)).await;
                    }
                });
            }
        });
        (format!("ws://{address}"), accepted)
    }

    fn stub_config(endpoint: String) -> doubao::DoubaoConfig {
        doubao::DoubaoConfig {
            api_key: "test-key".into(),
            resource_id: "volc.seedasr.sauc.duration".into(),
            endpoint,
            end_window_size: 800,
            hotwords: Vec::new(),
        }
    }

    /// 跑一场流式会话：推 0.5 秒音频后松开，返回会话结果与中间结果里见过的文字。
    async fn run_streaming_session(
        config: doubao::DoubaoConfig,
        warm: Option<WarmReceiver>,
    ) -> (SessionOutcome, Vec<String>) {
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let on_text: StreamingCallback = Arc::new(move |partial: StreamingPartial| {
            sink.lock().unwrap().push(partial.text);
        });
        audio_tx.send(vec![0i16; 8000]).unwrap();
        drop(audio_tx);
        let session =
            streaming_session(7, StreamingConfig::Doubao(config), "zh".into(), warm, audio_rx, cancel_rx, on_text, tx);
        tokio::time::timeout(Duration::from_secs(10), session).await.expect("会话应在超时前结束");
        let mut outcome = None;
        while let Ok(cmd) = rx.try_recv() {
            if let Cmd::Done(7, done) = cmd {
                outcome = Some(done);
            }
        }
        let texts = seen.lock().unwrap().clone();
        (outcome.expect("会话应回报结果"), texts)
    }

    /// 起一个保活任务并认领它握好手的连接。
    fn claim_warm(config: &doubao::DoubaoConfig) -> (WarmReceiver, tokio::task::JoinHandle<Option<Duration>>) {
        let (claim_tx, claim_rx) = oneshot::channel();
        let keeper = tokio::spawn(keep_warm(config.clone(), claim_rx));
        let (reply_tx, reply_rx) = oneshot::channel();
        assert!(claim_tx.send(reply_tx).is_ok());
        (reply_rx, keeper)
    }

    #[tokio::test]
    async fn 流式会话_复用预热连接_不再握手() {
        let (endpoint, accepted) = stub_doubao(vec![StubConn::Alive]).await;
        let config = stub_config(endpoint);
        let (warm, keeper) = claim_warm(&config);
        let (outcome, texts) = run_streaming_session(config, Some(warm)).await;
        assert_eq!(outcome.text, "你好世界");
        assert!(outcome.error.is_none() && !outcome.cancelled);
        assert_eq!(outcome.samples.len(), 8000);
        assert!(texts.contains(&"你好世界".to_string()));
        assert_eq!(accepted.load(Ordering::SeqCst), 1, "整场只握了预热那一次手");
        assert_eq!(keeper.await.unwrap(), None, "连接已交出，保活任务不回报");
    }

    #[tokio::test]
    async fn 流式会话_预热连接是死连接_回执超时后新建并重发() {
        // 第一条（预热）握手后再无回应；第二条（回退新建）正常
        let (endpoint, accepted) = stub_doubao(vec![StubConn::Dead, StubConn::Alive]).await;
        let config = stub_config(endpoint);
        let (warm, _keeper) = claim_warm(&config);
        let started = Instant::now();
        let (outcome, _) = run_streaming_session(config, Some(warm)).await;
        assert_eq!(outcome.text, "你好世界", "音频重发到新连接后照常出字");
        assert!(outcome.error.is_none());
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() >= WARM_ACK_TIMEOUT, "等满回执超时才回退");
        assert!(started.elapsed() < WARM_ACK_TIMEOUT + Duration::from_secs(3), "回退后不该再拖");
    }

    #[tokio::test]
    async fn 流式会话_未收末端包时保留正文并记录降级原因() {
        let (endpoint, _) = stub_doubao(vec![StubConn::NoFinal]).await;
        let (outcome, _) = run_streaming_session(stub_config(endpoint), None).await;
        assert_eq!(outcome.text, "你好世界");
        assert!(outcome.error.as_deref().unwrap().contains("识别可能不完整"));
        assert!(!outcome.cancelled);
    }

    #[tokio::test]
    async fn 流式会话_预热握手失败_直接新建() {
        let (endpoint, accepted) = stub_doubao(vec![StubConn::Alive]).await;
        let config = stub_config(endpoint);
        // 预热任务连的是一个没人监听的端口：握手失败，按下的人拿到失败后马上新建
        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_endpoint = format!("ws://{}", unused.local_addr().unwrap());
        drop(unused);
        let (warm, keeper) = claim_warm(&stub_config(dead_endpoint));
        let (outcome, _) = run_streaming_session(config, Some(warm)).await;
        assert_eq!(outcome.text, "你好世界");
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        assert_eq!(keeper.await.unwrap(), Some(Duration::ZERO), "建连失败回报存活 0");
    }

    #[tokio::test]
    async fn 预热保活_槽被丢弃就关连接_服务端断开回报存活时长() {
        use futures_util::StreamExt;

        // 槽被丢弃（设置变了）：保活任务关掉连接、不回报
        let (endpoint, accepted) = stub_doubao(vec![StubConn::Alive]).await;
        let (claim_tx, claim_rx) = oneshot::channel::<WarmSender>();
        let keeper = tokio::spawn(keep_warm(stub_config(endpoint), claim_rx));
        tokio::time::timeout(Duration::from_secs(2), async {
            while accepted.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("预热应握上手");
        drop(claim_tx);
        assert_eq!(tokio::time::timeout(Duration::from_secs(3), keeper).await.unwrap().unwrap(), None);

        // 服务端握手后马上断开：回报存活时长（很短，Actor 据此退避）
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.close(None).await;
            while ws.next().await.is_some() {}
        });
        let (_claim_tx, claim_rx) = oneshot::channel::<WarmSender>();
        let lived = tokio::time::timeout(Duration::from_secs(3), keep_warm(stub_config(endpoint), claim_rx))
            .await
            .expect("断开后保活任务应结束")
            .expect("断开要回报");
        assert!(lived < WARM_HEALTHY);
    }

    #[test]
    fn 预热连续失败_重试间隔翻倍并封顶() {
        assert_eq!(warm_backoff(1), WARM_TICK);
        assert_eq!(warm_backoff(2), WARM_TICK * 2);
        assert_eq!(warm_backoff(3), WARM_TICK * 4);
        assert_eq!(warm_backoff(10), WARM_RETRY_MAX);
        assert_eq!(warm_backoff(u32::MAX), WARM_RETRY_MAX);
        // 回执超时要盖得住正常回执（约 0.35 秒），又不能比重新握手还久
        assert!(WARM_ACK_TIMEOUT > Duration::from_millis(700) && WARM_ACK_TIMEOUT < WARM_CONNECT_WAIT);
    }
    #[test]
    fn 按住录音_按下开始_松开收尾() {
        assert_eq!(record_hotkey_step(Record, Pressed, Phase::Idle, false), RecordHotkeyStep::ResumeOrStart);
        assert_eq!(
            record_hotkey_step(Record, Pressed, Phase::Recording, true),
            RecordHotkeyStep::ResumeOrStart,
            "缓冲里重新按下 / 自动重复"
        );
        assert_eq!(record_hotkey_step(Record, Released, Phase::Recording, true), RecordHotkeyStep::Release);
        assert_eq!(record_hotkey_step(Record, Released, Phase::Connecting, true), RecordHotkeyStep::Release);
    }

    #[test]
    fn 点按录音_按一下开始_再按一下结束_松开不管() {
        assert_eq!(
            record_hotkey_step(RecordToggle, Pressed, Phase::Idle, false),
            RecordHotkeyStep::Start { held: false }
        );
        assert_eq!(record_hotkey_step(RecordToggle, Released, Phase::Recording, false), RecordHotkeyStep::Ignore);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Recording, false), RecordHotkeyStep::Stop);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Connecting, false), RecordHotkeyStep::Stop);
        assert_eq!(record_hotkey_step(RecordToggle, Pressed, Phase::Finalizing, false), RecordHotkeyStep::Nudge);
        assert_eq!(
            record_hotkey_step(RecordToggle, Pressed, Phase::Optimizing, false),
            RecordHotkeyStep::Start { held: false }
        );
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
    fn 录音取消绑定_只在非_idle_相位启用() {
        let held = ModSet::new(false, false, true, true);
        assert_eq!(cancel_binding_modifiers(Phase::Idle, held), None);
        assert_eq!(cancel_binding_modifiers(Phase::Connecting, held), Some(held));
        assert_eq!(cancel_binding_modifiers(Phase::Recording, held), Some(held));
        assert_eq!(
            cancel_binding_modifiers(Phase::Finalizing, held),
            Some(ModSet::default()),
            "收尾阶段已松开录音快捷键，单按 Esc 就要能取消"
        );
        assert_eq!(
            cancel_binding_modifiers(Phase::Optimizing, held),
            Some(ModSet::default()),
            "优化阶段不再按着录音快捷键，Esc 保持无修饰键"
        );
    }

    #[test]
    fn 新录音启动失败_保留正在进行的优化() {
        assert_eq!(start_failure_phase(true), Phase::Optimizing);
        assert_eq!(start_failure_phase(false), Phase::Idle);
    }

    #[test]
    fn 录音与优化会话号独立自增_连续选区优化不会撞号() {
        let mut recording = 0;
        let mut optimization = 0;

        assert_eq!(next_session_id(&mut recording), 1);
        assert_eq!(next_session_id(&mut optimization), 1, "两类会话各自从 1 开始");
        assert_eq!(next_session_id(&mut recording), 2);
        assert_eq!(next_session_id(&mut optimization), 2, "优化选中文本不推进录音会话号");
        assert_eq!(next_session_id(&mut optimization), 3);
        assert_eq!(next_session_id(&mut optimization), 4);
    }

    #[test]
    fn 看门狗归档保留全部已采集音频() {
        let samples = Mutex::new(Vec::new());
        append_captured_samples(&samples, &[1, 2, 3]);
        append_captured_samples(&samples, &[4, 5]);
        assert_eq!(snapshot_captured_samples(&samples), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn 识别为空只有超过三秒才保存失败记录() {
        assert!(!should_record_empty_transcript(MIN_DURATION_SECONDS));
        assert!(!should_record_empty_transcript(2.9));
        assert!(!should_record_empty_transcript(3.0));
        assert!(should_record_empty_transcript(10.0));
    }

    #[test]
    fn 语音改写录音时_esc_取消绑定跟随语音改写热键() {
        let mut settings = att_core::settings::UserSettings::with_platform(att_core::Platform::Macos);
        settings.hotkeys.iter_mut().find(|b| b.action == VoiceRewriteSelection).unwrap().enabled = true;
        settings.hotkeys.iter_mut().find(|b| b.action == VoiceRewriteSelection).unwrap().modifiers =
            ModSet::new(true, true, false, false);
        assert_eq!(
            start_cancel_modifiers(&settings, true),
            ModSet::new(true, true, false, false),
            "语音改写应使用自己的修饰键，不能沿用按住录音"
        );
        assert_eq!(
            start_cancel_modifiers(&settings, false),
            settings.binding_of(Record).unwrap().modifiers,
            "普通录音仍跟随按住录音"
        );
    }

    #[test]
    fn 录音状态_asr使用中判定覆盖采集与收尾() {
        for phase in ["connecting", "recording", "finalizing"] {
            assert!(recording_state_uses_asr(&json!({ "phase": phase })), "{phase}");
        }
        for phase in ["idle", "optimizing", "completed", "cancelled", "error"] {
            assert!(!recording_state_uses_asr(&json!({ "phase": phase })), "{phase}");
        }
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
    fn 语音改写_松开缓冲内重新按下时继续本段() {
        let released = Instant::now() - Duration::from_millis(100);
        assert!(voice_rewrite_can_resume(Phase::Recording, true, true, Some(released)));
        assert!(
            !voice_rewrite_can_resume(
                Phase::Recording,
                true,
                true,
                Some(Instant::now() - HOLD_RELEASE_GRACE - Duration::from_millis(1))
            ),
            "缓冲已经过期时不能继续，应交给到点收尾"
        );
        assert!(!voice_rewrite_can_resume(Phase::Recording, true, false, Some(released)));
        assert!(!voice_rewrite_can_resume(Phase::Finalizing, true, true, Some(released)));
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
    fn 语音改写_没说话时明确说原文未动_记录是要求加原文() {
        assert_eq!(quiet_end_message("没有检测到说话", true), "没有听到改写要求，选中文字未改动");
        assert_eq!(quiet_end_message("录音时间过短", true), "按键时间过短，选中文字未改动");
        assert_eq!(quiet_end_message("没有检测到说话", false), "没有检测到说话");
        assert_eq!(
            compose_voice_rewrite_raw_text("  改成更正式的语气\n", "这个方案\n大概可以。"),
            "改写要求：改成更正式的语气\n\n原文：\n这个方案\n大概可以。"
        );
    }

    #[test]
    fn 看门狗强制收尾_取消与安静结束不算识别超时() {
        assert!(counts_as_timeout(None, false));
        assert!(!counts_as_timeout(None, true));
        assert!(!counts_as_timeout(Some("没有检测到说话"), false));
    }

    #[test]
    fn 流式编排_豆包与_windows_微信输入法接受_批量引擎明确拒绝() {
        let doubao = stt::Engine::Doubao(doubao::DoubaoConfig {
            api_key: "test".into(),
            resource_id: "volc.bigasr.sauc.duration".into(),
            endpoint: "wss://example.invalid".into(),
            end_window_size: 800,
            hotwords: Vec::new(),
        });
        assert!(matches!(streaming_config_for(&doubao, Mode::Streaming), Ok(Some(StreamingConfig::Doubao(_)))));
        assert!(matches!(streaming_config_for(&doubao, Mode::Batch), Ok(None)));

        let chatgpt = stt::Engine::ChatGpt(stt::chatgpt::ChatGptConfig {
            api_key: "test".into(),
            endpoint: "https://example.invalid".into(),
            timeout: Duration::from_secs(30),
        });
        let error = streaming_config_for(&chatgpt, Mode::Streaming).unwrap_err();
        assert!(error.contains("暂不支持流式"), "{error}");

        #[cfg(windows)]
        {
            let wetype = stt::Engine::WeType(stt::wetype::WetypeConfig { pid: None, timeout: Duration::from_secs(60) });
            assert!(matches!(streaming_config_for(&wetype, Mode::Streaming), Ok(Some(StreamingConfig::WeType(_)))));
        }
    }

    #[test]
    fn 微信输入法流式中断只留记录_不自动输出半截() {
        let error = Some("微信输入法未确认识别结束".to_string());
        assert!(withhold_partial_stream_output(Mode::Streaming, "wetype_asr", &error));
        assert!(!withhold_partial_stream_output(Mode::Streaming, "wetype_asr", &None));
        assert!(!withhold_partial_stream_output(Mode::Streaming, "doubao_asr", &error));
        assert!(!withhold_partial_stream_output(Mode::Batch, "wetype_asr", &error));
    }

    #[test]
    fn 收尾看门狗上限_覆盖各引擎自身超时() {
        let timeout = Duration::from_secs(120);
        // 流式：建连 10 + 末包 6 + 关闭 2 + 余量 15
        assert_eq!(
            finalize_budget(Mode::Streaming, Duration::from_secs(30), timeout, BatchPlan::Single),
            Duration::from_secs(33)
        );
        // 多段：分段总等待 120 + 余量
        assert_eq!(
            finalize_budget(Mode::Multi, Duration::from_secs(30), timeout, BatchPlan::Single),
            Duration::from_secs(135)
        );
        // 单段：ChatGPT 的超时设置与豆包按时长加时取大
        assert_eq!(
            finalize_budget(Mode::Batch, Duration::from_secs(30), timeout, BatchPlan::Single),
            Duration::from_secs(135)
        );
        assert_eq!(
            finalize_budget(Mode::Batch, Duration::from_secs(600), timeout, BatchPlan::Single),
            Duration::from_secs(333)
        );
        // 微信输入法流式：按设置的单次请求超时（最低 30 秒）+ 关闭开销 + 余量
        assert_eq!(
            finalize_budget_for_provider(
                Mode::Streaming,
                "wetype_asr",
                Duration::from_secs(30),
                Duration::from_secs(90),
                BatchPlan::Single
            ),
            Duration::from_secs(107)
        );
    }

    #[test]
    fn 收尾看门狗_deepseek长录音按串行块数放大预算() {
        let attempts = att_core::stt::deepseek::MAX_ATTEMPTS_PER_CHUNK as u64;
        // 600 / 1800 / 7200 秒分别切成 6 / 16 / 63 块，最坏每块尝试 3 次、每次等满 120 秒
        for (seconds, chunks) in [(600u64, 6usize), (1800, 16), (7200, 63)] {
            let recorded = Duration::from_secs(seconds);
            let (timeout, plan) = batch_plan("deepseek_asr", recorded, 120, 30, 120, 60);
            assert_eq!(timeout, Duration::from_secs(120), "{seconds} 秒");
            assert_eq!(plan, BatchPlan::DeepSeek { chunks }, "{seconds} 秒");
            let budget = finalize_budget(Mode::Batch, recorded, timeout, plan);
            assert_eq!(budget, Duration::from_secs(chunks as u64 * 120 * attempts) + WATCHDOG_SLACK);
            assert!(budget >= timeout * (chunks as u32 * attempts as u32) + WATCHDOG_SLACK);
        }
        // 旧公式（max(单块超时, 豆包开销 + recorded/2)）600 秒只有 333 秒，挡不住 6 块串行
        assert_eq!(
            finalize_budget(Mode::Batch, Duration::from_secs(600), Duration::from_secs(120), BatchPlan::Single),
            Duration::from_secs(333)
        );
    }

    #[test]
    fn 收尾看门狗_deepseek短录音按尝试次数放大_其他引擎预算不变() {
        let attempts = att_core::stt::deepseek::MAX_ATTEMPTS_PER_CHUNK as u64;
        let (timeout, plan) = batch_plan("deepseek_asr", Duration::from_secs(30), 120, 30, 120, 60);
        assert_eq!(timeout, Duration::from_secs(120));
        assert_eq!(plan, BatchPlan::DeepSeek { chunks: 1 });
        assert_eq!(
            finalize_budget(Mode::Batch, Duration::from_secs(30), timeout, plan),
            Duration::from_secs(120 * attempts) + WATCHDOG_SLACK
        );

        // 单块超时按 deepseek_config 的实际范围（10～600 秒）取，预算不能跟着越界设置无限放大
        let (timeout, plan) = batch_plan("deepseek_asr", Duration::from_secs(30), 900, 30, 120, 60);
        assert_eq!(timeout, Duration::from_secs(600));
        assert_eq!(
            finalize_budget(Mode::Batch, Duration::from_secs(30), timeout, plan),
            Duration::from_secs(600 * attempts) + WATCHDOG_SLACK
        );

        let (timeout, plan) = batch_plan("wetype_asr", Duration::from_secs(30), 120, 30, 120, 90);
        assert_eq!(timeout, Duration::from_secs(90));
        assert_eq!(plan, BatchPlan::Single);
    }

    #[test]
    fn 收尾看门狗_豆包网页按实时推流与游客会话数放大预算() {
        let (timeout, plan) = batch_plan("doubao_web_asr", Duration::from_secs(600), 120, 30, 120, 60);
        assert_eq!(timeout, Duration::from_secs(30));
        assert_eq!(plan, BatchPlan::DoubaoWeb { sessions: 1 });
        let budget = finalize_budget(Mode::Batch, Duration::from_secs(600), timeout, plan);
        assert_eq!(budget, Duration::from_secs(797), "600 秒音频按实时推流 + 单会话控制开销");
        assert!(budget > Duration::from_secs(600));

        // 超过 13 分钟会再启动新的游客会话；预算必须覆盖每段各自的初始化、握手和四个控制等待。
        let (timeout, plan) = batch_plan("doubao_web_asr", Duration::from_secs(1800), 120, 30, 120, 60);
        assert_eq!(plan, BatchPlan::DoubaoWeb { sessions: 3 });
        assert_eq!(finalize_budget(Mode::Batch, Duration::from_secs(1800), timeout, plan), Duration::from_secs(2361));
    }

    #[test]
    fn 多段预算_豆包网页覆盖实时推流与四个控制等待() {
        let config = att_core::stt::doubao_web::DoubaoWebConfig::from_seconds(30);
        let engine = att_core::stt::Engine::DoubaoWeb(config.clone());
        let policy = MultiDrainPolicy::for_engine(&engine, MULTI_DRAIN_DEFAULT_FALLBACK);
        let expected = att_core::stt::doubao_web::estimated_timeout(
            &config,
            Duration::from_secs_f64(att_core::audio::MAX_SEGMENT_SECONDS),
        ) + WATCHDOG_SLACK;
        assert_eq!(policy.segment_timeout, expected);
        assert!(policy.segment_timeout > Duration::from_secs_f64(att_core::audio::MAX_SEGMENT_SECONDS));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn 多段识别_二十段并发不超过二_结果仍按原始段序拼接() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        let semaphore = Arc::new(Semaphore::new(MULTI_TRANSCRIBE_CONCURRENCY));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel();
        for index in 0..20 {
            let (semaphore, result_tx) = (semaphore.clone(), result_tx.clone());
            let (active, peak) = (active.clone(), peak.clone());
            spawn_multi_segment(index, semaphore, result_tx, Duration::from_secs(2), move || {
                let (active, peak) = (active.clone(), peak.clone());
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(att_core::stt::BatchTranscript { text: format!("第 {} 段", index + 1), warning: None })
                }
            });
        }
        drop(result_tx);

        let mut texts = vec![None; 20];
        for _ in 0..20 {
            let (index, result) = tokio::time::timeout(Duration::from_secs(2), result_rx.recv())
                .await
                .expect("等待分段识别结果超时")
                .expect("分段结果通道提前关闭");
            texts[index] = Some(result);
        }

        assert_eq!(peak.load(Ordering::SeqCst), MULTI_TRANSCRIBE_CONCURRENCY, "应恰好用满并发上限，而不是退化成串行");
        assert!(peak.load(Ordering::SeqCst) <= MULTI_TRANSCRIBE_CONCURRENCY, "同时在途的分段识别任务超过上限");
        let expected = (1..=20).map(|index| format!("第 {index} 段")).collect::<Vec<_>>().join("\n\n");
        assert_eq!(join_multi_texts(&texts), expected, "分段完成顺序不能影响最终拼接顺序");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn 多段积压超过一百二十秒_动态预算仍拼接全部尾段() {
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        // 用毫秒缩放：三轮分段约 300ms 完成，旧的 120ms 兜底会丢掉后两轮；
        // 单段预算放大到 2s，给并行测试调度留足余量。
        let policy = MultiDrainPolicy { segment_timeout: Duration::from_secs(2), fallback: Duration::from_millis(120) };
        let budget = policy.budget(6);
        assert_eq!(budget, Duration::from_secs(6) + MULTI_DRAIN_SCHEDULING_SLACK, "6 段、并发 2 必须给三轮单段超时");

        let semaphore = Arc::new(Semaphore::new(MULTI_TRANSCRIBE_CONCURRENCY));
        let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for index in 0..6 {
            tasks.push(spawn_multi_segment(
                index,
                semaphore.clone(),
                result_tx.clone(),
                policy.segment_timeout,
                move || async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(att_core::stt::BatchTranscript { text: format!("第 {} 段", index + 1), warning: None })
                },
            ));
        }
        drop(result_tx);

        let deadline = tokio::time::Instant::now() + budget;
        let mut texts = vec![None; 6];
        while texts.iter().any(Option::is_none) {
            let (index, result) = tokio::time::timeout_at(deadline, result_rx.recv())
                .await
                .expect("动态预算内应完成全部积压分段")
                .expect("分段结果通道提前关闭");
            texts[index] = Some(result);
        }
        abort_multi_segments(&mut tasks).await;

        let expected = (1..=6).map(|index| format!("第 {index} 段")).collect::<Vec<_>>().join("\n\n");
        assert_eq!(join_multi_texts(&texts), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn 多段取消后_排队任务不再发起识别请求() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        let semaphore = Arc::new(Semaphore::new(MULTI_TRANSCRIBE_CONCURRENCY));
        let calls = Arc::new(AtomicUsize::new(0));
        let (result_tx, _result_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for index in 0..10 {
            let calls = calls.clone();
            tasks.push(spawn_multi_segment(
                index,
                semaphore.clone(),
                result_tx.clone(),
                Duration::from_secs(5),
                move || async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(att_core::stt::BatchTranscript { text: String::new(), warning: None })
                },
            ));
        }
        drop(result_tx);

        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(std::sync::atomic::Ordering::SeqCst) < MULTI_TRANSCRIBE_CONCURRENCY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("在途分段没有启动");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), MULTI_TRANSCRIBE_CONCURRENCY);

        // 先关闭信号量，避免 abort 释放在途 permit 时，排队任务抢到许可后短暂进入桩函数。
        semaphore.close();
        abort_multi_segments(&mut tasks).await;
        let after_abort = calls.load(std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), after_abort, "取消后排队分段仍调用了 ASR");
        assert_eq!(after_abort, MULTI_TRANSCRIBE_CONCURRENCY);
    }

    #[test]
    fn 多段兜底超时可由环境变量调整且非法值回落默认() {
        assert_eq!(parse_multi_drain_fallback(None), MULTI_DRAIN_DEFAULT_FALLBACK);
        assert_eq!(parse_multi_drain_fallback(Some("300")), Duration::from_secs(300));
        assert_eq!(parse_multi_drain_fallback(Some("0")), MULTI_DRAIN_DEFAULT_FALLBACK);
        assert_eq!(parse_multi_drain_fallback(Some("bad")), MULTI_DRAIN_DEFAULT_FALLBACK);
    }

    #[test]
    fn 多段结果拼接_跳过失败与未完成分段() {
        let segment = |text: &str| att_core::stt::BatchTranscript { text: text.into(), warning: None };
        let texts: Vec<Option<Result<att_core::stt::BatchTranscript, String>>> = vec![
            Some(Ok(segment("第一段"))),
            Some(Err("429".into())),
            None,
            Some(Ok(segment("第四段"))),
            Some(Ok(segment(""))),
        ];
        assert_eq!(join_multi_texts(&texts), "第一段\n\n第四段");
    }

    #[tokio::test]
    async fn deepseek长录音_可控延迟桩内完成且不被收尾预算提前取消() {
        use att_core::llm::testing::{stub_routes, StubRoute};
        use att_core::stt::{deepseek, Engine};

        let (base, mut requests) = stub_routes(vec![
            StubRoute::json("POST /v1/tools/asr", 200, r#"{"text":"片段"}"#).after(Duration::from_millis(30))
        ])
        .await;
        let recorded = Duration::from_secs(600);
        let (timeout, plan) = batch_plan("deepseek_asr", recorded, 120, 30, 120, 60);
        assert_eq!(plan, BatchPlan::DeepSeek { chunks: 6 });
        let budget = finalize_budget(Mode::Batch, recorded, timeout, plan);
        assert_eq!(budget, Duration::from_secs(6 * 120 * deepseek::MAX_ATTEMPTS_PER_CHUNK as u64) + WATCHDOG_SLACK);

        let config = deepseek::DeepSeekConfig {
            api_key: "sk-test-123456".into(),
            endpoint: format!("{base}/v1/tools/asr"),
            timeout,
        };
        // 600 秒 16k 单声道 → 6 块；桩每块延迟 30ms，按块串行应远早于看门狗预算完成
        let samples = vec![0i16; 600 * deepseek::SAMPLE_RATE];
        let transcript = tokio::time::timeout(budget, Engine::DeepSeek(config).transcribe_pcm("zh", &samples))
            .await
            .expect("长录音被收尾预算提前取消")
            .expect("DeepSeek 桩识别失败");
        assert_eq!(transcript.text.lines().count(), 6, "应按块顺序拼接 6 段文本");
        assert!(transcript.warning.is_none(), "全部分段成功不应有降级原因");

        // 桩依次收到 6 个请求：块与块之间是 await 串行的，不是并发
        let mut count = 0;
        while requests.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 6);
    }

    #[test]
    fn 处理耗时_从松开算起_不含录音时长() {
        // 实测：流式录了 18s，松开后收尾 0.65s——记录里应是 0.65s，而不是 18.65s
        let started = Instant::now();
        let released = started + Duration::from_secs(18);
        let done = released + Duration::from_millis(650);
        assert!((processing_seconds(Some(released), done) - 0.65).abs() < 1e-9);
        // 没经过停止（麦克风中途断开）：记 0，界面不显示
        assert_eq!(processing_seconds(None, done), 0.0);
        // 时钟不会倒退成负数
        assert_eq!(processing_seconds(Some(done), released), 0.0);
    }

    #[test]
    fn 没听到人声_不输出且悬浮窗提示检查麦克风() {
        // 没有音频（强制收尾带回的空结果）不判定；有音频但全是静音则判定为没听到
        assert!(missing_voice(&[]).is_none());
        assert!(missing_voice(&[0; 32_000]).is_some());

        let mut payload = no_voice_state("Clean.", Value::Null);
        assert_eq!(payload["text"], "", "text 留空：主窗口只弹提示，不当成一次成功的转写");
        assert_eq!(payload["raw_text"], "Clean.");
        payload["phase"] = json!("completed");
        payload["mode"] = json!("streaming");
        let (floating, show) = crate::floating::from_recording_state(&payload).expect("悬浮窗要显示提示");
        assert!(show);
        assert_eq!(floating["phase"], "complete");
        assert_eq!(floating["ok"], false, "按「敗」收起，不报「成」");
        assert_eq!(floating["text"], NO_VOICE_MESSAGE);
    }
}
