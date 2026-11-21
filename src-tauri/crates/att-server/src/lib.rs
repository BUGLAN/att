//! 内嵌数据面 API（axum，仅绑定 127.0.0.1，优先固定端口，占用时依次 +1）。
//!
//! 鉴权：每次启动生成随机 token；除 `/api/health` 外所有端点要求 `X-ATT-Token`
//! （`<audio>` / `<img>` / 下载等带不了请求头的 GET 用 `?token=`）。来源按协议 + 主机名
//! 精确匹配本地页面与 Tauri 自定义来源，Host 只允许 `127.0.0.1` / `localhost`（挡住 DNS rebinding）；
//! `/api/health` 虽不要求令牌，也必须通过回环 Host 校验。
//!
//! 端点一览：
//! - GET /api/health（前端 runtimeStatus：model_service_* 与 streaming_*）
//! - GET/PUT /api/settings（白名单/脱敏契约保真）
//! - /api/records 全套（裸数组响应、乐观锁 409、音频播放、导出、重新识别）；
//!   搜索支持状态与本地日期筛选，/api/records/export 批量导出（Markdown / TXT / JSON / CSV）
//! - /api/prompts：列出（内置 + 自定义）、新建 / 修改 / 删除自定义预设、PUT /api/prompts/default 设默认
//! - /api/llm/* LLM v2 全套（服务商目录 / 档案 / 模型 / 验证，见 [`llm`]）
//! - 模型服务三端点（在线引擎凭据配齐即就绪；凭据验证确定失败时不就绪）
//! - /api/asr/credential-check：在线引擎凭据验证（见 [`asr`]）
//! - /api/update：检查更新与安装包下载（见 [`update`]）
//! - /api/storage：数据占用统计与清理孤儿音频（见 [`storage`]）
//! - 录音由 Rust 录音 Actor 直接完成（Tauri 命令），没有录音上传 / 多段会话类 HTTP 端点
//!
//! 两个 WebSocket 端点（/api/ws、/api/asr/stream）按 D4 决策**移除**：
//! 事件改由 Tauri events 下发（旧事件名重放）。

mod asr;
mod backup;
mod clipboard;
mod llm;
mod models;
mod storage;
mod transcribe;
mod update;

use att_core::records::export::{self as record_export, ExportFormat};
use att_core::records::{RecordError, RecordFilter, RecordUpdate};
use att_core::state::CoreState;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ServerState {
    pub core: Arc<CoreState>,
    /// 本次启动生成的数据面令牌：敏感端点（文件转写等）要求请求头 `X-ATT-Token` 匹配。
    pub token: String,
}

// ---------------- 路由 ----------------

pub fn router(state: Arc<ServerState>) -> Router {
    // 跨域策略：只放行「回环来源」（页面本身可能在 dev 的 8090、打包后的 tauri:// 或
    // 自定义协议下），服务端也只绑定 127.0.0.1，不引入外部暴露面。
    // 用 predicate 而非固定清单：避免端口/协议推断偏差导致整站请求被 CORS 拦掉
    // （早期实机验收曾出现所有请求失败 + 四类 toast，即此类问题）。
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            |origin: &axum::http::HeaderValue, _parts: &axum::http::request::Parts| {
                origin.to_str().map(is_allowed_origin).unwrap_or(false)
            },
        ))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    // 令牌校验在 CORS 内侧：预检由 CORS 层直接应答，不会要求 X-ATT-Token。
    // 除 /api/health 外所有端点都必须带本次启动的令牌；health 仍校验回环 Host。
    let guard_token = state.token.clone();
    let maintenance_core = state.core.clone();
    Router::new()
        .route("/", get(root))
        .route("/api/health", get(health))
        // 设置
        .route("/api/settings", get(settings_get).put(settings_put))
        // 记录
        .route("/api/records", get(records_list))
        .route("/api/records/search", get(records_search))
        // 静态段 export 优先于 {id}；GET 供下载链接用（带不了请求体），POST 与 GET 参数等价
        .route("/api/records/export", get(records_export_batch_get).post(records_export_batch_post))
        .route("/api/records/{id}", get(records_get).put(records_put).delete(records_delete))
        .route("/api/records/{id}/status", get(records_status))
        .route("/api/records/{id}/audio", get(records_audio))
        .route("/api/records/{id}/export", get(records_export))
        .route("/api/records/{id}/retranscribe", post(records_retranscribe))
        // 模型服务（骨架）
        .route("/api/model-service", get(model_service_status))
        .route("/api/model-service/start", post(model_service_start))
        .route("/api/model-service/stop", post(model_service_stop))
        // 提示词
        .route("/api/prompts", get(prompts_list).post(prompts_create))
        // 静态段 default 优先于 {id} 匹配；自定义 id 恒为 custom- 前缀，不会撞
        .route("/api/prompts/default", put(prompts_set_default))
        .route("/api/prompts/{id}", put(prompts_update).delete(prompts_delete))
        // LLM v2（服务商目录 / 档案 / 模型 / 验证）
        .merge(llm::routes())
        // 剪贴板历史
        .merge(clipboard::routes())
        // 本地文件转写
        .merge(transcribe::routes())
        // 在线识别引擎凭据验证
        .merge(asr::routes())
        // 本地模型清单 / 下载 / 删除
        .merge(models::routes())
        // 检查更新（设置页「关于」）
        .merge(update::routes())
        // 存储管理（设置页「诊断」）
        .merge(storage::routes())
        // 数据备份 / 恢复（设置页「诊断」）
        .merge(backup::routes())
        // 备份维护态下拒绝所有数据写请求；恢复成功后一直保持到进程重启。
        // 放在鉴权层内侧：未带令牌的请求仍先得到 401，不泄露更多状态。
        .layer(middleware::from_fn(move |req: Request, next: middleware::Next| {
            let core = maintenance_core.clone();
            async move {
                let path = req.uri().path();
                let backup_request = path == "/api/backup/export" || path == "/api/backup/import";
                let read_only = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
                if core.backup_maintenance_active() && !read_only && !backup_request {
                    return err(StatusCode::CONFLICT, "数据备份或恢复进行中，已暂停写入".into()).into_response();
                }
                next.run(req).await
            }
        }))
        .layer(middleware::from_fn(move |req, next| {
            let token = guard_token.clone();
            async move { data_plane_guard(token, req, next).await }
        }))
        .layer(cors)
        .with_state(state)
}

/// 常量时间比较，避免时序侧信道。
fn token_matches(expected: &str, actual: &str) -> bool {
    expected.len() == actual.len() && expected.bytes().zip(actual.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// 从查询串里取 `token=`（媒体元素带不了请求头时的兜底）。
fn token_from_query(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token" && !value.is_empty()).then_some(value)
    })
}

/// 数据面统一鉴权 / Host 校验。
///
/// - `/api/health` 不需要令牌，但仍须回环 Host；CORS 预检直接放行（浏览器预检不带令牌）；
/// - 其余请求必须带 `X-ATT-Token`；GET/HEAD（录音回放、图片、导出）也接受 `?token=`；
/// - 除预检外的请求 Host 只能是 `127.0.0.1[:端口]` / `localhost[:端口]`，挡住 DNS rebinding。
async fn data_plane_guard(token: String, req: axum::extract::Request, next: Next) -> axum::response::Response {
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }

    let host_ok = {
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .or_else(|| req.uri().authority().map(|authority| authority.as_str()))
            .unwrap_or("");
        is_allowed_host(host)
    };
    if req.uri().path() == "/api/health" {
        if !host_ok {
            return (StatusCode::FORBIDDEN, Json(json!({ "detail": "Host 不被允许" }))).into_response();
        }
        return next.run(req).await;
    }

    let header_token = req.headers().get("x-att-token").and_then(|value| value.to_str().ok());
    let query_token = (req.method() == Method::GET || req.method() == Method::HEAD)
        .then(|| req.uri().query().and_then(token_from_query))
        .flatten();
    let actual = header_token.or(query_token).unwrap_or("");
    if !token_matches(&token, actual) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "detail": "缺少或错误的 X-ATT-Token" }))).into_response();
    }

    if !host_ok {
        return (StatusCode::FORBIDDEN, Json(json!({ "detail": "Host 不被允许" }))).into_response();
    }

    next.run(req).await
}

/// 每个首选端口尝试的端口数（含首选端口本身）。
const PORT_BIND_MAX_ATTEMPTS: u16 = 10;

/// 同步绑定监听端口：按 preferred 顺序尝试固定端口（便于前端兜底探测）。
/// 每个首选端口被占用时依次 +1，最多尝试 [`PORT_BIND_MAX_ATTEMPTS`] 个；
/// 所有首选端口都不可用时才回退随机端口。
///
/// 注意：绑定与 accept 循环**分开**——绑定是同步的，accept 循环由调用方
/// 在自己确认可用的运行时里 spawn。这样不会出现「block_on 内 spawn 的任务
/// 在运行时退出后不再被驱动」的隐蔽故障（TCP 可连但无响应）。
pub fn bind_local(preferred: &[u16]) -> std::io::Result<(std::net::TcpListener, u16)> {
    for &start in preferred {
        for offset in 0..PORT_BIND_MAX_ATTEMPTS {
            let Some(port) = start.checked_add(offset) else {
                tracing::warn!("首选端口 {start} 递增超过 65535，回退随机端口");
                return bind_random_local();
            };
            match std::net::TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => return finalize_listener(listener),
                Err(error) => {
                    let has_retry = offset + 1 < PORT_BIND_MAX_ATTEMPTS;
                    match (has_retry, port.checked_add(1)) {
                        (true, Some(next)) => {
                            tracing::warn!("端口 {port} 不可用（{error}），尝试 {next}");
                        }
                        (true, None) => {
                            tracing::warn!("端口 {port} 不可用（{error}），65535 后无法继续递增，回退随机端口");
                            return bind_random_local();
                        }
                        (false, _) => {
                            tracing::warn!("端口 {port} 不可用（{error}），已到 {PORT_BIND_MAX_ATTEMPTS} 次重试上限");
                        }
                    }
                }
            }
        }
    }
    tracing::warn!("全部首选端口不可用，回退随机端口");
    bind_random_local()
}

