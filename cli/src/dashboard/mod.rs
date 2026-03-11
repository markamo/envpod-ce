// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Web dashboard for envpod — fleet overview, pod detail, audit, diff.
//!
//! Single binary, embedded static assets. `envpod dashboard` starts an
//! axum server on localhost:9090. Use `--daemon` to run in the background.

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

/// PID file location for the dashboard daemon.
fn pid_file(base_dir: &std::path::Path) -> PathBuf {
    base_dir.join("dashboard.pid")
}

/// Stop a running dashboard daemon.
fn stop_daemon(base_dir: &std::path::Path) -> Result<()> {
    let pidfile = pid_file(base_dir);
    let pid_str = std::fs::read_to_string(&pidfile)
        .with_context(|| "no running dashboard daemon (PID file not found)")?;
    let pid: i32 = pid_str.trim().parse()
        .with_context(|| format!("invalid PID in {}", pidfile.display()))?;

    // Check if process is alive
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => {
            // Process exists — send SIGTERM
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            ).context("failed to send SIGTERM")?;
            eprintln!("dashboard daemon stopped (PID {pid})");
            let _ = std::fs::remove_file(&pidfile);
            Ok(())
        }
        Err(_) => {
            // Process doesn't exist — stale PID file
            let _ = std::fs::remove_file(&pidfile);
            anyhow::bail!("dashboard daemon not running (stale PID file removed)");
        }
    }
}

/// Start the dashboard web server.
pub async fn run(base_dir: PathBuf, port: u16, no_open: bool, daemon: bool, stop: bool) -> Result<()> {
    if stop {
        return stop_daemon(&base_dir);
    }

    if daemon {
        return start_daemon(&base_dir, port);
    }

    serve(base_dir, port, no_open).await
}

/// Spawn a detached background process running the dashboard.
fn start_daemon(base_dir: &std::path::Path, port: u16) -> Result<()> {
    let pidfile = pid_file(base_dir);

    // Check if already running
    if pidfile.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pidfile) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
                    eprintln!("dashboard daemon already running (PID {pid})");
                    return Ok(());
                }
            }
        }
        // Stale PID file
        let _ = std::fs::remove_file(&pidfile);
    }

    // Re-exec ourselves without --daemon to get a clean tokio runtime.
    // fork() inside tokio corrupts the child's inherited runtime threads.
    let exe = std::env::current_exe().context("cannot find own executable")?;
    let devnull = std::fs::File::open("/dev/null")?;

    let child = std::process::Command::new(exe)
        .args(["dashboard", "--no-open", "--port", &port.to_string(), "--dir", &base_dir.display().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(devnull.try_clone()?)
        .stderr(std::process::Stdio::from(devnull))
        .spawn()
        .context("failed to spawn dashboard daemon")?;

    let pid = child.id();
    std::fs::write(&pidfile, format!("{pid}"))
        .with_context(|| format!("write PID file {}", pidfile.display()))?;
    eprintln!("dashboard daemon started (PID {pid}, port {port})");
    eprintln!("  http://127.0.0.1:{port}");
    eprintln!("  stop with: envpod dashboard --stop");
    Ok(())
}

/// The actual server loop.
async fn serve(base_dir: PathBuf, port: u16, no_open: bool) -> Result<()> {
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
