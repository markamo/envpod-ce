// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Action staging queue for pods.
//!
//! Every agent action is classified into a reversibility tier and routed through
//! a queue. Humans can inspect, approve, or cancel queued actions before execution.
//! Persisted as `{pod_dir}/queue.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{AuditAction, AuditEntry, AuditLog};

/// Reversibility tier for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionTier {
    /// COW-protected — executes immediately, reversible via overlay.
    ImmediateProtected,
    /// Auto-executes after a timeout (not implemented in MVP).
    Delayed,
    /// Requires explicit human approval.
    Staged,
    /// Denied outright by policy.
    Blocked,
}

impl std::fmt::Display for ActionTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ActionTier::ImmediateProtected => "immediate",
            ActionTier::Delayed => "delayed",
            ActionTier::Staged => "staged",
            ActionTier::Blocked => "blocked",
        };
        f.write_str(s)
    }
}

/// Lifecycle status of a queued action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Queued,
    Approved,
    Executed,
    Cancelled,
    Blocked,
}

impl std::fmt::Display for ActionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ActionStatus::Queued => "queued",
            ActionStatus::Approved => "approved",
            ActionStatus::Executed => "executed",
            ActionStatus::Cancelled => "cancelled",
            ActionStatus::Blocked => "blocked",
        };
        f.write_str(s)
    }
}

/// A single action in the staging queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedAction {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub tier: ActionTier,
    pub status: ActionStatus,
    pub description: String,
    pub updated_at: DateTime<Utc>,
    /// Delay in seconds before auto-execution (Delayed tier only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_seconds: Option<u64>,
    /// Timestamp after which a Delayed action should auto-execute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute_after: Option<DateTime<Utc>>,
    /// Optional structured payload used by the executor to dispatch on approval.
    /// Example: `{"type": "commit"}` or `{"type": "rollback"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Persisted queue state (serialized as JSON).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueueState {
    pub actions: Vec<QueuedAction>,
}

/// Action staging queue backed by a JSON file in the pod directory.
pub struct ActionQueue {
    path: PathBuf,
}

impl ActionQueue {
    /// Create a queue handle for the given pod directory.
    pub fn new(pod_dir: &Path) -> Self {
        Self {
            path: pod_dir.join("queue.json"),
        }
    }

    /// Load the current queue state from disk. Returns empty state if file is missing.
    pub fn load(&self) -> Result<QueueState> {
        match fs::read_to_string(&self.path) {
            Ok(json) => {
                serde_json::from_str(&json).context("parse queue.json")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(QueueState::default())
            }
            Err(e) => {
                Err(anyhow::Error::new(e)
                    .context(format!("read queue: {}", self.path.display())))
            }
        }
    }

    /// Save queue state to disk (pretty-printed JSON).
    pub fn save(&self, state: &QueueState) -> Result<()> {
        let json = serde_json::to_string_pretty(state).context("serialize queue state")?;
        fs::write(&self.path, json)
            .with_context(|| format!("write queue: {}", self.path.display()))?;
        Ok(())
    }

    /// Submit a new action to the queue. Blocked-tier actions get Blocked status immediately.
    /// Delayed-tier actions get a default 30-second delay.
    pub fn submit(&self, tier: ActionTier, description: &str) -> Result<QueuedAction> {
        self.submit_full(tier, description, None, None)
    }

    /// Submit with an optional structured payload for executor dispatch on approval.
    pub fn submit_with_payload(
        &self,
        tier: ActionTier,
        description: &str,
        payload: serde_json::Value,
    ) -> Result<QueuedAction> {
        self.submit_full(tier, description, None, Some(payload))
    }

    /// Submit a new action with an explicit delay (in seconds).
    ///
    /// For `Delayed` tier: computes `execute_after = now + delay_secs`.
    /// For other tiers: `delay_secs` is ignored.
    pub fn submit_with_delay(
        &self,
        tier: ActionTier,
        description: &str,
        delay_secs: Option<u64>,
    ) -> Result<QueuedAction> {
        self.submit_full(tier, description, delay_secs, None)
    }

