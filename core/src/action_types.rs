// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Built-in action types with fixed schemas and envpod-side executors.
//!
//! Every action type has:
//! - A fixed parameter schema (required + optional params).
//! - A default reversibility tier.
//! - A category (HTTP, Filesystem, Git).
//! - An executor — envpod makes the call, not the agent.
//!
//! Auth credentials always come from the vault (config: `auth_vault_key`).
//! The agent never sees the secret; envpod fetches and uses it at execution time.
//!
//! # Quick reference
//!
//! ```text
//! CATEGORY    TYPE             TIER       WHAT HAPPENS
//! ─────────────────────────────────────────────────────────────────
//! HTTP        http_get         immediate  GET request, return body
//!             http_post        staged     POST with body
//!             http_put         staged     PUT with body
//!             http_patch       staged     PATCH with body
//!             http_delete      staged     DELETE request
//!             webhook          staged     POST to webhook URL
//! Filesystem  file_create      immediate  Create file in overlay
//!             file_write       immediate  Write/append to file in overlay
//!             file_delete      delayed    Delete file from overlay (30s grace)
//!             file_copy        immediate  Copy in overlay
//!             file_move        delayed    Move in overlay (30s grace)
//!             dir_create       immediate  Create directory in overlay
//!             dir_delete       delayed    Delete directory (30s grace)
//! Git         git_commit       staged     Commit overlay changes
//!             git_push         staged     Push to remote
//!             git_pull         immediate  Pull/fetch from remote
//!             git_checkout     immediate  Checkout branch in workspace
//!             git_branch       immediate  Create/delete branch
//!             git_tag          immediate  Create/push tag
//! Custom      custom           staged     No built-in executor
//! ```
//!
//! Note: Messaging (send_email, send_sms, slack_message, discord_message, teams_message),
//! Database (db_query, db_execute), and System (shell_command) action types are
//! available in envpod Premium. See https://envpod.com for details.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::actions::ParamDef;
use crate::queue::ActionTier;

// ---------------------------------------------------------------------------
// ActionScope
// ---------------------------------------------------------------------------

/// Whether an action operates inside the pod or makes calls to the outside world.
///
/// This distinction drives security policy:
/// - **Internal** actions touch only the pod's overlay filesystem or run git
///   inside the workspace.  They are fully reversible via `envpod rollback`.
///   A failed or malicious internal action leaves no external footprint.
/// - **External** actions reach outside the pod — HTTP calls, webhooks, etc.
///   They may be irreversible (money charged, message sent).
///   Require stronger governance: higher default tier, shown prominently in
///   the dashboard, flagged as CRITICAL in air-gapped pod security audits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionScope {
    /// Operates only within the pod's COW overlay or workspace.
    /// Fully reversible via `envpod rollback`.
    Internal,
    /// Makes calls outside the pod (network, webhooks).
    /// Effects may be irreversible — requires human approval by default.
    External,
}

impl std::fmt::Display for ActionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionScope::Internal => f.write_str("internal"),
            ActionScope::External => f.write_str("external"),
        }
    }
}

/// Return the scope of an action type.
pub fn scope(t: &ActionType) -> ActionScope {
    use ActionType::*;
    match t {
        // Internal: all filesystem and git operations stay inside the pod overlay
        FileCreate | FileWrite | FileDelete | FileCopy | FileMove | DirCreate | DirDelete => {
            ActionScope::Internal
        }
        GitCommit | GitPush | GitPull | GitCheckout | GitBranch | GitTag => ActionScope::Internal,
        Custom => ActionScope::Internal, // conservative default
        // External: all HTTP
        HttpGet | HttpPost | HttpPut | HttpPatch | HttpDelete | Webhook => ActionScope::External,
    }
}

// ---------------------------------------------------------------------------
// Path safety
// ---------------------------------------------------------------------------

