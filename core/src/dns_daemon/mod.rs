// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Central pod discovery DNS daemon.
//!
//! Runs as a persistent host-side process. All running pods register their
//! name→IP mapping here via Unix socket IPC. Per-pod DNS servers forward
//! `*.pods.local` queries here instead of reading files, eliminating the
//! filesystem as an attack surface for pod discovery.
//!
//! ## Security properties
//! - Daemon binds only on `127.0.0.1` (not reachable from pod namespaces)
//! - Unix socket at `/var/lib/envpod/dns.sock` (host-only)
//! - In-memory registry — no file reads during query resolution
//! - `allow_pods` enforced centrally with full fleet context
//! - Fail-safe: daemon not running → NXDOMAIN for all `*.pods.local`
//!
//! ## Protocol
//! Newline-delimited JSON over Unix stream socket. Each connection handles
//! one request/response pair.
//!
//! Register:  `{"op":"register","name":"api","ip":"10.200.3.2","allow_discovery":true,"allow_pods":["client"]}`
//! Unregister:`{"op":"unregister","name":"api"}`
//! Lookup:    `{"op":"lookup","name":"api","from_pod":"client"}`
//!
//! Response:  `{"ok":true}` | `{"ip":"10.200.3.2"}` | `{"nxdomain":true}` | `{"error":"..."}`

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{watch, RwLock};

/// Default Unix socket path for the daemon.
pub const DAEMON_SOCK: &str = "/var/lib/envpod/dns.sock";

/// Default running pod registry directory (for persistence / crash recovery).
pub const RUNNING_DIR: &str = "/var/lib/envpod/running";

// ---------------------------------------------------------------------------
// Registry entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodEntry {
    pub ip: String,
    pub allow_discovery: bool,
    /// Pod names this pod may discover. `["*"]` = all discoverable pods.
    pub allow_pods: Vec<String>,
    /// PID of the pod's envpod process (for crash recovery GC).
    pub pid: u32,
}

// ---------------------------------------------------------------------------
// Protocol types (shared between server and client in this crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Register {
        name: String,
        ip: String,
        allow_discovery: bool,
        #[serde(default)]
        allow_pods: Vec<String>,
    },
    Unregister {
        name: String,
    },
    Lookup {
        name: String,
        from_pod: String,
    },
    /// Mutate discovery settings for a registered pod. Takes effect immediately.
    UpdateDiscovery {
        name: String,
        /// If present, overwrite allow_discovery.
        #[serde(skip_serializing_if = "Option::is_none")]
        allow_discovery: Option<bool>,
        /// Pod names to add to allow_pods.
        #[serde(default)]
        add_pods: Vec<String>,
        /// Pod names to remove from allow_pods. `["*"]` clears the entire list.
        #[serde(default)]
        remove_pods: Vec<String>,
    },
    /// Return the current discovery settings for a registered pod.
    QueryDiscovery {
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nxdomain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    // Populated only for QueryDiscovery / UpdateDiscovery responses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_discovery: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_pods: Option<Vec<String>>,
}

impl Response {
    fn ok() -> Self {
        Self { ok: Some(true), ip: None, nxdomain: None, error: None, allow_discovery: None, allow_pods: None }
    }
    fn ip(ip: String) -> Self {
        Self { ok: None, ip: Some(ip), nxdomain: None, error: None, allow_discovery: None, allow_pods: None }
    }
    fn nxdomain() -> Self {
        Self { ok: None, ip: None, nxdomain: Some(true), error: None, allow_discovery: None, allow_pods: None }
    }
    fn error(msg: impl Into<String>) -> Self {
        Self { ok: None, ip: None, nxdomain: None, error: Some(msg.into()), allow_discovery: None, allow_pods: None }
    }
    fn state(ip: &str, allow_discovery: bool, allow_pods: Vec<String>) -> Self {
        Self {
            ok: Some(true),
            ip: Some(ip.to_string()),
            nxdomain: None,
            error: None,
            allow_discovery: Some(allow_discovery),
            allow_pods: Some(allow_pods),
        }
    }
}

