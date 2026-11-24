//! 微信输入法语音（Windows）：直接注入输入法进程，复用其原生 Opus 编码与业务网络层。
//!
//! 这条链路没有外部桥进程：
//! 1. ToolHelp 发现 `wetype_update.exe`；
//! 2. 专用线程里用 Frida Core attach，先读取已加载模块，再按 PE 可执行节特征定位编码器；
//! 3. 加载内嵌 agent，每个请求使用独立 `voice_id` / owner；
//! 4. Rust 侧组装 protobuf、按 20ms Opus 帧切包、轮询明确 end_flag。
//!
//! 清理状态不确定时进程内暂停后续调用，要求用户重启输入法；不把半截文字当成成功结果。

use super::wetype_stream::{
    build_request, opus_packets, parse_response, stream_packet, validate_stream_frames, StreamTransport, StreamUpdate,
    FRAME_BYTES, FRAME_SAMPLES,
};
#[cfg(test)]
use super::wetype_stream::{bytes_value, field_blob, field_int, int_value, parse_fields, OPUS_HEADER};
pub use super::wetype_stream::{PartialCallback, WetypeSession};
use super::WetypeStatus;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

pub const LABEL: &str = "微信输入法语音";

const PROCESS_NAME: &str = "wetype_update.exe";
const REQUIRED_MODULES: [&str; 3] = ["wetype_update.exe", "wetap_business.dll", "ime_net.dll"];
const REFERENCE_SHA256: [(&str, &str); 3] = [
    ("wetype_update.exe", "580ce85a6108b5883afe6f25502fe4f0ef221148372de782f49c6ab15e63a201"),
    ("wetap_business.dll", "4c948dec0ff9a473b98316bd9f02031c4a67e9efcd75b516edccc4dd583b2b33"),
    ("ime_net.dll", "69f25af3a87662c5e8060324cf13fda91c42ce44b6a3515c5b8c04ac5f9cbb02"),
];
const AGENT_JS: &str = include_str!("wetype_agent.js");
const FRIDA_CORE_VERSION: &str = env!("ATT_FRIDA_CORE_VERSION");
const MAX_FRAMES_PER_PACKET: usize = 250;

const SIGNATURE_CONSTRUCT: &[u8] = &hex_bytes("33c0c74110803e00004889014889410866894124884126488bc1c74114010000");
const SIGNATURE_INITIALIZE: &[u8] = &hex_bytes("48895c241848896c2420565741574883ec40488bf94533ff488b09418bd9418b");
const SIGNATURE_ENCODE: &[u8] = &hex_bytes("4053574155415641574883ec304c8bac248800000033db4d8bf94c8bf2488bf9");
const SIGNATURE_RELEASE: &[u8] = &hex_bytes("40534883ec20488bd9488b094885c9740ce8fa4b000048c70300000000488b4b");

/// 运行期配置：PID 为 0 / None 时自动发现；超时秒数作为协议截止时间的下限。
#[derive(Debug, Clone)]
pub struct WetypeConfig {
    pub pid: Option<u32>,
    pub timeout: Duration,
}

impl WetypeConfig {
    pub fn ensure(&self) -> Result<(), String> {
        let status = status(self);
        if status.available {
            Ok(())
        } else {
            Err(status.message)
        }
    }
}

/// 轻量状态：只枚举进程，不 attach，供设置页与首页轮询。
pub fn status(config: &WetypeConfig) -> WetypeStatus {
    match resolve_pid(config.pid) {
        Ok(pid) => WetypeStatus {
            available: true,
            pid: Some(pid),
            message: format!("可用（PID {pid}）"),
            platform_supported: true,
        },
        Err(message) => WetypeStatus { available: false, pid: None, message, platform_supported: true },
    }
}

/// 完整探活：attach 输入法并核对必要模块/编码器特征，不发送识别请求。
pub fn probe(config: &WetypeConfig) -> Result<(), String> {
    let pid = resolve_pid(config.pid)?;
    ensure_host(pid).map(|_| ())
}

/// 输入必须是 16k/mono/PCM16；内部补足到 20ms 帧边界后交给输入法。
pub async fn transcribe_pcm(config: &WetypeConfig, samples: &[i16]) -> Result<String, String> {
    let samples = samples.to_vec();
    let config = config.clone();
    tokio::task::spawn_blocking(move || transcribe_blocking(&config, &samples))
        .await
        .map_err(|error| format!("微信输入法语音任务失败：{error}"))?
}

fn transcribe_blocking(config: &WetypeConfig, samples: &[i16]) -> Result<String, String> {
    if samples.is_empty() {
        return Err("没有可识别的音频".into());
    }
    let duration_secs = samples.len() as f64 / 16_000.0;
    let deadline = protocol_deadline(config.timeout, duration_secs);
    let pid = resolve_pid(config.pid)?;
    let host = ensure_host(pid)?;
    let (reply_tx, reply_rx) = mpsc::channel();
    if host.send(HostCommand::Recognize { samples: samples.to_vec(), deadline, reply: reply_tx }).is_err() {
        invalidate_host(pid);
        return Err("微信输入法连接已断开，请重试".into());
    }
    let wait = deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(20);
    match reply_rx.recv_timeout(wait) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(error)) => {
            invalidate_host(pid);
            Err(error)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            invalidate_host(pid);
            Err("微信输入法未在时限内完成识别。".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            invalidate_host(pid);
            Err("微信输入法连接意外断开，请退出输入法后重新启动。".into())
        }
    }
}

fn protocol_deadline(timeout: Duration, duration_secs: f64) -> Instant {
    let protocol = Duration::from_secs_f64((duration_secs * 1.5 + 15.0).max(30.0));
    Instant::now() + timeout.max(protocol)
}

fn resolve_pid(configured: Option<u32>) -> Result<u32, String> {
    let candidates = enumerate_wetype_pids()?;
    match configured.filter(|pid| *pid > 0) {
        Some(pid) if candidates.contains(&pid) => Ok(pid),
        Some(pid) => Err(format!("设置中的微信输入法进程 PID {pid} 不存在，请先启动微信输入法或把 PID 设为自动")),
        None if candidates.len() == 1 => Ok(candidates[0]),
        None if candidates.is_empty() => Err("未检测到微信输入法，请先启动微信输入法".into()),
        None => Err("多个微信输入法进程时，请在「设置 → 识别引擎」里指定 PID".into()),
    }
}

