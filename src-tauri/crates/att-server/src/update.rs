//! 检查更新路由（设置页「关于」）：
//! - `GET  /api/update`：当前版本、自动下载开关、上次检查结果与下载进度
//! - `POST /api/update/check`：发起一次检查（开启自动下载且有新版本时后台开始下载），返回最新状态
//! - `POST /api/update/download`：后台下载上次检查到的新版本安装包，返回最新状态（前端轮询 GET 看进度）

use crate::ServerState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/update", get(status))
        .route("/api/update/check", post(check))
        .route("/api/update/download", post(download))
}

fn snapshot(state: &ServerState) -> Value {
    let core = &state.core;
    core.updater.snapshot(&core.app_version, core.settings().auto_download_updates)
}

async fn status(State(state): Shared) -> Json<Value> {
    Json(snapshot(&state))
}

/// 检查失败也是 200：失败原因在 `check.error` 里，由页面统一展示（不是接口本身出错）。
async fn check(State(state): Shared) -> Json<Value> {
    att_core::update::check_and_maybe_download(state.core.clone()).await;
    Json(snapshot(&state))
}

async fn download(State(state): Shared) -> Response {
    // 同步占位：返回给前端时状态已是 downloading；不能下载（没有新版本 / 没有本机安装包）时 400 + 原因
    match state.core.updater.begin_download() {
        Ok(Some(pending)) => {
            let core = state.core.clone();
            tokio::spawn(async move {
                core.updater.run_download(pending).await;
            });
        }
        Ok(None) => {} // 已在下载或已下载完成
        Err(reason) => return (StatusCode::BAD_REQUEST, Json(json!({ "detail": reason }))).into_response(),
    }
    Json(snapshot(&state)).into_response()
}
