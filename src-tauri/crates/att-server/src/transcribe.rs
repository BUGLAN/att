//! 本地文件转写路由（契约同 Python `app/api/routers/transcribe_file.py`）。
//!
//! 默认鉴权：请求头 `X-ATT-Token` 必须匹配本次启动生成的数据面 token
//! （壳层经 `__ATT_BACKEND_TOKEN__` 注入给前端，由 `api/client.js` 统一带上）。

// 处理函数以 `Result<_, Response>` 提前返回错误响应是 axum 惯用写法
#![allow(clippy::result_large_err)]

use crate::ServerState;
use att_core::transcribe::{self, Outcome, TranscribeError, TranscribeRequest};
use axum::body::Bytes;
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

fn require_token(state: &ServerState, headers: &HeaderMap) -> Result<(), Response> {
    let actual = headers.get("x-att-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !state.token.is_empty() && token_matches(&state.token, actual) {
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

async fn transcribe_file(State(state): State<Arc<ServerState>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(response) = require_token(&state, &headers) {
        return response;
    }
    let body: Body = match serde_json::from_slice(&body) {
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
    if let Err(response) = require_token(&state, &headers) {
        return response;
    }
    match state.core.transcriber.get_job(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "detail": "任务不存在或已过期" }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router;
    use att_core::state::CoreState;
    use axum::body::Body;
    use axum::Router;
    use tower::ServiceExt;

    fn test_state() -> Arc<ServerState> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("att-transcribe-auth-{}-{stamp}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, att_core::Platform::Windows).unwrap();
        Arc::new(ServerState { core: Arc::new(core), token: "test-token".into() })
    }

    async fn post_transcribe(app: &Router, token: Option<&str>) -> StatusCode {
        post_body(app, token, r#"{"path":"/tmp/not-registered.mp3"}"#).await
    }

    async fn post_body(app: &Router, token: Option<&str>, body: &'static str) -> StatusCode {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transcribe")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("x-att-token", token);
        }
        app.clone()
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    async fn get_job(app: &Router, token: Option<&str>) -> StatusCode {
        let mut builder = axum::http::Request::builder().uri("/api/transcribe/jobs/nope");
        if let Some(token) = token {
            builder = builder.header("x-att-token", token);
        }
        app.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn 转写接口默认要求token() {
        let app = router(test_state());
        assert_eq!(post_transcribe(&app, None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(get_job(&app, None).await, StatusCode::UNAUTHORIZED);
        // 鉴权在 JSON 解析之前：请求体坏掉也不能绕过 token 检查
        assert_eq!(post_body(&app, None, "{").await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn 错误token返回401() {
        let app = router(test_state());
        assert_eq!(post_transcribe(&app, Some("wrong-token")).await, StatusCode::UNAUTHORIZED);
        assert_eq!(get_job(&app, Some("wrong-token")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn 正确token通过鉴权() {
        let app = router(test_state());
        // 已过鉴权：POST 进入引擎解析 / 路径校验；GET 任务不存在 → 404
        assert_ne!(post_transcribe(&app, Some("test-token")).await, StatusCode::UNAUTHORIZED);
        assert_eq!(get_job(&app, Some("test-token")).await, StatusCode::NOT_FOUND);
    }
}