fn enumerate_wetype_pids() -> Result<Vec<u32>, String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!("枚举微信输入法进程失败：{}", std::io::Error::last_os_error()));
    }
    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut pids = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        let end = entry.szExeFile.iter().position(|ch| *ch == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if name.eq_ignore_ascii_case(PROCESS_NAME) {
            pids.push(entry.th32ProcessID);
        }
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(pids)
}

enum HostCommand {
    Recognize { samples: Vec<i16>, deadline: Instant, reply: Sender<Result<String, String>> },
    StreamStart { voice_id: String, deadline: Instant, reply: Sender<Result<(), String>> },
    StreamChunk { voice_id: String, samples: Vec<i16>, deadline: Instant, reply: Sender<Result<StreamUpdate, String>> },
    StreamEnd { voice_id: String, deadline: Instant, reply: Sender<Result<StreamUpdate, String>> },
    StreamPoll { voice_id: String, deadline: Instant, reply: Sender<Result<StreamUpdate, String>> },
    StreamCancel { voice_id: String, reply: Sender<Result<(), String>> },
}

static HOSTS: OnceLock<Mutex<HashMap<u32, Sender<HostCommand>>>> = OnceLock::new();
static PAUSED: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

fn hosts() -> &'static Mutex<HashMap<u32, Sender<HostCommand>>> {
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn paused() -> &'static Mutex<Vec<u32>> {
    PAUSED.get_or_init(|| Mutex::new(Vec::new()))
}

fn ensure_host(pid: u32) -> Result<Sender<HostCommand>, String> {
    if paused().lock().unwrap_or_else(|error| error.into_inner()).contains(&pid) {
        return Err("微信输入法原生回调尚未清理，已暂停后续调用；请退出并重新启动微信输入法后重试".into());
    }
    let mut hosts = hosts().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(sender) = hosts.get(&pid) {
        return Ok(sender.clone());
    }
    let (ready_tx, ready_rx) = mpsc::channel();
    let (command_tx, command_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("att-wetype-{pid}"))
        .spawn(move || host_main(pid, ready_tx, command_rx))
        .map_err(|error| format!("启动微信输入法注入线程失败：{error}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(())) => {
            hosts.insert(pid, command_tx.clone());
            Ok(command_tx)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err("连接微信输入法超时，请确认输入法正在运行".into()),
    }
}

fn invalidate_host(pid: u32) {
    hosts().lock().unwrap_or_else(|error| error.into_inner()).remove(&pid);
}

fn pause_pid(pid: u32) {
    let mut paused = paused().lock().unwrap_or_else(|error| error.into_inner());
    if !paused.contains(&pid) {
        paused.push(pid);
    }
}

fn host_main(pid: u32, ready: Sender<Result<(), String>>, commands: mpsc::Receiver<HostCommand>) {
    let mut session = match AttachedSession::new(pid) {
        Ok(session) => session,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if let Err(error) = ready.send(Ok(())) {
        tracing::warn!("微信输入法 host 启动应答未送达：{error}");
        return;
    }

    while let Ok(command) = commands.recv() {
        let pause = match command {
            HostCommand::Recognize { samples, deadline, reply } => match session.recognize(&samples, deadline) {
                Ok(text) => {
                    let _ = reply.send(Ok(text));
                    false
                }
                Err(error) => {
                    let pause = error.starts_with("CLEANUP_PAUSE:");
                    let _ = reply.send(Err(error_without_pause(error)));
                    pause
                }
            },
            HostCommand::StreamStart { voice_id, deadline, reply } => match session.start_stream(&voice_id, deadline) {
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    false
                }
                Err(error) => {
                    let pause = error.starts_with("CLEANUP_PAUSE:");
                    let _ = reply.send(Err(error_without_pause(error)));
                    pause
                }
            },
            HostCommand::StreamChunk { voice_id, samples, deadline, reply } => {
                match session.stream_chunk(&voice_id, &samples, deadline) {
                    Ok(update) => {
                        let _ = reply.send(Ok(update));
                        false
                    }
                    Err(error) => {
                        let pause = error.starts_with("CLEANUP_PAUSE:");
                        let _ = reply.send(Err(error_without_pause(error)));
                        pause
                    }
                }
            }
            HostCommand::StreamEnd { voice_id, deadline, reply } => match session.stream_end(&voice_id, deadline) {
                Ok(update) => {
                    let _ = reply.send(Ok(update));
                    false
                }
                Err(error) => {
                    let pause = error.starts_with("CLEANUP_PAUSE:");
                    let _ = reply.send(Err(error_without_pause(error)));
                    pause
                }
            },
            HostCommand::StreamPoll { voice_id, deadline, reply } => match session.stream_poll(&voice_id, deadline) {
                Ok(update) => {
                    let _ = reply.send(Ok(update));
                    false
                }
                Err(error) => {
                    let pause = error.starts_with("CLEANUP_PAUSE:");
                    let _ = reply.send(Err(error_without_pause(error)));
                    pause
                }
            },
            HostCommand::StreamCancel { voice_id, reply } => match session.cancel_stream(&voice_id) {
                Ok(()) => {
                    let _ = reply.send(Ok(()));
                    false
                }
                Err(error) => {
                    let pause = error.starts_with("CLEANUP_PAUSE:");
                    let _ = reply.send(Err(error_without_pause(error)));
                    pause
                }
            },
        };
        if pause {
            pause_pid(pid);
            break;
        }
    }
    session.close();
}

fn error_without_pause(error: String) -> String {
    error.trim_start_matches("CLEANUP_PAUSE:").to_string()
}

struct HostTransport {
    commands: Sender<HostCommand>,
}

impl HostTransport {
    fn wait<T>(receiver: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
        match receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err("微信输入法注入线程响应超时".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("微信输入法注入线程已退出".into()),
        }
    }
}

impl StreamTransport for HostTransport {
    fn start(&self, voice_id: &str, deadline: Instant) -> Result<(), String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(HostCommand::StreamStart { voice_id: voice_id.to_string(), deadline, reply })
            .map_err(|_| "微信输入法注入线程已退出".to_string())?;
        Self::wait(receiver)
    }

    fn chunk(&self, voice_id: &str, samples: &[i16], deadline: Instant) -> Result<StreamUpdate, String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(HostCommand::StreamChunk {
                voice_id: voice_id.to_string(),
                samples: samples.to_vec(),
                deadline,
                reply,
            })
            .map_err(|_| "微信输入法注入线程已退出".to_string())?;
        Self::wait(receiver)
    }

    fn end(&self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(HostCommand::StreamEnd { voice_id: voice_id.to_string(), deadline, reply })
            .map_err(|_| "微信输入法注入线程已退出".to_string())?;
        Self::wait(receiver)
    }

    fn poll(&self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(HostCommand::StreamPoll { voice_id: voice_id.to_string(), deadline, reply })
            .map_err(|_| "微信输入法注入线程已退出".to_string())?;
        Self::wait(receiver)
    }

    fn cancel(&self, voice_id: &str) -> Result<(), String> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(HostCommand::StreamCancel { voice_id: voice_id.to_string(), reply })
            .map_err(|_| "微信输入法注入线程已退出".to_string())?;
        Self::wait(receiver)
    }
}

