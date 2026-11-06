//! 在线引擎凭据验证端点与就绪判定（不发真实网络请求：结论用 test-support 直接写入）。

use att_core::state::CoreState;
use att_core::stt::verify::{self, CheckStatus};
use att_server::{router, ServerState};
use axum::body::Body;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn temp_state(tag: &str) -> Arc<ServerState> {
    let dir = std::env::temp_dir().join(format!("att-asr-credential-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = CoreState::init(&dir, att_core::Platform::current()).unwrap();
    Arc::new(ServerState { core: Arc::new(core) })
}

async fn call(state: &Arc<ServerState>, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
    let mut request = axum::http::Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = router(state.clone()).oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn 验证确定失败时_health_不报就绪_并给出原因() {
    let state = temp_state("invalid");
    state
        .core
        .apply_settings_update(&json!({ "asr_provider": "doubao_asr", "doubao_asr_api_key": "token-x" }))
        .unwrap();

    // 没验过：凭据齐备即就绪（行为与以前一致）
    let (_, health) = call(&state, "GET", "/api/health", None).await;
    assert_eq!(health["model_service_ready"], json!(true));
    let (status, current) = call(&state, "GET", "/api/asr/credential-check", None).await;
    assert_eq!(status, 200);
    assert_eq!(current["provider"], "doubao_asr");
    assert!(current["check"].is_null());

    // 只是没验成（网络等）：不拦录音
    verify::record_for_test(&state.core, "doubao_asr", CheckStatus::Unknown, "连接豆包语音服务超时（10 秒）");
    let (_, health) = call(&state, "GET", "/api/health", None).await;
    assert_eq!(health["model_service_ready"], json!(true));

    // 确定失败：不就绪，原因可读
    verify::record_for_test(&state.core, "doubao_asr", CheckStatus::Invalid, "鉴权失败：Access Token 无效或已过期（401）");
    let (_, health) = call(&state, "GET", "/api/health", None).await;
    assert_eq!(health["model_service_ready"], json!(false));
    let error = health["model_service_error"].as_str().unwrap();
    assert!(error.contains("凭据验证失败") && error.contains("401"), "{error}");
    let (_, current) = call(&state, "GET", "/api/asr/credential-check?provider=doubao_asr", None).await;
    assert_eq!(current["check"]["status"], "invalid");

    // 换了 Token：旧结论失效，恢复「凭据齐备即就绪」
    state.core.apply_settings_update(&json!({ "doubao_asr_api_key": "token-y" })).unwrap();
    let (_, health) = call(&state, "GET", "/api/health", None).await;
    assert_eq!(health["model_service_ready"], json!(true));
}

#[tokio::test]
async fn 未配置凭据与本地引擎_不做验证() {
    let state = temp_state("reject");
    // 豆包没填 Token：409 + 去填写的提示
    let (status, body) = call(&state, "POST", "/api/asr/credential-check", Some(json!({ "provider": "doubao_asr" }))).await;
    assert_eq!(status, 409);
    assert!(body["detail"].as_str().unwrap().contains("Access Token"));
    // 本地引擎：422
    let (status, _) = call(&state, "POST", "/api/asr/credential-check", Some(json!({ "provider": "whisper" }))).await;
    assert_eq!(status, 422);
    let (status, _) = call(&state, "GET", "/api/asr/credential-check?provider=qwen_asr", None).await;
    assert_eq!(status, 422);
}
