// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! REST API handlers for the dashboard.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::Json as BodyJson;
use serde::{Deserialize, Serialize};

use envpod_core::audit::{AuditAction, AuditEntry, AuditLog};
use envpod_core::backend::create_backend;
use envpod_core::backend::native::state::NativeState;
use envpod_core::queue::ActionQueue;
use envpod_core::snapshot::SnapshotStore;
use envpod_core::store::PodStore;

use super::state as pod_state;

/// Shared application state.
pub struct AppState {
    pub store: PodStore,
    pub base_dir: PathBuf,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct FileDiffParams {
    pub path: String,
}

#[derive(Deserialize)]
pub struct CommitFilesBody {
    pub paths: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateSnapshotBody {
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct PromoteSnapshotBody {
    pub base_name: String,
}

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { error: msg.into() }))
}

/// GET /api/v1/pods
pub async fn list_pods(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<pod_state::PodSummary>>, (StatusCode, Json<ApiError>)> {
    pod_state::list_pods(&state.store, &state.base_dir)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))
}

/// GET /api/v1/pods/:id
pub async fn pod_detail(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<pod_state::PodDetail>, (StatusCode, Json<ApiError>)> {
    pod_state::pod_detail(&state.store, &state.base_dir, &name)
        .map(Json)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))
}

/// GET /api/v1/pods/:id/audit
pub async fn pod_audit(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);

    pod_state::read_audit(&state.store, &name, offset, limit)
        .map(|(entries, total)| {
            Json(serde_json::json!({
                "entries": entries,
                "total": total,
                "offset": offset,
                "limit": limit,
            }))
        })
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))
}

/// GET /api/v1/pods/:id/resources
pub async fn pod_resources(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let resources = pod_state::read_resources(&native_state);
    Ok(Json(serde_json::to_value(resources).unwrap_or_default()))
}

/// GET /api/v1/pods/:id/diff
pub async fn pod_diff(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Vec<pod_state::DiffEntry>>, (StatusCode, Json<ApiError>)> {
    pod_state::read_diff(&state.store, &state.base_dir, &name)
        .map(Json)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))
}

/// POST /api/v1/pods/:id/commit
pub async fn pod_commit(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (handle, native_state) = pod_state::get_state(&state.store, &name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;

    let backend = create_backend(&handle.backend, &state.base_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    backend.commit(&handle, None, None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    // Audit
    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::Commit,
        detail: "via dashboard".into(),
        success: true,
    });

    Ok(Json(serde_json::json!({ "status": "committed", "pod": name })))
}

/// POST /api/v1/pods/:id/rollback
pub async fn pod_rollback(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (handle, native_state) = pod_state::get_state(&state.store, &name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;

    let backend = create_backend(&handle.backend, &state.base_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    backend.rollback(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::Rollback,
        detail: "via dashboard".into(),
        success: true,
    });

    Ok(Json(serde_json::json!({ "status": "rolled back", "pod": name })))
}

/// POST /api/v1/pods/:id/freeze
pub async fn pod_freeze(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (handle, native_state) = pod_state::get_state(&state.store, &name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;

    let backend = create_backend(&handle.backend, &state.base_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    backend.freeze(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::Freeze,
        detail: "via dashboard".into(),
        success: true,
    });

    Ok(Json(serde_json::json!({ "status": "frozen", "pod": name })))
}

/// GET /api/v1/pods/:id/file-diff?path=<encoded_path>
pub async fn pod_file_diff(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<FileDiffParams>,
) -> Result<Json<pod_state::FileDiffResult>, (StatusCode, Json<ApiError>)> {
    pod_state::read_file_diff(&state.store, &state.base_dir, &name, &params.path)
        .map(Json)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))
}

/// POST /api/v1/pods/:id/commit-files
/// Body: { "paths": ["/path/one", "/path/two"] }
pub async fn pod_commit_files(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    BodyJson(body): BodyJson<CommitFilesBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (handle, native_state) = pod_state::get_state(&state.store, &name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;

    let backend = create_backend(&handle.backend, &state.base_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let paths: Vec<std::path::PathBuf> = body.paths.iter().map(std::path::PathBuf::from).collect();

    backend.commit(&handle, Some(&paths), None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let log = AuditLog::new(&native_state.pod_dir);
    let detail = format!("via dashboard: {} file(s)", paths.len());
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::Commit,
        detail,
        success: true,
    });

    Ok(Json(serde_json::json!({ "status": "committed", "pod": name, "count": paths.len() })))
}

/// POST /api/v1/pods/:id/resume
pub async fn pod_resume(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (handle, native_state) = pod_state::get_state(&state.store, &name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;

    let backend = create_backend(&handle.backend, &state.base_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    backend.resume(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::Resume,
        detail: "via dashboard".into(),
        success: true,
    });

    Ok(Json(serde_json::json!({ "status": "resumed", "pod": name })))
}

/// GET /api/v1/pods/:id/snapshots
pub async fn pod_snapshots(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let snap_store = SnapshotStore::new(&native_state.pod_dir);
    let snapshots = snap_store.list()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(serde_json::to_value(&snapshots).unwrap_or_default()))
}

/// POST /api/v1/pods/:id/snapshots
/// Body: { "name": "optional label" }
pub async fn pod_snapshot_create(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    BodyJson(body): BodyJson<CreateSnapshotBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let snap_store = SnapshotStore::new(&native_state.pod_dir);
    let upper_dir = native_state.pod_dir.join("upper");
    let snap = snap_store.create(&upper_dir, body.name.as_deref(), false)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({ "status": "created", "id": snap.id, "pod": name })))
}

