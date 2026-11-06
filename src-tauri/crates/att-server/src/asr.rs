//! 在线识别引擎的凭据验证（设置页「识别引擎」卡片与首页就绪判定用，见 `att_core::stt::verify`）。
//!
//! - GET  /api/asr/credential-check?provider=…：当前凭据最近一次的验证结果（没验过为 `null`），不发网络请求
//! - POST /api/asr/credential-check {provider?}：立即验证（豆包握手约 1～2 秒），返回结果
//!
//! provider 缺省取设置里的 `asr_provider`。

use crate::ServerState;
use att_core::stt::verify;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new().route("/api/asr/credential-check", get(check_get).post(check_post))
}

/// 请求里的 provider，缺省取当前设置；不是在线引擎时返回 422。
fn resolve_provider(state: &ServerState, requested: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    let provider = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.core.settings().asr_provider);
    if verify::is_online_provider(&provider) {
        Ok(provider.trim().to_string())
    } else {
        Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "detail": format!("{provider} 不是在线引擎，无需验证凭据") })),
        ))
    }
}

async fn check_get(State(state): Shared, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = match resolve_provider(&state, params.get("provider").map(String::as_str)) {
        Ok(provider) => provider,
        Err(response) => return response.into_response(),
    };
    let check = verify::current_check(&state.core, &provider);
    Json(json!({ "provider": provider, "check": check })).into_response()
}

async fn check_post(State(state): Shared, body: Option<Json<Value>>) -> impl IntoResponse {
    let requested = body.as_ref().and_then(|Json(value)| value.get("provider")).and_then(Value::as_str).map(str::to_string);
    let provider = match resolve_provider(&state, requested.as_deref()) {
        Ok(provider) => provider,
        Err(response) => return response.into_response(),
    };
    match verify::check(&state.core, &provider).await {
        Ok(check) => Json(json!({ "provider": provider, "check": check })).into_response(),
        // 没配齐凭据：409 + 去填写的提示（与录音时的报错同一句）
        Err(message) => (StatusCode::CONFLICT, Json(json!({ "detail": message }))).into_response(),
    }
}