/// Validate that a pod-internal path is safe to use in a filesystem action.
///
/// Rules:
/// 1. No `..` components — no traversal out of the overlay.
/// 2. Must not be empty.
/// 3. Path must stay within the given `overlay_upper` root after joining.
///
/// Call this before any `pod_path_to_overlay()` operation.
pub fn validate_pod_path(pod_path: &str, overlay_upper: &Path) -> Result<()> {
    use std::path::Component;
    if pod_path.is_empty() {
        anyhow::bail!("file path must not be empty");
    }
    let p = std::path::Path::new(pod_path);
    for component in p.components() {
        if component == Component::ParentDir {
            anyhow::bail!(
                "file path '{pod_path}' contains '..': path traversal is not allowed"
            );
        }
    }
    // Check the resolved path stays inside overlay_upper
    let stripped = pod_path.trim_start_matches('/');
    let resolved = overlay_upper.join(stripped);
    // canonicalize the parent (file may not exist yet), check it's under overlay_upper
    let check_base = if resolved.exists() {
        resolved.canonicalize().unwrap_or(resolved)
    } else if let Some(parent) = resolved.parent() {
        // Check closest existing ancestor
        let mut ancestor = parent.to_path_buf();
        while !ancestor.exists() {
            if let Some(p) = ancestor.parent() {
                ancestor = p.to_path_buf();
            } else {
                break;
            }
        }
        ancestor.canonicalize().unwrap_or(ancestor)
    } else {
        overlay_upper.to_path_buf()
    };

    let overlay_canonical = overlay_upper.canonicalize().unwrap_or_else(|_| overlay_upper.to_path_buf());
    if !check_base.starts_with(&overlay_canonical) {
        anyhow::bail!(
            "file path '{pod_path}' would escape the pod overlay — host filesystem access not allowed"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ActionType enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    // HTTP
    HttpGet,
    HttpPost,
    HttpPut,
    HttpPatch,
    HttpDelete,
    Webhook,
    // Filesystem (operates on pod overlay upper/)
    FileCreate,
    FileWrite,
    FileDelete,
    FileCopy,
    FileMove,
    DirCreate,
    DirDelete,
    // Git
    GitCommit,
    GitPush,
    GitPull,
    GitCheckout,
    GitBranch,
    GitTag,
    // Custom (user-defined schema, no built-in executor)
    Custom,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "custom".to_string());
        f.write_str(&s)
    }
}

// ---------------------------------------------------------------------------
// Schema: param definitions per type
// ---------------------------------------------------------------------------

/// Return the built-in parameter schema for an action type.
pub fn schema(t: &ActionType) -> Vec<ParamDef> {
    use ActionType::*;
    macro_rules! req {
        ($name:expr) => {
            ParamDef { name: $name.to_string(), description: None, required: true }
        };
        ($name:expr, $desc:expr) => {
            ParamDef {
                name: $name.to_string(),
                description: Some($desc.to_string()),
                required: true,
            }
        };
    }
    macro_rules! opt {
        ($name:expr) => {
            ParamDef { name: $name.to_string(), description: None, required: false }
        };
        ($name:expr, $desc:expr) => {
            ParamDef {
                name: $name.to_string(),
                description: Some($desc.to_string()),
                required: false,
            }
        };
    }

    match t {
        // -- HTTP -----------------------------------------------------------
        HttpGet => vec![
            req!("url", "Full URL to GET"),
            opt!("headers", "JSON object of extra request headers"),
        ],
        HttpPost | HttpPut | HttpPatch => vec![
            req!("url", "Full URL"),
            opt!("body", "Request body (JSON string or plain text)"),
            opt!("content_type", "Content-Type header (default: application/json)"),
            opt!("headers", "JSON object of extra request headers"),
        ],
        HttpDelete => vec![
            req!("url", "Full URL to DELETE"),
            opt!("headers", "JSON object of extra request headers"),
        ],
        Webhook => vec![
            req!("url", "Webhook URL (HTTPS)"),
            req!("payload", "JSON payload to POST"),
            opt!("secret_header", "Header name for HMAC signature (e.g. X-Hub-Signature)"),
            opt!("headers", "JSON object of extra request headers"),
        ],

        // -- Filesystem -----------------------------------------------------
        // Paths are relative to the pod workspace or absolute within the overlay.
        FileCreate => vec![
            req!("path", "File path inside pod (e.g. /workspace/out.txt)"),
            opt!("content", "Initial content (empty if omitted)"),
        ],
        FileWrite => vec![
            req!("path", "File path inside pod"),
            req!("content", "Content to write"),
            opt!("append", "If 'true', append instead of overwrite (default: false)"),
        ],
        FileDelete => vec![
            req!("path", "File path inside pod"),
        ],
        FileCopy => vec![
            req!("src", "Source path inside pod"),
            req!("dst", "Destination path inside pod"),
        ],
        FileMove => vec![
            req!("src", "Source path inside pod"),
            req!("dst", "Destination path inside pod"),
        ],
        DirCreate => vec![
            req!("path", "Directory path inside pod"),
        ],
        DirDelete => vec![
            req!("path", "Directory path inside pod"),
            opt!("recursive", "If 'true', remove directory and all contents (default: true)"),
        ],

        // -- Git ------------------------------------------------------------
        GitCommit => vec![
            req!("message", "Commit message"),
            opt!("paths", "Space-separated paths to commit (default: all staged)"),
            opt!("working_dir", "Working directory (default: pod workspace)"),
        ],
        GitPush => vec![
            opt!("remote", "Remote name (default: origin)"),
            opt!("branch", "Branch to push (default: current)"),
            opt!("working_dir", "Working directory"),
        ],
        GitPull => vec![
            opt!("remote", "Remote name (default: origin)"),
            opt!("branch", "Branch to pull (default: current)"),
            opt!("working_dir", "Working directory"),
        ],
        GitCheckout => vec![
            req!("branch", "Branch or ref to checkout"),
            opt!("create", "If 'true', create the branch (git checkout -b)"),
            opt!("working_dir", "Working directory"),
        ],
        GitBranch => vec![
            req!("name", "Branch name"),
            opt!("delete", "If 'true', delete the branch (git branch -d)"),
            opt!("working_dir", "Working directory"),
        ],
        GitTag => vec![
            req!("name", "Tag name"),
            opt!("message", "Tag annotation message (creates annotated tag if set)"),
            opt!("push", "If 'true', push tag to remote after creating"),
            opt!("working_dir", "Working directory"),
        ],

        // -- Custom ---------------------------------------------------------
        Custom => vec![],
    }
}

