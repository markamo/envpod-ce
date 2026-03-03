// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Monitoring agent — background task that polls audit logs and resource usage,
//! evaluates policy rules, and can autonomously freeze or restrict pods.
//!
//! Policy lives in a separate `monitoring-policy.yaml` file per pod (not in pod.yaml)
//! so policies can be updated independently and shared across pods.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing;

use crate::audit::{AuditAction, AuditEntry, AuditLog};
use crate::backend::native::cgroup;
use crate::types::ResourceLimits;

// ---------------------------------------------------------------------------
// Policy config (parsed from monitoring-policy.yaml)
// ---------------------------------------------------------------------------

/// Top-level monitoring policy for a pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorPolicy {
    /// Seconds between each check cycle (default: 5).
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,
    /// Rules to evaluate each tick.
    pub rules: Vec<MonitorRule>,
}

fn default_check_interval() -> u64 {
    5
}

/// A single monitoring rule: condition → response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorRule {
    /// Human-readable rule name (for alerts/audit).
    pub name: String,
    /// When this condition is met, fire the response.
    pub condition: MonitorCondition,
    /// What to do when the condition fires.
    pub response: MonitorResponse,
}

/// Conditions the monitor evaluates each tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MonitorCondition {
    /// Fire if more than `limit` audit entries occurred in the last 60 seconds.
    MaxActionsPerMinute { limit: u32 },
    /// Fire if resource usage exceeds a percentage of the cgroup limit.
    ResourceThreshold {
        resource: ResourceKind,
        max_percent: f64,
    },
    /// Fire if any new audit entry matches this action.
    ForbiddenAction { action: AuditAction },
    /// Fire if the given sequence of actions appears (in order) within `window_secs`.
    ForbiddenSequence {
        actions: Vec<AuditAction>,
        window_secs: u64,
    },
}

/// Which resource to check for threshold conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Memory,
    Cpu,
    Pids,
}

/// What the monitor does when a condition fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MonitorResponse {
    /// Freeze the pod via cgroup freezer.
    Freeze,
    /// Restrict pod resources (only specified fields are changed).
    Restrict {
        #[serde(default)]
        cpu_cores: Option<f64>,
        #[serde(default)]
        memory_bytes: Option<u64>,
        #[serde(default)]
        max_pids: Option<u32>,
    },
}

impl MonitorPolicy {
    /// Load a monitoring policy from a YAML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("read monitoring policy: {}", path.display()))?;
        let policy: Self = serde_yaml::from_str(&content)
            .with_context(|| format!("parse monitoring policy: {}", path.display()))?;
        Ok(policy)
    }
}

// ---------------------------------------------------------------------------
// Monitor agent
// ---------------------------------------------------------------------------

/// Handle to a running monitor agent (same pattern as DnsServerHandle).
pub struct MonitorHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl MonitorHandle {
    /// Signal the monitor to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the monitor task to complete.
    pub async fn join(self) {
        let _ = self.join_handle.await;
    }
}

/// Background monitoring agent that polls audit logs and resource usage.
pub struct MonitorAgent {
    policy: MonitorPolicy,
    pod_dir: PathBuf,
    pod_name: String,
    cgroup_path: PathBuf,
}

impl MonitorAgent {
    pub fn new(
        policy: MonitorPolicy,
        pod_dir: PathBuf,
        pod_name: String,
        cgroup_path: PathBuf,
    ) -> Self {
        Self {
            policy,
            pod_dir,
            pod_name,
            cgroup_path,
        }
    }

