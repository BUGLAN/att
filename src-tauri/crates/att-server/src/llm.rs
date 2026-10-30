//! LLM v2 路由（契约与旧 Python 版 `/api/llm/*` 一致）。
//!
//! - 只做「解析请求 → 调 store/gateway → 组装响应」，业务规则在 `att_core::llm`；
//! - 任何响应不得包含密钥明文/密文（档案统一走 `profile_public`）；
//! - 验证类端点返回 200 + `{ok, error_code, ...}`（验证失败是正常业务结果，不是传输错误）。

// 处理函数以 `Result<_, Response>` 提前返回错误响应是 axum 惯用写法，错误分支本就是最终响应
#![allow(clippy::result_large_err)]

use crate::ServerState;
use att_core::llm::errors::{LlmError, CODE_MODEL_NOT_FOUND, CODE_NETWORK, CODE_UNSUPPORTED_PROVIDER};
use att_core::llm::gateway::{self, Endpoint, VerifyOutcome};
use att_core::llm::optimizer::{self, OptimizeEvent, OptimizeOutcome, OptimizeSuccess};
use att_core::llm::registry;
use att_core::llm::store::{
    model_public, profile_public, DraftVerify, ModelItemCreate, ModelUpdate, ProfileCreate, ProfileUpdate, StoreError, VerifyStatus,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;
type Reply = Result<Json<Value>, Response>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/llm/providers", get(providers))
        .route("/api/llm/state", get(state))
        .route("/api/llm/profiles", get(profiles_list).post(profile_create))
        .route("/api/llm/profiles/{id}", get(profile_get).put(profile_update).delete(profile_delete))
        .route("/api/llm/profiles/{id}/verify", post(profile_verify))
        .route("/api/llm/profiles/{id}/remote-models", post(remote_models))
        .route("/api/llm/verify-draft", post(verify_draft))
        .route("/api/llm/models", get(models_list).post(models_create))
        .route("/api/llm/models/{id}", put(model_update).delete(model_delete))
        .route("/api/llm/models/{id}/verify", post(model_verify))
        .route("/api/llm/models/{id}/activate", post(model_activate))
        .route("/api/llm/settings", put(settings_update))
        // 记录优化（流式 NDJSON / 非流式 / 取消）
        .route("/api/records/{id}/optimize", post(record_optimize))
        .route("/api/records/{id}/optimize/stream", post(record_optimize_stream))
        .route("/api/records/{id}/optimize/cancel", post(record_optimize_cancel))
}