/// Default reversibility tier for an action type.
pub fn default_tier(t: &ActionType) -> ActionTier {
    use ActionType::*;
    match t {
        // Read-only: immediate
        HttpGet | GitPull | GitCheckout | GitBranch | GitTag => {
            ActionTier::ImmediateProtected
        }
        // Destructive with grace period: delayed
        FileDelete | FileMove | DirDelete => ActionTier::Delayed,
        // Safe writes (COW-protected): immediate
        FileCreate | FileWrite | FileCopy | DirCreate => ActionTier::ImmediateProtected,
        // Potentially irreversible: staged
        HttpPost | HttpPut | HttpPatch | HttpDelete | Webhook => ActionTier::Staged,
        GitCommit | GitPush => ActionTier::Staged,
        Custom => ActionTier::Staged,
    }
}

/// Human-readable category for an action type.
pub fn category(t: &ActionType) -> &'static str {
    use ActionType::*;
    match t {
        HttpGet | HttpPost | HttpPut | HttpPatch | HttpDelete | Webhook => "HTTP",
        FileCreate | FileWrite | FileDelete | FileCopy | FileMove | DirCreate | DirDelete => {
            "Filesystem"
        }
        GitCommit | GitPush | GitPull | GitCheckout | GitBranch | GitTag => "Git",
        Custom => "Custom",
    }
}

/// One-line description of the action type.
pub fn description(t: &ActionType) -> &'static str {
    use ActionType::*;
    match t {
        HttpGet => "HTTP GET request — read data from a URL",
        HttpPost => "HTTP POST request — send data to an endpoint",
        HttpPut => "HTTP PUT request — replace a resource",
        HttpPatch => "HTTP PATCH request — update part of a resource",
        HttpDelete => "HTTP DELETE request — remove a resource",
        Webhook => "POST a JSON payload to a webhook URL",
        FileCreate => "Create a new file inside the pod",
        FileWrite => "Write or append content to a file inside the pod",
        FileDelete => "Delete a file inside the pod (grace period: 30s)",
        FileCopy => "Copy a file inside the pod",
        FileMove => "Move or rename a file inside the pod (grace period: 30s)",
        DirCreate => "Create a directory inside the pod",
        DirDelete => "Delete a directory inside the pod (grace period: 30s)",
        GitCommit => "Commit changes in the pod workspace",
        GitPush => "Push commits to a remote repository",
        GitPull => "Pull updates from a remote repository",
        GitCheckout => "Checkout a branch or ref in the pod workspace",
        GitBranch => "Create or delete a branch in the pod workspace",
        GitTag => "Create a tag in the pod workspace",
        Custom => "User-defined action with no built-in executor",
    }
}

