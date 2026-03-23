// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Remote control server — per-pod Unix domain socket for external tools to
//! freeze/resume/kill/restrict/query a running pod.
//!
//! Binds `{pod_dir}/control.sock` (mode 0600, owner only).
//! Line-based protocol: client sends command, server responds with JSON.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::watch;
use tracing;

use crate::audit::{AuditAction, AuditEntry, AuditLog};
use crate::backend::native::cgroup;
use crate::types::ResourceLimits;
use envpod_dns::resolver::DnsPolicy;

// ---------------------------------------------------------------------------
// Response type
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ControlResponse {
    fn success(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data: None,
        }
    }

    fn success_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data: Some(data),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            data: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Control server
// ---------------------------------------------------------------------------

/// Handle to a running control server (same pattern as DnsServerHandle).
pub struct ControlHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: tokio::task::JoinHandle<()>,
    socket_path: PathBuf,
}

impl ControlHandle {
    /// Signal the server to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the server task to complete.
    pub async fn join(self) {
        let _ = self.join_handle.await;
        // Clean up socket file
        std::fs::remove_file(&self.socket_path).ok();
    }

    /// Path to the control socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

/// Per-pod Unix domain socket server for remote control.
pub struct ControlServer {
    pod_dir: PathBuf,
    pod_name: String,
    cgroup_path: PathBuf,
    dns_policy: Option<Arc<RwLock<DnsPolicy>>>,
}

impl ControlServer {
    pub fn new(pod_dir: PathBuf, pod_name: String, cgroup_path: PathBuf) -> Self {
        Self {
            pod_dir,
            pod_name,
            cgroup_path,
            dns_policy: None,
        }
    }

    /// Create a control server with a shared DNS policy for live mutation.
    pub fn with_dns_policy(
        pod_dir: PathBuf,
        pod_name: String,
        cgroup_path: PathBuf,
        dns_policy: Arc<RwLock<DnsPolicy>>,
    ) -> Self {
        Self {
            pod_dir,
            pod_name,
            cgroup_path,
            dns_policy: Some(dns_policy),
        }
    }

    /// Spawn the control server as a background tokio task.
    pub async fn spawn(self) -> Result<ControlHandle> {
        let socket_path = self.pod_dir.join("control.sock");

        // Remove stale socket if it exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).ok();
        }

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("bind control socket: {}", socket_path.display()))?;

        // Set socket permissions to 0600 (owner only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&socket_path, perms)
                .with_context(|| "set control socket permissions")?;
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let socket_path_clone = socket_path.clone();

        let join_handle = tokio::spawn(async move {
            self.run(listener, shutdown_rx).await;
        });

        Ok(ControlHandle {
            shutdown_tx,
            join_handle,
            socket_path: socket_path_clone,
        })
    }

    async fn run(self, listener: UnixListener, mut shutdown_rx: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let pod_dir = self.pod_dir.clone();
                            let pod_name = self.pod_name.clone();
                            let cgroup_path = self.cgroup_path.clone();
                            let dns_policy = self.dns_policy.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(
                                    stream, &pod_dir, &pod_name, &cgroup_path, dns_policy.as_ref(),
                                ).await {
                                    tracing::warn!(
                                        pod = %pod_name,
                                        error = %e,
                                        "control connection error"
                                    );
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "control accept error");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::debug!(pod = %self.pod_name, "control server shutting down");
                        return;
                    }
                }
            }
        }
    }
}

/// Handle a single client connection (one command per line, one response per line).
async fn handle_connection(
    stream: tokio::net::UnixStream,
    pod_dir: &Path,
    pod_name: &str,
    cgroup_path: &Path,
    dns_policy: Option<&Arc<RwLock<DnsPolicy>>>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let response = process_command(&line, pod_dir, pod_name, cgroup_path, dns_policy);
        let json = serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"ok":false,"message":"serialization error"}"#.to_string()
        });

        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Parse and execute a single command.
