//! 本地模型管理端点（设置页）：
//! - `GET    /api/models`：清单、安装状态、占用字节、模型目录与最近一次下载任务
//! - `POST   /api/models/{id}/download`：后台下载（Whisper 会把缺失的 Silero VAD 一并装上）
//! - `GET    /api/models/jobs/{job_id}`：任务进度（已下 / 总字节、速度、当前文件）
//! - `POST   /api/models/jobs/{job_id}/cancel`：取消（已下字节保留，可续传）
//! - `DELETE /api/models/{id}`：删除已安装模型（录音或文件转写进行中拒绝）

use crate::ServerState;
use att_core::stt::local::catalog::{self, DownloadSource, Engine};
use att_core::stt::local::download::StartError;
use att_core::stt::local::LocalKind;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/models", get(list))
        .route("/api/models/{id}/download", post(start_download))
        .route("/api/models/{id}", axum::routing::delete(delete_model))
        .route("/api/models/jobs/{job_id}", get(job))
        .route("/api/models/jobs/{job_id}/cancel", post(cancel))
}

fn source(state: &ServerState) -> DownloadSource {
    DownloadSource::from_setting(&state.core.settings().model_download_source)
}

async fn list(State(state): Shared) -> Response {
    let manager = state.core.model_manager.clone();
    let source = source(&state);
    match tokio::task::spawn_blocking(move || manager.list(source)).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            crate::err(StatusCode::INTERNAL_SERVER_ERROR, format!("读取模型目录失败：{error}")).into_response()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct DownloadQuery {
    source: Option<String>,
}

async fn start_download(State(state): Shared, Path(id): Path<String>, Query(query): Query<DownloadQuery>) -> Response {
    let selected = match query.source {
        Some(value) => match DownloadSource::parse(&value) {
            Some(source) => source,
            None => {
                return crate::err(StatusCode::BAD_REQUEST, format!("未知的模型下载源：{value}")).into_response();
            }
        },
        None => source(&state),
    };
    match state.core.model_manager.start(&id, selected) {
        Ok(job) => Json(json!({ "job": job })).into_response(),
        Err(StartError { status, message }) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
            crate::err(status, message).into_response()
        }
    }
}

async fn job(State(state): Shared, Path(job_id): Path<String>) -> Response {
    match state.core.model_manager.job(&job_id) {
        Some(job) => Json(json!({ "job": job })).into_response(),
        None => crate::err(StatusCode::NOT_FOUND, format!("没有这个下载任务：{job_id}")).into_response(),
    }
}

async fn cancel(State(state): Shared, Path(job_id): Path<String>) -> Response {
    match state.core.model_manager.cancel(&job_id) {
        Ok(job) => Json(json!({ "job": job })).into_response(),
        Err(error) => {
            crate::err(StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), error.message)
                .into_response()
        }
    }
}