    fn submit_full(
        &self,
        tier: ActionTier,
        description: &str,
        delay_secs: Option<u64>,
        payload: Option<serde_json::Value>,
    ) -> Result<QueuedAction> {
        let mut state = self.load()?;
        let now = Utc::now();

        let status = if tier == ActionTier::Blocked {
            ActionStatus::Blocked
        } else {
            ActionStatus::Queued
        };

        let (delay_seconds, execute_after) = if tier == ActionTier::Delayed {
            let secs = delay_secs.unwrap_or(30);
            let deadline = now + chrono::Duration::seconds(secs as i64);
            (Some(secs), Some(deadline))
        } else {
            (None, None)
        };

        let action = QueuedAction {
            id: Uuid::new_v4(),
            created_at: now,
            tier,
            status,
            description: description.to_string(),
            updated_at: now,
            delay_seconds,
            execute_after,
            payload,
        };

        state.actions.push(action.clone());
        self.save(&state)?;
        Ok(action)
    }

    /// Find Delayed actions past their `execute_after` deadline and mark them Executed.
    ///
    /// Returns the list of actions that were transitioned.
    pub fn execute_ready(&self) -> Result<Vec<QueuedAction>> {
        let mut state = self.load()?;
        let now = Utc::now();
        let mut executed = Vec::new();

        for action in &mut state.actions {
            if action.tier == ActionTier::Delayed
                && action.status == ActionStatus::Queued
            {
                if let Some(deadline) = action.execute_after {
                    if now >= deadline {
                        action.status = ActionStatus::Executed;
                        action.updated_at = now;
                        executed.push(action.clone());
                    }
                }
            }
        }

        if !executed.is_empty() {
            self.save(&state)?;
        }

        Ok(executed)
    }

    /// Approve a queued action. Only actions with status=Queued can be approved.
    pub fn approve(&self, action_id: Uuid) -> Result<QueuedAction> {
        let mut state = self.load()?;
        let action = state
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .with_context(|| format!("action not found: {action_id}"))?;

        if action.status != ActionStatus::Queued {
            bail!(
                "cannot approve action with status '{}' (must be 'queued')",
                action.status
            );
        }

        action.status = ActionStatus::Approved;
        action.updated_at = Utc::now();
        let result = action.clone();
        self.save(&state)?;
        Ok(result)
    }

    /// Cancel a queued action. Only actions with status=Queued can be cancelled.
    pub fn cancel(&self, action_id: Uuid) -> Result<QueuedAction> {
        let mut state = self.load()?;
        let action = state
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .with_context(|| format!("action not found: {action_id}"))?;

        if action.status != ActionStatus::Queued {
            bail!(
                "cannot cancel action with status '{}' (must be 'queued')",
                action.status
            );
        }

        action.status = ActionStatus::Cancelled;
        action.updated_at = Utc::now();
        let result = action.clone();
        self.save(&state)?;
        Ok(result)
    }

    /// List actions, optionally filtered by status.
    pub fn list(&self, status_filter: Option<ActionStatus>) -> Result<Vec<QueuedAction>> {
        let state = self.load()?;
        match status_filter {
            Some(status) => Ok(state
                .actions
                .into_iter()
                .filter(|a| a.status == status)
                .collect()),
            None => Ok(state.actions),
        }
    }

    /// Emit an audit entry for a queue operation. Non-fatal: logs a warning on error.
    pub fn emit_audit(
        pod_dir: &Path,
        pod_name: &str,
        audit_action: AuditAction,
        queued_action: &QueuedAction,
    ) {
        let log = AuditLog::new(pod_dir);
        let entry = AuditEntry {
            timestamp: Utc::now(),
            pod_name: pod_name.to_string(),
            action: audit_action,
            detail: format!(
                "id={} tier={} status={} desc={}",
                &queued_action.id.to_string()[..8],
                queued_action.tier,
                queued_action.status,
                queued_action.description,
            ),
            success: true,
        };
        if let Err(e) = log.append(&entry) {
            tracing::warn!(error = %e, "failed to write queue audit entry");
        }
    }
}