// ---------------------------------------------------------------------------
// Daemon server
// ---------------------------------------------------------------------------

/// Handle returned by `DnsDaemon::spawn()`.
pub struct DnsDaemonHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl DnsDaemonHandle {
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
    pub async fn join(self) {
        let _ = self.join_handle.await;
    }
}

/// Central pod discovery daemon.
pub struct DnsDaemon {
    sock_path: PathBuf,
    registry: Arc<RwLock<HashMap<String, PodEntry>>>,
}

impl DnsDaemon {
    /// Create a new daemon. Loads persisted state from the running directory.
    pub fn new(sock_path: PathBuf) -> Self {
        Self {
            sock_path,
            registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load persisted registry from disk (crash recovery). GCs stale entries.
    pub async fn load_persisted(&self) {
        let dir = Path::new(RUNNING_DIR);
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut registry = self.registry.write().await;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let Ok(data) = std::fs::read_to_string(&path) else {
                std::fs::remove_file(&path).ok();
                continue;
            };
            let Ok(pod) = serde_json::from_str::<PodEntry>(&data) else {
                std::fs::remove_file(&path).ok();
                continue;
            };
            // GC: remove if the registered process is no longer running
            if !Path::new(&format!("/proc/{}", pod.pid)).exists() {
                tracing::debug!("[dns-daemon] GC stale: {} (pid {} dead)", stem, pod.pid);
                std::fs::remove_file(&path).ok();
                continue;
            }
            tracing::debug!("[dns-daemon] restored: {} → {}", stem, pod.ip);
            registry.insert(stem.to_string(), pod);
        }
        tracing::info!("[dns-daemon] loaded {} pod(s) from persisted state", registry.len());
    }

    /// Auto-register pods that are already running when the daemon starts.
    ///
    /// Reads the pod store, finds pods whose `init_pid` is still alive, reads
    /// their `pod.yaml` for `allow_discovery` / `allow_pods`, and registers
    /// them in the in-memory registry. Only registers pods that are not already
    /// present (from `load_persisted`). Persists each entry so crash recovery
    /// works if the daemon restarts again.
    pub async fn load_from_store(&self, base_dir: &Path) {
        use crate::backend::native::state::NativeState;
        use crate::config::PodConfig;
        use crate::store::PodStore;

        let store = match PodStore::new(base_dir.join("state")) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("[dns-daemon] could not open pod store: {e}");
                return;
            }
        };

