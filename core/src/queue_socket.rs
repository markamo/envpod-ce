// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Unix domain socket server for the action queue.
//!
//! When `queue.socket: true` in pod.yaml, envpod pre-creates a socket at
//! `{pod_dir}/queue.sock` (host-side) and bind-mounts it into the pod at
//! `/run/envpod/queue.sock`.  Agents connect and exchange single-line JSON.
//! Multiple concurrent agent connections are handled via per-connection tasks.
//!
//! # Protocol (newline-delimited JSON, one request per connection)
//!
//! **List available actions:**
//! ```json
//! → {"type":"list_actions"}
//! ← {"actions":[{"name":"send_email","description":"...","tier":"staged","params":[...]}]}
//! ```
//!
//! **Call a catalog action (preferred — host-defined, param-validated):**
//! ```json
//! → {"type":"call","action":"send_email","params":{"to":"user@example.com","subject":"Hi"}}
//! ← {"id":"abc123ef","status":"queued","action":"send_email","message":"Waiting for approval..."}
//! ```
//!
//! **Free-form submit (weaker — agent controls description):**
//! ```json
//! → {"type":"submit","description":"charge customer $100","tier":"staged"}
//! ← {"id":"abc123ef","status":"queued","message":"Waiting for approval..."}
//! ```
//!
//! **Poll status:**
//! ```json
//! → {"type":"poll","id":"abc123ef"}
//! ← {"id":"abc123ef","status":"approved"}
//! ```
//!
//! Tiers: `"immediate"`, `"delayed"`, `"staged"` (default), `"blocked"`.
//! Statuses: `"queued"`, `"approved"`, `"executed"`, `"cancelled"`, `"blocked"`.
//!
//! # Security model
//! - `list_actions` is read-only, always safe.
//! - `call` validates params against the host-defined schema before queuing.
//! - `submit` (free-form) is rate-limited: max `SUBMIT_RATE_LIMIT` per minute.
//! - All request types are rate-limited per pod: max `GLOBAL_RATE_LIMIT` per minute.
//! - `queue.json` lives host-side — agent cannot write it.
//! - `actions.json` lives host-side — agent can only read via `list_actions`.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::actions::ActionCatalog;
use crate::audit::AuditAction;
use crate::queue::{ActionQueue, ActionStatus, ActionTier};

/// Maximum `submit` (free-form) requests per minute per pod.
const SUBMIT_RATE_LIMIT: u32 = 20;
/// Maximum total requests per minute per pod (all types combined).
const GLOBAL_RATE_LIMIT: u32 = 120;

// ---------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    /// List all actions from the host-defined catalog.
    ListActions,
    /// Call a catalog action with typed params. Preferred over Submit.
    Call {
        action: String,
        #[serde(default)]
        params: HashMap<String, serde_json::Value>,
    },
    /// Free-form action submission. Weaker: agent controls description.
    /// Rate-limited more aggressively than Call.
    Submit {
        description: String,
        #[serde(default = "default_tier_str")]
        tier: String,
    },
    /// Poll the status of a previously submitted action.
    Poll { id: String },
}

fn default_tier_str() -> String {
    "staged".to_string()
}

#[derive(Debug, Serialize)]
struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actions: Option<Vec<ActionListEntry>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionListEntry {
    pub name: String,
    pub description: String,
    pub tier: ActionTier,
    /// internal (overlay/filesystem/git) or external (HTTP/email/messaging)
    pub scope: crate::action_types::ActionScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<crate::action_types::ActionType>,
    pub params: Vec<crate::actions::ParamDef>,
}

// ---------------------------------------------------------------------------
// Rate limiter (token bucket, per pod)
// ---------------------------------------------------------------------------

struct RateLimiter {
    submit_tokens: u32,
    global_tokens: u32,
    last_refill: Instant,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            submit_tokens: SUBMIT_RATE_LIMIT,
            global_tokens: GLOBAL_RATE_LIMIT,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens if a minute has elapsed. Returns false if rate exceeded.
    fn check(&mut self, is_submit: bool) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_refill) >= Duration::from_secs(60) {
            self.submit_tokens = SUBMIT_RATE_LIMIT;
            self.global_tokens = GLOBAL_RATE_LIMIT;
            self.last_refill = now;
        }

        if self.global_tokens == 0 {
            return false;
        }
        self.global_tokens -= 1;

        if is_submit {
            if self.submit_tokens == 0 {
                return false;
            }
            self.submit_tokens -= 1;
        }

        true
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

pub struct QueueSocketServer {
    pod_dir: PathBuf,
    pod_name: String,
}