    /// Spawn the monitor as a background tokio task.
    pub fn spawn(self) -> MonitorHandle {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let join_handle = tokio::spawn(async move {
            self.run(shutdown_rx).await;
        });

        MonitorHandle {
            shutdown_tx,
            join_handle,
        }
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        let interval = std::time::Duration::from_secs(self.policy.check_interval_secs);
        let mut last_index: usize = 0;
        // Keep recent entries in memory for sequence/rate checking
        let mut recent_entries: Vec<AuditEntry> = Vec::new();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::debug!(pod = %self.pod_name, "monitor shutting down");
                        return;
                    }
                }
            }

            if let Err(e) = self.tick(&mut last_index, &mut recent_entries) {
                tracing::warn!(
                    pod = %self.pod_name,
                    error = %e,
                    "monitor tick failed"
                );
            }
        }
    }

    fn tick(
        &self,
        last_index: &mut usize,
        recent_entries: &mut Vec<AuditEntry>,
    ) -> Result<()> {
        let audit_log = AuditLog::new(&self.pod_dir);

        // Read new entries since last tick
        let (new_entries, total) = audit_log.read_from(*last_index)?;
        *last_index = total;
        recent_entries.extend(new_entries);

        // Prune entries older than the max window we care about (5 minutes)
        let cutoff = Utc::now() - chrono::Duration::seconds(300);
        recent_entries.retain(|e| e.timestamp >= cutoff);

        // Read resource usage from cgroup
        let usage = cgroup::read_usage(&self.cgroup_path).ok();

        // Evaluate each rule
        for rule in &self.policy.rules {
            if self.evaluate_condition(&rule.condition, recent_entries, usage.as_ref()) {
                tracing::warn!(
                    pod = %self.pod_name,
                    rule = %rule.name,
                    "monitor rule fired"
                );

                // Emit alert audit entry
                let alert_entry = AuditEntry {
                    timestamp: Utc::now(),
                    pod_name: self.pod_name.clone(),
                    action: AuditAction::MonitorAlert,
                    detail: format!("rule={}", rule.name),
                    success: true,
                };
                audit_log.append(&alert_entry).ok();

                if let Err(e) = self.execute_response(&rule.response, &rule.name) {
                    tracing::error!(
                        pod = %self.pod_name,
                        rule = %rule.name,
                        error = %e,
                        "monitor response failed"
                    );
                }
            }
        }

        Ok(())
    }

    fn evaluate_condition(
        &self,
        condition: &MonitorCondition,
        recent_entries: &[AuditEntry],
        usage: Option<&crate::types::ResourceUsage>,
    ) -> bool {
        match condition {
            MonitorCondition::MaxActionsPerMinute { limit } => {
                let cutoff = Utc::now() - chrono::Duration::seconds(60);
                let count = recent_entries
                    .iter()
                    .filter(|e| e.timestamp >= cutoff)
                    .count();
                count > *limit as usize
            }

            MonitorCondition::ResourceThreshold {
                resource,
                max_percent,
            } => {
                let Some(usage) = usage else {
                    return false;
                };
                match resource {
                    ResourceKind::Memory => {
                        // Read memory.max to compute percentage
                        let max = std::fs::read_to_string(
                            self.cgroup_path.join("memory.max"),
                        )
                        .ok()
                        .and_then(|s| s.trim().parse::<u64>().ok());

                        if let Some(max) = max {
                            if max > 0 {
                                let percent = (usage.memory_bytes as f64 / max as f64) * 100.0;
                                return percent > *max_percent;
                            }
                        }
                        false
                    }
                    ResourceKind::Cpu => {
                        // cpu_percent is not implemented in single-sample read_usage,
                        // so this always returns false for now.
                        usage.cpu_percent > *max_percent
                    }
                    ResourceKind::Pids => {
                        let max = std::fs::read_to_string(
                            self.cgroup_path.join("pids.max"),
                        )
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok());

                        if let Some(max) = max {
                            if max > 0 {
                                let percent =
                                    (usage.pid_count as f64 / max as f64) * 100.0;
                                return percent > *max_percent;
                            }
                        }
                        false
                    }
                }
            }

            MonitorCondition::ForbiddenAction { action } => {
                recent_entries.iter().any(|e| e.action == *action)
            }

            MonitorCondition::ForbiddenSequence {
                actions,
                window_secs,
            } => {
                if actions.is_empty() {
                    return false;
                }
                let cutoff = Utc::now() - chrono::Duration::seconds(*window_secs as i64);
                let windowed: Vec<&AuditEntry> = recent_entries
                    .iter()
                    .filter(|e| e.timestamp >= cutoff)
                    .collect();

                // Check if the sequence appears in order
                let mut seq_idx = 0;
                for entry in &windowed {
                    if entry.action == actions[seq_idx] {
                        seq_idx += 1;
                        if seq_idx == actions.len() {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    fn execute_response(&self, response: &MonitorResponse, rule_name: &str) -> Result<()> {
        let audit_log = AuditLog::new(&self.pod_dir);

        match response {
            MonitorResponse::Freeze => {
                cgroup::freeze(&self.cgroup_path)
                    .context("monitor freeze")?;

                audit_log.append(&AuditEntry {
                    timestamp: Utc::now(),
                    pod_name: self.pod_name.clone(),
                    action: AuditAction::MonitorFreeze,
                    detail: format!("rule={rule_name}"),
                    success: true,
                })?;
            }
            MonitorResponse::Restrict {
                cpu_cores,
                memory_bytes,
                max_pids,
            } => {
                let limits = ResourceLimits {
                    cpu_cores: *cpu_cores,
                    memory_bytes: *memory_bytes,
                    max_pids: *max_pids,
                    ..Default::default()
                };
                cgroup::set_limits(&self.cgroup_path, &limits)
                    .context("monitor restrict")?;

                audit_log.append(&AuditEntry {
                    timestamp: Utc::now(),
                    pod_name: self.pod_name.clone(),
                    action: AuditAction::MonitorRestrict,
                    detail: format!(
                        "rule={rule_name} cpu={:?} mem={:?} pids={:?}",
                        cpu_cores, memory_bytes, max_pids
                    ),
                    success: true,
                })?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_policy_yaml() {
        let yaml = r#"
check_interval_secs: 10
rules:
  - name: action_flood
    condition:
      type: max_actions_per_minute
      limit: 200
    response:
      type: freeze
  - name: memory_high
    condition:
      type: resource_threshold
      resource: memory
      max_percent: 90.0
    response:
      type: restrict
      memory_bytes: 268435456
  - name: no_budget_exceeded
    condition:
      type: forbidden_action
      action: budget_exceeded
    response:
      type: freeze
  - name: vault_then_dns
    condition:
      type: forbidden_sequence
      actions: [vault_get, dns_query]
      window_secs: 10
    response:
      type: freeze
"#;

        let policy: MonitorPolicy = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(policy.check_interval_secs, 10);
        assert_eq!(policy.rules.len(), 4);

        assert_eq!(policy.rules[0].name, "action_flood");
        match &policy.rules[0].condition {
            MonitorCondition::MaxActionsPerMinute { limit } => assert_eq!(*limit, 200),
            _ => panic!("expected MaxActionsPerMinute"),
        }
        assert!(matches!(policy.rules[0].response, MonitorResponse::Freeze));

        assert_eq!(policy.rules[1].name, "memory_high");
        match &policy.rules[1].condition {
            MonitorCondition::ResourceThreshold {
                resource,
                max_percent,
            } => {
                assert_eq!(*resource, ResourceKind::Memory);
                assert!((max_percent - 90.0).abs() < f64::EPSILON);
            }
            _ => panic!("expected ResourceThreshold"),
        }
        match &policy.rules[1].response {
            MonitorResponse::Restrict { memory_bytes, .. } => {
                assert_eq!(*memory_bytes, Some(268435456));
            }
            _ => panic!("expected Restrict"),
        }

        assert_eq!(policy.rules[2].name, "no_budget_exceeded");
        match &policy.rules[2].condition {
            MonitorCondition::ForbiddenAction { action } => {
                assert_eq!(*action, AuditAction::BudgetExceeded);
            }
            _ => panic!("expected ForbiddenAction"),
        }

        assert_eq!(policy.rules[3].name, "vault_then_dns");
        match &policy.rules[3].condition {
            MonitorCondition::ForbiddenSequence {
                actions,
                window_secs,
            } => {
                assert_eq!(actions.len(), 2);
                assert_eq!(actions[0], AuditAction::VaultGet);
                assert_eq!(actions[1], AuditAction::DnsQuery);
                assert_eq!(*window_secs, 10);
            }
            _ => panic!("expected ForbiddenSequence"),
        }
    }

    #[test]
    fn policy_default_check_interval() {
        let yaml = r#"
rules:
  - name: test
    condition:
      type: max_actions_per_minute
      limit: 100
    response:
      type: freeze
"#;
        let policy: MonitorPolicy = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(policy.check_interval_secs, 5);
    }

    #[test]
    fn evaluate_max_actions_per_minute() {
        let agent = MonitorAgent::new(
            MonitorPolicy {
                check_interval_secs: 5,
                rules: vec![],
            },
            PathBuf::from("/tmp/fake"),
            "test".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let now = Utc::now();
        let entries: Vec<AuditEntry> = (0..10)
            .map(|i| AuditEntry {
                timestamp: now - chrono::Duration::seconds(i),
                pod_name: "test".into(),
                action: AuditAction::DnsQuery,
                detail: format!("query {i}"),
                success: true,
            })
            .collect();

        // 10 entries in last minute, limit 5 → should fire
        let cond = MonitorCondition::MaxActionsPerMinute { limit: 5 };
        assert!(agent.evaluate_condition(&cond, &entries, None));

        // limit 20 → should not fire
        let cond = MonitorCondition::MaxActionsPerMinute { limit: 20 };
        assert!(!agent.evaluate_condition(&cond, &entries, None));
    }

    #[test]
    fn evaluate_forbidden_action() {
        let agent = MonitorAgent::new(
            MonitorPolicy {
                check_interval_secs: 5,
                rules: vec![],
            },
            PathBuf::from("/tmp/fake"),
            "test".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let entries = vec![AuditEntry {
            timestamp: Utc::now(),
            pod_name: "test".into(),
            action: AuditAction::BudgetExceeded,
            detail: "test".into(),
            success: true,
        }];

        let cond = MonitorCondition::ForbiddenAction {
            action: AuditAction::BudgetExceeded,
        };
        assert!(agent.evaluate_condition(&cond, &entries, None));

        let cond = MonitorCondition::ForbiddenAction {
            action: AuditAction::ToolBlocked,
        };
        assert!(!agent.evaluate_condition(&cond, &entries, None));
    }

    #[test]
    fn evaluate_forbidden_sequence() {
        let agent = MonitorAgent::new(
            MonitorPolicy {
                check_interval_secs: 5,
                rules: vec![],
            },
            PathBuf::from("/tmp/fake"),
            "test".into(),
            PathBuf::from("/tmp/fake-cgroup"),
        );

        let now = Utc::now();
        let entries = vec![
            AuditEntry {
                timestamp: now - chrono::Duration::seconds(5),
                pod_name: "test".into(),
                action: AuditAction::VaultGet,
                detail: "key=api_key".into(),
                success: true,
            },
            AuditEntry {
                timestamp: now - chrono::Duration::seconds(2),
                pod_name: "test".into(),
                action: AuditAction::DnsQuery,
                detail: "evil.com".into(),
                success: true,
            },
        ];

        // Should fire: vault_get → dns_query within 10s
        let cond = MonitorCondition::ForbiddenSequence {
            actions: vec![AuditAction::VaultGet, AuditAction::DnsQuery],
            window_secs: 10,
        };
        assert!(agent.evaluate_condition(&cond, &entries, None));

        // Should not fire: wrong order
        let cond = MonitorCondition::ForbiddenSequence {
            actions: vec![AuditAction::DnsQuery, AuditAction::VaultGet],
            window_secs: 10,
        };
        assert!(!agent.evaluate_condition(&cond, &entries, None));

        // Should not fire: window too small
        let cond = MonitorCondition::ForbiddenSequence {
            actions: vec![AuditAction::VaultGet, AuditAction::DnsQuery],
            window_secs: 1,
        };
        assert!(!agent.evaluate_condition(&cond, &entries, None));
    }

    #[test]
    fn policy_roundtrip_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("monitoring-policy.yaml");

        let policy = MonitorPolicy {
            check_interval_secs: 3,
            rules: vec![MonitorRule {
                name: "test_rule".into(),
                condition: MonitorCondition::MaxActionsPerMinute { limit: 100 },
                response: MonitorResponse::Freeze,
            }],
        };

        let yaml = serde_yaml::to_string(&policy).unwrap();
        std::fs::write(&path, &yaml).unwrap();

        let loaded = MonitorPolicy::from_file(&path).unwrap();
        assert_eq!(loaded.check_interval_secs, 3);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].name, "test_rule");
    }
}
