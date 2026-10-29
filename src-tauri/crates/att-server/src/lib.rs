//! 内嵌数据面 API（axum，仅绑定 127.0.0.1，随机端口）。
//!
//! 阶段 3a 范围（架构文档 §13）：
//! - GET /api/health（前端 runtimeStatus 契约：model_service_* 与 qwen_streaming_* 双命名）
//! - GET/PUT /api/settings（白名单/脱敏契约保真）
//! - /api/records 全套（裸数组响应、乐观锁 409、音频播放、导出）
//! - GET /api/prompts + PUT /api/prompts/default
//! - GET /api/llm/state|providers（3f 前返回空态）
//! - 模型服务三端点（骨架：尚未接引擎，返回「未就绪」定型载荷）
//! - 录音/上传/转写类端点 → 501（3b/3d/3e 接入）
//!
//! 两个 WebSocket 端点（/api/ws、/api/asr/stream）按 D4 决策**移除**：
//! 事件改由 Tauri events 下发（旧事件名重放）。

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
        .route("/api/records/{id}/optimize", post(ny_optimize))
        .route("/api/records/{id}/optimize/stream", post(ny_optimize))
        .route("/api/records/{id}/optimize/cancel", post(ny_optimize))
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
        .route("/api/prompts", get(prompts_list))
        .route("/api/prompts/default", put(prompts_set_default))
        // LLM v2（3f 前为空态）
        .route("/api/llm/state", get(llm_state))
        .route("/api/llm/providers", get(llm_providers))
        // 剪贴板：只读端点先返回空数据（3g 接入真实域），写操作标注未实现
        .route("/api/clipboard/items", get(clipboard_items_empty).post(ny_clipboard))
        .route("/api/clipboard/stats", get(clipboard_stats_empty))
        .route(
            "/api/clipboard/items/{id}",
            get(ny_clipboard).put(ny_clipboard).delete(ny_clipboard),
        )
        .route("/api/transcribe", post(ny_transcribe))
        .route("/api/transcribe/jobs/{job_id}", get(ny_transcribe))
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

async fn ny_optimize() -> impl IntoResponse {
    err(StatusCode::NOT_IMPLEMENTED, "LLM 优化将在阶段 3f 接入".into())
}

async fn ny_transcribe() -> impl IntoResponse {
    err(StatusCode::NOT_IMPLEMENTED, "文件转写将在阶段 3f 接入".into())
}

async fn ny_clipboard() -> impl IntoResponse {
    err(StatusCode::NOT_IMPLEMENTED, "剪贴板域将在阶段 3g 接入".into())
}

/// 空剪贴板列表（3a 占位：避免前端出现「加载剪贴板记录失败」的错误提示）。
async fn clipboard_items_empty() -> Json<Value> {
    Json(json!([]))
}

