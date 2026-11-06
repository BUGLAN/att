//! 剪贴板历史路由（契约同 Python `app/clipboard/routes.py`）。

// 处理函数以 `Result<_, Response>` 提前返回错误响应是 axum 惯用写法
#![allow(clippy::result_large_err)]

use crate::ServerState;
use att_core::clipboard::{CaptureRequest, ListQuery, UpdateRequest, ITEM_TYPES};
use att_core::events::AppEvent;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/clipboard/items", get(list).post(create))
        .route("/api/clipboard/stats", get(stats))
        .route("/api/clipboard/items/{id}", get(item).put(update).delete(remove))
        .route("/api/clipboard/items/{id}/image", get(image))
        .route("/api/clipboard/items/{id}/thumbnail", get(thumbnail))
}

fn detail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "detail": message }))).into_response()
}

const NOT_FOUND: &str = "Clipboard item not found";

#[derive(Deserialize)]
struct ListParams {
    query: Option<String>,
    item_type: Option<String>,
    #[serde(default)]
    favorite_only: bool,
    #[serde(default)]
    pinned_only: bool,
    limit: Option<usize>,
    /// 分段加载：跳过前多少条
    #[serde(default)]
    offset: usize,
    /// true 时返回 `{ items, total }`（剪贴板悬浮框分段加载用）；默认仍是数组，旧调用方不受影响
    #[serde(default)]
    with_total: bool,
}

async fn list(State(state): Shared, Query(params): Query<ListParams>) -> Result<Json<Value>, Response> {
    let item_type = params.item_type.filter(|t| !t.is_empty());
    // 逗号分隔表示多个类型（悬浮框「文本」= text,code）
    if item_type.as_deref().is_some_and(|t| t.split(',').map(str::trim).any(|t| !ITEM_TYPES.contains(&t))) {
        return Err(detail(StatusCode::UNPROCESSABLE_ENTITY, "item_type 只能是 text/link/code/file/image"));
    }
    let limit = params.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(detail(StatusCode::UNPROCESSABLE_ENTITY, "limit 必须在 1~500 之间"));
    }
    let query = ListQuery {
        query: params.query,
        item_type,
        favorite_only: params.favorite_only,
        pinned_only: params.pinned_only,
        offset: params.offset,
        limit,
    };
    let (items, total) = crate::blocking(move || state.core.clipboard.list_page(&query)).await.unwrap_or_default();
    if params.with_total {
        return Ok(Json(json!({ "items": items, "total": total })));
    }
    Ok(Json(json!(items)))
}

async fn stats(State(state): Shared) -> Json<Value> {
    Json(state.core.clipboard.stats())
}

async fn create(State(state): Shared, Json(body): Json<Value>) -> Result<Json<Value>, Response> {
    let request: CaptureRequest =
        serde_json::from_value(body).map_err(|e| detail(StatusCode::UNPROCESSABLE_ENTITY, &format!("请求参数不合法：{e}")))?;
    if request.content.is_empty() {
        return Err(detail(StatusCode::UNPROCESSABLE_ENTITY, "content 不能为空"));
    }
    let item = state.core.clipboard.capture(request).map_err(|e| detail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    state.core.events.publish(AppEvent::ClipboardCaptured { item_id: item.id.clone() });
    Ok(Json(json!(item)))
}

async fn item(State(state): Shared, Path(id): Path<String>) -> Result<Json<Value>, Response> {
    state.core.clipboard.get(&id).map(|i| Json(json!(i))).ok_or_else(|| detail(StatusCode::NOT_FOUND, NOT_FOUND))
}

async fn update(State(state): Shared, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, Response> {
    let request: UpdateRequest =
        serde_json::from_value(body).map_err(|e| detail(StatusCode::UNPROCESSABLE_ENTITY, &format!("请求参数不合法：{e}")))?;
    match state.core.clipboard.update(&id, request) {
        Ok(Some(item)) => Ok(Json(json!(item))),
        Ok(None) => Err(detail(StatusCode::NOT_FOUND, NOT_FOUND)),
        Err(error) => Err(detail(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())),
    }
}

async fn remove(State(state): Shared, Path(id): Path<String>) -> Result<Json<Value>, Response> {
    if !state.core.clipboard.delete(&id) {
        return Err(detail(StatusCode::NOT_FOUND, NOT_FOUND));
    }
    Ok(Json(json!({ "status": "deleted", "item_id": id })))
}

async fn image(State(state): Shared, Path(id): Path<String>) -> Result<Response, Response> {
    let path = state.core.clipboard.image_path(&id).ok_or_else(image_not_found)?;
    serve_image(&path).await
}

/// 缩略图（长边不超过 560px）：悬浮框卡片用，避免每次唤起都整张解码 2K~4K 原图。
/// 首次请求某张旧图时要解码原图生成，放到阻塞线程；原图够小或解不开时直接给原图。
async fn thumbnail(State(state): Shared, Path(id): Path<String>) -> Result<Response, Response> {
    let core = state.core.clone();
    let path = tokio::task::spawn_blocking(move || core.clipboard.thumbnail_path(&id))
        .await
        .ok()
        .flatten()
        .ok_or_else(image_not_found)?;
    serve_image(&path).await
}

fn image_not_found() -> Response {
    detail(StatusCode::NOT_FOUND, "Clipboard image not found")
}

async fn serve_image(path: &std::path::Path) -> Result<Response, Response> {
    let bytes = tokio::fs::read(path).await.map_err(|_| image_not_found())?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    };
    Ok(([(header::CONTENT_TYPE, mime)], bytes).into_response())
}