impl WetypeSession {
    /// 连接输入法并初始化本场流式会话；`on_partial` 收到的始终是累计文本。
    pub async fn connect(
        config: &WetypeConfig,
        _language: &str,
        on_partial: Option<PartialCallback>,
    ) -> Result<Self, String> {
        let pid = resolve_pid(config.pid)?;
        let commands = ensure_host(pid)?;
        let voice_id = format!("wt-att-{}", uuid::Uuid::new_v4().simple());
        let transport = Arc::new(HostTransport { commands });
        Self::from_transport(transport, voice_id, config.timeout, on_partial).await
    }
}

// ---------------- Frida Core 最小 FFI ----------------

#[repr(C)]
struct GError {
    domain: c_uint,
    code: c_int,
    message: *mut c_char,
}

#[repr(C)]
struct GBytes {
    _private: [u8; 0],
}

#[link(name = "frida-core", kind = "static")]
extern "C" {
    fn frida_init();
    fn frida_device_manager_new() -> *mut c_void;
    fn frida_device_manager_get_device_by_type_sync(
        manager: *mut c_void,
        kind: c_int,
        timeout: c_int,
        cancellable: *mut c_void,
        error: *mut *mut GError,
    ) -> *mut c_void;
    fn frida_device_enumerate_processes_sync(
        device: *mut c_void,
        options: *mut c_void,
        cancellable: *mut c_void,
        error: *mut *mut GError,
    ) -> *mut c_void;
    fn frida_process_list_size(list: *mut c_void) -> c_int;
    fn frida_process_list_get(list: *mut c_void, index: c_int) -> *mut c_void;
    fn frida_process_get_pid(process: *mut c_void) -> c_uint;
    fn frida_process_get_name(process: *mut c_void) -> *const c_char;
    fn frida_device_attach_sync(
        device: *mut c_void,
        pid: c_uint,
        options: *mut c_void,
        cancellable: *mut c_void,
        error: *mut *mut GError,
    ) -> *mut c_void;
    fn frida_session_create_script_sync(
        session: *mut c_void,
        source: *const c_char,
        options: *mut c_void,
        cancellable: *mut c_void,
        error: *mut *mut GError,
    ) -> *mut c_void;
    fn frida_session_is_detached(session: *mut c_void) -> c_int;
    fn frida_script_load_sync(script: *mut c_void, cancellable: *mut c_void, error: *mut *mut GError);
    fn frida_script_unload_sync(script: *mut c_void, cancellable: *mut c_void, error: *mut *mut GError);
    fn frida_script_post(script: *mut c_void, json: *const c_char, data: *mut GBytes);
    fn frida_session_detach_sync(session: *mut c_void, cancellable: *mut c_void, error: *mut *mut GError);
    fn frida_unref(object: *mut c_void);
    fn g_error_free(error: *mut GError);
    fn g_signal_connect_data(
        instance: *mut c_void,
        signal: *const c_char,
        handler: Option<unsafe extern "C" fn()>,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        flags: c_int,
    ) -> usize;
}

static FRIDA_INIT: Once = Once::new();

fn frida_init_once() {
    FRIDA_INIT.call_once(|| unsafe { frida_init() });
}

fn take_error(error: *mut GError) -> String {
    if error.is_null() {
        return String::new();
    }
    let message = unsafe { CStr::from_ptr((*error).message) }.to_string_lossy().into_owned();
    unsafe {
        g_error_free(error);
    }
    message
}

fn cstring(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "参数里包含 NUL 字符".to_string())
}

struct RpcRouter {
    next_id: std::sync::atomic::AtomicU64,
    pending: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
}

impl RpcRouter {
    fn new() -> Self {
        Self { next_id: std::sync::atomic::AtomicU64::new(0), pending: Mutex::new(HashMap::new()) }
    }
}

unsafe extern "C" fn on_script_message(
    _script: *mut c_void,
    message: *const c_char,
    _data: *mut GBytes,
    user_data: *mut c_void,
) {
    if message.is_null() || user_data.is_null() {
        return;
    }
    let text = match CStr::from_ptr(message).to_str() {
        Ok(text) => text,
        Err(_) => return,
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else { return };
    if value.get("type").and_then(Value::as_str) != Some("send") {
        return;
    }
    let Some(payload) = value.get("payload") else { return };
    let (id, result, returns) = if let Some(items) = payload.as_array() {
        if items.first().and_then(Value::as_str) != Some("frida:rpc") {
            return;
        }
        (
            items.get(1).and_then(Value::as_u64),
            items.get(2).and_then(Value::as_str),
            items.get(3).cloned().unwrap_or(Value::Null),
        )
    } else if let Some(items) = payload.as_object() {
        if items.get("type").and_then(Value::as_str) != Some("frida:rpc") {
            return;
        }
        (
            items.get("id").and_then(Value::as_u64),
            items.get("result").and_then(Value::as_str),
            items.get("returns").cloned().unwrap_or(Value::Null),
        )
    } else {
        return;
    };
    let Some(id) = id else { return };
    let router = &*(user_data as *const RpcRouter);
    let sender = router.pending.lock().unwrap_or_else(|error| error.into_inner()).remove(&id);
    if let Some(sender) = sender {
        let response = if result == Some("ok") { Ok(returns) } else { Err(returns.to_string()) };
        let _ = sender.send(response);
    }
}

fn rpc_call(
    script: *mut c_void,
    router: *mut RpcRouter,
    name: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let router_ref = unsafe { &*router };
    let id = router_ref.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let (sender, receiver) = mpsc::channel();
    router_ref.pending.lock().unwrap_or_else(|error| error.into_inner()).insert(id, sender);
    let request = json!(["frida:rpc", id, "call", name, args]);
    let request = cstring(&request.to_string())?;
    unsafe {
        frida_script_post(script, request.as_ptr(), std::ptr::null_mut());
    }
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            router_ref.pending.lock().unwrap_or_else(|error| error.into_inner()).remove(&id);
            Err(format!("agent RPC {name} 超时"))
        }
    }
}