async fn clipboard_stats_empty() -> Json<Value> {
    Json(json!({ "total": 0, "today": 0, "favorites": 0, "pinned": 0 }))
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

/// GET /api/health：前端 runtimeStatus 的全部字段（新旧双命名同源）。
async fn health(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let settings = state.core.settings();
    let engine_note = "引擎将在阶段 3b/3f 接入";
    Json(json!({
        "status": "ok",
        "runtime_environment": "tauri-rust",
        "asr_provider": settings.asr_provider,
        "recording_mode": settings.recording_mode,
        // 新命名
        "model_service_managed": true,
        "model_service_running": false,
        "model_service_loading": false,
        "model_service_ready": false,
        "model_service_error": engine_note,
        "model_service_supported": true,
        "model_service_mode": "provider",
        "streaming_available": false,
        "streaming_supported": false,
        "streaming_backend": Value::Null,
        "streaming_error": engine_note,
        // 旧命名（前端仍读取，见阶段① §3.3 双命名契约）
        "qwen_streaming_available": false,
        "qwen_streaming_backend": Value::Null,
        "qwen_streaming_error": engine_note,
        "qwen_streaming_service_managed": true,
        "qwen_streaming_service_running": false,
        "qwen_streaming_service_loading": false,
        "qwen_streaming_service_ready": false,
        "qwen_streaming_service_pid": Value::Null,
        "qwen_streaming_service_url": Value::Null,
        "qwen_streaming_service_error": engine_note,
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
                // 3f 接引擎后：这里触发停模型服务 + 重建 provider
                tracing::info!(
                    "运行时设置变更（引擎重建将在 3f 生效）: {:?}",
                    outcome.runtime_keys_changed
                );
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
    let status = params.get("status").map(String::as_str);
    // 裸数组响应（契约：非 {items,total} 包装）
    Json(serde_json::to_value(state.core.records.list(status, limit, offset)).unwrap_or_default())
}

async fn records_search(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let query = params.get("q").cloned().unwrap_or_default();
    let search_type = params.get("search_type").map(String::as_str).unwrap_or("all");
    let favorite_only = params.get("favorite_only").map(|v| v == "true" || v == "1").unwrap_or(false);
    let page = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let page_size = parse_usize(&params, "page_size", 100, 200);

    let (items, total) = state.core.records.search(&query, search_type, favorite_only, page, page_size);
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

// ---- 模型服务（骨架：3b/3f 接引擎后替换实现） ----

fn model_service_payload() -> Value {
    json!({
        "managed": true,
        "provider_type": null,
        "model_name": null,
        "recording_mode": "single",
        "loading": false,
        "running": false,
        "ready": false,
        "error": "引擎将在阶段 3b/3f 接入",
        "pid": null,
        "base_url": null,
        "streaming_available": false,
        "streaming_error": "引擎将在阶段 3d 接入",
        "model_service_supported": true,
        "model_service_mode": "provider",
        "streaming_supported": false,
        "streaming_backend": null,
        "health": Value::Null,
        "components": { "provider": null, "streaming_bridge": null },
    })
}

async fn model_service_status() -> Json<Value> {
    Json(model_service_payload())
}

async fn model_service_start() -> impl IntoResponse {
    err(StatusCode::SERVICE_UNAVAILABLE, "模型服务将在阶段 3b/3f 接入".into())
}

async fn model_service_stop() -> Json<Value> {
    Json(json!({ "success": true, "running": false, "loading": false, "ready": false }))
}

// ---- 提示词（只读，真相源 prompts/*.txt） ----

/// 定位提示词目录：优先数据目录同级的 prompts/（仓库布局），其次可执行文件旁。
fn prompts_dir(core: &CoreState) -> std::path::PathBuf {
    let sibling = core.data_dir.parent().map(|p| p.join("prompts"));
    if let Some(dir) = sibling.filter(|d| d.is_dir()) {
        return dir;
    }
    std::path::PathBuf::from("prompts")
}

async fn prompts_list(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let dir = prompts_dir(&state.core);
    let mut presets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
            .collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("preset").to_string();
            let Ok(template) = std::fs::read_to_string(&path) else { continue };
            presets.push(json!({
                "id": format!("preset-{stem}"),
                "name": stem.clone(),
                "description": format!("{stem} 预设"),
                "category": "general",
                "prompt_template": template,
                "is_builtin": true,
            }));
        }
    }
    let settings = state.core.settings();
    let valid_ids: Vec<&str> = presets.iter().filter_map(|p| p["id"].as_str()).collect();
    let default_id = settings
        .default_preset_id
        .clone()
        .filter(|id| valid_ids.contains(&id.as_str()))
        .or_else(|| valid_ids.first().map(|s| s.to_string()));
    Json(json!({
        "presets": presets,
        "default_preset_id": default_id,
        "last_used_preset_id": settings.last_used_preset_id,
    }))
}

async fn prompts_set_default(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, impl IntoResponse> {
    let Some(preset_id) = body.get("preset_id").and_then(|v| v.as_str()).map(str::to_string) else {
        return Err(err(StatusCode::BAD_REQUEST, "preset_id 不能为空".into()));
    };
    // 校验预设存在（真相源是 prompts/*.txt）
    let dir = prompts_dir(&state.core);
    let stem = preset_id.strip_prefix("preset-").unwrap_or(&preset_id);
    if !dir.join(format!("{stem}.txt")).is_file() {
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

// ---- LLM v2（3f 前空态） ----

async fn llm_state() -> Json<Value> {
    Json(json!({
        "version": 2,
        "profiles": [],
        "models": [],
        "default_model_id": Value::Null,
        "fallback_model_id": Value::Null,
        "skipped_entries": [],
        "note": "LLM 配置将在阶段 3f 接入；届时自动迁移 data/llm_v2.json"
    }))
}

async fn llm_providers() -> Json<Value> {
    Json(json!({ "providers": [], "note": "服务商注册表将在阶段 3f 接入" }))
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
