//! 内嵌数据面 API（axum，仅绑定 127.0.0.1，随机端口）。
//!
//! 阶段 3a 范围（架构文档 §13）：
//! - GET /api/health（前端 runtimeStatus 契约：model_service_* 与 qwen_streaming_* 双命名）
//! - GET/PUT /api/settings（白名单/脱敏契约保真）
//! - /api/records 全套（裸数组响应、乐观锁 409、音频播放、导出）
//! - /api/prompts：列出（内置 + 自定义）、新建 / 修改 / 删除自定义预设、PUT /api/prompts/default 设默认
//! - /api/llm/* LLM v2 全套（服务商目录 / 档案 / 模型 / 验证，见 [`llm`]）
//! - 模型服务三端点（在线引擎凭据配齐即就绪）
//! - /api/update：检查更新与安装包下载（见 [`update`]）
//! - 录音由 Rust 录音 Actor 直接完成（Tauri 命令），上传/会话/转写类 HTTP 端点 → 501
//!
//! 两个 WebSocket 端点（/api/ws、/api/asr/stream）按 D4 决策**移除**：
//! 事件改由 Tauri events 下发（旧事件名重放）。

mod clipboard;
mod llm;
mod transcribe;
mod update;

use att_core::records::{RecordError, RecordUpdate};
use att_core::state::CoreState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ServerState {
    pub core: Arc<CoreState>,
}

// ---------------- 路由 ----------------

pub fn router(state: Arc<ServerState>) -> Router {
    // 跨域策略：只放行「回环来源」（页面本身可能在 dev 的 8090、打包后的 tauri:// 或
    // 自定义协议下），服务端也只绑定 127.0.0.1，不引入外部暴露面。
    // 用 predicate 而非固定清单：避免端口/协议推断偏差导致整站请求被 CORS 拦掉
    // （3a 实机验收曾出现所有请求失败 + 四类 toast，即此类问题）。
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            |origin: &axum::http::HeaderValue, _parts: &axum::http::request::Parts| {
                origin
                    .to_str()
                    .map(is_allowed_origin)
                    .unwrap_or(false)
            },
        ))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    Router::new()
        .route("/", get(root))
        .route("/api/health", get(health))
        // 设置
        .route("/api/settings", get(settings_get).put(settings_put))
        // 记录
        .route("/api/records", get(records_list))
        .route("/api/records/search", get(records_search))
        .route("/api/records/{id}", get(records_get).put(records_put).delete(records_delete))
        .route("/api/records/{id}/status", get(records_status))
        .route("/api/records/{id}/audio", get(records_audio))
        .route("/api/records/{id}/export", get(records_export))
        .route("/api/audio/upload", post(ny_upload))
        // 会话（多段）
        .route("/api/sessions", post(ny_sessions))
        .route("/api/sessions/{id}", get(ny_sessions).delete(ny_sessions))
        .route("/api/sessions/{id}/segments", post(ny_sessions))
        .route("/api/sessions/{id}/merge", post(ny_sessions))
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
        // 检查更新（设置页「关于」）
        .merge(update::routes())
        .layer(cors)
        .with_state(state)
}