fn bind_random_local() -> std::io::Result<(std::net::TcpListener, u16)> {
    finalize_listener(std::net::TcpListener::bind("127.0.0.1:0")?)
}

fn finalize_listener(listener: std::net::TcpListener) -> std::io::Result<(std::net::TcpListener, u16)> {
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// 在调用方的运行时里跑 accept 循环（直到 shutdown 置位）。
pub async fn serve_on(
    std_listener: std::net::TcpListener,
    state: Arc<ServerState>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let listener = match tokio::net::TcpListener::from_std(std_listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("监听器转换失败: {error}");
            return;
        }
    };
    let app = router(state);
    let mut shutdown = shutdown;
    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.wait_for(|flag| *flag).await;
        })
        .await
    {
        tracing::error!("数据面服务退出: {error}");
    }
}

/// 便捷入口（测试用）：绑定 + 在当前运行时 spawn accept 循环。
pub async fn serve(
    state: Arc<ServerState>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    preferred: &[u16],
) -> std::io::Result<u16> {
    let (listener, port) = bind_local(preferred)?;
    tokio::spawn(serve_on(listener, state, shutdown));
    Ok(port)
}

/// 同步存储调用挪到阻塞线程池执行：记录 / 剪贴板首次读取要整目录扫盘，不能占着异步工作线程。
/// 任务 panic 时返回 `None`（调用方按空结果处理，并留日志）。
pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::error!("存储任务异常退出: {error}");
            None
        }
    }
}

/// 拆出 authority 的主机与可选端口；端口为空 / 非法由调用方继续判断。
fn split_host_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if authority.is_empty() {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (!host.is_empty() && !port.is_empty()).then_some((host, Some(port))),
        None => Some((authority, None)),
    }
}

/// 允许的来源判定：协议 + 主机名精确匹配（任意端口）。
///
/// 只接受本地页面（dev 的 localhost / 127.0.0.1）与 Tauri 自定义来源；
/// 不接受 `null`，也不做 `starts_with` 前缀匹配（`http://localhost.evil.com` 必须被拒）。
fn is_allowed_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if rest.is_empty() || rest.chars().any(|c| matches!(c, '/' | '?' | '#' | '@')) {
        return false;
    }
    let Some((host, port)) = split_host_port(rest) else {
        return false;
    };
    if port.is_some_and(|port| port.parse::<u16>().is_err()) {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "tauri" => host.eq_ignore_ascii_case("localhost"),
        "http" | "https" => {
            host.eq_ignore_ascii_case("localhost")
                || host.eq_ignore_ascii_case("127.0.0.1")
                || host.eq_ignore_ascii_case("tauri.localhost")
        }
        _ => false,
    }
}

/// Host 只能是回环地址（端口可选、必须是数字），挡住 DNS rebinding。
fn is_allowed_host(host: &str) -> bool {
    let Some((name, port)) = split_host_port(host) else {
        return false;
    };
    if port.is_some_and(|port| port.parse::<u16>().is_err()) {
        return false;
    }
    name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost")
}

/// 数据面自检：用裸 TCP 发一次 `GET /api/health`，返回状态行。
/// setup 阶段调用，把「数据面是否真的在 TCP 上可达」写进日志——
/// 避免再出现「前端全挂但不知道是端口还是 CORS」的排查困境。
pub fn self_check_health(port: u16) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().map_err(|e| format!("非法地址: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| format!("连接失败: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("发送失败: {e}"))?;
    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|e| format!("读取失败: {e}"))?;
    Ok(response.lines().next().unwrap_or("(空响应)").to_string())
}

// ---------------- 通用工具 ----------------

/// 错误响应统一形状 `{detail}`。注意保持非泛型（String），
/// 否则不同调用点会实例化出不同的 opaque 返回类型导致 match 臂类型不合。
fn err(status: StatusCode, detail: String) -> impl IntoResponse {
    (status, Json(json!({ "detail": detail })))
}