// ---------------------------------------------------------------------------
// Executor config (per-catalog-entry, non-secret)
// ---------------------------------------------------------------------------

/// Executor configuration stored in the catalog entry (non-secret values).
/// Secrets (API keys, passwords) are referenced by vault key name, not stored directly.
///
/// Common fields:
/// - `auth_vault_key`: vault key whose value is used for authentication
/// - `auth_scheme`: "bearer" | "basic" | "header:<Name>" (default: bearer)
/// - `base_url`: prefix prepended to `url` param (for http_* and webhook)
/// - `workspace`: working directory for git/file ops (default: /workspace)
pub type ExecutorConfig = HashMap<String, String>;

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Result of an action execution by envpod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    /// HTTP status code, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

impl ExecutionResult {
    fn ok(output: impl Into<String>) -> Self {
        Self { success: true, output: Some(output.into()), error: None, status_code: None }
    }
    fn fail(error: impl Into<String>) -> Self {
        Self { success: false, output: None, error: Some(error.into()), status_code: None }
    }
    fn http_ok(status: u16, body: String) -> Self {
        Self { success: status < 400, output: Some(body), error: None, status_code: Some(status) }
    }
}

/// Executes an approved action on behalf of envpod.
///
/// All execution happens host-side.  Agents never make the calls directly.
/// File paths are mapped into the pod's overlay upper/ directory.
pub struct ActionExecutor {
    pod_dir: PathBuf,
}

impl ActionExecutor {
    pub fn new(pod_dir: &Path) -> Self {
        Self { pod_dir: pod_dir.to_path_buf() }
    }

    /// Vault instance for loading secrets at execution time.
    fn vault(&self) -> Result<crate::vault::Vault> {
        crate::vault::Vault::new(&self.pod_dir)
    }

    /// Get a secret from vault by key. Used for auth_vault_key lookups.
    fn vault_get(&self, key: &str) -> Result<String> {
        let vault = self.vault()?;
        let secrets = vault.load_all()?;
        secrets
            .get(key)
            .cloned()
            .with_context(|| format!("vault key not found: '{key}'"))
    }

    /// Overlay upper/ path for file operations.
    fn overlay_upper(&self) -> PathBuf {
        self.pod_dir.join("upper")
    }

    /// Translate an in-pod path (e.g. /workspace/file.txt) to the overlay upper path.
    fn pod_path_to_overlay(&self, pod_path: &str) -> PathBuf {
        let stripped = pod_path.trim_start_matches('/');
        self.overlay_upper().join(stripped)
    }

    /// Workspace directory for git ops.
    fn workspace_dir(&self, config: &ExecutorConfig, params: &HashMap<String, serde_json::Value>) -> PathBuf {
        // Priority: params.working_dir > config.workspace > pod upper/workspace
        if let Some(v) = params.get("working_dir").and_then(|v| v.as_str()) {
            return self.pod_path_to_overlay(v);
        }
        if let Some(v) = config.get("workspace") {
            return self.pod_path_to_overlay(v);
        }
        self.pod_path_to_overlay("/workspace")
    }

    /// Execute an action with the given params and per-catalog-entry config.
    pub async fn execute(
        &self,
        action_type: &ActionType,
        params: &HashMap<String, serde_json::Value>,
        config: &ExecutorConfig,
    ) -> ExecutionResult {
        match self.execute_inner(action_type, params, config).await {
            Ok(result) => result,
            Err(e) => ExecutionResult::fail(format!("{e:#}")),
        }
    }