/// 同步绑定监听端口：优先 preferred 列表（固定端口，便于前端兜底探测），
/// 全部被占用时回退随机端口。
///
/// 注意：绑定与 accept 循环**分开**——绑定是同步的，accept 循环由调用方
/// 在自己确认可用的运行时里 spawn。这样不会出现「block_on 内 spawn 的任务
/// 在运行时退出后不再被驱动」的隐蔽故障（TCP 可连但无响应）。
pub fn bind_local(preferred: &[u16]) -> std::io::Result<(std::net::TcpListener, u16)> {
    for port in preferred {
        match std::net::TcpListener::bind(("127.0.0.1", *port)) {
            Ok(listener) => return finalize_listener(listener),
            Err(error) => tracing::warn!("端口 {port} 不可用（{error}），尝试下一个"),
        }
    }
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

/// 允许的来源判定：localhost / 127.0.0.1 任意端口、Tauri 自定义协议、null（file://）。
fn is_allowed_origin(origin: &str) -> bool {
    origin == "null"
        || origin.starts_with("http://localhost")
        || origin.starts_with("http://127.0.0.1")
        || origin.starts_with("https://localhost")
        || origin.starts_with("https://127.0.0.1")
        || origin.starts_with("tauri://")
        || origin.starts_with("http://tauri.localhost")
        || origin.starts_with("https://tauri.localhost")
}

/// 数据面自检：用裸 TCP 发一次 `GET /api/health`，返回状态行。
/// setup 阶段调用，把「数据面是否真的在 TCP 上可达」写进日志——
/// 避免再出现「前端全挂但不知道是端口还是 CORS」的排查困境。
pub fn self_check_health(port: u16) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e| format!("非法地址: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("连接失败: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("发送失败: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("读取失败: {e}"))?;
    Ok(response.lines().next().unwrap_or("(空响应)").to_string())
}

// ---------------- 通用工具 ----------------

/// 错误响应统一形状 `{detail}`。注意保持非泛型（String），
/// 否则不同调用点会实例化出不同的 opaque 返回类型导致 match 臂类型不合。
fn err(status: StatusCode, detail: String) -> impl IntoResponse {
    (status, Json(json!({ "detail": detail })))
}

async fn ny_upload() -> impl IntoResponse {
    err(StatusCode::NOT_IMPLEMENTED, "录音上传将在阶段 3b 接入".into())
}

async fn ny_sessions() -> impl IntoResponse {
    err(StatusCode::NOT_IMPLEMENTED, "多段会话将在阶段 3e 接入".into())
}

fn map_record_error(error: RecordError) -> impl IntoResponse {
    match error {
        RecordError::NotFound => err(StatusCode::NOT_FOUND, "记录不存在".into()),
        RecordError::Conflict => err(StatusCode::CONFLICT, RecordError::Conflict.to_string()),
        RecordError::Io(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn parse_usize(params: &HashMap<String, String>, key: &str, default: usize, max: usize) -> usize {
    params.get(key)
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(1, max))
        .unwrap_or(default)
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
        let load_error = status.load_error.as_ref().map(|error| format!("{}加载失败：{error}", kind.label()));
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
    match att_core::stt::Engine::online_from_settings(core) {
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
        // ChatGPT 听写：批量接口，流式模式下不可用（与 Python get_runtime_capabilities 一致）
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
fn local_engine_payload(core: &CoreState) -> Value {
    let Some(kind) = att_core::stt::local_kind(&core.settings().asr_provider) else {
        return Value::Null;
    };
    let status = core.local_models.status(core, kind);
    json!({
        "provider": kind.provider(),
        "label": kind.label(),
        "available": status.available,
        "loaded": status.loaded,
        "loading": status.loading,
        "error": status.error,
        "load_error": status.load_error,
        "model_dir": status.model_dir.as_ref().map(|path| path.display().to_string()),
        "device": status.device,
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
        // 新命名
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
        // 旧命名（前端仍读取，见阶段① §3.3 双命名契约）
        "qwen_streaming_available": engine.streaming_available,
        "qwen_streaming_backend": engine.streaming_backend,
        "qwen_streaming_error": engine.error,
        "qwen_streaming_service_managed": true,
        "qwen_streaming_service_running": engine.running,
        "qwen_streaming_service_loading": engine.loading,
        "qwen_streaming_service_ready": engine.ready,
        "qwen_streaming_service_pid": Value::Null,
        "qwen_streaming_service_url": Value::Null,
        "qwen_streaming_service_error": engine.error,
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
                // 3c 接热键后：这里触发重绑
                tracing::info!("热键设置变更（重绑将在 3c 生效）");
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
    let records = blocking(move || state.core.records.list(status.as_deref(), limit, offset)).await.unwrap_or_default();
    // 裸数组响应（契约：非 {items,total} 包装）
    Json(serde_json::to_value(records).unwrap_or_default())
}

async fn records_search(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let query = params.get("q").cloned().unwrap_or_default();
    let search_type = params.get("search_type").cloned().unwrap_or_else(|| "all".into());
    let favorite_only = params.get("favorite_only").map(|v| v == "true" || v == "1").unwrap_or(false);
    let page = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let page_size = parse_usize(&params, "page_size", 100, 200);

    let (items, total) =
        blocking(move || state.core.records.search(&query, &search_type, favorite_only, page, page_size)).await.unwrap_or_default();
    let total_pages = if total == 0 { 0 } else { (total + page_size - 1) / page_size };
    Json(json!({
        "items": serde_json::to_value(items).unwrap_or_default(),
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": total_pages,
    }))
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

/// 音频播放：完整文件回传（Content-Length 齐备即可供 <audio> 播放）。
async fn records_audio(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.core.records.audio_file(&id) {
        Ok(Some(path)) => match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let mut headers = HeaderMap::new();
                headers.insert(header::CONTENT_TYPE, "audio/wav".parse().unwrap());
                headers.insert(header::ACCEPT_RANGES, "none".parse().unwrap());
                (StatusCode::OK, headers, bytes).into_response()
            }
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "音频不存在".into()).into_response(),
        Err(e) => map_record_error(e).into_response(),
    }
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
    let prefer_opt = use_optimized
        && record.optimized_text.as_deref().map(|t| !t.is_empty()).unwrap_or(false);
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

// ---- 模型服务（在线引擎：凭据配齐即就绪；本地引擎：start 真正加载并预热、stop 释放） ----

fn model_service_payload(core: &CoreState) -> Value {
    let settings = core.settings();
    let engine = engine_status(core);
    let local = att_core::stt::local_kind(&settings.asr_provider).map(|kind| core.local_models.status(core, kind));
    let local_model_dir = local
        .as_ref()
        .and_then(|status| status.model_dir.as_ref())
        .map(|path| path.display().to_string());
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
async fn prompts_delete(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
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
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

// ---------------- 契约测试 ----------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
        Arc::new(ServerState { core: Arc::new(core) })
    }

    async fn get_json(state: &Arc<ServerState>, uri: &str) -> (StatusCode, Value) {
        let response = router(state.clone())
            .oneshot(axum::http::Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn request_with_body(
        state: &Arc<ServerState>,
        method: &str,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
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

    #[tokio::test]
    async fn 提示词_自定义预设增改删_删除被选用者回退() {
        let root = std::env::temp_dir().join(format!("att-server-prompts-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        std::fs::write(root.join("prompts/通用优化.txt"), "通用模板").unwrap();
        std::fs::write(root.join("prompts/程序员模式.txt"), "程序员模板").unwrap();
        let mut core = CoreState::init(&root.join("data"), att_core::Platform::Macos).unwrap();
        core.prompts_dir = root.join("prompts");
        let state = Arc::new(ServerState { core: Arc::new(core) });

        // 旧数据没有 custom_prompts.json：只有内置
        let (status, list) = get_json(&state, "/api/prompts").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["presets"].as_array().unwrap().len(), 2);
        assert_eq!(list["default_preset_id"], json!("preset-通用优化"));

        // 新建
        let (status, created) = request_with_body(&state, "POST", "/api/prompts", json!({
            "name": "周报", "prompt_template": "整理成周报"
        })).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["is_builtin"], json!(false));
        let id = created["id"].as_str().unwrap().to_string();
        let (status, body) = request_with_body(&state, "POST", "/api/prompts", json!({"name": "通用优化", "prompt_template": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "与内置重名：{body}");

        // 修改
        let (status, updated) = request_with_body(&state, "PUT", &format!("/api/prompts/{id}"), json!({"prompt_template": "新正文"})).await;
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
            state.core.prompt_library().resolve_template(&[state.core.settings().rewrite_preset_id.as_deref()]).as_deref(),
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
        let state = Arc::new(ServerState { core: Arc::new(core) });

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

        let (status, _) = request_with_body(&state, "PUT", "/api/settings", json!({"auto_download_updates": true})).await;
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

        let (status, profile) = request_with_body(&state, "POST", "/api/llm/profiles", json!({
            "provider": "deepseek", "name": "DS", "api_key": "sk-secret-123456"
        })).await;
        assert_eq!(status, StatusCode::OK, "{profile}");
        assert!(profile.get("api_key_encrypted").is_none());
        assert_eq!(profile["api_key_hint"], json!("sk-****3456"));
        let profile_id = profile["id"].as_str().unwrap().to_string();

        let (status, created) = request_with_body(&state, "POST", "/api/llm/models", json!({
            "items": [{"profile_id": profile_id, "model": "deepseek-chat"}]
        })).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let model_id = created["created"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(created["created"][0]["model"], json!("deepseek/deepseek-chat"));

        let (_, overview) = get_json(&state, "/api/llm/state").await;
        assert_eq!(overview["default_model_id"], json!(model_id));
        assert!(!overview.to_string().contains("sk-secret-123456"));
        assert!(!overview.to_string().contains("api_key_encrypted"));

        // 默认模型所在档案不能删
        let (status, _) = request_with_body(&state, "DELETE", &format!("/api/llm/profiles/{profile_id}"), json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        // fallback 显式 null = 清除；指向不存在的模型 404
        let (status, body) = request_with_body(&state, "PUT", "/api/llm/settings", json!({"fallback_model_id": null})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["fallback_model_id"], Value::Null);
        let (status, _) = request_with_body(&state, "PUT", "/api/llm/settings", json!({"fallback_model_id": "nope"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // 校验错误：缺必填字段 422、custom 无端点 400
        let (status, _) = request_with_body(&state, "POST", "/api/llm/profiles", json!({"name": "x"})).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, error) = request_with_body(&state, "POST", "/api/llm/profiles", json!({"provider": "custom", "name": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["detail"].as_str().unwrap().contains("API Base"));
        // 不支持列取的服务商：409 + manual_only
        let (_, anthropic) = request_with_body(&state, "POST", "/api/llm/profiles", json!({"provider": "anthropic", "name": "A"})).await;
        let (status, error) = request_with_body(
            &state, "POST", &format!("/api/llm/profiles/{}/remote-models", anthropic["id"].as_str().unwrap()), json!({})
        ).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["detail"]["manual_only"], json!(true));
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
        let (status, _) = request_with_body(&state, "PUT", "/api/records/rec-x", json!({
            "raw_text": "新文本", "revision": 0
        })).await;
        assert_eq!(status, StatusCode::OK);

        let (status, error) = request_with_body(&state, "PUT", "/api/records/rec-x", json!({
            "raw_text": "冲突写", "revision": 0
        })).await;
        assert_eq!(status, StatusCode::CONFLICT, "过期 revision 必须 409");
        assert!(error["detail"].as_str().unwrap().contains("其他端"));

        let (status, _) = request_with_body(&state, "PUT", "/api/records/missing", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_双命名字段齐备() {
        let state = test_state("health").await;
        let (status, body) = get_json(&state, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        for field in [
            "model_service_ready", "streaming_available", "qwen_streaming_service_ready",
            "qwen_streaming_available", "model_service_mode", "streaming_backend",
        ] {
            assert!(body.get(field).is_some(), "health 缺字段 {field}");
        }
    }

    #[tokio::test]
    async fn 未接端点返回501() {
        let state = test_state("501").await;
        let (status, body) = request_with_body(&state, "POST", "/api/audio/upload", json!({})).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(body["detail"].as_str().unwrap().contains("3b"));
    }
}