fn map_record_error(error: RecordError) -> impl IntoResponse {
    match error {
        RecordError::NotFound => err(StatusCode::NOT_FOUND, "记录不存在".into()),
        RecordError::Conflict => err(StatusCode::CONFLICT, RecordError::Conflict.to_string()),
        RecordError::Io(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn parse_usize(params: &HashMap<String, String>, key: &str, default: usize, max: usize) -> usize {
    params.get(key).and_then(|v| v.parse::<usize>().ok()).map(|v| v.clamp(1, max)).unwrap_or(default)
}

// ---------------- 处理器 ----------------

async fn root() -> Json<Value> {
    Json(json!({ "name": "att-server", "status": "ok", "note": "前端由 Tauri 直接服务，本端点仅供探活" }))
}

/// 引擎运行态（在线 provider 无需「开启模型」，凭据配齐即就绪）。
struct EngineStatus {
    ready: bool,
    error: Option<String>,
    streaming_supported: bool,
    streaming_available: bool,
    streaming_backend: Option<&'static str>,
    mode: &'static str,
    /// 本地引擎：模型是否已加载常驻（在线引擎同 `ready`）
    running: bool,
    /// 本地引擎：是否正在后台加载
    loading: bool,
}

fn engine_status(core: &CoreState) -> EngineStatus {
    let settings = core.settings();
    let streaming = settings.recording_mode == "streaming";
    // 本地引擎（faster-whisper / Qwen3-ASR）：模型文件就位即可录音（首次推理时懒加载），流式不支持；
    // 加载失败过（文件在但模型/推理库起不来）则不再报就绪，直到设置变化或「开启模型」重试成功
    if let Some(kind) = att_core::stt::local_kind(&settings.asr_provider) {
        let status = core.local_models.status(core, kind);
        let streaming_error = streaming.then(|| format!("{}暂不支持流式录音，请使用单段或多段录音模式", kind.label()));
        // 这里只给一句结论（首页、侧栏、诊断页直接显示）；缺模型的获取方式、加载失败的原因
        // 在 local_engine.error_detail / load_error 里，由设置页折叠展示
        let load_error = status.load_error.as_ref().map(|_| format!("{}模型加载失败", kind.label()));
        let error = if status.available { load_error.clone() } else { status.error.clone() };
        return EngineStatus {
            ready: status.available && load_error.is_none() && !streaming,
            error: error.or(streaming_error),
            streaming_supported: false,
            streaming_available: false,
            streaming_backend: None,
            mode: "provider",
            running: status.loaded,
            loading: status.loading,
        };
    }
    // 凭据验证「确定不可用」（Token 无效、Resource ID 不符、接入点不存在）时不报就绪；
    // 没验过或只是没验成（网络等）按凭据齐备处理，不拦录音
    let invalid = att_core::stt::verify::invalid_reason(core, &settings.asr_provider);
    match att_core::stt::Engine::online_from_settings(core) {
        Ok(engine) if invalid.is_some() => EngineStatus {
            ready: false,
            error: invalid.map(|reason| format!("{}凭据验证失败：{reason}", engine.label())),
            streaming_supported: false,
            streaming_available: false,
            streaming_backend: None,
            mode: "online",
            running: false,
            loading: false,
        },
        Err(message) => EngineStatus {
            ready: false,
            error: Some(message),
            streaming_supported: false,
            streaming_available: false,
            streaming_backend: None,
            mode: "online",
            running: false,
            loading: false,
        },
        Ok(engine) if engine.supports_streaming() => EngineStatus {
            ready: true,
            error: None,
            streaming_supported: streaming,
            streaming_available: streaming,
            streaming_backend: Some("doubao_sauc"),
            mode: "online",
            running: true,
            loading: false,
        },
        // ChatGPT 听写 / DeepSeek：批量接口，流式模式下不可用（与 Python get_runtime_capabilities 一致）
        Ok(engine) => EngineStatus {
            ready: !streaming,
            error: streaming.then(|| format!("{}暂不支持流式录音，请使用单段或多段录音模式", engine.label())),
            streaming_supported: false,
            streaming_available: false,
            streaming_backend: None,
            mode: "online",
            running: !streaming,
            loading: false,
        },
    }
}

/// 本地引擎详情（设置页的「加载中 / 已加载 / 缺模型」提示用）；在线引擎为 `null`。
///
/// `error` 是一句结论；`error_detail` 是缺模型时的目录与获取方式（Tab 缩进行为命令）；
/// `load_error` 是文件在、但加载失败时的完整原因。
fn local_engine_payload(core: &CoreState) -> Value {
    let Some(kind) = att_core::stt::local_kind(&core.settings().asr_provider) else {
        return Value::Null;
    };
    let status = core.local_models.status(core, kind);
    // VAD 只看文件是否就位（只给 Whisper 用），不做加载，避免状态轮询触发推理库加载
    let vad_ready = att_core::stt::local::catalog::vad_installed(&core.models_dir);
    json!({
        "provider": kind.provider(),
        "label": kind.label(),
        "available": status.available,
        "loaded": status.loaded,
        "loading": status.loading,
        "error": status.error,
        "error_detail": status.error_detail,
        "load_error": status.load_error,
        "model_dir": status.model_dir.as_ref().map(|path| path.display().to_string()),
        "device": status.device,
        "vad_ready": vad_ready,
        "vad_loaded": core.local_models.vad_loaded(),
    })
}

/// GET /api/health：前端 runtimeStatus 的全部字段（新旧双命名同源）。
async fn health(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let settings = state.core.settings();
    let engine = engine_status(&state.core);
    Json(json!({
        "status": "ok",
        "runtime_environment": "tauri-rust",
        "asr_provider": settings.asr_provider,
        "recording_mode": settings.recording_mode,
        "online_ready": engine.ready,
        "local_engine": local_engine_payload(&state.core),
        "model_service_managed": true,
        "model_service_running": engine.running,
        "model_service_loading": engine.loading,
        "model_service_ready": engine.ready,
        "model_service_error": engine.error,
        "model_service_supported": true,
        "model_service_mode": engine.mode,
        "streaming_available": engine.streaming_available,
        "streaming_supported": engine.streaming_supported,
        "streaming_backend": engine.streaming_backend,
        "streaming_error": engine.error,
    }))
}

async fn settings_get(State(state): State<Arc<ServerState>>) -> Json<Value> {
    Json(state.core.public_settings())
}

async fn settings_put(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, impl IntoResponse> {
    match state.core.apply_settings_update(&body) {
        Ok(outcome) => {
            // 改了剪贴板历史上限：立即按新上限清理一次（不限时什么都不做）
            if body.get("clipboard_max_items").is_some() || body.get("clipboard_retention_days").is_some() {
                let core = state.core.clone();
                tokio::task::spawn_blocking(move || core.prune_clipboard());
            }
            // 改了录音音频保留天数：立即按新天数删一次过期音频（永久保留时什么都不做）
            if body.get("audio_retention_days").is_some() {
                let core = state.core.clone();
                tokio::task::spawn_blocking(move || core.prune_record_audio());
            }
            if !outcome.runtime_keys_changed.is_empty() {
                tracing::info!("运行时设置变更: {:?}", outcome.runtime_keys_changed);
                // 切到本地引擎（或改了它的模型/设备）就在后台预加载，设置页据 /api/health 的
                // local_engine.loading 显示「加载中」；不再使用的本地模型释放掉，免得常驻占内存
                let current = att_core::stt::local_kind(&state.core.settings().asr_provider);
                for kind in [att_core::stt::local::LocalKind::Whisper, att_core::stt::local::LocalKind::Qwen] {
                    if Some(kind) != current && state.core.local_models.is_loaded(kind) {
                        state.core.local_models.unload(Some(kind));
                    }
                }
                if let Some(kind) = current {
                    att_core::stt::preload_local(state.core.clone(), kind);
                }
            }
            if outcome.restart_hotkey_backend {
                // 热键监听每次按键都读取最新设置，这里不需要重新注册
                tracing::debug!("快捷键设置已更新");
            }
            Ok(Json(state.core.public_settings()))
        }
        Err(message) => Err(err(StatusCode::BAD_REQUEST, message)),
    }
}

async fn records_list(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let limit = parse_usize(&params, "limit", 200, 1000);
    let offset = params.get("offset").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let status = params.get("status").cloned();
    let with_total = params.get("with_total").map(|v| v == "true" || v == "1").unwrap_or(false);
    let (records, total) =
        blocking(move || state.core.records.list_page(status.as_deref(), limit, offset)).await.unwrap_or_default();
    if with_total {
        // with_total=true：分页加载方拿总数算页数（剪贴板列表同款契约）
        return Json(json!({
            "items": serde_json::to_value(records).unwrap_or_default(),
            "total": total,
        }));
    }
    // 裸数组响应（契约：非 {items,total} 包装）
    Json(serde_json::to_value(records).unwrap_or_default())
}

/// 记录状态的合法取值（筛选参数校验用）。
const RECORD_STATUSES: [&str; 6] = ["pending", "transcribing", "transcribed", "optimizing", "optimized", "failed"];

/// 搜索 / 批量导出共用的筛选参数：q、search_type、favorite_only、status、date_from、date_to（本地日期 YYYY-MM-DD）。
fn record_filter(params: &HashMap<String, String>) -> Result<RecordFilter, String> {
    let text = |key: &str| params.get(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let date = |key: &str| -> Result<Option<chrono::NaiveDate>, String> {
        text(key)
            .map(|value| {
                chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
                    .map_err(|_| format!("{key} 必须是 YYYY-MM-DD 格式的日期：{value}"))
            })
            .transpose()
    };
    let status = text("status");
    if let Some(status) = status.as_deref().filter(|s| !RECORD_STATUSES.contains(s)) {
        return Err(format!("status 只能是 {}：{status}", RECORD_STATUSES.join(" / ")));
    }
    Ok(RecordFilter {
        query: params.get("q").cloned().unwrap_or_default(),
        search_type: text("search_type").unwrap_or_else(|| "all".into()),
        favorite_only: params.get("favorite_only").map(|v| v == "true" || v == "1").unwrap_or(false),
        status,
        date_from: date("date_from")?,
        date_to: date("date_to")?,
    })
}

async fn records_search(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, axum::response::Response> {
    let filter =
        record_filter(&params).map_err(|message| err(StatusCode::UNPROCESSABLE_ENTITY, message).into_response())?;
    let page = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let page_size = parse_usize(&params, "page_size", 100, 200);

    let (items, total) =
        blocking(move || state.core.records.search_filtered(&filter, page, page_size)).await.unwrap_or_default();
    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Ok(Json(json!({
        "items": serde_json::to_value(items).unwrap_or_default(),
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": total_pages,
    })))
}

/// 批量导出要导出哪些记录：选中的 id，或筛选条件下的全部结果（不分页）。
enum ExportTarget {
    Ids(Vec<String>),
    Filter(RecordFilter),
}

/// 批量导出：按目标取记录 → 生成文件 → 以附件返回（Content-Disposition 带文件名）。
async fn export_records(
    state: Arc<ServerState>,
    target: ExportTarget,
    format: ExportFormat,
    use_optimized: bool,
) -> axum::response::Response {
    let records = blocking(move || match target {
        ExportTarget::Ids(ids) => state.core.records.get_many(&ids),
        ExportTarget::Filter(filter) => state.core.records.filtered(&filter),
    })
    .await
    .unwrap_or_default();
    if records.is_empty() {
        return err(StatusCode::NOT_FOUND, "没有可导出的记录".into()).into_response();
    }
    let body = record_export::render(&records, format, use_optimized);
    let filename = record_export::file_name(format, records.len(), chrono::Local::now());
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, format.content_type().parse().unwrap());
    if let Ok(disposition) = format!("attachment; filename=\"{filename}\"").parse() {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    (StatusCode::OK, headers, body).into_response()
}

fn export_format(value: Option<&str>) -> Result<ExportFormat, axum::response::Response> {
    let value = value.unwrap_or("");
    ExportFormat::parse(value).ok_or_else(|| {
        err(StatusCode::UNPROCESSABLE_ENTITY, format!("format 只能是 md / txt / json / csv：{value}")).into_response()
    })
}

/// GET /api/records/export?format=&use_optimized=&ids=a,b,c 或筛选参数（与 /records/search 相同）：
/// 下载链接带不了请求体，前端用这个（令牌走 ?token=）。
async fn records_export_batch_get(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let format = match export_format(params.get("format").map(String::as_str)) {
        Ok(format) => format,
        Err(response) => return response,
    };
    let use_optimized = params.get("use_optimized").map(|v| v == "true" || v == "1").unwrap_or(true);
    let target = match params.get("ids") {
        Some(ids) => {
            ExportTarget::Ids(ids.split(',').map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect())
        }
        None => match record_filter(&params) {
            Ok(filter) => ExportTarget::Filter(filter),
            Err(message) => return err(StatusCode::UNPROCESSABLE_ENTITY, message).into_response(),
        },
    };
    export_records(state, target, format, use_optimized).await
}

/// POST /api/records/export `{ ids?: [...], query?: {同 /records/search 的筛选条件}, format, use_optimized }`：
/// ids 与 query 二选一（query = 当前筛选的全部结果，不受分页限制）。
async fn records_export_batch_post(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let format = match export_format(body.get("format").and_then(Value::as_str)) {
        Ok(format) => format,
        Err(response) => return response,
    };
    let use_optimized = body.get("use_optimized").and_then(Value::as_bool).unwrap_or(true);
    let target = match (body.get("ids").filter(|v| !v.is_null()), body.get("query").filter(|v| !v.is_null())) {
        (Some(ids), None) => match ids.as_array() {
            Some(ids) if ids.iter().all(Value::is_string) => {
                ExportTarget::Ids(ids.iter().filter_map(Value::as_str).map(String::from).collect())
            }
            _ => return err(StatusCode::UNPROCESSABLE_ENTITY, "ids 必须是字符串数组".into()).into_response(),
        },
        (None, Some(Value::Object(query))) => {
            let params: HashMap<String, String> = query
                .iter()
                .filter_map(|(key, value)| match value {
                    Value::String(text) => Some((key.clone(), text.clone())),
                    Value::Bool(flag) => Some((key.clone(), flag.to_string())),
                    Value::Number(number) => Some((key.clone(), number.to_string())),
                    _ => None,
                })
                .collect();
            match record_filter(&params) {
                Ok(filter) => ExportTarget::Filter(filter),
                Err(message) => return err(StatusCode::UNPROCESSABLE_ENTITY, message).into_response(),
            }
        }
        _ => {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "ids 与 query 必须二选一（query 是筛选条件对象）".into())
                .into_response()
        }
    };
    export_records(state, target, format, use_optimized).await
}

async fn records_get(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, impl IntoResponse> {
    match state.core.records.get(&id) {
        Ok(record) => Ok(Json(serde_json::to_value(record).unwrap_or_default())),
        Err(e) => Err(map_record_error(e)),
    }
}

async fn records_status(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, impl IntoResponse> {
    match state.core.records.get(&id) {
        Ok(record) => Ok(Json(record.status_view())),
        Err(e) => Err(map_record_error(e)),
    }
}

async fn records_put(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, impl IntoResponse> {
    let opt_str = |key: &str| body.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let opt_bool = |key: &str| body.get(key).and_then(|v| v.as_bool());
    let update = RecordUpdate {
        raw_text: opt_str("raw_text"),
        optimized_text: opt_str("optimized_text"),
        is_modified: opt_bool("is_modified"),
        is_favorite: opt_bool("is_favorite"),
        revision: body.get("revision").and_then(|v| v.as_u64()),
    };
    match state.core.records.update(&id, update) {
        Ok(record) => Ok(Json(serde_json::to_value(record).unwrap_or_default())),
        Err(e) => Err(map_record_error(e)),
    }
}

async fn records_delete(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, impl IntoResponse> {
    match state.core.records.delete(&id) {
        Ok(()) => Ok(Json(json!({ "status": "deleted", "record_id": id }))),
        Err(e) => Err(map_record_error(e)),
    }
}

/// 音频播放：支持单段 `Range` 请求（WebView 拖动进度条时按 206 回传），其余完整回传。
async fn records_audio(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    request_headers: HeaderMap,
) -> impl IntoResponse {
    match state.core.records.audio_file(&id) {
        Ok(Some(path)) => match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let total = bytes.len() as u64;
                let mut headers = HeaderMap::new();
                headers.insert(header::CONTENT_TYPE, att_core::audio::audio_content_type(&path).parse().unwrap());
                headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
                let range = request_headers.get(header::RANGE).and_then(|value| value.to_str().ok());
                match parse_byte_range(range, total) {
                    ByteRange::Unsatisfiable => {
                        headers.insert(header::CONTENT_RANGE, format!("bytes */{total}").parse().unwrap());
                        (StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response()
                    }
                    ByteRange::Full => {
                        headers.insert(header::CONTENT_LENGTH, total.to_string().parse().unwrap());
                        (StatusCode::OK, headers, bytes).into_response()
                    }
                    ByteRange::Partial { start, end } => {
                        let body = bytes[start as usize..=end as usize].to_vec();
                        headers.insert(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}").parse().unwrap());
                        headers.insert(header::CONTENT_LENGTH, (end - start + 1).to_string().parse().unwrap());
                        (StatusCode::PARTIAL_CONTENT, headers, body).into_response()
                    }
                }
            }
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "音频不存在".into()).into_response(),
        Err(e) => map_record_error(e).into_response(),
    }
}

/// 单段 `Range` 的解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteRange {
    /// 没有 `Range` 头，或语法不认识（其他单位 / 多段 / 非法）：按完整文件返回。
    Full,
    /// 可满足的单段闭区间（`end` 已按文件长度夹取）。
    Partial { start: u64, end: u64 },
    /// 语法合法但与文件没有交集：返回 416。
    Unsatisfiable,
}

/// 解析 `Range: bytes=...`。只支持单段范围；多段、其他单位和非法语法一律忽略，
/// 由调用方按完整文件 200 返回（RFC 9110 允许服务端忽略 Range）。
fn parse_byte_range(value: Option<&str>, len: u64) -> ByteRange {
    let Some(value) = value else {
        return ByteRange::Full;
    };
    let Some((unit, spec)) = value.trim().split_once('=') else {
        return ByteRange::Full;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return ByteRange::Full;
    }
    let spec = spec.trim();
    if spec.is_empty() || spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return ByteRange::Full;
    };
    let start_text = start_text.trim();
    let end_text = end_text.trim();

    // `-N`：最后 N 个字节；N 为 0 或空文件时不可满足。
    if start_text.is_empty() {
        let Ok(suffix) = end_text.parse::<u64>() else {
            return ByteRange::Full;
        };
        if suffix == 0 || len == 0 {
            return ByteRange::Unsatisfiable;
        }
        return ByteRange::Partial { start: len.saturating_sub(suffix), end: len - 1 };
    }

    let Ok(start) = start_text.parse::<u64>() else {
        return ByteRange::Full;
    };
    if start >= len {
        return ByteRange::Unsatisfiable;
    }
    if end_text.is_empty() {
        return ByteRange::Partial { start, end: len - 1 };
    }
    let Ok(end) = end_text.parse::<u64>() else {
        return ByteRange::Full;
    };
    if end < start {
        // `bytes=5-3` 语法非法，按忽略处理。
        return ByteRange::Full;
    }
    ByteRange::Partial { start, end: end.min(len - 1) }
}

async fn records_export(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let format = params.get("format").map(String::as_str).unwrap_or("txt");
    let use_optimized = params.get("use_optimized").map(|v| v == "true" || v == "1").unwrap_or(true);
    let record = match state.core.records.get(&id) {
        Ok(record) => record,
        Err(e) => return map_record_error(e).into_response(),
    };
    let prefer_opt = use_optimized && record.optimized_text.as_deref().map(|t| !t.is_empty()).unwrap_or(false);
    let text = if prefer_opt {
        record.optimized_text.clone().unwrap_or_default()
    } else {
        record.raw_text.clone().unwrap_or_default()
    };

    let (content_type, body, filename) = match format {
        "json" => {
            let payload = json!({
                "id": record.id,
                "created_at": record.created_at,
                "duration": record.duration,
                "raw_text": record.raw_text,
                "optimized_text": record.optimized_text,
                "status": record.status.as_str(),
            });
            ("application/json", serde_json::to_string_pretty(&payload).unwrap_or_default(), format!("att-{id}.json"))
        }
        _ => ("text/plain; charset=utf-8", text, format!("att-{id}.txt")),
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    if let Ok(disposition) = format!("attachment; filename=\"{filename}\"").parse() {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    (StatusCode::OK, headers, body).into_response()
}

/// POST /api/records/{id}/retranscribe `{provider?}`：用记录的录音重新识别（不给 provider 用当前设置的引擎）。
///
/// 检查都过了返回 202 + 置为 `transcribing` 的记录，识别在后台跑，前端按记录状态轮询；
/// 记录或音频不在 404、引擎不可用（缺模型 / 未配凭据）或正在识别 409、引擎不认识 / 音频解不了码 422。
async fn records_retranscribe(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    body: Option<Json<Value>>,
) -> axum::response::Response {
    let provider =
        body.as_ref().and_then(|Json(value)| value.get("provider")).and_then(Value::as_str).map(str::to_string);
    match att_core::records::retranscribe::start(state.core.clone(), &id, provider.as_deref()).await {
        Ok(record) => (StatusCode::ACCEPTED, Json(serde_json::to_value(record).unwrap_or_default())).into_response(),
        Err(error) => {
            let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            err(status, error.message()).into_response()
        }
    }
}

// ---- 模型服务（在线引擎：凭据配齐即就绪；本地引擎：start 真正加载并预热、stop 释放） ----

fn model_service_payload(core: &CoreState) -> Value {
    let settings = core.settings();
    let engine = engine_status(core);
    let local = att_core::stt::local_kind(&settings.asr_provider).map(|kind| core.local_models.status(core, kind));
    let local_model_dir =
        local.as_ref().and_then(|status| status.model_dir.as_ref()).map(|path| path.display().to_string());
    let model_name = match &local {
        Some(status) => status
            .model_dir
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|name| Value::from(name.to_string()))
            .unwrap_or(Value::Null),
        None => match settings.asr_provider.as_str() {
            "doubao_asr" => Value::from("bigmodel"),
            "chatgpt_asr" => Value::from("chatgpt-dictation"),
            "deepseek_asr" => Value::from("deepseek-asr"),
            _ => Value::Null,
        },
    };
    json!({
        "managed": true,
        "provider_type": settings.asr_provider,
        "model_name": model_name,
        "model_dir": local_model_dir,
        "device": local.as_ref().and_then(|status| status.device.clone()),
        "recording_mode": settings.recording_mode,
        "loading": engine.loading,
        "running": engine.running,
        "ready": engine.ready,
        "error": engine.error,
        "pid": null,
        "base_url": null,
        "streaming_available": engine.streaming_available,
        "streaming_error": engine.error,
        "model_service_supported": true,
        "model_service_mode": engine.mode,
        "online_ready": engine.ready,
        "streaming_supported": engine.streaming_supported,
        "streaming_backend": engine.streaming_backend,
        "health": Value::Null,
        "local_engine": local_engine_payload(core),
        "components": { "provider": settings.asr_provider, "streaming_bridge": null },
    })
}

async fn model_service_status(State(state): State<Arc<ServerState>>) -> Json<Value> {
    Json(model_service_payload(&state.core))
}

/// 本地引擎：加载模型并预热（首次 1~5 秒）；在线引擎：就绪即回报，否则返回不可用原因。
async fn model_service_start(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    if let Some(kind) = att_core::stt::local_kind(&state.core.settings().asr_provider) {
        return match att_core::stt::warmup_local(state.core.clone(), kind).await {
            Ok(_) => (StatusCode::OK, Json(model_service_payload(&state.core))).into_response(),
            Err(detail) => err(StatusCode::SERVICE_UNAVAILABLE, detail).into_response(),
        };
    }
    let payload = model_service_payload(&state.core);
    if payload["ready"].as_bool().unwrap_or(false) {
        return (StatusCode::OK, Json(payload)).into_response();
    }
    let detail = payload["error"].as_str().unwrap_or("引擎不可用").to_string();
    err(StatusCode::SERVICE_UNAVAILABLE, detail).into_response()
}

/// 停止：本地引擎释放模型（在线引擎无本地资源，直接回报）。
async fn model_service_stop(State(state): State<Arc<ServerState>>) -> Json<Value> {
    state.core.local_models.unload(None);
    Json(json!({ "success": true, "running": false, "loading": false, "ready": false }))
}

// ---- 提示词（内置只读：prompts/*.txt；自定义：data/custom_prompts.json） ----

async fn prompts_list(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let presets = state.core.prompt_library().load_all();
    let settings = state.core.settings();
    // 设置里的默认预设已不存在（文件被删/改名）时回退内置默认，与 Python normalize 行为一致
    let default_id = settings
        .default_preset_id
        .filter(|id| presets.iter().any(|p| &p.id == id))
        .or_else(|| att_core::prompts::default_id(&presets));
    Json(json!({
        "presets": presets.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
        "default_preset_id": default_id,
        "last_used_preset_id": default_id,
    }))
}

fn preset_error(error: att_core::prompts::PresetError) -> axum::response::Response {
    use att_core::prompts::PresetError;
    let status = match &error {
        PresetError::Invalid(_) => StatusCode::BAD_REQUEST,
        PresetError::Builtin => StatusCode::FORBIDDEN,
        PresetError::NotFound => StatusCode::NOT_FOUND,
        PresetError::Corrupt(_) => StatusCode::CONFLICT,
        PresetError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(status, error.to_string()).into_response()
}

async fn prompts_create(
    State(state): State<Arc<ServerState>>,
    Json(draft): Json<att_core::prompts::PresetDraft>,
) -> axum::response::Response {
    match state.core.prompt_library().create(&draft) {
        Ok(preset) => (StatusCode::CREATED, Json(preset.to_json())).into_response(),
        Err(e) => preset_error(e),
    }
}

async fn prompts_update(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(draft): Json<att_core::prompts::PresetDraft>,
) -> axum::response::Response {
    match state.core.prompt_library().update(&id, &draft) {
        Ok(preset) => Json(preset.to_json()).into_response(),
        Err(e) => preset_error(e),
    }
}

/// 删除自定义预设；正被选用时设置会回退（见 `CoreState::delete_custom_preset`），回报回退后的值。
async fn prompts_delete(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> axum::response::Response {
    match state.core.delete_custom_preset(&id) {
        Ok(fallback) => Json(json!({
            "status": "deleted",
            "id": id,
            "default_preset_id": fallback.default_preset_id,
            "rewrite_preset_id": fallback.rewrite_preset_id,
        }))
        .into_response(),
        Err(e) => preset_error(e),
    }
}

async fn prompts_set_default(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, impl IntoResponse> {
    let Some(preset_id) = body.get("preset_id").and_then(|v| v.as_str()).map(str::to_string) else {
        return Err(err(StatusCode::BAD_REQUEST, "preset_id 不能为空".into()));
    };
    // 校验预设存在（内置或自定义）
    if state.core.prompt_library().find(&preset_id).is_none() {
        return Err(err(StatusCode::NOT_FOUND, "预设不存在".into()));
    }
    let result = state.core.update_settings(|settings| {
        settings.default_preset_id = Some(preset_id.clone());
        settings.last_used_preset_id = Some(preset_id.clone());
    });
    match result {
        Ok(()) => Ok(Json(json!({ "status": "updated", "default_preset_id": preset_id }))),
        Err(e) => {
            tracing::error!("保存默认预设失败: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "无法保存默认预设".into()))
        }
    }
}

// ---------------- 契约测试 ----------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use tower::ServiceExt;

    static PORT_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 占住一个随机端口，并确认它的下一个端口当前可用。
    fn occupied_port_with_free_successor() -> (TcpListener, u16) {
        loop {
            let occupied = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = occupied.local_addr().unwrap().port();
            let Some(next) = port.checked_add(1) else {
                continue;
            };
            if port.checked_add(PORT_BIND_MAX_ATTEMPTS).is_none() {
                continue;
            }
            if TcpListener::bind(("127.0.0.1", next)).is_ok() {
                return (occupied, port);
            }
        }
    }

    /// 找一段连续的可用端口并全部占住，用于验证重试上限。
    fn occupy_consecutive_ports(count: u16) -> (u16, Vec<TcpListener>) {
        for start in (30_000..=60_000).step_by(count as usize) {
            let mut listeners = Vec::with_capacity(count as usize);
            for offset in 0..count {
                match TcpListener::bind(("127.0.0.1", start + offset)) {
                    Ok(listener) => listeners.push(listener),
                    Err(_) => break,
                }
            }
            if listeners.len() == count as usize {
                return (start, listeners);
            }
        }
        panic!("找不到 {count} 个连续可用端口");
    }

    #[test]
    fn 首选端口被占用时绑定下一个端口() {
        let _guard = PORT_TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_occupied, start) = occupied_port_with_free_successor();
        let (listener, port) = bind_local(&[start]).unwrap();
        assert_eq!(port, start + 1);
        drop(listener);
    }

    #[test]
    fn 首选端口被占用时返回值在递增范围内且不是被占用端口() {
        let _guard = PORT_TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (_occupied, start) = occupied_port_with_free_successor();
        let (listener, port) = bind_local(&[start]).unwrap();
        assert!(port > start);
        assert!(port < start + PORT_BIND_MAX_ATTEMPTS);
        drop(listener);
    }

    #[test]
    fn 连续端口都不可用时回退随机端口() {
        let _guard = PORT_TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (start, _occupied) = occupy_consecutive_ports(PORT_BIND_MAX_ATTEMPTS);
        let range_end = start + PORT_BIND_MAX_ATTEMPTS - 1;
        let (listener, port) = bind_local(&[start]).unwrap();
        assert!(!(start..=range_end).contains(&port));
        drop(listener);
    }

    async fn test_state(tag: &str) -> Arc<ServerState> {
        let dir = std::env::temp_dir().join(format!("att-server-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, att_core::Platform::Windows).unwrap();
        core.records
            .insert(att_core::records::Record {
                id: "rec-x".into(),
                raw_text: Some("你好".into()),
                optimized_text: Some("你好。".into()),
                ..Default::default()
            })
            .unwrap();
        Arc::new(ServerState { core: Arc::new(core), token: "test-token".into() })
    }

    async fn get_json(state: &Arc<ServerState>, uri: &str) -> (StatusCode, Value) {
        let response = router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .header("x-att-token", "test-token")
                    .header("host", "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get_raw(
        state: &Arc<ServerState>,
        uri: &str,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut builder =
            axum::http::Request::builder().uri(uri).header("x-att-token", "test-token").header("host", "127.0.0.1");
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let response = router(state.clone()).oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes.to_vec())
    }

    async fn request_with_body(state: &Arc<ServerState>, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-att-token", "test-token")
                    .header("host", "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[test]
    fn 来源精确匹配_拒绝前缀绕过与null() {
        for ok in [
            "http://127.0.0.1:8090",
            "http://localhost:8090",
            "https://localhost:1234",
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(is_allowed_origin(ok), "应放行：{ok}");
        }
        for bad in [
            "null",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://localhost:8090.evil.com",
            "http://evil.com",
            "http://user@localhost:8090",
            "http://localhost:8090/",
        ] {
            assert!(!is_allowed_origin(bad), "应拒绝：{bad}");
        }
    }

    #[test]
    fn host_只允许回环地址() {
        for ok in ["127.0.0.1:17890", "localhost:17890", "127.0.0.1", "localhost"] {
            assert!(is_allowed_host(ok), "应放行：{ok}");
        }
        for bad in ["evil.com", "127.0.0.1.evil.com", "localhost.evil.com:17890", "127.0.0.1:abc", "", "[::1]:17890"] {
            assert!(!is_allowed_host(bad), "应拒绝：{bad}");
        }
    }

    #[tokio::test]
    async fn 全局鉴权_health只校验host_其余要求token与回环host() {
        let state = test_state("guard").await;
        let app = router(state);

        // health 不需要 token，但非回环 Host 仍拒绝
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/health")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let _ = response.into_body().collect().await;

        // health + 回环 Host、不带 token → 200
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/health")
                    .header("host", "127.0.0.1:17890")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await;

        // 缺少 token → 401（Host 缺失也先判 token）
        let response = app
            .clone()
            .oneshot(axum::http::Request::builder().uri("/api/settings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // token 正确但 Host 非回环 → 403
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/settings")
                    .header("x-att-token", "test-token")
                    .header("host", "evil.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // token + 回环 Host → 200
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/settings")
                    .header("x-att-token", "test-token")
                    .header("host", "127.0.0.1:17890")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await;

        // GET 媒体 / 导出类接受查询参数 token（<audio>/<img> 带不了请求头）
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/settings?token=test-token")
                    .header("host", "127.0.0.1:17890")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.into_body().collect().await;
    }

    #[tokio::test]
    async fn cors预检不需要token() {
        let state = test_state("preflight").await;
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("OPTIONS")
                    .uri("/api/settings")
                    .header("origin", "http://127.0.0.1:8090")
                    .header("access-control-request-method", "PUT")
                    .header("access-control-request-headers", "x-att-token,content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success(), "预检应成功，实际 {}", response.status());
        assert_eq!(response.headers()["access-control-allow-origin"], "http://127.0.0.1:8090");
    }

    #[tokio::test]
    async fn 音频回放_单段范围返回206_越界返回416() {
        let state = test_state("audio-range").await;
        let content: Vec<u8> = (0..=255u8).collect();
        std::fs::write(state.core.records.audio_dir().join("rec-x.flac"), &content).unwrap();
        state.core.records.set_audio_path("rec-x", "rec-x.flac").unwrap();

        // 没有 Range：完整回传，但声明支持字节范围
        let (status, headers, body) = get_raw(&state, "/api/records/rec-x/audio", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        assert_eq!(headers[header::CONTENT_TYPE], "audio/flac");
        assert_eq!(headers[header::CONTENT_LENGTH], "256");
        assert_eq!(body, content);

        // 闭区间、开放结尾、后缀、超过文件长度、覆盖整个文件
        for (range, expected_start, expected_end) in [
            ("bytes=10-19", 10, 19),
            ("bytes=250-", 250, 255),
            ("bytes=-6", 250, 255),
            ("bytes=0-9999", 0, 255),
            ("bytes=0-", 0, 255),
        ] {
            let (status, headers, body) = get_raw(&state, "/api/records/rec-x/audio", &[("range", range)]).await;
            assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{range}");
            assert_eq!(headers[header::ACCEPT_RANGES], "bytes", "{range}");
            assert_eq!(
                headers[header::CONTENT_RANGE].to_str().unwrap(),
                format!("bytes {expected_start}-{expected_end}/256"),
                "{range}"
            );
            assert_eq!(
                headers[header::CONTENT_LENGTH].to_str().unwrap(),
                (expected_end - expected_start + 1).to_string(),
                "{range}"
            );
            assert_eq!(body.as_slice(), &content[expected_start..=expected_end], "{range}");
        }

        // 语法合法但与文件没有交集：416 + `Content-Range: bytes */总长`
        for range in ["bytes=256-", "bytes=999-1000", "bytes=-0"] {
            let (status, headers, body) = get_raw(&state, "/api/records/rec-x/audio", &[("range", range)]).await;
            assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
            assert_eq!(headers[header::CONTENT_RANGE], "bytes */256", "{range}");
            assert!(body.is_empty(), "{range}");
        }

        // 非法语法 / 多段范围 / 其他单位：忽略 Range，按完整文件返回
        for range in ["bytes=abc", "bytes=5-3", "bytes=0-1,4-5", "items=0-1", "bytes="] {
            let (status, _headers, body) = get_raw(&state, "/api/records/rec-x/audio", &[("range", range)]).await;
            assert_eq!(status, StatusCode::OK, "{range}");
            assert_eq!(body, content, "{range}");
        }
    }

    #[test]
    fn byte_range_解析_空文件与边界() {
        assert_eq!(parse_byte_range(None, 10), ByteRange::Full);
        assert_eq!(parse_byte_range(Some("bytes=0-0"), 10), ByteRange::Partial { start: 0, end: 0 });
        assert_eq!(parse_byte_range(Some("bytes=9-"), 10), ByteRange::Partial { start: 9, end: 9 });
        assert_eq!(parse_byte_range(Some("bytes=-1"), 10), ByteRange::Partial { start: 9, end: 9 });
        assert_eq!(parse_byte_range(Some("bytes=-99"), 10), ByteRange::Partial { start: 0, end: 9 });
        assert_eq!(parse_byte_range(Some("bytes=10-"), 10), ByteRange::Unsatisfiable);
        assert_eq!(parse_byte_range(Some("bytes=0-0"), 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_byte_range(Some("bytes=-1"), 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_byte_range(Some("BYTES=0-0"), 10), ByteRange::Partial { start: 0, end: 0 });
    }

    #[tokio::test]
    async fn 提示词_自定义预设增改删_删除被选用者回退() {
        let root = std::env::temp_dir().join(format!(
            "att-server-prompts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        std::fs::write(root.join("prompts/通用优化.txt"), "通用模板").unwrap();
        std::fs::write(root.join("prompts/程序员模式.txt"), "程序员模板").unwrap();
        let mut core = CoreState::init(&root.join("data"), att_core::Platform::Macos).unwrap();
        core.prompts_dir = root.join("prompts");
        let state = Arc::new(ServerState { core: Arc::new(core), token: "test-token".into() });

        // 旧数据没有 custom_prompts.json：只有内置
        let (status, list) = get_json(&state, "/api/prompts").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["presets"].as_array().unwrap().len(), 2);
        assert_eq!(list["default_preset_id"], json!("preset-通用优化"));

        // 新建
        let (status, created) = request_with_body(
            &state,
            "POST",
            "/api/prompts",
            json!({
                "name": "周报", "prompt_template": "整理成周报"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["is_builtin"], json!(false));
        let id = created["id"].as_str().unwrap().to_string();
        let (status, body) =
            request_with_body(&state, "POST", "/api/prompts", json!({"name": "通用优化", "prompt_template": "x"}))
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "与内置重名：{body}");

        // 修改
        let (status, updated) =
            request_with_body(&state, "PUT", &format!("/api/prompts/{id}"), json!({"prompt_template": "新正文"})).await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!((updated["name"].as_str(), updated["prompt_template"].as_str()), (Some("周报"), Some("新正文")));
        // 内置只读（id 含中文，按 URL 编码传）
        let builtin_uri = "/api/prompts/preset-%E9%80%9A%E7%94%A8%E4%BC%98%E5%8C%96";
        let (status, _) = request_with_body(&state, "PUT", builtin_uri, json!({"prompt_template": "x"})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = request_with_body(&state, "DELETE", builtin_uri, json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // 选用：录音默认 + 优化选中文本
        let (status, _) = request_with_body(&state, "PUT", "/api/prompts/default", json!({"preset_id": id})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = request_with_body(&state, "PUT", "/api/settings", json!({"rewrite_preset_id": id})).await;
        assert_eq!(status, StatusCode::OK);
        let (_, list) = get_json(&state, "/api/prompts").await;
        assert_eq!(list["default_preset_id"], json!(id));
        assert_eq!(
            state
                .core
                .prompt_library()
                .resolve_template(&[state.core.settings().rewrite_preset_id.as_deref()])
                .as_deref(),
            Some("新正文")
        );

        // 删除被选用的预设：默认回退「通用优化」，优化选中文本回退「跟随默认」
        let (status, deleted) = request_with_body(&state, "DELETE", &format!("/api/prompts/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::OK, "{deleted}");
        assert_eq!(deleted["default_preset_id"], json!("preset-通用优化"));
        assert_eq!(deleted["rewrite_preset_id"], Value::Null);
        let settings = state.core.settings();
        assert_eq!(settings.default_preset_id.as_deref(), Some("preset-通用优化"));
        assert_eq!(settings.last_used_preset_id.as_deref(), Some("preset-通用优化"));
        assert!(settings.rewrite_preset_id.is_none());
        let (status, _) = request_with_body(&state, "DELETE", &format!("/api/prompts/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 检查更新_失败带原因_未检查不能下载_开关随设置() {
        let (base, _rx) = att_core::llm::testing::stub_routes(vec![att_core::llm::testing::StubRoute::json(
            "GET /latest ",
            404,
            r#"{"message":"Not Found"}"#,
        )])
        .await;
        let dir = std::env::temp_dir().join(format!("att-server-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut core = CoreState::init(&dir, att_core::Platform::Macos).unwrap();
        core.updater = std::sync::Arc::new(att_core::update::Updater::with_url(&dir, format!("{base}/latest")));
        core.app_version = "2.5.1".into();
        let state = Arc::new(ServerState { core: Arc::new(core), token: "test-token".into() });

        let (status, body) = get_json(&state, "/api/update").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["current_version"], json!("2.5.1"));
        assert_eq!(body["auto_download"], json!(false));
        assert_eq!(body["check"], Value::Null);
        assert_eq!(body["download"]["status"], json!("idle"));

        let (status, body) = request_with_body(&state, "POST", "/api/update/download", json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["detail"].as_str().unwrap().contains("先检查更新"));

        let (status, body) = request_with_body(&state, "POST", "/api/update/check", json!({})).await;
        assert_eq!(status, StatusCode::OK, "检查失败不是接口错误");
        assert_eq!(body["check"]["status"], json!("error"));
        assert!(body["check"]["error"].as_str().unwrap().contains("HTTP 404"));

        let (status, _) =
            request_with_body(&state, "PUT", "/api/settings", json!({"auto_download_updates": true})).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = get_json(&state, "/api/update").await;
        assert_eq!(body["auto_download"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn llm_目录_档案_模型全链路且不外发密钥() {
        let state = test_state("llm").await;
        let (status, catalog) = get_json(&state, "/api/llm/providers").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(catalog["providers"].as_array().unwrap().len(), 14, "默认服务商目录不能为空");

        let (status, profile) = request_with_body(
            &state,
            "POST",
            "/api/llm/profiles",
            json!({
                "provider": "deepseek", "name": "DS", "api_key": "sk-secret-123456"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{profile}");
        assert!(profile.get("api_key_encrypted").is_none());
        assert_eq!(profile["api_key_hint"], json!("sk-****3456"));
        let profile_id = profile["id"].as_str().unwrap().to_string();

        let (status, created) = request_with_body(
            &state,
            "POST",
            "/api/llm/models",
            json!({
                "items": [{"profile_id": profile_id, "model": "deepseek-chat"}]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let model_id = created["created"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(created["created"][0]["model"], json!("deepseek/deepseek-chat"));

        let (_, overview) = get_json(&state, "/api/llm/state").await;
        assert_eq!(overview["default_model_id"], json!(model_id));
        assert!(!overview.to_string().contains("sk-secret-123456"));
        assert!(!overview.to_string().contains("api_key_encrypted"));

        // 默认模型所在档案不能删
        let (status, _) =
            request_with_body(&state, "DELETE", &format!("/api/llm/profiles/{profile_id}"), json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        // fallback 显式 null = 清除；指向不存在的模型 404
        let (status, body) =
            request_with_body(&state, "PUT", "/api/llm/settings", json!({"fallback_model_id": null})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["fallback_model_id"], Value::Null);
        let (status, _) =
            request_with_body(&state, "PUT", "/api/llm/settings", json!({"fallback_model_id": "nope"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // 校验错误：缺必填字段 422、custom 无端点 400
        let (status, _) = request_with_body(&state, "POST", "/api/llm/profiles", json!({"name": "x"})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, error) =
            request_with_body(&state, "POST", "/api/llm/profiles", json!({"provider": "custom", "name": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["detail"].as_str().unwrap().contains("API Base"));
        // 不支持列取的服务商：409 + manual_only
        let (_, anthropic) =
            request_with_body(&state, "POST", "/api/llm/profiles", json!({"provider": "anthropic", "name": "A"})).await;
        let (status, error) = request_with_body(
            &state,
            "POST",
            &format!("/api/llm/profiles/{}/remote-models", anthropic["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["detail"]["manual_only"], json!(true));
    }

    /// 原样取回响应体（导出是文件，不是 JSON）。
    async fn request_raw(
        state: &Arc<ServerState>,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("x-att-token", "test-token")
            .header("host", "127.0.0.1");
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap();
        let response = router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&bytes).to_string())
    }

    /// 250 条记录（超过一页）：每 10 条一条失败的，其余已识别；时间依次往前推一分钟。
    fn insert_many_records(core: &CoreState) {
        let now = chrono::Utc::now();
        for n in 0..250 {
            let id = format!("rec-batch-{n:03}");
            let status = if n % 10 == 0 {
                att_core::records::RecordStatus::Failed
            } else {
                att_core::records::RecordStatus::Transcribed
            };
            core.records
                .insert(att_core::records::Record {
                    id: id.clone(),
                    raw_text: Some(format!("第 {n} 条")),
                    status,
                    ..Default::default()
                })
                .unwrap();
            let created = (now - chrono::Duration::minutes(n)).to_rfc3339();
            core.records.patch(&id, |r| r.created_at = created.clone()).unwrap();
        }
    }

    #[tokio::test]
    async fn records_搜索_状态与日期筛选_与分页总数同步() {
        let state = test_state("search-filters").await;
        insert_many_records(&state.core);
        let (status, body) = get_json(&state, "/api/records/search?status=failed&page_size=10").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["total"], json!(25), "250 条里每 10 条一条失败");
        assert_eq!(body["total_pages"], json!(3));
        assert!(body["items"].as_array().unwrap().iter().all(|r| r["status"] == "failed"));

        let today = chrono::Local::now().date_naive();
        let tomorrow = today + chrono::Duration::days(1);
        let (_, body) = get_json(&state, &format!("/api/records/search?date_from={tomorrow}")).await;
        assert_eq!(body["total"], json!(0), "明天起没有记录");
        let (_, body) = get_json(
            &state,
            &format!("/api/records/search?date_from={}&date_to={today}", today - chrono::Duration::days(1)),
        )
        .await;
        assert_eq!(body["total"], json!(251), "250 条加 test_state 的 rec-x 都在这两天内");

        let (status, _) = get_json(&state, "/api/records/search?date_from=2026-13-40").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _) = get_json(&state, "/api/records/search?status=nope").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn records_批量导出_选中与当前结果_超过一页也完整() {
        let state = test_state("batch-export").await;
        insert_many_records(&state.core);

        // 选中的几条 → Markdown
        let (status, headers, body) = request_raw(
            &state,
            "POST",
            "/api/records/export",
            Some(json!({ "ids": ["rec-batch-001", "rec-batch-002"], "format": "md", "use_optimized": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/markdown"));
        let disposition = headers[header::CONTENT_DISPOSITION].to_str().unwrap();
        assert!(disposition.contains("attachment") && disposition.contains("-2.md"), "{disposition}");
        assert_eq!(body.matches("\n## ").count(), 2, "{body}");
        assert!(body.contains("第 1 条") && body.contains("第 2 条"));

        // 当前筛选的全部结果（失败的 25 条，超过一页 10 条）→ CSV
        let (status, _, csv) = request_raw(
            &state,
            "POST",
            "/api/records/export",
            Some(json!({ "query": { "status": "failed" }, "format": "csv" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(csv.trim_end().split("\r\n").count(), 26, "表头 + 25 行");

        // 下载链接用的 GET：ids 逗号分隔
        let (status, _, json_body) =
            request_raw(&state, "GET", "/api/records/export?format=json&ids=rec-batch-003,rec-batch-004", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(serde_json::from_str::<Value>(&json_body).unwrap().as_array().unwrap().len(), 2);
        // GET 不带 ids：按筛选参数导出全部（这里是全部 251 条）
        let (status, _, txt) = request_raw(&state, "GET", "/api/records/export?format=txt", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(txt.matches("\n[").count() + 1, 251);

        // 参数错误与空结果
        for (body, expected) in [
            (json!({ "ids": ["rec-batch-001"], "format": "docx" }), StatusCode::UNPROCESSABLE_ENTITY),
            (json!({ "format": "md" }), StatusCode::UNPROCESSABLE_ENTITY),
            (json!({ "ids": ["a"], "query": {}, "format": "md" }), StatusCode::UNPROCESSABLE_ENTITY),
            (json!({ "ids": ["missing"], "format": "md" }), StatusCode::NOT_FOUND),
        ] {
            let (status, _, text) = request_raw(&state, "POST", "/api/records/export", Some(body.clone())).await;
            assert_eq!(status, expected, "{body} → {text}");
        }
    }

    #[tokio::test]
    async fn 存储_各部分占用_清理孤儿音频后重算() {
        let state = test_state("storage").await;
        std::fs::write(state.core.records.audio_dir().join("a.flac"), vec![0u8; 300]).unwrap();
        let orphans = state.core.data_dir.join("audio").join("orphans");
        std::fs::create_dir_all(&orphans).unwrap();
        std::fs::write(orphans.join("old.wav"), vec![0u8; 50]).unwrap();

        let (status, report) = get_json(&state, "/api/storage?refresh=true").await;
        assert_eq!(status, StatusCode::OK, "{report}");
        let item = |report: &Value, key: &str| {
            report["items"].as_array().unwrap().iter().find(|i| i["key"] == key).cloned().unwrap()
        };
        let keys: Vec<&str> = report["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["audio", "records", "clipboard", "logs", "orphans", "models"]);
        assert_eq!(item(&report, "audio")["bytes"], json!(300));
        assert_eq!(item(&report, "orphans")["files"], json!(1));
        assert!(report["total_bytes"].as_u64().unwrap() >= 350);

        let (status, cleared) = request_with_body(&state, "POST", "/api/storage/orphans/clear", json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cleared, json!({ "deleted": 1, "bytes": 50 }));
        // 清理后作废缓存：不带 refresh 也能看到新数字
        let (_, report) = get_json(&state, "/api/storage").await;
        assert_eq!(item(&report, "orphans")["bytes"], json!(0));
        assert!(orphans.is_dir(), "孤儿音频目录本身保留");
    }

    #[tokio::test]
    async fn 剪贴板_批量删除_清空保留置顶收藏_改上限立即清理() {
        let state = test_state("clip-bulk").await;
        let mut ids = Vec::new();
        for n in 0..5 {
            let (status, item) =
                request_with_body(&state, "POST", "/api/clipboard/items", json!({ "content": format!("条目 {n}") }))
                    .await;
            assert_eq!(status, StatusCode::OK, "{item}");
            ids.push(item["id"].as_str().unwrap().to_string());
        }
        request_with_body(&state, "PUT", &format!("/api/clipboard/items/{}", ids[0]), json!({ "is_pinned": true }))
            .await;
        request_with_body(&state, "PUT", &format!("/api/clipboard/items/{}", ids[1]), json!({ "is_favorite": true }))
            .await;
        let count = |state: Arc<ServerState>| async move {
            get_json(&state, "/api/clipboard/items?limit=500").await.1.as_array().map(Vec::len).unwrap_or(0)
        };

        let (status, body) =
            request_with_body(&state, "DELETE", "/api/clipboard/items", json!({ "ids": [ids[2], "missing"] })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["deleted"], json!(1));
        assert_eq!(count(state.clone()).await, 4);

        let (status, _) = request_with_body(&state, "DELETE", "/api/clipboard/items", json!({ "ids": "x" })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // 改上限：立即清理（后台跑），置顶 + 收藏占 2 个名额，普通条目至少留最新的一条
        let (status, _) = request_with_body(&state, "PUT", "/api/settings", json!({ "clipboard_max_items": 3 })).await;
        assert_eq!(status, StatusCode::OK);
        let mut remaining = 0;
        for _ in 0..50 {
            remaining = count(state.clone()).await;
            if remaining == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(remaining, 3, "超出上限的最旧普通条目被清掉");

        let (status, body) = request_with_body(&state, "DELETE", "/api/clipboard/items", json!({ "all": true })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "deleted": 1, "kept": 2 }));
        let (_, items) = get_json(&state, "/api/clipboard/items?limit=500").await;
        let mut kept: Vec<&str> = items.as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
        kept.sort();
        let mut expected = vec![ids[0].as_str(), ids[1].as_str()];
        expected.sort();
        assert_eq!(kept, expected, "清空后置顶与收藏还在");
    }

    #[tokio::test]
    async fn 剪贴板缩略图_大图返回缩小的_png_找不到给404() {
        let state = test_state("clip-thumb").await;
        let clipboard = &state.core.clipboard;
        std::fs::create_dir_all(clipboard.images_dir()).unwrap();
        image::RgbaImage::from_pixel(1600, 900, image::Rgba([200, 10, 10, 255]))
            .save(clipboard.images_dir().join("shot.png"))
            .unwrap();
        let item = clipboard
            .capture(att_core::clipboard::CaptureRequest {
                content: "image:shot".into(),
                item_type: Some("image".into()),
                metadata: Some(serde_json::from_value(json!({"image_path": "images/shot.png"})).unwrap()),
                ..Default::default()
            })
            .unwrap();

        let response = router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/clipboard/items/{}/thumbnail", item.id))
                    .header("x-att-token", "test-token")
                    .header("host", "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let thumb = image::load_from_memory(&bytes).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (560, 315));

        let (status, _) = get_json(&state, "/api/clipboard/items/nope/thumbnail").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn records_列表是裸数组() {
        let state = test_state("list").await;
        let (status, body) = get_json(&state, "/api/records").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_array(), "契约：GET /api/records 响应必须是裸数组");
        assert_eq!(body[0]["id"], json!("rec-x"));
    }

    #[tokio::test]
    async fn records_超过两百条仍可分页_带总数时返回items与total() {
        let state = test_state("records-page").await;
        // test_state 已插入 rec-x，这里补到 250 条
        for index in 0..249 {
            state
                .core
                .records
                .insert(att_core::records::Record {
                    id: format!("rec-page-{index:03}"),
                    raw_text: Some(format!("第 {index} 条")),
                    ..Default::default()
                })
                .unwrap();
        }

        // 旧契约：不带参数仍是裸数组，默认最多 200 条
        let (status, body) = get_json(&state, "/api/records").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_array(), "契约：GET /api/records 响应必须是裸数组");
        assert_eq!(body.as_array().unwrap().len(), 200);

        // offset 越过前 200 条仍能取到数据
        let (status, body) = get_json(&state, "/api/records?limit=50&offset=200").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 50, "第 201 条之后必须有数据");

        // with_total=true：{items, total}，total 是过滤后的总数
        let (status, body) = get_json(&state, "/api/records?limit=50&offset=200&with_total=true").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 50);
        assert_eq!(body["total"], json!(250));
        let (status, body) = get_json(&state, "/api/records?status=failed&with_total=true").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
        assert_eq!(body["total"], json!(0), "total 必须按 status 过滤后统计");

        // 记录集页面用的搜索分页：第 3 页（offset 200）不是空白页
        let (status, body) = get_json(&state, "/api/records/search?page=3&page_size=100").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 50);
        assert_eq!(body["total"], json!(250));
        assert_eq!(body["total_pages"], json!(3));
    }

    #[tokio::test]
    async fn settings_密文不外发_白名单与类型契约() {
        let state = test_state("settings").await;
        let (status, view) = get_json(&state, "/api/settings").await;
        assert_eq!(status, StatusCode::OK);
        assert!(view.get("doubao_asr_api_key_encrypted").is_none());
        assert!(view["hotkey"].is_object());
        assert_eq!(view["recording_mode"], json!("single"));

        // 未知键丢弃 + 类型错误 400 中文
        let (status, _) = request_with_body(&state, "PUT", "/api/settings", json!({"unknown": 1})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, error) = request_with_body(&state, "PUT", "/api/settings", json!({"auto_optimize": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["detail"].as_str().unwrap().contains("auto_optimize"));
    }

    #[tokio::test]
    async fn records_更新与乐观锁409() {
        let state = test_state("put").await;
        let (status, _) = request_with_body(
            &state,
            "PUT",
            "/api/records/rec-x",
            json!({
                "raw_text": "新文本", "revision": 0
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, error) = request_with_body(
            &state,
            "PUT",
            "/api/records/rec-x",
            json!({
                "raw_text": "冲突写", "revision": 0
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "过期 revision 必须 409");
        assert!(error["detail"].as_str().unwrap().contains("其他端"));

        let (status, _) = request_with_body(&state, "PUT", "/api/records/missing", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// 带一段 2 秒录音的记录（已优化、已编辑过），重新识别用。
    fn insert_audio_record(core: &CoreState, id: &str) {
        let name = format!("{id}.flac");
        att_core::audio::write_recording_16k(&core.records.audio_dir().join(&name), &vec![0i16; 32_000]).unwrap();
        core.records
            .insert(att_core::records::Record {
                id: id.into(),
                audio_path: name,
                duration: 2.0,
                raw_text: Some("旧原文".into()),
                optimized_text: Some("旧优化".into()),
                is_modified: true,
                status: att_core::records::RecordStatus::Optimized,
                ..Default::default()
            })
            .unwrap();
    }

    #[tokio::test]
    async fn records_重新识别_成功覆盖原文并记下引擎与语言() {
        let (base, _requests) = att_core::llm::testing::stub_routes(vec![att_core::llm::testing::StubRoute::json(
            "POST /v1/tools/asr",
            200,
            r#"{"text":"重新识别的文本"}"#,
        )])
        .await;
        let state = test_state("retranscribe-ok").await;
        state
            .core
            .apply_settings_update(&json!({
                "deepseek_asr_api_key": "sk-test-123456",
                "deepseek_asr_endpoint": format!("{base}/v1/tools/asr"),
            }))
            .unwrap();
        insert_audio_record(&state.core, "rec-audio");

        let (status, accepted) = request_with_body(
            &state,
            "POST",
            "/api/records/rec-audio/retranscribe",
            json!({"provider": "deepseek_asr"}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
        assert_eq!(accepted["status"], json!("transcribing"), "受理后记录置为识别中");

        // 前端沿用状态查询：轮询到状态离开 transcribing
        let mut record = Value::Null;
        for _ in 0..100 {
            let (_, current) = get_json(&state, "/api/records/rec-audio/status").await;
            if current["status"] != json!("transcribing") {
                record = get_json(&state, "/api/records/rec-audio").await.1;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(record["status"], json!("transcribed"), "{record}");
        assert_eq!(record["raw_text"], json!("重新识别的文本"));
        assert_eq!(record["optimized_text"], Value::Null, "重新识别清空旧的优化结果");
        assert_eq!(record["is_modified"], json!(false));
        assert_eq!(record["engine"], json!("deepseek_asr"));
        assert_eq!(record["language"], json!("auto"), "DeepSeek 不能指定语言，记为自动检测");
        assert!(!record.to_string().contains("sk-test-123456"), "响应里不能出现密钥");
        assert!(!state.core.retranscribe.is_active("rec-audio"), "结束后释放占用");
    }

    #[tokio::test]
    async fn records_重新识别_没有音频404_引擎不可用409_不认识422() {
        let state = test_state("retranscribe-errors").await;
        // test_state 的 rec-x 是没有音频的文本记录
        let (status, error) = request_with_body(&state, "POST", "/api/records/rec-x/retranscribe", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(error["detail"].as_str().unwrap().contains("没有录音音频"), "{error}");
        let (status, _) = request_with_body(&state, "POST", "/api/records/missing/retranscribe", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        insert_audio_record(&state.core, "rec-audio");
        // 本地引擎缺模型：409 + 现有的一句结论（与识别引擎页、录音时同一句）
        let (status, error) =
            request_with_body(&state, "POST", "/api/records/rec-audio/retranscribe", json!({"provider": "whisper"}))
                .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        let summary = state
            .core
            .local_models
            .status(&state.core, att_core::stt::local::LocalKind::Whisper)
            .error
            .expect("临时数据目录没有 Whisper 模型");
        assert_eq!(error["detail"], json!(summary));
        // 在线引擎没配凭据：409
        let (status, error) =
            request_with_body(&state, "POST", "/api/records/rec-audio/retranscribe", json!({"provider": "doubao_asr"}))
                .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(error["detail"].as_str().unwrap().contains("豆包"), "{error}");
        let (status, _) =
            request_with_body(&state, "POST", "/api/records/rec-audio/retranscribe", json!({"provider": "nope"})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // 都没受理：记录原样
        let (_, record) = get_json(&state, "/api/records/rec-audio").await;
        assert_eq!(record["status"], json!("optimized"));
        assert_eq!(record["raw_text"], json!("旧原文"));
    }

    #[tokio::test]
    async fn asr_engines_列出全部引擎与可用性() {
        let state = test_state("asr-engines").await;
        let (status, body) = get_json(&state, "/api/asr/engines").await;
        assert_eq!(status, StatusCode::OK);
        let providers: Vec<&str> =
            body["engines"].as_array().unwrap().iter().map(|e| e["provider"].as_str().unwrap()).collect();
        assert_eq!(providers, ["doubao_asr", "chatgpt_asr", "deepseek_asr", "whisper", "qwen_asr"]);
        assert!(body["engines"].as_array().unwrap().iter().all(|e| e["available"] == json!(false)), "{body}");
        assert_eq!(body["current"], json!(state.core.settings().asr_provider));

        state.core.apply_settings_update(&json!({"deepseek_asr_api_key": "sk-test-123456"})).unwrap();
        let (_, body) = get_json(&state, "/api/asr/engines").await;
        let deepseek = body["engines"].as_array().unwrap().iter().find(|e| e["provider"] == "deepseek_asr").unwrap();
        assert_eq!(deepseek["available"], json!(true));
        assert_eq!(deepseek["error"], Value::Null);
        assert!(!body.to_string().contains("sk-test-123456"));
    }

    #[tokio::test]
    async fn health_运行时字段齐备_旧双命名已去掉() {
        let state = test_state("health").await;
        let (status, body) = get_json(&state, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        for field in [
            "model_service_ready",
            "model_service_error",
            "model_service_mode",
            "model_service_loading",
            "streaming_available",
            "streaming_backend",
            "streaming_error",
            "recording_mode",
            "local_engine",
        ] {
            assert!(body.get(field).is_some(), "health 缺字段 {field}");
        }
        // 旧 Python 时代的 qwen_streaming_* 双命名前端已不再读取
        assert!(body.as_object().unwrap().keys().all(|key| !key.starts_with("qwen_streaming")));
    }

    #[tokio::test]
    async fn 备份导出导入_返回快照与重启提示_拒绝相对路径() {
        let state = test_state("backup").await;
        let zip = state.core.data_dir.with_extension("zip");
        let destination = zip.display().to_string();

        let (status, error) =
            request_with_body(&state, "POST", "/api/backup/export", json!({"path": "relative.zip"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["detail"].as_str().unwrap().contains("绝对路径"));

        let (status, error) =
            request_with_body(&state, "POST", "/api/backup/export", json!({"path": destination})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["detail"].as_str().unwrap().contains("未经系统对话框授权"));

        assert!(state.core.backup_paths.authorize_export(&zip));
        let (status, body) = request_with_body(
            &state,
            "POST",
            "/api/backup/export",
            json!({"path": destination, "include_key": false, "include_audio": true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(zip.is_file());
        assert_eq!(body["manifest"]["format"], json!("att-backup"));
        assert_eq!(body["manifest"]["sections"]["records"]["files"], json!(1));

        assert!(state.core.backup_paths.authorize_import(&zip));
        let (status, body) =
            request_with_body(&state, "POST", "/api/backup/import", json!({"path": destination})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["restart_required"], json!(true));
        assert_eq!(body["credentials_reset"], json!(0));
        assert!(std::path::PathBuf::from(body["pre_import_backup"].as_str().unwrap()).is_dir());
        assert!(state.core.data_dir.join("records/rec-x.json").is_file());
        assert!(state.core.backup_maintenance_active(), "恢复成功后必须保持维护态直到重启");
        let (status, error) =
            request_with_body(&state, "PUT", "/api/settings", json!({"asr_provider": "qwen_asr"})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(error["detail"].as_str().unwrap().contains("暂停写入"));

        let _ = std::fs::remove_dir_all(&state.core.data_dir);
        let _ = std::fs::remove_file(&zip);
    }
}