#[derive(Debug)]
struct StreamState {
    voice_id: String,
    sequence: u64,
    total_bytes: usize,
    encoder_open: bool,
}

struct AttachedSession {
    pid: u32,
    manager: *mut c_void,
    device: *mut c_void,
    session: *mut c_void,
    script: *mut c_void,
    router: *mut RpcRouter,
    keep_agent: bool,
    stream: Option<StreamState>,
}

impl AttachedSession {
    fn new(pid: u32) -> Result<Self, String> {
        frida_init_once();
        let manager = unsafe { frida_device_manager_new() };
        if manager.is_null() {
            return Err("初始化 Frida 失败".into());
        }
        let mut error = std::ptr::null_mut();
        let device =
            unsafe { frida_device_manager_get_device_by_type_sync(manager, 0, 5000, std::ptr::null_mut(), &mut error) };
        if device.is_null() {
            unsafe { frida_unref(manager) };
            return Err(format!("获取本机 Frida 设备失败：{}", take_error(error)));
        }
        let process_name = enumerate_process_name(device, pid)?;
        if !process_name.eq_ignore_ascii_case(PROCESS_NAME) {
            unsafe {
                frida_unref(device);
                frida_unref(manager);
            }
            return Err(format!("PID {pid} 不是微信输入法进程（{process_name}）"));
        }
        let mut error = std::ptr::null_mut();
        let session =
            unsafe { frida_device_attach_sync(device, pid, std::ptr::null_mut(), std::ptr::null_mut(), &mut error) };
        if session.is_null() {
            unsafe {
                frida_unref(device);
                frida_unref(manager);
            }
            return Err(format!("连接微信输入法失败：{}", take_error(error)));
        }

        let addresses = match discover_encoder_addresses(session) {
            Ok(addresses) => addresses,
            Err(error) => {
                unsafe {
                    frida_session_detach_sync(session, std::ptr::null_mut(), std::ptr::null_mut());
                    frida_unref(session);
                    frida_unref(device);
                    frida_unref(manager);
                }
                return Err(error);
            }
        };
        let source =
            AGENT_JS.replace("__WETYPE_ENCODERS__", &serde_json::to_string(&addresses).unwrap_or_else(|_| "{}".into()));
        let source = cstring(&source)?;
        let mut error = std::ptr::null_mut();
        let script = unsafe {
            frida_session_create_script_sync(
                session,
                source.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut error,
            )
        };
        if script.is_null() {
            unsafe {
                frida_session_detach_sync(session, std::ptr::null_mut(), std::ptr::null_mut());
                frida_unref(session);
                frida_unref(device);
                frida_unref(manager);
            }
            return Err(format!("创建微信输入法 agent 失败：{}", take_error(error)));
        }
        let router = Box::into_raw(Box::new(RpcRouter::new()));
        let signal = cstring("message")?;
        unsafe {
            g_signal_connect_data(
                script,
                signal.as_ptr(),
                Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(on_script_message as *const ())),
                router as *mut c_void,
                None,
                0,
            );
        }
        let mut error = std::ptr::null_mut();
        unsafe {
            frida_script_load_sync(script, std::ptr::null_mut(), &mut error);
        }
        if !error.is_null() {
            let message = take_error(error);
            unsafe {
                frida_unref(script);
                frida_session_detach_sync(session, std::ptr::null_mut(), std::ptr::null_mut());
                frida_unref(session);
                frida_unref(device);
                frida_unref(manager);
            }
            return Err(format!("加载微信输入法 agent 失败：{message}"));
        }
        tracing::info!("微信输入法 agent 已连接（PID {pid}，Frida {FRIDA_CORE_VERSION}）");
        Ok(Self { pid, manager, device, session, script, router, keep_agent: false, stream: None })
    }

    fn recognize(&mut self, samples: &[i16], deadline: Instant) -> Result<String, String> {
        if self.stream.is_some() {
            return Err("微信输入法正在流式识别，请先结束当前录音".into());
        }
        self.ensure_attached()?;
        let mut pcm = pcm_bytes(samples);
        let padded = pcm.len().div_ceil(FRAME_BYTES) * FRAME_BYTES;
        pcm.resize(padded, 0);
        let expected_frames = pcm.len() / FRAME_BYTES;
        let encoded = rpc_call(self.script, self.router, "encode", json!([pcm]), Duration::from_secs(120))?;
        let encoded = json_byte_array(&encoded, "agent 返回的 Opus 流")?;
        let packets = opus_packets(&encoded, MAX_FRAMES_PER_PACKET)?;
        if packets.frames != expected_frames {
            return Err(format!("Opus 帧数与输入音频不一致：期望 {expected_frames}，实际 {}", packets.frames));
        }

        let voice_id = format!("wt-att-{}", uuid::Uuid::new_v4().simple());
        let outcome = self.submit_and_poll(&voice_id, &packets.packets, deadline);
        let cleanup = rpc_call(self.script, self.router, "finalize", json!([voice_id]), Duration::from_secs(10));
        match cleanup {
            Ok(value) if value.get("cleaned").and_then(Value::as_bool) == Some(true) => outcome,
            Ok(value) => {
                self.keep_agent = true;
                Err(format!(
                    "CLEANUP_PAUSE:微信输入法原生回调清理未完成（{}），已暂停后续调用；请退出并重新启动微信输入法后重试",
                    value
                ))
            }
            Err(error) => {
                self.keep_agent = true;
                Err(format!(
                    "CLEANUP_PAUSE:微信输入法原生回调清理失败（{error}），已暂停后续调用；请退出并重新启动微信输入法后重试"
                ))
            }
        }
    }

    fn submit_and_poll(&mut self, voice_id: &str, packets: &[&[u8]], deadline: Instant) -> Result<String, String> {
        let mut total_bytes = 0usize;
        for (index, packet) in packets.iter().enumerate() {
            ensure_before(deadline, "输入法未在时限内完成识别")?;
            total_bytes += packet.len();
            let request = build_request(voice_id, packet, index as u64 + 1, total_bytes, false);
            let reply = self.submit(voice_id, &request)?;
            parse_response(&reply, voice_id)?;
        }
        ensure_before(deadline, "输入法未在时限内完成识别")?;
        let end_sequence = packets.len() as u64 + 1;
        let request = build_request(voice_id, &[], end_sequence, total_bytes, true);
        let reply = self.submit(voice_id, &request)?;
        let mut current = parse_response(&reply, voice_id)?;
        loop {
            if current.end_flag != 0 {
                return Ok(current.text);
            }
            ensure_before(deadline, "输入法未确认识别结束，未交付可能不完整的文字")?;
            std::thread::sleep(Duration::from_millis(current.interval_ms.clamp(100, 1000)));
            let request = build_request(voice_id, &[], 0, total_bytes, true);
            let reply = self.submit(voice_id, &request)?;
            current = parse_response(&reply, voice_id)?;
        }
    }

    fn ensure_attached(&mut self) -> Result<(), String> {
        if unsafe { frida_session_is_detached(self.session) } != 0 {
            self.keep_agent = true;
            return Err("CLEANUP_PAUSE:微信输入法连接意外断开，请退出并重新启动微信输入法后重试".into());
        }
        Ok(())
    }

    fn start_stream(&mut self, voice_id: &str, deadline: Instant) -> Result<(), String> {
        ensure_before(deadline, "连接微信输入法流式会话超时")?;
        self.ensure_attached()?;
        if let Some(stale) = self.stream.as_ref() {
            // 建连途中用户取消时 WetypeSession 可能来不及构造，host 里会留下上一场会话。
            // 下一次开始前先清掉它，避免后续录音被「已有正在进行的流式会话」永久挡住。
            let stale_voice = stale.voice_id.clone();
            tracing::warn!(%stale_voice, "清理遗留的微信输入法流式会话");
            self.cleanup_or_pause(&stale_voice)?;
        }
        rpc_call(self.script, self.router, "encoderInit", json!([]), Duration::from_secs(20))
            .map_err(|error| format!("初始化微信输入法常驻编码器失败：{error}"))?;
        self.stream =
            Some(StreamState { voice_id: voice_id.to_string(), sequence: 0, total_bytes: 0, encoder_open: true });
        Ok(())
    }

    fn stream_chunk(&mut self, voice_id: &str, samples: &[i16], deadline: Instant) -> Result<StreamUpdate, String> {
        match self.stream_chunk_inner(voice_id, samples, deadline) {
            Ok(update) => Ok(update),
            Err(error) => Err(self.stream_failure(voice_id, error)),
        }
    }

    fn stream_chunk_inner(
        &mut self,
        voice_id: &str,
        samples: &[i16],
        deadline: Instant,
    ) -> Result<StreamUpdate, String> {
        ensure_before(deadline, "微信输入法流式请求超时")?;
        self.ensure_attached()?;
        if samples.is_empty() || samples.len() % FRAME_SAMPLES != 0 {
            return Err("流式音频没有按 20ms 帧对齐".into());
        }
        let (sequence, total_bytes) = match self.stream.as_ref() {
            Some(state) if state.voice_id == voice_id => (state.sequence, state.total_bytes),
            Some(_) => return Err("流式会话标识不匹配".into()),
            None => return Err("微信输入法流式会话尚未建立".into()),
        };
        let pcm = pcm_bytes(samples);
        let encoded = rpc_call(self.script, self.router, "encoderEncode", json!([pcm]), Duration::from_secs(30))
            .map_err(|error| format!("微信输入法流式编码失败：{error}"))?;
        let frames = json_byte_array(&encoded, "agent 返回的 Opus 帧")?;
        validate_stream_frames(&frames, samples.len() / FRAME_SAMPLES)?;
        let packet = stream_packet(&frames, sequence == 0);
        let next_total = total_bytes + packet.len();
        let request = build_request(voice_id, &packet, sequence + 1, next_total, false);
        let reply = self.submit(voice_id, &request)?;
        let response = parse_response(&reply, voice_id)?;
        if let Some(state) = self.stream.as_mut() {
            state.sequence = sequence + 1;
            state.total_bytes = next_total;
        }
        Ok(response.into())
    }

    fn stream_end(&mut self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        match self.stream_end_inner(voice_id, deadline) {
            Ok(update) => Ok(update),
            Err(error) => Err(self.stream_failure(voice_id, error)),
        }
    }

    fn stream_end_inner(&mut self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        ensure_before(deadline, "微信输入法流式收尾超时")?;
        self.ensure_attached()?;
        self.close_encoder()?;
        let (sequence, total_bytes) = match self.stream.as_ref() {
            Some(state) if state.voice_id == voice_id => (state.sequence, state.total_bytes),
            Some(_) => return Err("流式会话标识不匹配".into()),
            None => return Err("微信输入法流式会话尚未建立".into()),
        };
        let request = build_request(voice_id, &[], sequence + 1, total_bytes, true);
        let reply = self.submit(voice_id, &request)?;
        let response = parse_response(&reply, voice_id)?;
        if response.end_flag != 0 {
            self.cleanup_or_pause(voice_id)?;
        }
        Ok(response.into())
    }

    fn stream_poll(&mut self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        match self.stream_poll_inner(voice_id, deadline) {
            Ok(update) => Ok(update),
            Err(error) => Err(self.stream_failure(voice_id, error)),
        }
    }

    fn stream_poll_inner(&mut self, voice_id: &str, deadline: Instant) -> Result<StreamUpdate, String> {
        ensure_before(deadline, "微信输入法未在时限内确认识别结束")?;
        self.ensure_attached()?;
        let total_bytes = match self.stream.as_ref() {
            Some(state) if state.voice_id == voice_id => state.total_bytes,
            Some(_) => return Err("流式会话标识不匹配".into()),
            None => return Err("微信输入法流式会话尚未建立".into()),
        };
        let request = build_request(voice_id, &[], 0, total_bytes, true);
        let reply = self.submit(voice_id, &request)?;
        let response = parse_response(&reply, voice_id)?;
        if response.end_flag != 0 {
            self.cleanup_or_pause(voice_id)?;
        }
        Ok(response.into())
    }

    fn cancel_stream(&mut self, voice_id: &str) -> Result<(), String> {
        match self.cleanup_stream(voice_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.keep_agent = true;
                Err(format!("CLEANUP_PAUSE:微信输入法流式会话清理失败（{error}），已暂停后续调用；请退出并重新启动微信输入法后重试"))
            }
        }
    }

    fn stream_failure(&mut self, voice_id: &str, error: String) -> String {
        if error.starts_with("CLEANUP_PAUSE:") {
            self.keep_agent = true;
            return error;
        }
        match self.cleanup_stream(voice_id) {
            Ok(()) => error,
            Err(cleanup) => {
                self.keep_agent = true;
                format!("CLEANUP_PAUSE:{error}；会话清理也失败（{cleanup}），已暂停后续调用；请退出并重新启动微信输入法后重试")
            }
        }
    }

    fn close_encoder(&mut self) -> Result<(), String> {
        let Some(state) = self.stream.as_ref() else { return Ok(()) };
        if !state.encoder_open {
            return Ok(());
        }
        rpc_call(self.script, self.router, "encoderClose", json!([]), Duration::from_secs(10))
            .map_err(|error| format!("关闭微信输入法常驻编码器失败：{error}"))?;
        if let Some(state) = self.stream.as_mut() {
            state.encoder_open = false;
        }
        Ok(())
    }

    fn cleanup_stream(&mut self, voice_id: &str) -> Result<(), String> {
        let Some(stream) = self.stream.as_ref() else { return Ok(()) };
        if stream.voice_id != voice_id {
            return Err("流式会话标识不匹配".into());
        }
        let mut errors = Vec::new();
        match rpc_call(self.script, self.router, "finalize", json!([voice_id]), Duration::from_secs(10)) {
            Ok(value) if value.get("cleaned").and_then(Value::as_bool) == Some(true) => {}
            Ok(value) => errors.push(format!("原生回调尚未清理（{value}）")),
            Err(error) => errors.push(format!("原生回调清理失败：{error}")),
        }
        if stream.encoder_open {
            if let Err(error) = rpc_call(self.script, self.router, "encoderClose", json!([]), Duration::from_secs(10)) {
                errors.push(format!("常驻编码器关闭失败：{error}"));
            }
        }
        self.stream = None;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    fn cleanup_or_pause(&mut self, voice_id: &str) -> Result<(), String> {
        match self.cleanup_stream(voice_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.keep_agent = true;
                Err(format!("CLEANUP_PAUSE:微信输入法流式会话清理失败（{error}），已暂停后续调用；请退出并重新启动微信输入法后重试"))
            }
        }
    }

    fn submit(&mut self, owner: &str, request: &[u8]) -> Result<Vec<u8>, String> {
        let reply = rpc_call(self.script, self.router, "request", json!([request, owner]), Duration::from_secs(20))?;
        let code = reply.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            let task = reply.get("taskId").map(Value::to_string).unwrap_or_else(|| "unknown".into());
            return Err(format!("输入法语音网络请求失败：原生错误码 {code}，任务 ID {task}"));
        }
        let data = reply.get("data").ok_or("输入法响应缺少 data")?;
        json_byte_array(data, "输入法响应")
    }

    fn close(&mut self) {
        if self.keep_agent {
            tracing::warn!(
                "微信输入法原生回调清理状态不确定，保留 agent 与 Frida session（PID {}），等待输入法进程退出",
                self.pid
            );
            // 有意不 unref / unload：原生回调仍可能引用脚本与 user_data。
            self.script = std::ptr::null_mut();
            self.session = std::ptr::null_mut();
            self.device = std::ptr::null_mut();
            self.manager = std::ptr::null_mut();
            return;
        }
        if self.script.is_null() {
            return;
        }
        unsafe {
            let mut error = std::ptr::null_mut();
            frida_script_unload_sync(self.script, std::ptr::null_mut(), &mut error);
            if !error.is_null() {
                tracing::warn!("卸载微信输入法 agent 失败：{}", take_error(error));
            }
            frida_unref(self.script);
            self.script = std::ptr::null_mut();
            if self.session.is_null() {
                return;
            }
            if frida_session_is_detached(self.session) == 0 {
                frida_session_detach_sync(self.session, std::ptr::null_mut(), std::ptr::null_mut());
            }
            frida_unref(self.session);
            self.session = std::ptr::null_mut();
            frida_unref(self.device);
            self.device = std::ptr::null_mut();
            frida_unref(self.manager);
            self.manager = std::ptr::null_mut();
        }
        tracing::info!("微信输入法 agent 已断开（PID {}）", self.pid);
    }
}