pub struct QueueSocketHandle {
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl QueueSocketHandle {
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub async fn join(self) {
        let _ = self.join_handle.await;
    }
}

impl QueueSocketServer {
    pub fn new(pod_dir: PathBuf, pod_name: String) -> Self {
        Self { pod_dir, pod_name }
    }

    /// Path of the socket file in the pod directory (host-side).
    pub fn socket_path(pod_dir: &Path) -> PathBuf {
        pod_dir.join("queue.sock")
    }

    /// Create and bind the UnixListener.  Must be called BEFORE the pod's mount
    /// namespace is set up so the socket file exists for the bind-mount.
    pub fn bind(pod_dir: &Path) -> Result<UnixListener> {
        let sock_path = Self::socket_path(pod_dir);
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path)?;
        // Agent user (UID 60000) needs write permission to connect to a Unix socket
        let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o777));
        Ok(listener)
    }

    /// Spawn the server with an already-bound listener (see [`bind`]).
    pub fn spawn_with_listener(self, listener: UnixListener) -> QueueSocketHandle {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        let pod_dir = Arc::new(self.pod_dir);
        let pod_name = Arc::new(self.pod_name);
        // Rate limiter shared across all connections to this pod's socket
        let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));

        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept = listener.accept() => {
                        match accept {
                            Ok((stream, _)) => {
                                let pod_dir = pod_dir.clone();
                                let pod_name = pod_name.clone();
                                let rate_limiter = rate_limiter.clone();
                                tokio::spawn(async move {
                                    handle_connection(stream, &pod_dir, &pod_name, rate_limiter).await;
                                });
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "queue socket accept error");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            return;
                        }
                    }
                }
            }
        });

        QueueSocketHandle { shutdown_tx, join_handle }
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

async fn handle_connection(
    stream: UnixStream,
    pod_dir: &Path,
    pod_name: &str,
    rate_limiter: Arc<Mutex<RateLimiter>>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    let line = match lines.next_line().await {
        Ok(Some(l)) => l,
        _ => return,
    };

    let response = dispatch_request(&line, pod_dir, pod_name, rate_limiter);

    let mut json = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"status":"error","message":"serialization failed"}"#.to_string()
    });
    json.push('\n');
    let _ = writer.write_all(json.as_bytes()).await;
}