        let handles = match store.list() {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("[dns-daemon] could not list pods: {e}");
                return;
            }
        };

        let mut registry = self.registry.write().await;
        let mut count = 0;

        for handle in handles {
            // Skip already-registered pods (loaded from persisted files)
            if registry.contains_key(&handle.name) {
                continue;
            }

            // Parse native backend state
            let native = match NativeState::from_handle(&handle) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Must be running with a live PID
            let pid = match native.init_pid {
                Some(p) => p,
                None => continue,
            };
            if !Path::new(&format!("/proc/{pid}")).exists() {
                continue;
            }

            // Must have network state (IP assigned)
            let net = match &native.network {
                Some(n) => n,
                None => continue,
            };

            // Read pod config for discovery settings
            let config = PodConfig::from_file(&native.pod_dir.join("pod.yaml"))
                .unwrap_or_default();
            let allow_discovery = config.network.allow_discovery;
            let allow_pods = config.network.allow_pods.clone();

            // Skip pods that don't participate in discovery at all
            if !allow_discovery && allow_pods.is_empty() {
                continue;
            }

            let entry = PodEntry {
                ip: net.pod_ip.clone(),
                allow_discovery,
                allow_pods: allow_pods.clone(),
                pid,
            };

            // Persist so crash-recovery works on next daemon restart
            persist_entry(&handle.name, &entry).ok();

            tracing::info!(
                "[dns-daemon] auto-registered from store: {} → {} (discovery={}, allow_pods={:?})",
                handle.name, entry.ip, allow_discovery, allow_pods
            );
            registry.insert(handle.name.clone(), entry);
            count += 1;
        }

        if count > 0 {
            eprintln!("envpod-dns: auto-registered {count} already-running pod(s)");
        }
        tracing::info!("[dns-daemon] load_from_store: registered {count} pod(s)");
    }

    /// Spawn the daemon as a tokio task. Returns a handle for shutdown.
    pub async fn spawn(self) -> Result<DnsDaemonHandle> {
        // Remove stale socket file
        if self.sock_path.exists() {
            std::fs::remove_file(&self.sock_path).ok();
        }
        std::fs::create_dir_all(
            self.sock_path.parent().unwrap_or(Path::new("/")),
        ).context("create daemon socket directory")?;

        let listener = UnixListener::bind(&self.sock_path)
            .with_context(|| format!("bind daemon socket: {:?}", self.sock_path))?;

        // chmod 0600 — only root can connect
        std::fs::set_permissions(
            &self.sock_path,
            std::fs::Permissions::from_mode(0o600),
        ).ok();

        tracing::info!("[dns-daemon] listening on {:?}", self.sock_path);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let registry = self.registry.clone();

        let join_handle = tokio::spawn(Self::run_loop(listener, registry, shutdown_rx));

        Ok(DnsDaemonHandle { shutdown_tx, join_handle })
    }

    async fn run_loop(
        listener: UnixListener,
        registry: Arc<RwLock<HashMap<String, PodEntry>>>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let reg = registry.clone();
                            tokio::spawn(Self::handle_client(stream, reg));
                        }
                        Err(e) => {
                            tracing::warn!("[dns-daemon] accept error: {e}");
                        }
                    }
                }
                result = shutdown_rx.changed() => {
                    match result {
                        Ok(()) => tracing::debug!("[dns-daemon] shutdown signal"),
                        Err(_) => tracing::debug!("[dns-daemon] sender dropped"),
                    }
                    break;
                }
            }
        }
        tracing::debug!("[dns-daemon] run_loop exited");
    }

    async fn handle_client(
        stream: UnixStream,
        registry: Arc<RwLock<HashMap<String, PodEntry>>>,
    ) {
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();

        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }

        let response = match serde_json::from_str::<Request>(line.trim()) {
            Ok(req) => Self::dispatch(req, &registry).await,
            Err(e) => Response::error(format!("parse error: {e}")),
        };

        let mut out = serde_json::to_string(&response).unwrap_or_default();
        out.push('\n');
        write_half.write_all(out.as_bytes()).await.ok();
    }

    async fn dispatch(
        req: Request,
        registry: &Arc<RwLock<HashMap<String, PodEntry>>>,
    ) -> Response {
        match req {
            Request::Register { name, ip, allow_discovery, allow_pods } => {
                let entry = PodEntry {
                    ip: ip.clone(),
                    allow_discovery,
                    allow_pods: allow_pods.clone(),
                    pid: 0, // updated below
                };
                // Persist to disk (crash recovery only — never read during queries)
                let entry_with_pid = PodEntry {
                    pid: std::process::id(),
                    ..entry.clone()
                };
                let _ = persist_entry(&name, &entry_with_pid);

                let mut reg = registry.write().await;
                reg.insert(name.clone(), PodEntry { pid: std::process::id(), ..entry });
                tracing::info!("[dns-daemon] registered: {} → {} (discovery={}, allow_pods={:?})",
                    name, ip, allow_discovery, allow_pods);
                Response::ok()
            }

            Request::Unregister { name } => {
                let mut reg = registry.write().await;
                reg.remove(&name);
                remove_entry(&name);
                tracing::info!("[dns-daemon] unregistered: {}", name);
                Response::ok()
            }

            Request::UpdateDiscovery { name, allow_discovery, add_pods, remove_pods } => {
                let mut reg = registry.write().await;
                let Some(entry) = reg.get_mut(&name) else {
                    return Response::error(format!("pod '{name}' not registered"));
                };
                if let Some(v) = allow_discovery {
                    entry.allow_discovery = v;
                }
                if remove_pods.iter().any(|p| p == "*") {
                    entry.allow_pods.clear();
                } else {
                    entry.allow_pods.retain(|p| !remove_pods.contains(p));
                }
                for p in &add_pods {
                    if !entry.allow_pods.contains(p) {
                        entry.allow_pods.push(p.clone());
                    }
                }
                let entry = entry.clone();
                persist_entry(&name, &entry).ok();
                tracing::info!(
                    "[dns-daemon] updated: {} discovery={} allow_pods={:?}",
                    name, entry.allow_discovery, entry.allow_pods
                );
                Response::state(&entry.ip, entry.allow_discovery, entry.allow_pods)
            }

            Request::QueryDiscovery { name } => {
                let reg = registry.read().await;
                match reg.get(&name) {
                    Some(e) => Response::state(&e.ip, e.allow_discovery, e.allow_pods.clone()),
                    None => Response::error(format!("pod '{name}' not registered")),
                }
            }

            Request::Lookup { name, from_pod } => {
                let reg = registry.read().await;

                // Target must exist and have allow_discovery: true
                let target = match reg.get(&name) {
                    Some(e) if e.allow_discovery => e,
                    _ => {
                        tracing::debug!("[dns-daemon] lookup: {} from {} → NXDOMAIN (not discoverable)", name, from_pod);
                        return Response::nxdomain();
                    }
                };

                // Source must have name in its allow_pods list
                let allowed = match reg.get(&from_pod) {
                    Some(source) => {
                        source.allow_pods.iter().any(|p| p == &name || p == "*")
                    }
                    None => false, // unknown source pod = no permissions
                };

                if !allowed {
                    tracing::debug!("[dns-daemon] lookup: {} from {} → NXDOMAIN (not in allow_pods)", name, from_pod);
                    return Response::nxdomain();
                }

                tracing::info!("[dns-daemon] lookup: {} from {} → {}", name, from_pod, target.ip);
                Response::ip(target.ip.clone())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File persistence helpers (daemon-internal, crash recovery only)
// ---------------------------------------------------------------------------

fn persist_entry(name: &str, entry: &PodEntry) -> Result<()> {
    let dir = Path::new(RUNNING_DIR);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.json"));
    std::fs::write(path, serde_json::to_string(entry)?)?;
    Ok(())
}

fn remove_entry(name: &str) {
    let path = Path::new(RUNNING_DIR).join(format!("{name}.json"));
    std::fs::remove_file(path).ok();
}

// ---------------------------------------------------------------------------
// Daemon client (for cmd_run / cmd_destroy)
// ---------------------------------------------------------------------------

/// Client for communicating with the envpod-dns daemon.
pub struct DaemonClient {
    sock_path: PathBuf,
}

impl DaemonClient {
    pub fn new(sock_path: impl Into<PathBuf>) -> Self {
        Self { sock_path: sock_path.into() }
    }

    pub fn default_path() -> PathBuf {
        PathBuf::from(DAEMON_SOCK)
    }

    /// Register a pod with the daemon. All pods should register so the daemon
    /// knows their `allow_pods` list for bilateral policy enforcement.
    pub async fn register(
        &self,
        name: &str,
        ip: &str,
        allow_discovery: bool,
        allow_pods: &[String],
    ) -> Result<()> {
        let req = serde_json::json!({
            "op": "register",
            "name": name,
            "ip": ip,
            "allow_discovery": allow_discovery,
            "allow_pods": allow_pods,
        });
        let resp = self.send(req.to_string()).await?;
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
            anyhow::bail!("daemon register failed: {err}")
        }
    }

    /// Unregister a pod. Called on clean exit and on destroy.
    pub async fn unregister(&self, name: &str) -> Result<()> {
        let req = serde_json::json!({"op": "unregister", "name": name});
        self.send(req.to_string()).await?;
        Ok(())
    }

    /// Mutate discovery settings for a registered pod. Changes take effect immediately.
    /// Returns the pod's state after the update.
    pub async fn update_discovery(
        &self,
        name: &str,
        allow_discovery: Option<bool>,
        add_pods: &[String],
        remove_pods: &[String],
    ) -> Result<Response> {
        let req = serde_json::json!({
            "op": "update_discovery",
            "name": name,
            "allow_discovery": allow_discovery,
            "add_pods": add_pods,
            "remove_pods": remove_pods,
        });
        let val = self.send(req.to_string()).await?;
        serde_json::from_value(val).context("parse update_discovery response")
    }

    /// Query the current discovery settings for a registered pod.
    pub async fn query_discovery(&self, name: &str) -> Result<Response> {
        let req = serde_json::json!({"op": "query_discovery", "name": name});
        let val = self.send(req.to_string()).await?;
        serde_json::from_value(val).context("parse query_discovery response")
    }

    async fn send(&self, req: String) -> Result<serde_json::Value> {
        let mut stream = UnixStream::connect(&self.sock_path)
            .await
            .with_context(|| format!("connect to envpod-dns daemon: {:?}", self.sock_path))?;

        let mut payload = req;
        payload.push('\n');
        stream.write_all(payload.as_bytes()).await.context("send to daemon")?;

        let (read_half, _) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.context("read daemon response")?;

        serde_json::from_str(line.trim()).context("parse daemon response")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entry(ip: &str, allow_discovery: bool, allow_pods: &[&str]) -> PodEntry {
        PodEntry {
            ip: ip.into(),
            allow_discovery,
            allow_pods: allow_pods.iter().map(|s| s.to_string()).collect(),
            pid: std::process::id(),
        }
    }

    async fn registry_with(pods: &[(&str, PodEntry)]) -> Arc<RwLock<HashMap<String, PodEntry>>> {
        let reg = Arc::new(RwLock::new(HashMap::new()));
        let mut w = reg.write().await;
        for (name, entry) in pods {
            w.insert(name.to_string(), entry.clone());
        }
        drop(w);
        reg
    }

    #[tokio::test]
    async fn lookup_allowed() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
            ("client-pod", make_entry("10.200.2.2", false, &["api-pod"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert_eq!(resp.ip.as_deref(), Some("10.200.1.2"));
        assert!(resp.nxdomain.is_none());
    }

    #[tokio::test]
    async fn lookup_denied_not_in_allow_pods() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
            ("client-pod", make_entry("10.200.2.2", false, &[])), // allow_pods is empty
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true));
        assert!(resp.ip.is_none());
    }

    #[tokio::test]
    async fn lookup_denied_target_not_discoverable() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", false, &[])), // allow_discovery: false
            ("client-pod", make_entry("10.200.2.2", false, &["api-pod"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true));
    }

    #[tokio::test]
    async fn lookup_wildcard_allow_pods() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
            ("client-pod", make_entry("10.200.2.2", false, &["*"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert_eq!(resp.ip.as_deref(), Some("10.200.1.2"));
    }

    #[tokio::test]
    async fn lookup_unknown_source_pod() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
            // client-pod not registered at all
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true));
    }

    #[tokio::test]
    async fn lookup_unknown_target() {
        let reg = registry_with(&[
            ("client-pod", make_entry("10.200.2.2", false, &["*"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true));
    }

    #[tokio::test]
    async fn daemon_roundtrip_via_socket() {
        let dir = tempdir().unwrap();
        let sock_path = dir.path().join("dns.sock");

        let daemon = DnsDaemon::new(sock_path.clone());
        let handle = daemon.spawn().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = DaemonClient::new(&sock_path);

        // Register two pods
        client.register("api-pod", "10.200.1.2", true, &["client-pod".into()]).await.unwrap();
        client.register("client-pod", "10.200.2.2", false, &["api-pod".into()]).await.unwrap();

        // Lookup via daemon (using raw send to test lookup response)
        let req = serde_json::json!({"op":"lookup","name":"api-pod","from_pod":"client-pod"});
        let resp = client.send(req.to_string()).await.unwrap();
        assert_eq!(resp["ip"].as_str(), Some("10.200.1.2"));

        // Unregister
        client.unregister("api-pod").await.unwrap();
        let resp = client.send(req.to_string()).await.unwrap();
        assert_eq!(resp["nxdomain"].as_bool(), Some(true));

        handle.shutdown();
        handle.join().await;
    }

    // ---------------------------------------------------------------------------
    // UpdateDiscovery tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_discovery_enables_allow_discovery() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", false, &[])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: Some(true),
                add_pods: vec![],
                remove_pods: vec![],
            },
            &reg,
        ).await;
        assert!(resp.error.is_none(), "no error: {:?}", resp.error);
        assert_eq!(resp.allow_discovery, Some(true));
        assert_eq!(reg.read().await.get("api-pod").unwrap().allow_discovery, true);
    }

    #[tokio::test]
    async fn update_discovery_disables_allow_discovery() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: Some(false),
                add_pods: vec![],
                remove_pods: vec![],
            },
            &reg,
        ).await;
        assert_eq!(resp.allow_discovery, Some(false));
        assert_eq!(reg.read().await.get("api-pod").unwrap().allow_discovery, false);
    }

    #[tokio::test]
    async fn update_discovery_adds_pods_to_allow_list() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &[])),
        ]).await;

        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: None,
                add_pods: vec!["client-a".into(), "client-b".into()],
                remove_pods: vec![],
            },
            &reg,
        ).await;

        let reg_r = reg.read().await;
        let entry = reg_r.get("api-pod").unwrap();
        assert!(entry.allow_pods.contains(&"client-a".to_string()));
        assert!(entry.allow_pods.contains(&"client-b".to_string()));
    }

    #[tokio::test]
    async fn update_discovery_no_duplicates_on_add() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &["client-a"])),
        ]).await;

        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: None,
                add_pods: vec!["client-a".into()], // already present
                remove_pods: vec![],
            },
            &reg,
        ).await;

        let reg_r = reg.read().await;
        let count = reg_r.get("api-pod").unwrap().allow_pods.iter().filter(|p| *p == "client-a").count();
        assert_eq!(count, 1, "no duplicates when adding existing pod name");
    }

    #[tokio::test]
    async fn update_discovery_removes_specific_pod() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &["client-a", "client-b"])),
        ]).await;

        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: None,
                add_pods: vec![],
                remove_pods: vec!["client-a".into()],
            },
            &reg,
        ).await;

        let reg_r = reg.read().await;
        let pods = &reg_r.get("api-pod").unwrap().allow_pods;
        assert!(!pods.contains(&"client-a".to_string()), "client-a removed");
        assert!(pods.contains(&"client-b".to_string()), "client-b untouched");
    }

    #[tokio::test]
    async fn update_discovery_wildcard_remove_clears_all() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &["client-a", "client-b", "orchestrator"])),
        ]).await;

        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: None,
                add_pods: vec![],
                remove_pods: vec!["*".into()],
            },
            &reg,
        ).await;

        assert!(
            reg.read().await.get("api-pod").unwrap().allow_pods.is_empty(),
            "remove_pods=[\"*\"] clears the entire list"
        );
    }

    #[tokio::test]
    async fn update_discovery_unknown_pod_returns_error() {
        let reg = registry_with(&[]).await;

        let resp = DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "ghost-pod".into(),
                allow_discovery: Some(true),
                add_pods: vec![],
                remove_pods: vec![],
            },
            &reg,
        ).await;
        assert!(resp.error.is_some(), "error for unknown pod");
    }

    #[tokio::test]
    async fn update_discovery_none_fields_leave_existing_unchanged() {
        // allow_discovery: None should not change the existing value
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &["client-a"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "api-pod".into(),
                allow_discovery: None, // don't change
                add_pods: vec![],
                remove_pods: vec![],
            },
            &reg,
        ).await;
        assert_eq!(resp.allow_discovery, Some(true), "unchanged allow_discovery reflected in response");
        assert!(resp.allow_pods.as_ref().unwrap().contains(&"client-a".to_string()));
    }

    // After update, subsequent lookups reflect the new policy.
    // Bilateral: api-pod must have allow_discovery=true AND
    //            client-pod.allow_pods must contain "api-pod".
    #[tokio::test]
    async fn update_then_lookup_reflects_new_policy() {
        let reg = registry_with(&[
            ("api-pod",    make_entry("10.200.1.2", true, &[])),
            ("client-pod", make_entry("10.200.2.2", false, &[])), // allow_pods empty — cannot discover yet
        ]).await;

        // Before update: NXDOMAIN (client-pod.allow_pods doesn't include api-pod)
        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true), "NXDOMAIN before update");

        // Grant permission by adding "api-pod" to client-pod's own allow_pods list
        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "client-pod".into(),
                allow_discovery: None,
                add_pods: vec!["api-pod".into()],
                remove_pods: vec![],
            },
            &reg,
        ).await;

        // After update: client-pod.allow_pods = ["api-pod"] → resolves
        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert_eq!(resp.ip.as_deref(), Some("10.200.1.2"), "resolves after update");
    }

    // Revoke: removing api-pod from client-pod.allow_pods makes it NXDOMAIN again
    #[tokio::test]
    async fn update_revoke_lookup_returns_nxdomain() {
        let reg = registry_with(&[
            ("api-pod",    make_entry("10.200.1.2", true, &[])),
            ("client-pod", make_entry("10.200.2.2", false, &["api-pod"])), // currently has access
        ]).await;

        // Before revoke: resolves
        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert_eq!(resp.ip.as_deref(), Some("10.200.1.2"), "resolves before revoke");

        // Revoke: remove api-pod from client-pod's allow_pods
        DnsDaemon::dispatch(
            Request::UpdateDiscovery {
                name: "client-pod".into(),
                allow_discovery: None,
                add_pods: vec![],
                remove_pods: vec!["api-pod".into()],
            },
            &reg,
        ).await;

        // After revoke: NXDOMAIN
        let resp = DnsDaemon::dispatch(
            Request::Lookup { name: "api-pod".into(), from_pod: "client-pod".into() },
            &reg,
        ).await;
        assert!(resp.nxdomain == Some(true), "NXDOMAIN after revoke");
    }

    // ---------------------------------------------------------------------------
    // QueryDiscovery tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn query_discovery_returns_current_state() {
        let reg = registry_with(&[
            ("api-pod", make_entry("10.200.1.2", true, &["client-a", "client-b"])),
        ]).await;

        let resp = DnsDaemon::dispatch(
            Request::QueryDiscovery { name: "api-pod".into() },
            &reg,
        ).await;
        assert!(resp.error.is_none());
        assert_eq!(resp.ip.as_deref(), Some("10.200.1.2"));
        assert_eq!(resp.allow_discovery, Some(true));
        let pods = resp.allow_pods.unwrap();
        assert!(pods.contains(&"client-a".to_string()));
        assert!(pods.contains(&"client-b".to_string()));
    }

    #[tokio::test]
    async fn query_discovery_unknown_pod_returns_error() {
        let reg = registry_with(&[]).await;

        let resp = DnsDaemon::dispatch(
            Request::QueryDiscovery { name: "ghost-pod".into() },
            &reg,
        ).await;
        assert!(resp.error.is_some());
        assert!(resp.ip.is_none());
    }

    #[tokio::test]
    async fn query_and_update_roundtrip_via_socket() {
        let dir = tempdir().unwrap();
        let sock_path = dir.path().join("dns2.sock");

        let daemon = DnsDaemon::new(sock_path.clone());
        let handle = daemon.spawn().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = DaemonClient::new(&sock_path);

        // Register a pod
        client.register("api-pod", "10.200.1.2", false, &[]).await.unwrap();

        // Query: should be not discoverable, no allow_pods
        let q = client.query_discovery("api-pod").await.unwrap();
        assert_eq!(q.allow_discovery, Some(false));
        assert_eq!(q.allow_pods.as_deref(), Some([].as_slice()));

        // Update: enable discovery, add a pod
        let u = client.update_discovery("api-pod", Some(true), &["client-pod".into()], &[]).await.unwrap();
        assert_eq!(u.allow_discovery, Some(true));
        assert!(u.allow_pods.as_ref().unwrap().contains(&"client-pod".to_string()));

        // Query again: confirm live state
        let q2 = client.query_discovery("api-pod").await.unwrap();
        assert_eq!(q2.allow_discovery, Some(true));
        assert!(q2.allow_pods.as_ref().unwrap().contains(&"client-pod".to_string()));

        handle.shutdown();
        handle.join().await;
    }
}
