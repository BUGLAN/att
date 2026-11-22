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
use axum::http::{HeaderValue, StatusCode};
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
        .route("/api/transcribe/jobs/{job_id}/cancel", post(cancel_job))
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
    /// 一律走后台任务（拿得到进度、可以取消）；缓存命中仍直接返回结果
    #[serde(default)]
    async_job: bool,
}

fn default_language() -> String {
    "zh".into()
}
fn default_engine() -> String {
    "auto".into()
}

async fn transcribe_file(State(state): State<Arc<ServerState>>, body: Bytes) -> Response {
    let body: Body = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "detail": format!("请求参数不合法：{error}") })))
                .into_response()
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
        async_job: body.async_job,
    };
    match transcribe::handle(state.core.clone(), request).await {
        Ok(Outcome::Result(payload)) => Json(payload).into_response(),
        Ok(Outcome::Job(job_id)) => (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))).into_response(),
        Err(error) => error_response(error),
    }
}

async fn job(State(state): State<Arc<ServerState>>, Path(job_id): Path<String>) -> Response {
    match state.core.transcriber.get_job(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "detail": "任务不存在或已过期" }))).into_response(),
    }
}

/// 取消后台任务：返回当前快照（`cancel_requested=true`，任务在下一个检查点退出后变为 `cancelled`）；
/// 已结束的任务原样返回。
async fn cancel_job(State(state): State<Arc<ServerState>>, Path(job_id): Path<String>) -> Response {
    match state.core.transcriber.cancel_job(&job_id) {
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
            .header("host", "127.0.0.1")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("x-att-token", token);
        }
        app.clone().oneshot(builder.body(Body::from(body)).unwrap()).await.unwrap().status()
    }

    async fn post_cancel(app: &Router, token: Option<&str>) -> StatusCode {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transcribe/jobs/nope/cancel")
            .header("host", "127.0.0.1");
        if let Some(token) = token {
            builder = builder.header("x-att-token", token);
        }
        app.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    async fn get_job(app: &Router, token: Option<&str>) -> StatusCode {
        let mut builder = axum::http::Request::builder().uri("/api/transcribe/jobs/nope").header("host", "127.0.0.1");
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
        assert_eq!(post_cancel(&app, None).await, StatusCode::UNAUTHORIZED);
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
        assert_eq!(post_cancel(&app, Some("test-token")).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn 省略engine_只配_deepseek_时不再返回409() {
        let state = test_state();
        state.core.apply_settings_update(&json!({ "deepseek_asr_api_key": "sk-test" })).unwrap();
        let app = router(state);
        // 默认 engine=auto 应已解析到 DeepSeek；随后路径不存在 → 404，而不是引擎不可用 409
        let status = post_body(&app, Some("test-token"), r#"{"path":"C:\\not-registered\\a.mp3"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "auto 没有回落到已配置凭据的 DeepSeek");
    }

    #[tokio::test]
    async fn 省略engine_没有任何配置时回落到豆包网页() {
        let app = router(test_state());
        // 新数据目录没有本地模型和豆包 / DeepSeek 凭据；auto 应回落到无需密钥的豆包网页，
        // 随后因为路径未登记返回 404，而不是「没有可用引擎」409。
        let status = post_body(&app, Some("test-token"), r#"{"path":"C:\\not-registered\\a.mp3"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "auto 没有回落到豆包网页语音");
    }
}