fn detail(status: StatusCode, detail: impl Into<Value>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

/// 请求体解析失败 → 422（对应 FastAPI 的校验错误；detail 用可读中文而非对象数组）。
fn parse<T: DeserializeOwned>(body: Value) -> Result<T, Response> {
    serde_json::from_value(body).map_err(|e| detail(StatusCode::UNPROCESSABLE_ENTITY, format!("请求参数不合法：{e}")))
}

fn store_error(error: StoreError, not_found: &str, conflict: &str) -> Response {
    match error {
        StoreError::NotFound => detail(StatusCode::NOT_FOUND, not_found),
        StoreError::ConflictDefault => detail(StatusCode::CONFLICT, conflict),
        StoreError::Invalid(message) => detail(StatusCode::BAD_REQUEST, message),
        StoreError::Io(message) => detail(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

const PROFILE_NOT_FOUND: &str = "供应商档案不存在";
const MODEL_NOT_FOUND: &str = "模型不存在";

fn spec_of(provider: &str) -> Result<&'static registry::ProviderSpec, LlmError> {
    registry::get(provider).ok_or_else(|| LlmError::plain(CODE_UNSUPPORTED_PROVIDER, format!("未知服务商：{provider}")))
}

// ==================== 服务商目录 / 总览 ====================

async fn providers() -> Json<Value> {
    Json(json!({ "providers": registry::catalog() }))
}

async fn state(State(state): Shared) -> Json<Value> {
    Json(state.core.llm.overview())
}

// ==================== 供应商档案 ====================

async fn profiles_list(State(state): Shared) -> Json<Value> {
    let profiles: Vec<Value> = state.core.llm.list_profiles().iter().map(profile_public).collect();
    Json(json!({ "profiles": profiles }))
}

async fn profile_create(State(state): Shared, Json(body): Json<Value>) -> Reply {
    let request: ProfileCreate = parse(body)?;
    let profile = state.core.llm.create_profile(request).map_err(|e| store_error(e, PROFILE_NOT_FOUND, ""))?;
    Ok(Json(profile_public(&profile)))
}

async fn profile_get(State(state): Shared, Path(id): Path<String>) -> Reply {
    let profile = state.core.llm.get_profile(&id).ok_or_else(|| detail(StatusCode::NOT_FOUND, PROFILE_NOT_FOUND))?;
    Ok(Json(profile_public(&profile)))
}

async fn profile_update(State(state): Shared, Path(id): Path<String>, Json(body): Json<Value>) -> Reply {
    let request: ProfileUpdate = parse(body)?;
    let profile = state.core.llm.update_profile(&id, request).map_err(|e| store_error(e, PROFILE_NOT_FOUND, ""))?;
    Ok(Json(profile_public(&profile)))
}

async fn profile_delete(State(state): Shared, Path(id): Path<String>) -> Reply {
    state.core.llm.delete_profile(&id).map_err(|e| {
        store_error(e, PROFILE_NOT_FOUND, "该档案下包含默认优化模型，请先把默认模型切换到其他模型再删除")
    })?;
    Ok(Json(json!({ "status": "deleted", "profile_id": id })))
}

/// 验证已存档案：鉴权 + 尽力列取上游模型；结果持久化到档案状态。
async fn profile_verify(State(state): Shared, Path(id): Path<String>) -> Reply {
    let store = &state.core.llm;
    let profile = store.get_profile(&id).ok_or_else(|| detail(StatusCode::NOT_FOUND, PROFILE_NOT_FOUND))?;
    let endpoint = spec_of(&profile.provider)
        .and_then(|spec| {
            let key = store.decrypt_key(&profile)?;
            Endpoint::resolve(spec, profile.api_base.as_deref(), &key, &profile.extra_headers)
        });
    let outcome = match endpoint {
        Ok(endpoint) => gateway::verify_profile(&endpoint).await,
        Err(error) => VerifyOutcome::failed(&error),
    };
    store.set_profile_status(&id, VerifyStatus::from_outcome(&outcome));
    Ok(Json(outcome.to_json()))
}

/// 拉取上游模型列表；上游不支持列取时返回 409 + manual_only。
async fn remote_models(State(state): Shared, Path(id): Path<String>) -> Reply {
    let store = &state.core.llm;
    let profile = store.get_profile(&id).ok_or_else(|| detail(StatusCode::NOT_FOUND, PROFILE_NOT_FOUND))?;
    let manual_only = |reason: String| detail(StatusCode::CONFLICT, json!({ "manual_only": true, "reason": reason }));
    let spec = registry::get(&profile.provider)
        .filter(|spec| spec.supports_model_listing)
        .ok_or_else(|| manual_only("该服务商不提供 OpenAI 兼容的模型列表接口，请手动输入模型名".into()))?;
    let key = store.decrypt_key(&profile).map_err(|e| detail(StatusCode::BAD_REQUEST, e.user_message))?;
    let endpoint = Endpoint::resolve(spec, profile.api_base.as_deref(), &key, &profile.extra_headers)
        .map_err(|e| manual_only(e.message))?;
    let models = gateway::list_remote_models(&endpoint).await.map_err(manual_only)?;
    Ok(Json(json!({ "profile_id": id, "models": models })))
}

/// 保存前验证：草稿密钥只活在本次请求内存里，验证后即弃，不落任何文件。
async fn verify_draft(Json(body): Json<Value>) -> Reply {
    let request: DraftVerify = parse(body)?;
    let api_base = request.api_base.as_deref().map(str::trim).filter(|b| !b.is_empty());
    let outcome = async {
        let spec = spec_of(&request.provider)?;
        if spec.id == "custom" && api_base.is_none() {
            return Err(LlmError::plain(CODE_NETWORK, "自定义 OpenAI 兼容端点必须填写 API Base"));
        }
        let endpoint = Endpoint::resolve(spec, api_base, &request.api_key, &request.extra_headers)?;
        match request.model_name.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            Some(model_name) => {
                let model = registry::compose_model_string(spec.id, model_name, api_base)
                    .map_err(|e| LlmError::plain(CODE_MODEL_NOT_FOUND, e))?;
                Ok(gateway::verify_model(&endpoint, &model).await)
            }
            None => Ok(gateway::verify_profile(&endpoint).await),
        }
    }
    .await
    .unwrap_or_else(|error| VerifyOutcome::failed(&error));
    Ok(Json(outcome.to_json()))
}

// ==================== 模型条目 ====================

#[derive(Deserialize)]
struct ModelsQuery {
    profile_id: Option<String>,
}

async fn models_list(State(state): Shared, Query(query): Query<ModelsQuery>) -> Json<Value> {
    let models: Vec<Value> = state.core.llm.list_models(query.profile_id.as_deref()).iter().map(model_public).collect();
    Json(json!({ "models": models }))
}

#[derive(Deserialize)]
struct ModelsBatchCreate {
    items: Vec<ModelItemCreate>,
}

/// 批量创建（向导勾选多个模型一次提交）。
async fn models_create(State(state): Shared, Json(body): Json<Value>) -> Reply {
    let request: ModelsBatchCreate = parse(body)?;
    if request.items.is_empty() || request.items.len() > 50 {
        return Err(detail(StatusCode::UNPROCESSABLE_ENTITY, "一次需添加 1~50 个模型"));
    }
    let (created, skipped) = state.core.llm.create_models(request.items).map_err(|e| store_error(e, PROFILE_NOT_FOUND, ""))?;
    let created: Vec<Value> = created.iter().map(model_public).collect();
    Ok(Json(json!({ "created": created, "skipped": skipped })))
}

async fn model_update(State(state): Shared, Path(id): Path<String>, Json(body): Json<Value>) -> Reply {
    let request: ModelUpdate = parse(body)?;
    let entry = state.core.llm.update_model(&id, request).map_err(|e| store_error(e, MODEL_NOT_FOUND, ""))?;
    Ok(Json(model_public(&entry)))
}

async fn model_delete(State(state): Shared, Path(id): Path<String>) -> Reply {
    state
        .core
        .llm
        .delete_model(&id)
        .map_err(|e| store_error(e, MODEL_NOT_FOUND, "该模型是当前默认优化模型，请先切换默认模型再删除"))?;
    Ok(Json(json!({ "status": "deleted", "model_id": id })))
}

/// 单模型轻量验证（约 5 token 补全）；结果持久化到模型状态。
async fn model_verify(State(state): Shared, Path(id): Path<String>) -> Reply {
    let store = &state.core.llm;
    let target = store.resolve_runtime(Some(&id)).and_then(|target| {
        let spec = spec_of(&target.profile.provider)?;
        let endpoint = Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)?;
        Ok((endpoint, target.model.model))
    });
    let outcome = match target {
        Ok((endpoint, model)) => gateway::verify_model(&endpoint, &model).await,
        Err(error) => VerifyOutcome::failed(&error),
    };
    store.set_model_status(&id, VerifyStatus::from_outcome(&outcome));
    Ok(Json(outcome.to_json()))
}

/// 设为默认优化模型（即时生效）。
async fn model_activate(State(state): Shared, Path(id): Path<String>) -> Reply {
    state.core.llm.set_default_model(&id).map_err(|e| store_error(e, MODEL_NOT_FOUND, ""))?;
    Ok(Json(json!({ "status": "updated", "default_model_id": id })))
}

/// 更新默认/备用模型绑定；fallback 显式传 null 表示清除。
async fn settings_update(State(state): Shared, Json(body): Json<Value>) -> Reply {
    let store = &state.core.llm;
    if let Some(default_id) = body.get("default_model_id").and_then(Value::as_str) {
        store.set_default_model(default_id).map_err(|e| store_error(e, "默认模型不存在", ""))?;
    }
    if let Some(fallback) = body.get("fallback_model_id") {
        let fallback_id = match fallback {
            Value::Null => None,
            Value::String(id) => Some(id.as_str()),
            _ => return Err(detail(StatusCode::UNPROCESSABLE_ENTITY, "fallback_model_id 必须是字符串或 null")),
        };
        store.set_fallback_model(fallback_id).map_err(|e| store_error(e, "备用模型不存在", ""))?;
    }
    Ok(Json(json!({
        "status": "updated",
        "default_model_id": store.default_model_id(),
        "fallback_model_id": store.fallback_model_id(),
    })))
}

// ==================== 记录优化 ====================

/// 取出待优化的原文：记录不存在 404，没有原文 400（与 Python 版文案一致）。
fn optimizable_text(state: &ServerState, id: &str) -> Result<String, Response> {
    let record = state.core.records.get(id).map_err(|_| detail(StatusCode::NOT_FOUND, "Record not found"))?;
    record
        .raw_text
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| detail(StatusCode::BAD_REQUEST, "No text to optimize"))
}

