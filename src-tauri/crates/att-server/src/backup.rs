//! 数据备份 / 恢复路由（设置 → 诊断 → 本地文件）：
//! - `POST /api/backup/export`：把白名单数据打包到用户选定的 zip 路径
//! - `POST /api/backup/import`：校验并覆盖导入备份，返回导入前快照路径与重启提示
//!
//! 路径来自桌面端系统对话框；HTTP 层只要求非空绝对路径，实际读写都在阻塞线程执行。

use crate::{blocking, err, ServerState};
use att_core::backup::{self, BackupError, ExportOptions};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

type Shared = State<Arc<ServerState>>;

pub fn routes() -> Router<Arc<ServerState>> {
    Router::new().route("/api/backup/export", post(export)).route("/api/backup/import", post(import))
}

#[derive(Debug, Deserialize)]
struct ExportRequest {
    path: String,
    #[serde(default)]
    include_key: bool,
    #[serde(default = "default_true")]
    include_audio: bool,
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    path: String,
}

fn default_true() -> bool {
    true
}

fn checked_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("备份路径不能为空".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("备份路径必须是绝对路径".into());
    }
    Ok(path)
}

fn map_error(error: BackupError, importing: bool) -> Response {
    let status = match &error {
        BackupError::Invalid(_) => StatusCode::BAD_REQUEST,
        BackupError::Zip(_) if importing => StatusCode::BAD_REQUEST,
        BackupError::Zip(_) => StatusCode::INTERNAL_SERVER_ERROR,
        BackupError::Io(_) | BackupError::RolledBack { .. } | BackupError::Rollback { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    err(status, error.to_string()).into_response()
}

async fn export(State(state): Shared, Json(body): Json<ExportRequest>) -> Response {
    let destination = match checked_path(&body.path) {
        Ok(path) => path,
        Err(message) => return err(StatusCode::BAD_REQUEST, message).into_response(),
    };
    if !state.core.backup_paths.consume_export(&destination) {
        return err(StatusCode::BAD_REQUEST, "备份保存路径未经系统对话框授权，请重新选择".into()).into_response();
    }
    if !state.core.begin_backup_maintenance() {
        return err(StatusCode::CONFLICT, "另一个备份或恢复任务正在进行".into()).into_response();
    }
    let data_dir = state.core.data_dir.clone();
    let app_version = state.core.app_version.clone();
    let options = ExportOptions { include_key: body.include_key, include_audio: body.include_audio };
    let Some(result) = blocking(move || backup::export_backup(&data_dir, &app_version, &destination, options)).await
    else {
        state.core.end_backup_maintenance();
        return err(StatusCode::INTERNAL_SERVER_ERROR, "备份任务异常退出".into()).into_response();
    };
    state.core.end_backup_maintenance();
    match result {
        Ok(result) => Json(json!(result)).into_response(),
        Err(error) => map_error(error, false),
    }
}

async fn import(State(state): Shared, Json(body): Json<ImportRequest>) -> Response {
    let source = match checked_path(&body.path) {
        Ok(path) => path,
        Err(message) => return err(StatusCode::BAD_REQUEST, message).into_response(),
    };
    if !state.core.backup_paths.consume_import(&source) {
        return err(StatusCode::BAD_REQUEST, "备份文件未经系统对话框授权，请重新选择".into()).into_response();
    }
    if !state.core.begin_backup_maintenance() {
        return err(StatusCode::CONFLICT, "另一个备份或恢复任务正在进行".into()).into_response();
    }
    let data_dir = state.core.data_dir.clone();
    let crypto = state.core.crypto.clone();
    let Some(result) = blocking(move || backup::import_backup(&data_dir, &source, Some(crypto.as_ref()))).await else {
        state.core.end_backup_maintenance();
        return err(StatusCode::INTERNAL_SERVER_ERROR, "恢复任务异常退出".into()).into_response();
    };
    match result {
        Ok(result) => Json(json!({
            "status": "imported",
            "restart_required": result.restart_required,
            "pre_import_backup": result.pre_import_backup,
            "credentials_reset": result.credentials_reset,
            "manifest": result.manifest,
        }))
        .into_response(),
        Err(error) => {
            state.core.end_backup_maintenance();
            map_error(error, true)
        }
    }
}