async fn delete_model(State(state): Shared, Path(id): Path<String>) -> Response {
    if state.core.is_recording_active() {
        return crate::err(StatusCode::CONFLICT, "正在录音，结束后再删除模型".into()).into_response();
    }
    if state.core.transcriber.is_busy() {
        return crate::err(StatusCode::CONFLICT, "正在转写文件，结束后再删除模型".into()).into_response();
    }
    if state.core.retranscribe.is_any_active() {
        return crate::err(StatusCode::CONFLICT, "正在重新识别录音，结束后再删除模型".into()).into_response();
    }
    if catalog::find(&id).is_none() {
        return crate::err(StatusCode::NOT_FOUND, format!("未知的模型：{id}")).into_response();
    }
    let _model_gate = match state.core.model_gate.clone().try_write_owned() {
        Ok(guard) => guard,
        Err(_) => {
            return crate::err(StatusCode::CONFLICT, "本地模型正在被录音、转写、预加载或维护，请稍后再删除".into())
                .into_response();
        }
    };
    let core = state.core.clone();
    let delete_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        core.model_manager.delete(&delete_id, |engine| match engine {
            // 已验证模型已安装后才走到这里；Windows 上必须先释放映射文件。
            Engine::Whisper => core.local_models.unload(Some(LocalKind::Whisper)),
            Engine::Qwen => core.local_models.unload(Some(LocalKind::Qwen)),
            Engine::SenseVoice => core.local_models.unload(Some(LocalKind::SenseVoice)),
            Engine::Vad => core.local_models.unload_vad(),
        })
    })
    .await;
    match result {
        Ok(Ok(())) => Json(json!({ "success": true, "id": id })).into_response(),
        Ok(Err(error)) => {
            crate::err(StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), error.message)
                .into_response()
        }
        Err(error) => {
            crate::err(StatusCode::INTERNAL_SERVER_ERROR, format!("删除模型任务失败：{error}")).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerState;
    use att_core::state::CoreState;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state(tag: &str) -> Arc<ServerState> {
        let dir = std::env::temp_dir().join(format!("att-server-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, att_core::Platform::Windows).unwrap();
        Arc::new(ServerState { core: Arc::new(core), token: "test-token".into() })
    }

    async fn request(state: &Arc<ServerState>, method: &str, uri: &str) -> (StatusCode, Value) {
        let response = crate::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("x-att-token", "test-token")
                    .header("host", "127.0.0.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn 模型清单_包含全部项与_vad() {
        let state = test_state("list");
        let (status, body) = request(&state, "GET", "/api/models").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "official");
        let models = body["models"].as_array().unwrap();
        assert!(models.iter().any(|model| model["id"] == "whisper-large-v3-turbo"));
        assert!(models.iter().any(|model| model["id"] == "qwen3-asr-0.6b-int8"));
        assert!(models.iter().any(|model| model["id"] == "sense-voice-zh-en-ja-ko-yue-int8"));
        assert!(models.iter().any(|model| model["id"] == "silero-vad"));
        assert!(models
            .iter()
            .filter(|model| model["engine"] == "whisper")
            .all(|model| model["mirror_supported"] == true));
        let qwen = models.iter().find(|model| model["id"] == "qwen3-asr-0.6b-int8").unwrap();
        assert_eq!(qwen["official_only"], true);
        assert_eq!(qwen["archive"], true);
        let sense_voice = models.iter().find(|model| model["id"] == "sense-voice-zh-en-ja-ko-yue-int8").unwrap();
        assert_eq!(sense_voice["official_only"], true);
        assert_eq!(sense_voice["archive"], true);

        state
            .core
            .apply_settings_update(&json!({ "model_download_source": "mirror", "asr_provider": "whisper" }))
            .unwrap();
        let (status, body) = request(&state, "GET", "/api/models").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source"], "mirror");
        std::fs::create_dir_all(&state.core.models_dir).unwrap();
        std::fs::write(state.core.models_dir.join("silero_vad.onnx"), b"x").unwrap();
        let (status, health) = request(&state, "GET", "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(health["local_engine"]["vad_ready"], true);
    }

    #[tokio::test]
    async fn 模型清单_旧目录只有短名模型时_large_v3_为_missing() {
        let state = test_state("legacy-status");
        let nested = state.core.models_dir.join("faster_whisper").join("faster-whisper-small");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("model.bin"), b"x").unwrap();
        std::fs::write(nested.join("tokenizer.json"), b"{}").unwrap();

        let (status, body) = request(&state, "GET", "/api/models").await;
        assert_eq!(status, StatusCode::OK);
        let models = body["models"].as_array().unwrap();
        let small = models.iter().find(|model| model["id"] == "whisper-small").unwrap();
        let large = models.iter().find(|model| model["id"] == "whisper-large-v3").unwrap();
        assert_eq!(small["status"], "installed");
        assert_eq!(large["status"], "missing");
        assert_eq!(large["partial"], false);
    }

    #[tokio::test]
    async fn 删除模型_录音或转写中拒绝_空闲时删除() {
        let state = test_state("delete");
        // 伪造一份已安装的 Whisper small
        let target = state.core.models_dir.join("faster-whisper-small");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("model.bin"), b"x").unwrap();
        std::fs::write(target.join("tokenizer.json"), b"{}").unwrap();

        state.core.set_recording_active(true);
        let (status, body) = request(&state, "DELETE", "/api/models/whisper-small").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["detail"].as_str().unwrap().contains("录音"));
        assert!(target.exists());
        state.core.set_recording_active(false);

        let (status, body) = request(&state, "DELETE", "/api/models/whisper-small").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert!(!target.exists());

        // 再删一次：未安装 → 400
        let (status, _) = request(&state, "DELETE", "/api/models/whisper-small").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn 未知模型下载与未知任务_返回可读错误() {
        let state = test_state("errors");
        let (status, body) = request(&state, "POST", "/api/models/nope/download").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["detail"].as_str().unwrap().contains("未知的模型"));
        let (status, _) = request(&state, "GET", "/api/models/jobs/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request(&state, "POST", "/api/models/jobs/nope/cancel").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn 下载源_显式参数校验_模型使用中拒绝删除() {
        let state = test_state("source-and-gate");
        let (status, body) = request(&state, "POST", "/api/models/whisper-small/download?source=ftp").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["detail"].as_str().unwrap().contains("下载源"));

        let target = state.core.models_dir.join("faster-whisper-small");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("model.bin"), b"x").unwrap();
        std::fs::write(target.join("tokenizer.json"), b"{}").unwrap();
        let gate = state.core.model_gate.clone().try_read_owned().unwrap();
        let (status, body) = request(&state, "DELETE", "/api/models/whisper-small").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["detail"].as_str().unwrap().contains("本地模型正在被"));
        assert!(target.exists());
        drop(gate);
    }
}