fn done_event(id: &str, success: &OptimizeSuccess) -> Value {
    json!({
        "type": "done",
        "record_id": id,
        "optimized_text": success.optimized_text,
        "processing_time": success.processing_time,
        "tokens_used": success.tokens_used,
        "truncated": success.truncated,
        "model_string": success.model_string,
        "degraded": success.degraded,
    })
}

/// 非流式优化（旧调用方与前端降级路径）：失败 502 + 已脱敏原因，记录已复位。
async fn record_optimize(State(state): Shared, Path(id): Path<String>) -> Reply {
    let text = optimizable_text(&state, &id)?;
    match optimizer::optimize_record(&state.core, &id, &text, |_| {}).await {
        OptimizeOutcome::Completed(success) => Ok(Json(json!({
            "record_id": id,
            "original_text": text,
            "optimized_text": success.optimized_text,
            "processing_time": success.processing_time,
            "tokens_used": success.tokens_used,
            "truncated": success.truncated,
        }))),
        OptimizeOutcome::Failed(message) => Err(detail(StatusCode::BAD_GATEWAY, format!("文本优化失败：{message}"))),
        OptimizeOutcome::Cancelled => Err(detail(StatusCode::CONFLICT, "已取消优化")),
    }
}

/// 流式优化：NDJSON 逐行返回 start / delta / done / error。
///
/// 优化在独立任务里跑：客户端中途断开不影响结果落盘（Python 版断开即复位为 transcribed，
/// 这里让已经付费生成的结果照常保存，记录状态同样不会卡在 optimizing）。
async fn record_optimize_stream(State(state): Shared, Path(id): Path<String>) -> Result<Response, Response> {
    let text = optimizable_text(&state, &id)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let core = state.core.clone();
    tokio::spawn(async move {
        let _ = tx.send(json!({ "type": "start", "record_id": id, "original_text": text }));
        let delta_tx = tx.clone();
        let delta_id = id.clone();
        let outcome = optimizer::optimize_record(&core, &id, &text, move |event| {
            if let OptimizeEvent::Delta(chunk) = event {
                let _ = delta_tx.send(json!({ "type": "delta", "record_id": delta_id, "text": chunk }));
            }
        })
        .await;
        let terminal = match outcome {
            OptimizeOutcome::Completed(success) => done_event(&id, &success),
            // 取消也以 error 收尾：前端只在「没收到任何终止事件」时才回退非流式重跑
            OptimizeOutcome::Failed(message) => {
                json!({ "type": "error", "record_id": id, "error": message, "status": "transcribed", "raw_text": text })
            }
            OptimizeOutcome::Cancelled => {
                json!({ "type": "error", "record_id": id, "error": "已取消优化", "status": "transcribed", "raw_text": text, "cancelled": true })
            }
        };
        let _ = tx.send(terminal);
    });
    let lines = futures_util::stream::unfold(rx, |mut rx| async move {
        let event = rx.recv().await?;
        Some((Ok::<_, std::convert::Infallible>(format!("{event}\n")), rx))
    });
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/x-ndjson"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        axum::body::Body::from_stream(lines),
    )
        .into_response())
}

/// 取消该记录正在执行的优化（手动流式与录音后自动优化共用）。
async fn record_optimize_cancel(State(state): Shared, Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "record_id": id, "cancelled": state.core.optimizer.cancel(&id) }))
}