fn dispatch_request(
    line: &str,
    pod_dir: &Path,
    pod_name: &str,
    rate_limiter: Arc<Mutex<RateLimiter>>,
) -> Response {
    match serde_json::from_str::<Request>(line) {
        // ------------------------------------------------------------------
        // list_actions — read catalog, global rate limit only
        // ------------------------------------------------------------------
        Ok(Request::ListActions) => {
            if !rate_limiter.lock().unwrap().check(false) {
                return rate_limit_response();
            }
            let catalog = ActionCatalog::new(pod_dir);
            match catalog.load() {
                Ok(defs) => {
                    let entries = defs
                        .into_iter()
                        .map(|d| ActionListEntry {
                            scope: d.scope(),
                            action_type: d.action_type.clone(),
                            params: d.effective_params(),
                            name: d.name,
                            description: d.description,
                            tier: d.tier,
                        })
                        .collect();
                    Response {
                        id: None,
                        status: "ok".to_string(),
                        action: None,
                        message: None,
                        actions: Some(entries),
                    }
                }
                Err(e) => error_response(e.to_string()),
            }
        }

        // ------------------------------------------------------------------
        // call — use catalog, validate params, queue with catalog tier
        // ------------------------------------------------------------------
        Ok(Request::Call { action, params }) => {
            // Rate limit (global only — Call is stronger than Submit)
            if !rate_limiter.lock().unwrap().check(false) {
                return rate_limit_response();
            }

            let catalog = ActionCatalog::new(pod_dir);
            let def = match catalog.validate_call(&action, &params) {
                Ok(d) => d,
                Err(e) => return error_response(e.to_string()),
            };

            let tier = def.tier;
            let description = format!(
                "call '{}' — {}",
                action,
                serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string())
            );
            let payload = serde_json::json!({
                "type": "action_call",
                "action": action,
                "action_type": def.action_type,
                "scope": def.scope(),
                "params": params,
                "config": def.config,
            });

            let queue = ActionQueue::new(pod_dir);
            match queue.submit_with_payload(tier, &description, payload) {
                Ok(entry) => {
                    ActionQueue::emit_audit(
                        pod_dir,
                        pod_name,
                        AuditAction::QueueSubmit,
                        &entry,
                    );
                    let message = match entry.status {
                        ActionStatus::Blocked => "Action blocked by policy".to_string(),
                        ActionStatus::Queued => format!(
                            "Queued — approve: envpod approve {} {}",
                            pod_name,
                            &entry.id.to_string()[..8]
                        ),
                        _ => entry.status.to_string(),
                    };
                    Response {
                        id: Some(entry.id.to_string()),
                        status: entry.status.to_string(),
                        action: Some(action),
                        message: Some(message),
                        actions: None,
                    }
                }
                Err(e) => error_response(e.to_string()),
            }
        }

        // ------------------------------------------------------------------
        // submit — free-form, agent controls description, rate-limited harder
        // ------------------------------------------------------------------
        Ok(Request::Submit { description, tier }) => {
            if !rate_limiter.lock().unwrap().check(true) {
                return rate_limit_response();
            }

            let tier_parsed = parse_tier_str(&tier);
            let queue = ActionQueue::new(pod_dir);
            match queue.submit(tier_parsed, &description) {
                Ok(entry) => {
                    ActionQueue::emit_audit(
                        pod_dir,
                        pod_name,
                        AuditAction::QueueSubmit,
                        &entry,
                    );
                    let message = match entry.status {
                        ActionStatus::Blocked => "Action blocked by policy".to_string(),
                        ActionStatus::Queued => format!(
                            "Queued — approve: envpod approve {} {}",
                            pod_name,
                            &entry.id.to_string()[..8]
                        ),
                        _ => format!("Immediate — proceed"),
                    };
                    Response {
                        id: Some(entry.id.to_string()),
                        status: entry.status.to_string(),
                        action: None,
                        message: Some(message),
                        actions: None,
                    }
                }
                Err(e) => error_response(e.to_string()),
            }
        }

        // ------------------------------------------------------------------
        // poll — check action status
        // ------------------------------------------------------------------
        Ok(Request::Poll { id }) => {
            let queue = ActionQueue::new(pod_dir);
            match queue.list(None) {
                Ok(actions) => {
                    let found = actions
                        .iter()
                        .find(|a| a.id.to_string() == id || a.id.to_string().starts_with(&id));
                    match found {
                        Some(entry) => {
                            let action_name = entry
                                .payload
                                .as_ref()
                                .and_then(|p| p.get("action"))
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                            Response {
                                id: Some(entry.id.to_string()),
                                status: entry.status.to_string(),
                                action: action_name,
                                message: None,
                                actions: None,
                            }
                        }
                        None => error_response(format!("action not found: {id}")),
                    }
                }
                Err(e) => error_response(e.to_string()),
            }
        }

        Err(e) => error_response(format!("invalid request: {e}")),
    }
}

fn error_response(message: String) -> Response {
    Response {
        id: None,
        status: "error".to_string(),
        action: None,
        message: Some(message),
        actions: None,
    }
}

fn rate_limit_response() -> Response {
    Response {
        id: None,
        status: "rate_limited".to_string(),
        action: None,
        message: Some("Too many requests — slow down".to_string()),
        actions: None,
    }
}