fn process_command(
    line: &str,
    pod_dir: &Path,
    pod_name: &str,
    cgroup_path: &Path,
    dns_policy: Option<&Arc<RwLock<DnsPolicy>>>,
) -> ControlResponse {
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let payload = parts.get(1).map(|s| s.trim());

    match cmd.as_str() {
        "freeze" => cmd_freeze(pod_dir, pod_name, cgroup_path),
        "resume" => cmd_resume(pod_dir, pod_name, cgroup_path),
        "kill" => cmd_kill(pod_dir, pod_name, cgroup_path),
        "restrict" => match payload {
            Some(json) => cmd_restrict(pod_dir, pod_name, cgroup_path, json),
            None => ControlResponse::error("restrict requires JSON payload"),
        },
        "status" => cmd_status(cgroup_path),
        "alerts" => cmd_alerts(pod_dir),
        "dns-reload" => cmd_dns_reload(pod_dir, pod_name, dns_policy),
        _ => ControlResponse::error(format!("unknown command: {cmd}")),
    }
}

/// Reload DNS policy from persisted network-state.json and swap via write lock.
fn cmd_dns_reload(
    pod_dir: &Path,
    pod_name: &str,
    dns_policy: Option<&Arc<RwLock<DnsPolicy>>>,
) -> ControlResponse {
    let dns_policy = match dns_policy {
        Some(p) => p,
        None => return ControlResponse::error("DNS policy not available (no network isolation)"),
    };

    let net_state = match crate::backend::native::state::NetworkState::load(pod_dir) {
        Ok(Some(state)) => state,
        Ok(None) => return ControlResponse::error("network-state.json not found"),
        Err(e) => return ControlResponse::error(format!("load network state: {e}")),
    };

    let new_policy = build_dns_policy_from_state(&net_state);

    match dns_policy.write() {
        Ok(mut guard) => {
            *guard = new_policy;
            emit_audit(pod_dir, pod_name, AuditAction::DnsQuery, "dns-reload: policy updated");
            ControlResponse::success(format!(
                "DNS policy reloaded (allow: {}, deny: {})",
                net_state.dns_allow.len(),
                net_state.dns_deny.len(),
            ))
        }
        Err(e) => ControlResponse::error(format!("DNS policy lock poisoned: {e}")),
    }
}

/// Build a DnsPolicy from persisted NetworkState.
pub fn build_dns_policy_from_state(net: &crate::backend::native::state::NetworkState) -> DnsPolicy {
    use envpod_dns::resolver::DnsPolicyMode;

    let mode = match net.dns_mode.as_str() {
        "blacklist" | "denylist" => DnsPolicyMode::Denylist,
        "monitor" => DnsPolicyMode::Monitor,
        "whitelist" | "allowlist" | _ => DnsPolicyMode::Allowlist,
    };

    DnsPolicy {
        mode,
        allowed_domains: net.dns_allow.clone(),
        denied_domains: net.dns_deny.clone(),
        remap: net.dns_remap.clone(),
    }
}

fn cmd_freeze(pod_dir: &Path, pod_name: &str, cgroup_path: &Path) -> ControlResponse {
    match cgroup::freeze(cgroup_path) {
        Ok(()) => {
            emit_audit(pod_dir, pod_name, AuditAction::RemoteFreeze, "remote freeze");
            ControlResponse::success("pod frozen")
        }
        Err(e) => ControlResponse::error(format!("freeze failed: {e}")),
    }
}

fn cmd_resume(pod_dir: &Path, pod_name: &str, cgroup_path: &Path) -> ControlResponse {
    match cgroup::thaw(cgroup_path) {
        Ok(()) => {
            emit_audit(pod_dir, pod_name, AuditAction::RemoteResume, "remote resume");
            ControlResponse::success("pod resumed")
        }
        Err(e) => ControlResponse::error(format!("resume failed: {e}")),
    }
}