    async fn execute_inner(
        &self,
        action_type: &ActionType,
        params: &HashMap<String, serde_json::Value>,
        config: &ExecutorConfig,
    ) -> Result<ExecutionResult> {
        use ActionType::*;
        Ok(match action_type {
            // -- HTTP -------------------------------------------------------
            HttpGet => self.exec_http("GET", params, config).await?,
            HttpPost => self.exec_http("POST", params, config).await?,
            HttpPut => self.exec_http("PUT", params, config).await?,
            HttpPatch => self.exec_http("PATCH", params, config).await?,
            HttpDelete => self.exec_http("DELETE", params, config).await?,
            Webhook => self.exec_webhook(params, config).await?,

            // -- Filesystem -------------------------------------------------
            FileCreate => self.exec_file_create(params)?,
            FileWrite => self.exec_file_write(params)?,
            FileDelete => self.exec_file_delete(params)?,
            FileCopy => self.exec_file_copy(params)?,
            FileMove => self.exec_file_move(params)?,
            DirCreate => self.exec_dir_create(params)?,
            DirDelete => self.exec_dir_delete(params)?,

            // -- Git --------------------------------------------------------
            GitCommit => self.exec_git(&["commit", "-m", &param_str(params, "message")?], params, config)?,
            GitPush => {
                let remote = params.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = params.get("branch").and_then(|v| v.as_str()).unwrap_or("");
                if branch.is_empty() {
                    self.exec_git(&["push", remote], params, config)?
                } else {
                    self.exec_git(&["push", remote, branch], params, config)?
                }
            }
            GitPull => {
                let remote = params.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = params.get("branch").and_then(|v| v.as_str()).unwrap_or("");
                if branch.is_empty() {
                    self.exec_git(&["pull", remote], params, config)?
                } else {
                    self.exec_git(&["pull", remote, branch], params, config)?
                }
            }
            GitCheckout => {
                let branch = param_str(params, "branch")?;
                let create = params.get("create").and_then(|v| v.as_str()) == Some("true");
                if create {
                    self.exec_git(&["checkout", "-b", &branch], params, config)?
                } else {
                    self.exec_git(&["checkout", &branch], params, config)?
                }
            }
            GitBranch => {
                let name = param_str(params, "name")?;
                let delete = params.get("delete").and_then(|v| v.as_str()) == Some("true");
                if delete {
                    self.exec_git(&["branch", "-d", &name], params, config)?
                } else {
                    self.exec_git(&["branch", &name], params, config)?
                }
            }
            GitTag => {
                let name = param_str(params, "name")?;
                let message = params.get("message").and_then(|v| v.as_str());
                let push = params.get("push").and_then(|v| v.as_str()) == Some("true");
                let tag_result = if let Some(msg) = message {
                    self.exec_git(&["tag", "-a", &name, "-m", msg], params, config)?
                } else {
                    self.exec_git(&["tag", &name], params, config)?
                };
                if push && tag_result.success {
                    let remote = config.get("remote").map(String::as_str).unwrap_or("origin");
                    self.exec_git(&["push", remote, &name], params, config)?
                } else {
                    tag_result
                }
            }

            // -- Custom (no executor) ---------------------------------------
            Custom => ExecutionResult::ok(
                "Custom action approved — no built-in executor. \
                 Implement execution in your own workflow."
                    .to_string(),
            ),
        })
    }

    // -- HTTP helpers -------------------------------------------------------

    async fn exec_http(
        &self,
        method: &str,
        params: &HashMap<String, serde_json::Value>,
        config: &ExecutorConfig,
    ) -> Result<ExecutionResult> {
        let url = build_url(params, config)?;
        let body = params
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content_type = params
            .get("content_type")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("content_type").map(String::as_str))
            .unwrap_or("application/json");
        let extra_headers = parse_headers_param(params)?;