impl Drop for AttachedSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn enumerate_process_name(device: *mut c_void, pid: u32) -> Result<String, String> {
    let mut error = std::ptr::null_mut();
    let list = unsafe {
        frida_device_enumerate_processes_sync(device, std::ptr::null_mut(), std::ptr::null_mut(), &mut error)
    };
    if list.is_null() {
        return Err(format!("枚举进程失败：{}", take_error(error)));
    }
    let count = unsafe { frida_process_list_size(list) };
    let mut name = None;
    for index in 0..count {
        let process = unsafe { frida_process_list_get(list, index) };
        if process.is_null() {
            continue;
        }
        if unsafe { frida_process_get_pid(process) } == pid {
            let raw = unsafe { frida_process_get_name(process) };
            if !raw.is_null() {
                name = Some(unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned());
            }
        }
        unsafe { frida_unref(process) };
    }
    unsafe { frida_unref(list) };
    name.ok_or_else(|| format!("找不到 PID {pid} 的进程"))
}

fn discover_encoder_addresses(session: *mut c_void) -> Result<EncoderAddresses, String> {
    let source =
        cstring("rpc.exports.modules = () => Process.enumerateModules().map(m => ({name:m.name,path:m.path}));")?;
    let mut error = std::ptr::null_mut();
    let script = unsafe {
        frida_session_create_script_sync(
            session,
            source.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut error,
        )
    };
    if script.is_null() {
        return Err(format!("读取微信输入法模块失败：{}", take_error(error)));
    }
    let router = Box::into_raw(Box::new(RpcRouter::new()));
    let signal = cstring("message")?;
    unsafe {
        g_signal_connect_data(
            script,
            signal.as_ptr(),
            Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(on_script_message as *const ())),
            router as *mut c_void,
            None,
            0,
        );
    }
    let mut error = std::ptr::null_mut();
    unsafe {
        frida_script_load_sync(script, std::ptr::null_mut(), &mut error);
    }
    if !error.is_null() {
        let message = take_error(error);
        unsafe { frida_unref(script) };
        return Err(format!("加载微信输入法模块探针失败：{message}"));
    }
    let modules = rpc_call(script, router, "modules", json!([]), Duration::from_secs(10));
    unsafe {
        let mut error = std::ptr::null_mut();
        frida_script_unload_sync(script, std::ptr::null_mut(), &mut error);
        if !error.is_null() {
            let _ = take_error(error);
        }
        frida_unref(script);
    }
    let modules = modules?;
    let paths = verify_modules(&modules)?;
    let exe = paths.get("wetype_update.exe").ok_or("输入法模块清单缺少 wetype_update.exe")?;
    let bytes = std::fs::read(exe).map_err(|error| format!("读取 {} 失败：{error}", exe.display()))?;
    for (name, expected) in REFERENCE_SHA256 {
        let Some(path) = paths.get(name) else { continue };
        match std::fs::read(path) {
            Ok(bytes) => {
                let actual = hex_sha256(&bytes);
                if actual != expected {
                    tracing::warn!(
                        module = name,
                        path = %path.display(),
                        actual,
                        "微信输入法模块与参考版本不同，继续按特征扫描编码器"
                    );
                }
            }
            Err(error) => tracing::warn!(module = name, path = %path.display(), %error, "读取模块以校验版本失败"),
        }
    }
    resolve_encoder_addresses(&bytes)
}

