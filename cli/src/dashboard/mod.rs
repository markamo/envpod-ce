// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Web dashboard for envpod — fleet overview, pod detail, audit, diff.
//!
//! Single binary, embedded static assets. `envpod dashboard` starts an
//! axum server on localhost:9090.

pub mod api;
pub mod state;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use rust_embed::Embed;
use tower_http::cors::CorsLayer;

use envpod_core::store::PodStore;

use api::AppState;

/// Static assets embedded in the binary at compile time.
#[derive(Embed)]
#[folder = "src/dashboard/static/"]
struct Assets;

/// Start the dashboard web server.
pub async fn run(base_dir: PathBuf, port: u16, no_open: bool) -> Result<()> {
    let store = PodStore::new(base_dir.join("state"))?;
    let app_state = Arc::new(AppState { store, base_dir });

    let app = Router::new()
        // API routes
        .route("/api/v1/pods", get(api::list_pods))
        .route("/api/v1/pods/{id}", get(api::pod_detail))
        .route("/api/v1/pods/{id}/audit", get(api::pod_audit))
        .route("/api/v1/pods/{id}/resources", get(api::pod_resources))
        .route("/api/v1/pods/{id}/diff", get(api::pod_diff))
        .route("/api/v1/pods/{id}/file-diff", get(api::pod_file_diff))
        .route("/api/v1/pods/{id}/commit", post(api::pod_commit))
        .route("/api/v1/pods/{id}/commit-files", post(api::pod_commit_files))
        .route("/api/v1/pods/{id}/rollback", post(api::pod_rollback))
        .route("/api/v1/pods/{id}/freeze", post(api::pod_freeze))
        .route("/api/v1/pods/{id}/resume", post(api::pod_resume))
        .route("/api/v1/pods/{id}/snapshots", get(api::pod_snapshots).post(api::pod_snapshot_create))
        .route("/api/v1/pods/{id}/snapshots/{snap_id}/restore", post(api::pod_snapshot_restore))
        .route("/api/v1/pods/{id}/snapshots/{snap_id}/promote", post(api::pod_snapshot_promote))
        .route("/api/v1/pods/{id}/snapshots/{snap_id}", delete(api::pod_snapshot_destroy))
        .route("/api/v1/pods/{id}/queue", get(api::pod_queue))
        .route("/api/v1/pods/{id}/queue/{action_id}/approve", post(api::pod_queue_approve))
        .route("/api/v1/pods/{id}/queue/{action_id}/cancel", post(api::pod_queue_cancel))
        // Static assets
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind dashboard on {addr}"))?;

    let url = format!("http://{addr}");
    eprintln!("envpod dashboard running at {url}");

    if !no_open {
        let _ = open::that(&url);
    }

    axum::serve(listener, app)
        .await
        .context("dashboard server error")?;

    Ok(())
}

/// Serve embedded static assets.
async fn static_handler(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');

    // Default to index.html
    let path = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path
    };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data.to_vec(),
            )
                .into_response()
        }
        None => {
            // Try index.html for SPA routing
            match Assets::get("index.html") {
                Some(content) => {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "text/html")],
                        content.data.to_vec(),
                    )
                        .into_response()
                }
                None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
            }
        }
    }
}