        self.http_request(method, &url, &body, content_type, extra_headers, config).await
    }

    async fn exec_webhook(
        &self,
        params: &HashMap<String, serde_json::Value>,
        config: &ExecutorConfig,
    ) -> Result<ExecutionResult> {
        let url = build_url(params, config)?;
        let payload = param_str(params, "payload")?;
        let extra_headers = parse_headers_param(params)?;
        self.http_request("POST", &url, &payload, "application/json", extra_headers, config).await
    }

    async fn http_request(
        &self,
        method: &str,
        url: &str,
        body: &str,
        content_type: &str,
        extra_headers: Vec<(String, String)>,
        config: &ExecutorConfig,
    ) -> Result<ExecutionResult> {
        use http_body_util::{BodyExt, Full};
        use hyper::body::Bytes;
        use hyper::Request;
        use hyper_rustls::HttpsConnectorBuilder;
        use hyper_util::client::legacy::Client;
        use hyper_util::rt::TokioExecutor;

        let https = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let client: Client<_, Full<Bytes>> =
            Client::builder(TokioExecutor::new()).build(https);

        let body_bytes = Bytes::from(body.to_string());
        let mut req_builder = Request::builder()
            .method(method)
            .uri(url)
            .header("content-type", content_type)
            .header("user-agent", "envpod-action-executor/0.1");

        // Auth header
        if let Some(vault_key) = config.get("auth_vault_key") {
            if let Ok(secret) = self.vault_get(vault_key) {
                let scheme = config.get("auth_scheme").map(String::as_str).unwrap_or("bearer");
                let auth_value = match scheme {
                    "bearer" => format!("Bearer {secret}"),
                    "basic" => {
                        let encoded = {
                            use std::io::Write;
                            let mut buf = Vec::new();
                            write!(buf, "{secret}").ok();
                            base64_encode(&buf)
                        };
                        format!("Basic {encoded}")
                    }
                    other if other.starts_with("header:") => {
                        // auth_scheme: "header:X-API-Key" → sets X-API-Key: {secret}
                        let header_name = &other["header:".len()..];
                        req_builder = req_builder.header(header_name, &secret);
                        String::new() // skip standard Authorization
                    }
                    _ => format!("Bearer {secret}"),
                };
                if !auth_value.is_empty() {
                    req_builder = req_builder.header("authorization", auth_value);
                }
            }
        }

        for (k, v) in &extra_headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let req = req_builder
            .body(Full::new(body_bytes))
            .context("build request")?;

        let resp = client.request(req).await.context("http request")?;
        let status = resp.status().as_u16();
        let body_bytes: Bytes = resp.into_body().collect().await.context("read body")?.to_bytes();
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        Ok(ExecutionResult::http_ok(status, body_str))
    }

    // -- Filesystem helpers -------------------------------------------------

    fn exec_file_create(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let pod_path = param_str(params, "path")?;
        validate_pod_path(&pod_path, &self.overlay_upper())?;
        let path = self.pod_path_to_overlay(&pod_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create parent dirs")?;
        }
        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
        std::fs::write(&path, content).with_context(|| format!("create {}", path.display()))?;
        Ok(ExecutionResult::ok(format!("Created {pod_path}")))
    }

    fn exec_file_write(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let pod_path = param_str(params, "path")?;
        validate_pod_path(&pod_path, &self.overlay_upper())?;
        let path = self.pod_path_to_overlay(&pod_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create parent dirs")?;
        }
        let content = param_str(params, "content")?;
        let append = params.get("append").and_then(|v| v.as_str()) == Some("true");
        if append {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            write!(file, "{content}").context("append")?;
        } else {
            std::fs::write(&path, &content)
                .with_context(|| format!("write {}", path.display()))?;
        }
        Ok(ExecutionResult::ok(format!("{} bytes written to {pod_path}", content.len())))
    }

    fn exec_file_delete(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let pod_path = param_str(params, "path")?;
        validate_pod_path(&pod_path, &self.overlay_upper())?;
        let path = self.pod_path_to_overlay(&pod_path);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("remove dir {}", path.display()))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(ExecutionResult::ok(format!("Deleted {pod_path}")))
    }

    fn exec_file_copy(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let src_pod = param_str(params, "src")?;
        let dst_pod = param_str(params, "dst")?;
        validate_pod_path(&src_pod, &self.overlay_upper())?;
        validate_pod_path(&dst_pod, &self.overlay_upper())?;
        let src = self.pod_path_to_overlay(&src_pod);
        let dst = self.pod_path_to_overlay(&dst_pod);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).context("create dst parent")?;
        }
        std::fs::copy(&src, &dst)
            .with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
        Ok(ExecutionResult::ok(format!("Copied {src_pod} → {dst_pod}")))
    }

    fn exec_file_move(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let src_pod = param_str(params, "src")?;
        let dst_pod = param_str(params, "dst")?;
        validate_pod_path(&src_pod, &self.overlay_upper())?;
        validate_pod_path(&dst_pod, &self.overlay_upper())?;
        let src = self.pod_path_to_overlay(&src_pod);
        let dst = self.pod_path_to_overlay(&dst_pod);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).context("create dst parent")?;
        }
        std::fs::rename(&src, &dst)
            .with_context(|| format!("move {} → {}", src.display(), dst.display()))?;
        Ok(ExecutionResult::ok(format!("Moved {src_pod} → {dst_pod}")))
    }

    fn exec_dir_create(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let pod_path = param_str(params, "path")?;
        validate_pod_path(&pod_path, &self.overlay_upper())?;
        let path = self.pod_path_to_overlay(&pod_path);
        std::fs::create_dir_all(&path)
            .with_context(|| format!("mkdir -p {}", path.display()))?;
        Ok(ExecutionResult::ok(format!("Created directory {pod_path}")))
    }

    fn exec_dir_delete(&self, params: &HashMap<String, serde_json::Value>) -> Result<ExecutionResult> {
        let pod_path = param_str(params, "path")?;
        validate_pod_path(&pod_path, &self.overlay_upper())?;
        let path = self.pod_path_to_overlay(&pod_path);
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("rm -rf {}", path.display()))?;
        Ok(ExecutionResult::ok(format!("Deleted directory {pod_path}")))
    }

    // -- Git helper ---------------------------------------------------------

    fn exec_git(
        &self,
        args: &[&str],
        params: &HashMap<String, serde_json::Value>,
        config: &ExecutorConfig,
    ) -> Result<ExecutionResult> {
        let working_dir = self.workspace_dir(config, params);
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&working_dir)
            .output()
            .context("run git")?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let combined = if stderr.is_empty() {
            stdout.trim().to_string()
        } else {
            format!("{}\n{}", stdout.trim(), stderr.trim())
        };
        if out.status.success() {
            Ok(ExecutionResult::ok(combined))
        } else {
            Ok(ExecutionResult::fail(combined))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn param_str(params: &HashMap<String, serde_json::Value>, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .with_context(|| format!("missing required param '{key}'"))
}

fn build_url(params: &HashMap<String, serde_json::Value>, config: &ExecutorConfig) -> Result<String> {
    let url = param_str(params, "url")?;
    if let Some(base) = config.get("base_url") {
        if !url.starts_with("http") {
            return Ok(format!("{}/{}", base.trim_end_matches('/'), url.trim_start_matches('/')));
        }
    }
    Ok(url)
}

fn parse_headers_param(
    params: &HashMap<String, serde_json::Value>,
) -> Result<Vec<(String, String)>> {
    let Some(hdr) = params.get("headers") else { return Ok(vec![]) };
    let obj = hdr
        .as_object()
        .with_context(|| "headers must be a JSON object")?;
    Ok(obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect())
}

/// Minimal base64 encoder (no external dep needed for this).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        result.push(CHARS[b0 >> 2] as char);
        result.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((b1 & 15) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[b2 & 63] as char);
        } else {
            result.push('=');
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_types_have_non_empty_description() {
        use ActionType::*;
        let types = [
            HttpGet, HttpPost, HttpPut, HttpPatch, HttpDelete, Webhook,
            FileCreate, FileWrite, FileDelete, FileCopy, FileMove, DirCreate, DirDelete,
            GitCommit, GitPush, GitPull, GitCheckout, GitBranch, GitTag,
            Custom,
        ];
        for t in &types {
            assert!(!description(t).is_empty(), "{t:?} has no description");
            assert!(!category(t).is_empty(), "{t:?} has no category");
        }
    }

    #[test]
    fn schema_required_params_present() {
        // HttpPost requires url
        let s = schema(&ActionType::HttpPost);
        assert!(s.iter().any(|p| p.name == "url" && p.required));
        // file_create requires path
        let s = schema(&ActionType::FileCreate);
        assert!(s.iter().any(|p| p.name == "path" && p.required));
    }

    #[test]
    fn http_get_is_immediate() {
        assert_eq!(default_tier(&ActionType::HttpGet), ActionTier::ImmediateProtected);
    }

    #[test]
    fn http_post_is_staged() {
        assert_eq!(default_tier(&ActionType::HttpPost), ActionTier::Staged);
    }

    #[test]
    fn file_delete_is_delayed() {
        assert_eq!(default_tier(&ActionType::FileDelete), ActionTier::Delayed);
    }

    #[test]
    fn file_operations_executor() {
        let tmp = tempfile::tempdir().unwrap();
        let executor = ActionExecutor::new(tmp.path());

        // Create overlay upper/
        std::fs::create_dir_all(tmp.path().join("upper")).unwrap();

        // file_create
        let params: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("/workspace/hello.txt")),
            ("content".to_string(), serde_json::json!("Hello, world!")),
        ].into_iter().collect();
        let result = executor.exec_file_create(&params).unwrap();
        assert!(result.success);
        let created = tmp.path().join("upper/workspace/hello.txt");
        assert!(created.exists());
        assert_eq!(std::fs::read_to_string(&created).unwrap(), "Hello, world!");

        // file_write (append)
        let params2: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("/workspace/hello.txt")),
            ("content".to_string(), serde_json::json!("\nAppended.")),
            ("append".to_string(), serde_json::json!("true")),
        ].into_iter().collect();
        let result2 = executor.exec_file_write(&params2).unwrap();
        assert!(result2.success);
        assert_eq!(
            std::fs::read_to_string(&created).unwrap(),
            "Hello, world!\nAppended."
        );

        // file_copy
        let params3: HashMap<String, serde_json::Value> = [
            ("src".to_string(), serde_json::json!("/workspace/hello.txt")),
            ("dst".to_string(), serde_json::json!("/workspace/hello2.txt")),
        ].into_iter().collect();
        let result3 = executor.exec_file_copy(&params3).unwrap();
        assert!(result3.success);
        assert!(tmp.path().join("upper/workspace/hello2.txt").exists());

        // file_delete
        let params4: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("/workspace/hello2.txt")),
        ].into_iter().collect();
        let result4 = executor.exec_file_delete(&params4).unwrap();
        assert!(result4.success);
        assert!(!tmp.path().join("upper/workspace/hello2.txt").exists());
    }

    #[test]
    fn dir_operations_executor() {
        let tmp = tempfile::tempdir().unwrap();
        let executor = ActionExecutor::new(tmp.path());
        std::fs::create_dir_all(tmp.path().join("upper")).unwrap();

        let params: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("/workspace/subdir/nested")),
        ].into_iter().collect();
        let result = executor.exec_dir_create(&params).unwrap();
        assert!(result.success);
        assert!(tmp.path().join("upper/workspace/subdir/nested").is_dir());

        let del_params: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("/workspace/subdir")),
        ].into_iter().collect();
        let del_result = executor.exec_dir_delete(&del_params).unwrap();
        assert!(del_result.success);
        assert!(!tmp.path().join("upper/workspace/subdir").exists());
    }

    #[test]
    fn scope_classification() {
        use ActionScope::*;
        // Filesystem = internal
        assert_eq!(scope(&ActionType::FileCreate), Internal);
        assert_eq!(scope(&ActionType::FileDelete), Internal);
        assert_eq!(scope(&ActionType::DirCreate), Internal);
        // Git = internal
        assert_eq!(scope(&ActionType::GitCommit), Internal);
        assert_eq!(scope(&ActionType::GitPush), Internal);
        // HTTP = external
        assert_eq!(scope(&ActionType::HttpPost), External);
        assert_eq!(scope(&ActionType::Webhook), External);
    }

    #[test]
    fn validate_pod_path_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        std::fs::create_dir_all(&upper).unwrap();

        // .. traversal should be rejected
        let err = validate_pod_path("../../../etc/passwd", &upper).unwrap_err();
        assert!(err.to_string().contains(".."));

        // /workspace/../../../etc also rejected
        let err2 = validate_pod_path("/workspace/../../etc/shadow", &upper).unwrap_err();
        assert!(err2.to_string().contains(".."));

        // valid paths should pass
        validate_pod_path("/workspace/output.txt", &upper).unwrap();
        validate_pod_path("workspace/nested/dir/file.txt", &upper).unwrap();
    }

    #[test]
    fn validate_pod_path_rejects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        let err = validate_pod_path("", &upper).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn file_ops_reject_traversal_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let executor = ActionExecutor::new(tmp.path());
        std::fs::create_dir_all(tmp.path().join("upper")).unwrap();

        let params: HashMap<String, serde_json::Value> = [
            ("path".to_string(), serde_json::json!("../../etc/passwd")),
            ("content".to_string(), serde_json::json!("bad")),
        ].into_iter().collect();
        let err = executor.exec_file_create(&params).unwrap_err();
        assert!(err.to_string().contains(".."));
    }

    #[test]
    fn base64_encode_roundtrip() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"hello:world"), "aGVsbG86d29ybGQ=");
    }
}