/// POST /api/v1/pods/:id/snapshots/:snap_id/restore
pub async fn pod_snapshot_restore(
    State(state): State<Arc<AppState>>,
    Path((name, snap_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let snap_store = SnapshotStore::new(&native_state.pod_dir);
    let upper_dir = native_state.pod_dir.join("upper");
    snap_store.restore(&upper_dir, &snap_id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({ "status": "restored", "id": snap_id, "pod": name })))
}

/// POST /api/v1/pods/:id/snapshots/:snap_id/promote
/// Body: { "base_name": "my-base" }
pub async fn pod_snapshot_promote(
    State(state): State<Arc<AppState>>,
    Path((name, snap_id)): Path<(String, String)>,
    BodyJson(body): BodyJson<PromoteSnapshotBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    use envpod_core::backend::native::has_base;

    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let bases_dir = state.base_dir.join("bases");
    if has_base(&bases_dir, &body.base_name) {
        return Err(err(StatusCode::CONFLICT, format!("base '{}' already exists", body.base_name)));
    }

    let snap_store = SnapshotStore::new(&native_state.pod_dir);
    let snap = snap_store.get(&snap_id)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let snap_dir = native_state.pod_dir.join("snapshots").join(&snap.id);

    let base_pod_dir = bases_dir.join(&body.base_name);
    std::fs::create_dir_all(&base_pod_dir)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("create base dir: {e:#}")))?;

    // Symlink rootfs
    let pod_rootfs = native_state.pod_dir.join("rootfs");
    let canonical_rootfs = std::fs::canonicalize(&pod_rootfs).unwrap_or(pod_rootfs);
    std::os::unix::fs::symlink(&canonical_rootfs, base_pod_dir.join("rootfs"))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("symlink rootfs: {e:#}")))?;

    // Copy snapshot → base_upper
    let status = std::process::Command::new("cp")
        .args(["--reflink=auto", "-a", "--",
               &snap_dir.to_string_lossy(),
               &base_pod_dir.join("base_upper").to_string_lossy()])
        .status()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("cp: {e:#}")))?;
    if !status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "cp snapshot → base_upper failed"));
    }

    // Copy pod.yaml
    let pod_yaml = native_state.pod_dir.join("pod.yaml");
    if pod_yaml.exists() {
        let _ = std::fs::copy(&pod_yaml, base_pod_dir.join("pod.yaml"));
    }

    Ok(Json(serde_json::json!({
        "status": "promoted",
        "snapshot_id": snap.id,
        "base_name": body.base_name,
        "pod": name,
    })))
}

/// DELETE /api/v1/pods/:id/snapshots/:snap_id
pub async fn pod_snapshot_destroy(
    State(state): State<Arc<AppState>>,
    Path((name, snap_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let snap_store = SnapshotStore::new(&native_state.pod_dir);
    let meta = snap_store.destroy(&snap_id)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({ "status": "deleted", "id": meta.id, "pod": name })))
}

/// GET /api/v1/pods/:id/queue
pub async fn pod_queue(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let queue = ActionQueue::new(&native_state.pod_dir);
    let actions = queue.list(None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(serde_json::to_value(&actions).unwrap_or_default()))
}

/// POST /api/v1/pods/:id/queue/:action_id/approve
pub async fn pod_queue_approve(
    State(state): State<Arc<AppState>>,
    Path((name, action_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let queue = ActionQueue::new(&native_state.pod_dir);
    let actions = queue.list(None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let action = actions.iter()
        .find(|a| a.id.to_string().starts_with(&action_id))
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("action '{action_id}' not found")))?;
    let approved = queue.approve(action.id)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::QueueApprove,
        detail: format!("id={} via dashboard", &approved.id.to_string()[..8]),
        success: true,
    });
    Ok(Json(serde_json::json!({ "status": "approved", "id": approved.id, "pod": name })))
}

/// POST /api/v1/pods/:id/queue/:action_id/cancel
pub async fn pod_queue_cancel(
    State(state): State<Arc<AppState>>,
    Path((name, action_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let handle = state.store.load(&name)
        .map_err(|e| err(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let native_state = NativeState::from_handle(&handle)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let queue = ActionQueue::new(&native_state.pod_dir);
    let actions = queue.list(None)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    let action = actions.iter()
        .find(|a| a.id.to_string().starts_with(&action_id))
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("action '{action_id}' not found")))?;
    let cancelled = queue.cancel(action.id)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    let log = AuditLog::new(&native_state.pod_dir);
    let _ = log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.clone(),
        action: AuditAction::QueueCancel,
        detail: format!("id={} via dashboard", &cancelled.id.to_string()[..8]),
        success: true,
    });
    Ok(Json(serde_json::json!({ "status": "cancelled", "id": cancelled.id, "pod": name })))
}