fn verify_modules(modules: &Value) -> Result<HashMap<String, PathBuf>, String> {
    let items = modules.as_array().ok_or("agent 返回的模块清单不是数组")?;
    let mut paths = HashMap::new();
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else { continue };
        let Some(path) = item.get("path").and_then(Value::as_str) else { continue };
        paths.insert(name.to_ascii_lowercase(), PathBuf::from(path));
    }
    let missing: Vec<&str> =
        REQUIRED_MODULES.iter().copied().filter(|name| !paths.contains_key(&name.to_ascii_lowercase())).collect();
    if !missing.is_empty() {
        return Err(format!("输入法未加载必要的语音网络组件：{}", missing.join(", ")));
    }
    Ok(REQUIRED_MODULES
        .iter()
        .filter_map(|name| paths.get(&name.to_ascii_lowercase()).cloned().map(|path| (name.to_string(), path)))
        .collect())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------- PE 特征扫描 ----------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct EncoderAddresses {
    construct: u32,
    initialize: u32,
    encode: u32,
    release: u32,
}

fn resolve_encoder_addresses(data: &[u8]) -> Result<EncoderAddresses, String> {
    if data.len() < 0x40 || &data[..2] != b"MZ" {
        return Err("输入法可执行文件不是有效 PE".into());
    }
    let pe = read_u32(data, 0x3c).ok_or("PE 头偏移无效")? as usize;
    if read_u32(data, pe) != Some(u32::from_le_bytes(*b"PE\0\0")) {
        return Err("PE 签名无效".into());
    }
    if read_u16(data, pe + 4) != Some(0x8664) {
        return Err("输入法不是 x64 PE".into());
    }
    let sections = read_u16(data, pe + 6).ok_or("PE 节数量无效")? as usize;
    let optional_size = read_u16(data, pe + 20).ok_or("PE 可选头大小无效")? as usize;
    let table = pe.checked_add(24 + optional_size).ok_or("PE 节表偏移溢出")?;
    let mut executable = Vec::new();
    for index in 0..sections {
        let offset = table.checked_add(index * 40).ok_or("PE 节表偏移溢出")?;
        let rva = read_u32(data, offset + 12).ok_or("PE 节 RVA 无效")?;
        let size = read_u32(data, offset + 16).ok_or("PE 节大小无效")? as usize;
        let raw = read_u32(data, offset + 20).ok_or("PE 节文件偏移无效")? as usize;
        let flags = read_u32(data, offset + 36).ok_or("PE 节属性无效")?;
        if flags & 0x2000_0000 == 0 {
            continue;
        }
        let end = raw.checked_add(size).ok_or("PE 节越界")?;
        if end > data.len() {
            return Err("PE 可执行节被截断".into());
        }
        executable.push((rva, &data[raw..end]));
    }
    if executable.is_empty() {
        return Err("PE 没有可执行节".into());
    }
    let find = |signature: &[u8], label: &str| -> Result<u32, String> {
        let mut found = Vec::new();
        for (rva, section) in &executable {
            let mut offset = 0usize;
            while offset + signature.len() <= section.len() {
                if section[offset..].starts_with(signature) {
                    found.push(rva.saturating_add(offset as u32));
                }
                offset += 1;
            }
        }
        if found.len() != 1 {
            return Err(format!("编码器 {label} 特征缺失或不唯一（匹配 {} 处）", found.len()));
        }
        Ok(found[0])
    };
    Ok(EncoderAddresses {
        construct: find(SIGNATURE_CONSTRUCT, "construct")?,
        initialize: find(SIGNATURE_INITIALIZE, "initialize")?,
        encode: find(SIGNATURE_ENCODE, "encode")?,
        release: find(SIGNATURE_RELEASE, "release")?,
    })
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?))
}