// ---------------------------------------------------------------------------
// Queue executor — background task for auto-executing delayed actions
// ---------------------------------------------------------------------------

/// Background executor that polls the queue every second and auto-executes
/// delayed actions that have passed their `execute_after` deadline.
pub struct QueueExecutor {
    pod_dir: PathBuf,
    pod_name: String,
}

/// Handle returned by `QueueExecutor::spawn()` — used to shut down the executor.
pub struct QueueExecutorHandle {
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl QueueExecutorHandle {
    /// Signal the executor to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the executor task to finish.
    pub async fn join(self) {
        let _ = self.join_handle.await;
    }
}

impl QueueExecutor {
    pub fn new(pod_dir: PathBuf, pod_name: String) -> Self {
        Self { pod_dir, pod_name }
    }

    /// Spawn the executor as a background tokio task.
    pub fn spawn(self) -> QueueExecutorHandle {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        let queue = ActionQueue::new(&self.pod_dir);
                        match queue.execute_ready() {
                            Ok(executed) => {
                                for action in &executed {
                                    tracing::info!(
                                        pod = %self.pod_name,
                                        id = %&action.id.to_string()[..8],
                                        desc = %action.description,
                                        "delayed action auto-executed"
                                    );
                                    ActionQueue::emit_audit(
                                        &self.pod_dir,
                                        &self.pod_name,
                                        AuditAction::QueueAutoExecute,
                                        action,
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "queue executor poll failed");
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

        QueueExecutorHandle {
            shutdown_tx,
            join_handle,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_and_list_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        queue.submit(ActionTier::Delayed, "send email").unwrap();
        queue.submit(ActionTier::Staged, "stripe charge").unwrap();

        let actions = queue.list(None).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].description, "send email");
        assert_eq!(actions[1].description, "stripe charge");
    }

    #[test]
    fn blocked_tier_gets_blocked_status() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Blocked, "DROP TABLE").unwrap();
        assert_eq!(action.status, ActionStatus::Blocked);
        assert_eq!(action.tier, ActionTier::Blocked);
    }

    #[test]
    fn approve_changes_status() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Staged, "charge card").unwrap();
        assert_eq!(action.status, ActionStatus::Queued);