fn parse_tier_str(s: &str) -> ActionTier {
    match s.to_lowercase().as_str() {
        "immediate" | "immediate_protected" => ActionTier::ImmediateProtected,
        "delayed" => ActionTier::Delayed,
        "blocked" => ActionTier::Blocked,
        _ => ActionTier::Staged,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{ActionDef, ParamDef};

    fn make_rate_limiter() -> Arc<Mutex<RateLimiter>> {
        Arc::new(Mutex::new(RateLimiter::new()))
    }

    fn setup_catalog(pod_dir: &Path) {
        let catalog = ActionCatalog::new(pod_dir);
        catalog
            .upsert(ActionDef {
                name: "send_email".to_string(),
                description: "Send an email".to_string(),
                tier: ActionTier::Staged,
                params: vec![
                    ParamDef { name: "to".to_string(), description: None, required: true },
                    ParamDef { name: "subject".to_string(), description: None, required: true },
                ],
                ..Default::default()
            })
            .unwrap();
        catalog
            .upsert(ActionDef {
                name: "get_status".to_string(),
                description: "Get system status".to_string(),
                tier: ActionTier::ImmediateProtected,
                params: vec![],
                ..Default::default()
            })
            .unwrap();
    }

    #[test]
    fn list_actions_returns_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();
        let resp = dispatch_request(r#"{"type":"list_actions"}"#, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "ok");
        let actions = resp.actions.unwrap();
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().any(|a| a.name == "send_email"));
    }

    #[test]
    fn list_actions_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let rl = make_rate_limiter();
        let resp = dispatch_request(r#"{"type":"list_actions"}"#, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.actions.unwrap().len(), 0);
    }

    #[test]
    fn call_valid_action_queued() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();
        let line = r#"{"type":"call","action":"send_email","params":{"to":"a@b.com","subject":"Hi"}}"#;
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "queued");
        assert!(resp.id.is_some());
        assert_eq!(resp.action.as_deref(), Some("send_email"));

        // Verify it's in queue with action_call payload
        let queue = ActionQueue::new(tmp.path());
        let actions = queue.list(None).unwrap();
        assert_eq!(actions.len(), 1);
        let payload = actions[0].payload.as_ref().unwrap();
        assert_eq!(payload["type"], "action_call");
        assert_eq!(payload["action"], "send_email");
    }

    #[test]
    fn call_missing_required_param_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();
        // Missing "subject"
        let line = r#"{"type":"call","action":"send_email","params":{"to":"a@b.com"}}"#;
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "error");
        assert!(resp.message.unwrap().contains("missing required param"));
    }

    #[test]
    fn call_unknown_action_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();
        let line = r#"{"type":"call","action":"nonexistent","params":{}}"#;
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "error");
        assert!(resp.message.unwrap().contains("action not found"));
    }

    #[test]
    fn call_unknown_param_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();
        let line = r#"{"type":"call","action":"send_email","params":{"to":"a@b.com","subject":"Hi","extra":"bad"}}"#;
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "error");
        assert!(resp.message.unwrap().contains("unknown param"));
    }

    #[test]
    fn submit_free_form_queued() {
        let tmp = tempfile::tempdir().unwrap();
        let rl = make_rate_limiter();
        let line = r#"{"type":"submit","description":"my free-form task","tier":"staged"}"#;
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "queued");
        assert!(resp.id.is_some());
    }

    #[test]
    fn poll_found_after_call() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();

        let call_line = r#"{"type":"call","action":"send_email","params":{"to":"a@b.com","subject":"Hi"}}"#;
        let call_resp = dispatch_request(call_line, tmp.path(), "pod", rl.clone());
        let id = call_resp.id.unwrap();

        let poll_line = format!(r#"{{"type":"poll","id":"{id}"}}"#);
        let poll_resp = dispatch_request(&poll_line, tmp.path(), "pod", rl);
        assert_eq!(poll_resp.status, "queued");
        assert_eq!(poll_resp.action.as_deref(), Some("send_email"));
    }

    #[test]
    fn poll_prefix_match() {
        let tmp = tempfile::tempdir().unwrap();
        setup_catalog(tmp.path());
        let rl = make_rate_limiter();

        let call_line = r#"{"type":"call","action":"send_email","params":{"to":"a@b.com","subject":"Hi"}}"#;
        let call_resp = dispatch_request(call_line, tmp.path(), "pod", rl.clone());
        let prefix = &call_resp.id.unwrap()[..8];

        let poll_line = format!(r#"{{"type":"poll","id":"{prefix}"}}"#);
        let poll_resp = dispatch_request(&poll_line, tmp.path(), "pod", rl);
        assert_eq!(poll_resp.status, "queued");
    }

    #[test]
    fn submit_rate_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let rl = Arc::new(Mutex::new(RateLimiter {
            submit_tokens: 1,
            global_tokens: GLOBAL_RATE_LIMIT,
            last_refill: Instant::now(),
        }));

        let line = r#"{"type":"submit","description":"task","tier":"staged"}"#;
        dispatch_request(line, tmp.path(), "pod", rl.clone()); // uses the token
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "rate_limited");
    }

    #[test]
    fn global_rate_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let rl = Arc::new(Mutex::new(RateLimiter {
            submit_tokens: SUBMIT_RATE_LIMIT,
            global_tokens: 1,
            last_refill: Instant::now(),
        }));

        let line = r#"{"type":"list_actions"}"#;
        dispatch_request(line, tmp.path(), "pod", rl.clone()); // uses global token
        let resp = dispatch_request(line, tmp.path(), "pod", rl);
        assert_eq!(resp.status, "rate_limited");
    }

    #[test]
    fn invalid_json_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let rl = make_rate_limiter();
        let resp = dispatch_request("not json", tmp.path(), "pod", rl);
        assert_eq!(resp.status, "error");
    }

    #[test]
    fn parse_tier_str_all_variants() {
        assert_eq!(parse_tier_str("staged"), ActionTier::Staged);
        assert_eq!(parse_tier_str("delayed"), ActionTier::Delayed);
        assert_eq!(parse_tier_str("immediate"), ActionTier::ImmediateProtected);
        assert_eq!(parse_tier_str("blocked"), ActionTier::Blocked);
        assert_eq!(parse_tier_str("anything_else"), ActionTier::Staged);
    }
}