// ---------------- 协议与 Opus 分包 ----------------

fn json_byte_array(value: &Value, label: &str) -> Result<Vec<u8>, String> {
    let items = value.as_array().ok_or_else(|| format!("{label}不是字节数组"))?;
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .filter(|byte| *byte <= 255)
                .map(|byte| byte as u8)
                .ok_or_else(|| format!("{label}包含非法字节"))
        })
        .collect()
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

fn ensure_before(deadline: Instant, message: &str) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err(message.into())
    } else {
        Ok(())
    }
}

const fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

const fn hex_bytes(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    let mut output = [0u8; 32];
    let mut index = 0;
    while index < output.len() {
        output[index] = (hex_nibble(bytes[index * 2]) << 4) | hex_nibble(bytes[index * 2 + 1]);
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_response(voice_id: &str, text: &str, end: u64) -> Vec<u8> {
        let mut base = field_int(1, 0);
        let inner = [
            field_blob(1, &base),
            field_blob(2, voice_id.as_bytes()),
            field_int(3, 1),
            field_blob(4, text.as_bytes()),
            field_int(5, end),
            field_int(6, 100),
        ]
        .concat();
        base.clear();
        field_blob(1, &inner)
    }

    fn test_pe_with_payload(payload: &[u8]) -> Vec<u8> {
        let pe = 0x80usize;
        let section_table = pe + 24 + 0xf0;
        let raw = 0x200usize;
        let mut data = vec![0u8; raw + payload.len()];
        data[..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&(pe as u32).to_le_bytes());
        data[pe..pe + 4].copy_from_slice(b"PE\0\0");
        data[pe + 4..pe + 6].copy_from_slice(&0x8664u16.to_le_bytes());
        data[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
        data[pe + 20..pe + 22].copy_from_slice(&0xf0u16.to_le_bytes());
        data[section_table + 12..section_table + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[section_table + 16..section_table + 20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        data[section_table + 20..section_table + 24].copy_from_slice(&(raw as u32).to_le_bytes());
        data[section_table + 36..section_table + 40].copy_from_slice(&0x2000_0040u32.to_le_bytes());
        data[raw..].copy_from_slice(payload);
        data
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
        assert_eq!(parsed.sequence, 1);
        assert_eq!(parsed.end_flag, 1);
        assert_eq!(parsed.interval_ms, 100);
        assert!(parse_response(&response, "voice-2").unwrap_err().contains("会话标识不匹配"));
    }

    #[test]
    fn 响应业务错误不交付文字() {
        let mut base = field_int(1, 7);
        let inner = [field_blob(1, &base), field_blob(2, b"voice-1"), field_blob(4, "半截".as_bytes())].concat();
        base.clear();
        let response = field_blob(1, &inner);
        assert!(parse_response(&response, "voice-1").unwrap_err().contains("业务返回错误 7"));
    }

    #[test]
    fn opus_按帧边界切包且只在首包带流头() {
        let mut encoded = OPUS_HEADER.to_vec();
        for payload in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            encoded.extend((payload.len() as u16).to_le_bytes());
            encoded.extend_from_slice(payload);
        }
        let packets = opus_packets(&encoded, 2).unwrap();
        assert_eq!(packets.frames, 3);
        assert_eq!(packets.packets.len(), 2);
        assert!(packets.packets[0].starts_with(OPUS_HEADER));
        assert!(!packets.packets[1].starts_with(OPUS_HEADER));
        assert_eq!(packets.packets.concat().len(), encoded.len());
    }

    #[test]
    fn opus_拒绝截断帧() {
        let mut encoded = OPUS_HEADER.to_vec();
        encoded.extend([10, 0, 1, 2]);
        assert!(opus_packets(&encoded, 250).unwrap_err().contains("帧不完整"));
    }

    #[test]
    fn 输入法未运行或_pid_无效时给出可执行提示() {
        let status = status(&WetypeConfig { pid: Some(u32::MAX), timeout: Duration::from_secs(60) });
        assert!(!status.available);
        assert!(status.message.contains("请先启动微信输入法"), "{}", status.message);
    }

    #[test]
    fn pe_特征扫描返回实际地址() {
        let payload = [SIGNATURE_CONSTRUCT, SIGNATURE_INITIALIZE, SIGNATURE_ENCODE, SIGNATURE_RELEASE].concat();
        let pe = test_pe_with_payload(&payload);
        let addresses = resolve_encoder_addresses(&pe).unwrap();
        assert_eq!(addresses.construct, 0x1000);
        assert_eq!(addresses.initialize, 0x1020);
        assert_eq!(addresses.encode, 0x1040);
        assert_eq!(addresses.release, 0x1060);
    }

    #[test]
    fn pe_特征缺失或多义都报错() {
        let missing = [SIGNATURE_CONSTRUCT, SIGNATURE_INITIALIZE, SIGNATURE_ENCODE].concat();
        let error = resolve_encoder_addresses(&test_pe_with_payload(&missing)).unwrap_err();
        assert!(error.contains("release"));

        let duplicate = [SIGNATURE_CONSTRUCT, SIGNATURE_CONSTRUCT].concat();
        let error = resolve_encoder_addresses(&test_pe_with_payload(&duplicate)).unwrap_err();
        assert!(error.contains("不唯一"));
    }

    #[test]
    #[ignore = "需要本机运行微信输入法，并提供 ATT_TEST_WAV"]
    fn 真机_微信输入法识别指定音频() {
        let path = std::env::var("ATT_TEST_WAV").expect("请设置 ATT_TEST_WAV 指向 16k/mono 音频");
        let samples = crate::audio::read_audio_16k_mono(std::path::Path::new(&path)).expect("读取测试音频失败");
        let config = WetypeConfig { pid: None, timeout: Duration::from_secs(60) };
        let text = transcribe_blocking(&config, &samples).expect("微信输入法识别失败");
        eprintln!("微信输入法识别结果：{text}");
        assert!(!text.trim().is_empty(), "识别结果为空");
    }

    #[test]
    #[ignore = "需要本机运行微信输入法，并提供 ATT_TEST_WAV"]
    fn 真机_微信输入法流式输出累计中间字幕() {
        let path = std::env::var("ATT_TEST_WAV").expect("请设置 ATT_TEST_WAV 指向 16k/mono 音频");
        let samples = crate::audio::read_audio_16k_mono(std::path::Path::new(&path)).expect("读取测试音频失败");
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let partial_seen = seen.clone();
        let callback: PartialCallback = Arc::new(move |text| {
            let mut seen = partial_seen.lock().unwrap_or_else(|error| error.into_inner());
            if seen.last() != Some(&text) {
                seen.push(text);
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let final_text = runtime
            .block_on(async move {
                let config = WetypeConfig { pid: None, timeout: Duration::from_secs(60) };
                let mut session = WetypeSession::connect(&config, "auto", Some(callback)).await?;
                for chunk in samples.chunks(super::super::wetype_stream::STREAM_CHUNK_SAMPLES) {
                    // 按真实录音节奏每 1 秒送一包；一次性灌完会让服务端退回批量式出字，测不到中间字幕
                    let sent_at = Instant::now();
                    session.push(chunk).await?;
                    tokio::time::sleep(Duration::from_secs(1).saturating_sub(sent_at.elapsed())).await;
                }
                session.finish(Duration::from_secs(60)).await
            })
            .expect("微信输入法流式识别失败");
        let seen = seen.lock().unwrap_or_else(|error| error.into_inner());
        eprintln!("微信输入法流式结果：{final_text}；中间文本 {} 次", seen.len());
        assert!(!final_text.trim().is_empty(), "最终识别结果为空");
        assert!(seen.len() >= 10, "中间字幕不足 10 次：{seen:?}");
    }
}