        let approved = queue.approve(action.id).unwrap();
        assert_eq!(approved.status, ActionStatus::Approved);
        assert!(approved.updated_at >= action.created_at);
    }

    #[test]
    fn cancel_changes_status() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Delayed, "send email").unwrap();
        let cancelled = queue.cancel(action.id).unwrap();
        assert_eq!(cancelled.status, ActionStatus::Cancelled);
    }

    #[test]
    fn approve_non_queued_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Blocked, "DROP TABLE").unwrap();
        assert_eq!(action.status, ActionStatus::Blocked);

        let err = queue.approve(action.id).unwrap_err();
        assert!(err.to_string().contains("cannot approve"));
    }

    #[test]
    fn cancel_non_queued_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Staged, "charge").unwrap();
        queue.approve(action.id).unwrap();

        let err = queue.cancel(action.id).unwrap_err();
        assert!(err.to_string().contains("cannot cancel"));
    }

    #[test]
    fn approve_nonexistent_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let err = queue.approve(Uuid::new_v4()).unwrap_err();
        assert!(err.to_string().contains("action not found"));
    }

    #[test]
    fn empty_queue_returns_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let actions = queue.list(None).unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn list_with_status_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let a1 = queue.submit(ActionTier::Delayed, "email").unwrap();
        queue.submit(ActionTier::Staged, "charge").unwrap();
        queue.submit(ActionTier::Blocked, "drop table").unwrap();
        queue.approve(a1.id).unwrap();

        let queued = queue.list(Some(ActionStatus::Queued)).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].description, "charge");

        let approved = queue.list(Some(ActionStatus::Approved)).unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].description, "email");

        let blocked = queue.list(Some(ActionStatus::Blocked)).unwrap();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].description, "drop table");
    }

    #[test]
    fn emit_audit_writes_to_audit_log() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Staged, "charge card").unwrap();
        ActionQueue::emit_audit(tmp.path(), "test-pod", AuditAction::QueueSubmit, &action);

        let log = AuditLog::new(tmp.path());
        let entries = log.read_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, AuditAction::QueueSubmit);
        assert!(entries[0].detail.contains("charge card"));
        assert_eq!(entries[0].pod_name, "test-pod");
    }

    // -- Delayed tier tests --

    #[test]
    fn delayed_sets_execute_after() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Delayed, "send email").unwrap();
        assert!(action.execute_after.is_some(), "delayed action should have execute_after");
        assert_eq!(action.delay_seconds, Some(30), "default delay should be 30s");
    }

    #[test]
    fn delayed_custom_delay() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit_with_delay(ActionTier::Delayed, "deploy", Some(120)).unwrap();
        assert_eq!(action.delay_seconds, Some(120));
        assert!(action.execute_after.is_some());

        let expected = action.created_at + chrono::Duration::seconds(120);
        let diff = (action.execute_after.unwrap() - expected).num_milliseconds().abs();
        assert!(diff < 100, "execute_after should be ~120s from created_at");
    }

    #[test]
    fn staged_has_no_execute_after() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        let action = queue.submit(ActionTier::Staged, "charge card").unwrap();
        assert!(action.execute_after.is_none(), "staged actions should not have execute_after");
        assert!(action.delay_seconds.is_none(), "staged actions should not have delay_seconds");
    }

    #[test]
    fn execute_ready_transitions_past_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        // Submit with 0-second delay (immediately ready)
        let action = queue.submit_with_delay(ActionTier::Delayed, "immediate", Some(0)).unwrap();
        assert_eq!(action.status, ActionStatus::Queued);

        // Small sleep to ensure we're past the deadline
        std::thread::sleep(std::time::Duration::from_millis(10));

        let executed = queue.execute_ready().unwrap();
        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].id, action.id);
        assert_eq!(executed[0].status, ActionStatus::Executed);

        // Verify persisted state
        let reloaded = queue.list(Some(ActionStatus::Executed)).unwrap();
        assert_eq!(reloaded.len(), 1);
    }

    #[test]
    fn execute_ready_ignores_future_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        queue.submit_with_delay(ActionTier::Delayed, "future", Some(3600)).unwrap();

        let executed = queue.execute_ready().unwrap();
        assert!(executed.is_empty(), "should not execute actions with future deadline");
    }

    #[test]
    fn execute_ready_ignores_non_delayed() {
        let tmp = tempfile::tempdir().unwrap();
        let queue = ActionQueue::new(tmp.path());

        queue.submit(ActionTier::Staged, "staged action").unwrap();
        queue.submit(ActionTier::Blocked, "blocked action").unwrap();

        let executed = queue.execute_ready().unwrap();
        assert!(executed.is_empty(), "should not execute non-delayed actions");
    }

    #[test]
    fn backward_compat_deserialization_without_delay_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let queue_path = tmp.path().join("queue.json");

        // Write old-format JSON without delay_seconds/execute_after
        let old_json = serde_json::json!({
            "actions": [{
                "id": "00000000-0000-0000-0000-000000000001",
                "created_at": "2026-01-01T00:00:00Z",
                "tier": "delayed",
                "status": "queued",
                "description": "legacy action",
                "updated_at": "2026-01-01T00:00:00Z"
            }]
        });
        std::fs::write(&queue_path, serde_json::to_string_pretty(&old_json).unwrap()).unwrap();

        let queue = ActionQueue::new(tmp.path());
        let state = queue.load().unwrap();
        assert_eq!(state.actions.len(), 1);
        assert!(state.actions[0].delay_seconds.is_none());
        assert!(state.actions[0].execute_after.is_none());
    }
}
