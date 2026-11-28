//! 存储管理（设置 → 诊断 →「存储」，见 `att_core::storage::usage`）：
//!
//! - GET  /api/storage[?refresh=true]：录音音频、记录、剪贴板（含图片）、日志、孤儿音频、本地模型各自的文件数与字节数
//!   （阻塞线程里遍历目录，结果缓存 1 分钟；refresh=true 时重新统计）
//! - POST /api/storage/orphans/clear：清理 `audio/orphans/`（前端二次确认）：仍被记录引用的移回录音目录，其余删除，
//!   返回 `{ deleted, bytes, restored, kept }`（kept = 仍被引用但录音目录里已有同名文件、留在原处）

use crate::ServerState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new().route("/api/storage", get(usage)).route("/api/storage/orphans/clear", post(clear_orphans))
}

fn failure(message: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "detail": message }))).into_response()
}

#[derive(Deserialize)]
struct UsageParams {
    #[serde(default)]
    refresh: bool,
}

async fn usage(State(state): Shared, Query(params): Query<UsageParams>) -> Result<Json<Value>, Response> {
    let core = state.core.clone();
    let report = crate::blocking(move || core.storage_usage.get_or_measure(&core, params.refresh))
        .await
        .ok_or_else(|| failure("统计数据占用失败".into()))?;
    Ok(Json(json!(report)))
}

async fn clear_orphans(State(state): Shared) -> Result<Json<Value>, Response> {
    let core = state.core.clone();
    match crate::blocking(move || core.clear_orphan_audio()).await {
        Some(Ok(result)) => Ok(Json(json!(result))),
        Some(Err(error)) => Err(failure(format!("清理孤儿音频失败：{error}"))),
        None => Err(failure("清理孤儿音频失败".into())),
    }
}