fn cmd_kill(pod_dir: &Path, pod_name: &str, cgroup_path: &Path) -> ControlResponse {
    // SIGTERM first, then SIGKILL
    use nix::sys::signal::Signal;

    if let Err(e) = cgroup::kill_all(cgroup_path, Signal::SIGTERM) {
        return ControlResponse::error(format!("SIGTERM failed: {e}"));
    }

    // Brief delay then SIGKILL
    std::thread::sleep(std::time::Duration::from_millis(500));

    if let Err(e) = cgroup::kill_all(cgroup_path, Signal::SIGKILL) {
        return ControlResponse::error(format!("SIGKILL failed: {e}"));
    }

    emit_audit(pod_dir, pod_name, AuditAction::RemoteKill, "remote kill");
    ControlResponse::success("pod killed (SIGTERM + SIGKILL)")
}

fn cmd_restrict(
    pod_dir: &Path,
    pod_name: &str,
    cgroup_path: &Path,
    json: &str,
) -> ControlResponse {
    let limits: ResourceLimits = match serde_json::from_str(json) {
        Ok(l) => l,
        Err(e) => return ControlResponse::error(format!("invalid JSON: {e}")),
    };

    match cgroup::set_limits(cgroup_path, &limits) {
        Ok(()) => {
            emit_audit(
                pod_dir,
                pod_name,
                AuditAction::RemoteRestrict,
                &format!("remote restrict: {json}"),
            );
            ControlResponse::success("limits applied")
        }
        Err(e) => ControlResponse::error(format!("set_limits failed: {e}")),
    }
}

fn cmd_status(cgroup_path: &Path) -> ControlResponse {
    match cgroup::read_usage(cgroup_path) {
        Ok(usage) => {
            let data = serde_json::to_value(&usage).unwrap_or(serde_json::Value::Null);
            ControlResponse::success_with_data("ok", data)
        }
        Err(e) => ControlResponse::error(format!("read_usage failed: {e}")),
    }
}

fn cmd_alerts(pod_dir: &Path) -> ControlResponse {
    let log = AuditLog::new(pod_dir);
    match log.read_all() {
        Ok(entries) => {
            let monitor_entries: Vec<&AuditEntry> = entries
                .iter()
                .filter(|e| matches!(
                    e.action,
                    AuditAction::MonitorAlert
                    | AuditAction::MonitorFreeze
                    | AuditAction::MonitorRestrict
                ))
                .collect();
            let data = serde_json::to_value(&monitor_entries).unwrap_or(serde_json::Value::Null);
            ControlResponse::success_with_data(
                format!("{} alert(s)", monitor_entries.len()),
                data,
            )
        }
        Err(e) => ControlResponse::error(format!("read audit log: {e}")),
    }
}

