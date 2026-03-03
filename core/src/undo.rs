// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Undo registry for reversible pod actions.
//!
//! Tracks executed actions and how to reverse them. Each pod gets an
//! `undo.json` file that persists across CLI invocations. The `envpod undo`
//! command reads this file to list and execute undo operations.
//!
//! This is separate from the action queue (which handles approval flow).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::ResourceLimits;

/// Status of an undo entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoStatus {
    /// Can still be undone.
    Pending,
    /// Already reversed.
    Undone,
}

/// How to reverse an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UndoMechanism {
    /// Unmount a path that was mounted.
    Unmount { path: PathBuf },
    /// Rollback the overlay (envpod rollback).
    Rollback,
    /// Resume (thaw) a frozen pod.
    Thaw,
    /// Restore previous resource limits.
    RestoreLimits { limits: ResourceLimits },
}

/// A single reversible action record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub mechanism: UndoMechanism,
    pub status: UndoStatus,
}

/// Persistent registry of undo-able actions for a pod.
pub struct UndoRegistry {
    path: PathBuf,
}

impl UndoRegistry {
    /// Open (or create) the undo registry for a pod directory.
    ///
    /// The registry file is `{pod_dir}/undo.json`.
    pub fn new(pod_dir: &Path) -> Self {
        Self {
            path: pod_dir.join("undo.json"),
        }
    }

    /// Register a new reversible action. Returns the created entry.
    pub fn register(&self, description: &str, mechanism: UndoMechanism) -> Result<UndoEntry> {
        let entry = UndoEntry {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            description: description.to_string(),
            mechanism,
            status: UndoStatus::Pending,
        };

        let mut entries = self.load()?;
        entries.push(entry.clone());
        self.save(&entries)?;

        Ok(entry)
    }

    /// List all entries (pending and undone).
    pub fn list(&self) -> Result<Vec<UndoEntry>> {
        self.load()
    }

    /// List only pending (not yet undone) entries.
    pub fn list_pending(&self) -> Result<Vec<UndoEntry>> {
        Ok(self
            .load()?
            .into_iter()
            .filter(|e| e.status == UndoStatus::Pending)
            .collect())
    }

    /// Mark a single entry as undone by ID.
    ///
    /// Returns the mechanism so the caller can execute it.
    /// Does NOT execute the undo — the caller is responsible for that.
    pub fn mark_undone(&self, id: Uuid) -> Result<UndoEntry> {
        let mut entries = self.load()?;
        let entry = entries
            .iter_mut()
            .find(|e| e.id == id)
            .with_context(|| format!("undo entry {id} not found"))?;

        if entry.status == UndoStatus::Undone {
            anyhow::bail!("action {} is already undone", &id.to_string()[..8]);
        }

        entry.status = UndoStatus::Undone;
        let result = entry.clone();
        self.save(&entries)?;

        Ok(result)
    }

    /// Path to the underlying JSON file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // -- internal ---------------------------------------------------------

    fn load(&self) -> Result<Vec<UndoEntry>> {
        match fs::read_to_string(&self.path) {
            Ok(content) => {
                let entries: Vec<UndoEntry> = serde_json::from_str(&content)
                    .with_context(|| format!("parse undo registry: {}", self.path.display()))?;
                Ok(entries)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("read undo registry: {}", self.path.display()))),
        }
    }

    fn save(&self, entries: &[UndoEntry]) -> Result<()> {
        let json =
            serde_json::to_string_pretty(entries).context("serialize undo registry")?;
        fs::write(&self.path, json)
            .with_context(|| format!("write undo registry: {}", self.path.display()))?;
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
    fn register_and_list() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = UndoRegistry::new(tmp.path());

        let e1 = reg
            .register("mounted /data", UndoMechanism::Unmount { path: "/data".into() })
            .unwrap();
        let e2 = reg
            .register("froze pod", UndoMechanism::Thaw)
            .unwrap();

        let all = reg.list().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, e1.id);
        assert_eq!(all[1].id, e2.id);
        assert!(all.iter().all(|e| e.status == UndoStatus::Pending));
    }

    #[test]
    fn list_pending_filters_undone() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = UndoRegistry::new(tmp.path());

        let e1 = reg
            .register("mounted /data", UndoMechanism::Unmount { path: "/data".into() })
            .unwrap();
        reg.register("froze pod", UndoMechanism::Thaw).unwrap();

        reg.mark_undone(e1.id).unwrap();

        let pending = reg.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].description, "froze pod");
    }

    #[test]
    fn mark_undone_idempotent_error() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = UndoRegistry::new(tmp.path());

        let entry = reg
            .register("froze pod", UndoMechanism::Thaw)
            .unwrap();

        reg.mark_undone(entry.id).unwrap();
        let err = reg.mark_undone(entry.id).unwrap_err();
        assert!(err.to_string().contains("already undone"));
    }

    #[test]
    fn empty_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = UndoRegistry::new(tmp.path());

        assert!(reg.list().unwrap().is_empty());
        assert!(reg.list_pending().unwrap().is_empty());
    }

    #[test]
    fn persistence_across_instances() {
        let tmp = tempfile::tempdir().unwrap();

        // First instance — register
        {
            let reg = UndoRegistry::new(tmp.path());
            reg.register("mounted /data", UndoMechanism::Unmount { path: "/data".into() })
                .unwrap();
        }

        // Second instance — should see the entry
        {
            let reg = UndoRegistry::new(tmp.path());
            let all = reg.list().unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].description, "mounted /data");
        }
    }

    #[test]
    fn mechanism_serialization() {
        let mechanisms = vec![
            UndoMechanism::Unmount { path: "/mnt/data".into() },
            UndoMechanism::Rollback,
            UndoMechanism::Thaw,
            UndoMechanism::RestoreLimits {
                limits: ResourceLimits {
                    cpu_cores: Some(2.0),
                    memory_bytes: Some(1024 * 1024 * 512),
                    ..Default::default()
                },
            },
        ];

        for mech in mechanisms {
            let json = serde_json::to_string(&mech).unwrap();
            let recovered: UndoMechanism = serde_json::from_str(&json).unwrap();
            // Just verify round-trip doesn't panic
            let _ = format!("{recovered:?}");
        }
    }
}
