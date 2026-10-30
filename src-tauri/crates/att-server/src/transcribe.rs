//! 本地文件转写路由（契约同 Python `app/api/routers/transcribe_file.py`）。
//!
//! 可选鉴权：环境变量 `TRANSCRIBE_FILE_API_TOKEN` 非空时要求请求头 `X-ATT-Token` 匹配
//! （默认回环监听即不鉴权；供外部采集层脚本调用）。

// 处理函数以 `Result<_, Response>` 提前返回错误响应是 axum 惯用写法
#![allow(clippy::result_large_err)]

use crate::ServerState;
use att_core::transcribe::{self, Outcome, TranscribeError, TranscribeRequest};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/transcribe", post(transcribe_file))
        .route("/api/transcribe/jobs/{job_id}", get(job))
}

fn error_response(error: TranscribeError) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Json(json!({ "detail": error.message }))).into_response();
    if let Some(seconds) = error.retry_after {
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert("Retry-After", value);
        }
    }
    response
}

/// 常量时间比较，避免时序侧信道。
fn token_matches(expected: &str, actual: &str) -> bool {
    expected.len() == actual.len() && expected.bytes().zip(actual.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn require_token(headers: &HeaderMap) -> Result<(), Response> {
    let expected = std::env::var("TRANSCRIBE_FILE_API_TOKEN").unwrap_or_default();
    if expected.is_empty() {
        return Ok(());
    }
    let actual = headers.get("x-att-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    if token_matches(&expected, actual) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, Json(json!({ "detail": "缺少或错误的 X-ATT-Token" }))).into_response())
    }
}

#[derive(Deserialize)]
struct Body {
    path: String,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default)]
    want_segments: bool,
    #[serde(default = "default_engine")]
    engine: String,
    #[serde(default)]
    proofread: bool,
    #[serde(default)]
    proofread_model_id: Option<String>,
    #[serde(default)]
    proofread_options: Option<Value>,
}

fn default_language() -> String {
    "zh".into()
}
fn default_engine() -> String {
    "auto".into()
}

async fn transcribe_file(State(state): State<Arc<ServerState>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if let Err(response) = require_token(&headers) {
        return response;
    }
    let body: Body = match serde_json::from_value(body) {
        Ok(body) => body,
        Err(error) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "detail": format!("请求参数不合法：{error}") }))).into_response()
        }
    };
    let request = TranscribeRequest {
        path: body.path,
        language: body.language,
        want_segments: body.want_segments,
        engine: body.engine,
        proofread: body.proofread,
        proofread_model_id: body.proofread_model_id.filter(|id| !id.is_empty()),
        proofread_options: body.proofread_options,
    };
    match transcribe::handle(state.core.clone(), request).await {
        Ok(Outcome::Result(payload)) => Json(payload).into_response(),
        Ok(Outcome::Job(job_id)) => (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))).into_response(),
        Err(error) => error_response(error),
    }
}

async fn job(State(state): State<Arc<ServerState>>, headers: HeaderMap, Path(job_id): Path<String>) -> Response {
    if let Err(response) = require_token(&headers) {
        return response;
    }
    match state.core.transcriber.get_job(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "detail": "任务不存在或已过期" }))).into_response(),
    }
}
