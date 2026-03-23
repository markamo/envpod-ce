// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Action-level audit log for pods.
//!
//! Each pod gets an append-only `audit.jsonl` file (one JSON object per line).
//! Every lifecycle action (create, start, stop, etc.) emits an entry.
//! The CLI reads this file for `envpod audit <pod>`.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Actions that can be recorded in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Create,
    Start,
    Stop,
    Kill,
    Freeze,
    Resume,
    Destroy,
    Diff,
    Commit,
    Rollback,
    Mount,
    Unmount,
    SetLimits,
    DnsQuery,
    QueueSubmit,
    QueueApprove,
    QueueCancel,
    QueueBlock,
    BudgetExceeded,
    ToolBlocked,
    VaultSet,
    VaultGet,
    VaultRemove,
    MonitorAlert,
    MonitorFreeze,
    MonitorRestrict,
    RemoteFreeze,
    RemoteResume,
    RemoteKill,
    RemoteRestrict,
    Undo,
    Restore,
    QueueAutoExecute,
    Clone,
    WebDisplayStart,
    WebDisplayStop,
    FileUpload,
    Resize,
    VaultProxy,
    AgentRegister,
    AgentRemove,
    AgentTokenGenerate,
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}"));
        f.write_str(&s)
    }
}

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub pod_name: String,
    pub action: AuditAction,
    pub detail: String,
    pub success: bool,
    /// Agent name if action was performed within an agent context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Append-only audit log backed by a JSONL file.
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// Open (or create) the audit log for a pod directory.
    ///
    /// The log file is `{pod_dir}/audit.jsonl`.
    pub fn new(pod_dir: &Path) -> Self {
        Self {
            path: pod_dir.join("audit.jsonl"),
        }
    }

    /// Append a single entry to the log.
    pub fn append(&self, entry: &AuditEntry) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open audit log: {}", self.path.display()))?;

        let line = serde_json::to_string(entry).context("serialize audit entry")?;
        writeln!(file, "{line}").context("write audit entry")?;
        Ok(())
    }

    /// Read entries from an arbitrary JSONL file (e.g. in-pod upload log).
    /// Skips malformed lines instead of failing.
    pub fn read_file(path: &Path) -> Result<Vec<AuditEntry>> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Ok(entry) = serde_json::from_str::<AuditEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Read all entries from the log. Returns an empty vec if the file doesn't exist.
    pub fn read_all(&self) -> Result<Vec<AuditEntry>> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("open audit log: {}", self.path.display())))
            }
        };

        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (i, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read audit line {}", i + 1))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(line)
                .with_context(|| format!("parse audit line {}: {line}", i + 1))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    /// Read entries starting from a given index.
    ///
    /// Returns `(entries, total_count)` where `total_count` is the total number
    /// of entries in the log. This allows incremental polling — call with the
    /// previously returned `total_count` to get only new entries.
    pub fn read_from(&self, start_index: usize) -> Result<(Vec<AuditEntry>, usize)> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("open audit log: {}", self.path.display())))
            }
        };

        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut total = 0usize;

        for (i, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read audit line {}", i + 1))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            total += 1;
            if total > start_index {
                let entry: AuditEntry = serde_json::from_str(line)
                    .with_context(|| format!("parse audit line {}: {line}", i + 1))?;
                entries.push(entry);
            }
        }

        Ok((entries, total))
    }

    /// Path to the underlying JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(action: AuditAction, detail: &str) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            pod_name: "test-pod".into(),
            action,
            detail: detail.into(),
            success: true,
            agent: None,
        }
    }

    #[test]
    fn append_and_read_all_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::new(tmp.path());

        let entries = vec![
            sample_entry(AuditAction::Create, "backend=native"),
            sample_entry(AuditAction::Start, "pid=1234, cmd=/bin/sh"),
            sample_entry(AuditAction::Destroy, "cleanup"),
        ];

        for e in &entries {
            log.append(e).unwrap();
        }

        let read_back = log.read_all().unwrap();
        assert_eq!(read_back.len(), 3);
        assert_eq!(read_back[0].action, AuditAction::Create);
        assert_eq!(read_back[0].detail, "backend=native");
        assert_eq!(read_back[1].action, AuditAction::Start);
        assert_eq!(read_back[2].action, AuditAction::Destroy);
        assert!(read_back.iter().all(|e| e.success));
    }

    #[test]
    fn read_empty_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::new(tmp.path());

        // File doesn't exist yet — should return empty vec, not error
        let entries = log.read_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn entry_serialization() {
        let entry = sample_entry(AuditAction::Commit, "3 files");
        let json = serde_json::to_string(&entry).unwrap();
        let recovered: AuditEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.action, AuditAction::Commit);
        assert_eq!(recovered.detail, "3 files");
        assert_eq!(recovered.pod_name, "test-pod");
        assert!(recovered.success);
    }

    #[test]
    fn action_display() {
        assert_eq!(AuditAction::Create.to_string(), "create");
        assert_eq!(AuditAction::Kill.to_string(), "kill");
        assert_eq!(AuditAction::SetLimits.to_string(), "set_limits");
        assert_eq!(AuditAction::DnsQuery.to_string(), "dns_query");
        assert_eq!(AuditAction::QueueSubmit.to_string(), "queue_submit");
        assert_eq!(AuditAction::QueueApprove.to_string(), "queue_approve");
        assert_eq!(AuditAction::QueueCancel.to_string(), "queue_cancel");
        assert_eq!(AuditAction::QueueBlock.to_string(), "queue_block");
        assert_eq!(AuditAction::MonitorAlert.to_string(), "monitor_alert");
        assert_eq!(AuditAction::MonitorFreeze.to_string(), "monitor_freeze");
        assert_eq!(AuditAction::RemoteFreeze.to_string(), "remote_freeze");
        assert_eq!(AuditAction::RemoteKill.to_string(), "remote_kill");
    }

    #[test]
    fn read_from_incremental() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::new(tmp.path());

        // Write 5 entries
        for i in 0..5 {
            log.append(&sample_entry(AuditAction::DnsQuery, &format!("query {i}")))
                .unwrap();
        }

        // Read all from 0
        let (entries, total) = log.read_from(0).unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(total, 5);

        // Read from index 3 (should get entries 4 and 5)
        let (entries, total) = log.read_from(3).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(total, 5);
        assert_eq!(entries[0].detail, "query 3");
        assert_eq!(entries[1].detail, "query 4");

        // Read from total (should get nothing)
        let (entries, total) = log.read_from(5).unwrap();
        assert_eq!(entries.len(), 0);
        assert_eq!(total, 5);

        // Append more, then incremental read
        log.append(&sample_entry(AuditAction::MonitorAlert, "threshold"))
            .unwrap();
        let (entries, total) = log.read_from(5).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(total, 6);
        assert_eq!(entries[0].action, AuditAction::MonitorAlert);
    }

    #[test]
    fn read_from_empty_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::new(tmp.path());

        let (entries, total) = log.read_from(0).unwrap();
        assert!(entries.is_empty());
        assert_eq!(total, 0);
    }
}