/// Emit an audit entry (best-effort).
fn emit_audit(pod_dir: &Path, pod_name: &str, action: AuditAction, detail: &str) {
    let log = AuditLog::new(pod_dir);
    log.append(&AuditEntry {
        timestamp: Utc::now(),
        pod_name: pod_name.into(),
        action,
        detail: detail.into(),
        success: true,
            agent: None,
    })
    .ok();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_response_serialization() {
        let resp = ControlResponse::success("test");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""message":"test""#));
        // data should be omitted when None
        assert!(!json.contains("data"));

        let resp = ControlResponse::success_with_data(
            "ok",
            serde_json::json!({"memory_bytes": 1024}),
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""data""#));
        assert!(json.contains(r#""memory_bytes":1024"#));
    }

    #[test]
    fn process_unknown_command() {
        let resp = process_command(
            "foobar",
            Path::new("/tmp/fake"),
            "test",
            Path::new("/tmp/fake-cgroup"),
            None,
        );
        assert!(!resp.ok);
        assert!(resp.message.contains("unknown command"));
    }

    #[test]
    fn process_restrict_missing_payload() {
        let resp = process_command(
            "restrict",
            Path::new("/tmp/fake"),
            "test",
            Path::new("/tmp/fake-cgroup"),
            None,
        );
        assert!(!resp.ok);
        assert!(resp.message.contains("requires JSON"));
    }

    #[test]
    fn process_restrict_invalid_json() {
        let resp = process_command(
            "restrict not-json",
            Path::new("/tmp/fake"),
            "test",
            Path::new("/tmp/fake-cgroup"),
            None,
        );
        assert!(!resp.ok);
        assert!(resp.message.contains("invalid JSON"));
    }

    #[test]
    fn dns_reload_without_policy_returns_error() {
        let resp = process_command(
            "dns-reload",
            Path::new("/tmp/fake"),
            "test",
            Path::new("/tmp/fake-cgroup"),
            None,
        );
        assert!(!resp.ok);
        assert!(resp.message.contains("not available"));
    }

    #[test]
    fn dns_reload_with_policy_and_state() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a network-state.json
        let net = crate::backend::native::state::NetworkState {
            netns_name: "test".into(),
            netns_path: PathBuf::from("/run/netns/test"),
            host_veth: "vh".into(),
            pod_veth: "vp".into(),
            host_ip: "10.200.1.1".into(),
            pod_ip: "10.200.1.2".into(),
            pod_index: 1,
            host_interface: "eth0".into(),
            dns_mode: "whitelist".into(),
            dns_allow: vec!["new-domain.com".into()],
            dns_deny: Vec::new(),
            dns_remap: std::collections::HashMap::new(),
            subnet_base: "10.200".into(),
        };
        net.save(tmp.path()).unwrap();

        // Create an initial policy
        let policy = Arc::new(RwLock::new(DnsPolicy {
            mode: envpod_dns::resolver::DnsPolicyMode::Allowlist,
            allowed_domains: vec!["old-domain.com".into()],
            denied_domains: Vec::new(),
            remap: std::collections::HashMap::new(),
        }));

        let resp = process_command(
            "dns-reload",
            tmp.path(),
            "test-pod",
            Path::new("/tmp/fake-cgroup"),
            Some(&policy),
        );
        assert!(resp.ok, "dns-reload should succeed: {}", resp.message);

        // Verify the policy was updated
        let guard = policy.read().unwrap();
        assert_eq!(guard.allowed_domains, vec!["new-domain.com"]);
    }

    #[tokio::test]
    async fn control_server_spawn_and_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ControlServer::new(
            tmp.path().to_path_buf(),
            "test-pod".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let handle = server.spawn().await.unwrap();
        assert!(handle.socket_path().exists());

        handle.shutdown();
        handle.join().await;
        // Socket should be cleaned up
        assert!(!tmp.path().join("control.sock").exists());
    }

    #[tokio::test]
    async fn control_server_unknown_command() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ControlServer::new(
            tmp.path().to_path_buf(),
            "test-pod".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let handle = server.spawn().await.unwrap();
        let socket_path = handle.socket_path().to_path_buf();

        // Connect and send command
        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        writer.write_all(b"whoami\n").await.unwrap();
        writer.flush().await.unwrap();

        let response_line = lines.next_line().await.unwrap().unwrap();
        let resp: ControlResponse = serde_json::from_str(&response_line).unwrap();
        assert!(!resp.ok);
        assert!(resp.message.contains("unknown command"));

        handle.shutdown();
        handle.join().await;
    }

    #[tokio::test]
    async fn control_server_alerts_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let server = ControlServer::new(
            tmp.path().to_path_buf(),
            "test-pod".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let handle = server.spawn().await.unwrap();
        let socket_path = handle.socket_path().to_path_buf();

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        writer.write_all(b"alerts\n").await.unwrap();
        writer.flush().await.unwrap();

        let response_line = lines.next_line().await.unwrap().unwrap();
        let resp: ControlResponse = serde_json::from_str(&response_line).unwrap();
        assert!(resp.ok);
        assert!(resp.message.contains("0 alert(s)"));

        handle.shutdown();
        handle.join().await;
    }
}
