// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Pod overlay snapshots — named checkpoints of the overlay upper/ directory.
//!
//! A snapshot is a copy of `{pod_dir}/upper/` taken at a point in time.
//! Snapshots can be created manually or automatically before each `envpod run`.
//! Restoring replaces the current upper/ with the snapshot's saved state.
//!
//! Layout:
//!   {pod_dir}/snapshots/
//!     index.json     — array of SnapshotMeta, ordered oldest → newest
//!     {id}/          — copy of upper/ at snapshot time
//!       ...
//!
//! Restore requires the pod to be stopped (overlay not mounted).
//! Create / list / destroy work on running or stopped pods.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Metadata for a single snapshot, stored in index.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// Short 8-char hex ID (unique per pod).
    pub id: String,
    /// Optional human-readable label.
    pub name: Option<String>,
    /// Creation timestamp.
    pub timestamp: DateTime<Utc>,
    /// Number of files in upper/ at snapshot time.
    pub file_count: usize,
    /// Total size of upper/ contents in bytes.
    pub size_bytes: u64,
    /// True if created automatically (e.g. before a run session).
    pub auto: bool,
}

impl SnapshotMeta {
    /// Human-readable label: name if set, else "auto" or the id.
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            if self.auto { "auto".into() } else { self.id.clone() }
        })
    }
}

// ---------------------------------------------------------------------------
// SnapshotStore
// ---------------------------------------------------------------------------

/// Manages snapshots for a single pod.
pub struct SnapshotStore {
    dir: PathBuf,    // {pod_dir}/snapshots/
    index: PathBuf,  // {pod_dir}/snapshots/index.json
}

impl SnapshotStore {
    pub fn new(pod_dir: &Path) -> Self {
        let dir = pod_dir.join("snapshots");
        let index = dir.join("index.json");
        Self { dir, index }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).context("create snapshots dir")
    }

    /// Create a snapshot of the current overlay upper/ directory.
    pub fn create(&self, upper_dir: &Path, name: Option<&str>, auto: bool) -> Result<SnapshotMeta> {
        self.ensure_dirs()?;
        let id: String = Uuid::new_v4().simple().to_string()[..8].to_string();
        let snap_dir = self.dir.join(&id);
        fs::create_dir_all(&snap_dir).context("create snapshot dir")?;

        let (file_count, size_bytes) = copy_dir_recursive(upper_dir, &snap_dir)?;

        let meta = SnapshotMeta {
            id,
            name: name.map(|s| s.to_string()),
            timestamp: Utc::now(),
            file_count,
            size_bytes,
            auto,
        };

        let mut index = self.read_index()?;
        index.push(meta.clone());
        self.write_index(&index)?;

        Ok(meta)
    }

    /// List all snapshots, newest first.
    pub fn list(&self) -> Result<Vec<SnapshotMeta>> {
        let mut index = self.read_index()?;
        index.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(index)
    }

    /// Get a single snapshot by exact id or unique prefix.
    pub fn get(&self, id: &str) -> Result<SnapshotMeta> {
        let index = self.read_index()?;
        // Exact match
        if let Some(s) = index.iter().find(|s| s.id == id) {
            return Ok(s.clone());
        }
        // Prefix match
        let matches: Vec<&SnapshotMeta> = index.iter().filter(|s| s.id.starts_with(id)).collect();
        match matches.len() {
            0 => anyhow::bail!("snapshot '{}' not found", id),
            1 => Ok(matches[0].clone()),
            _ => anyhow::bail!("ambiguous id '{}' matches {} snapshots", id, matches.len()),
        }
    }

    /// Restore upper/ to the state at a given snapshot.
    ///
    /// **The pod must be stopped** — the overlayfs must not be mounted.
    /// Clears the current upper/ and replaces it with the snapshot contents.
    pub fn restore(&self, upper_dir: &Path, id: &str) -> Result<()> {
        let snap = self.get(id)?;
        let snap_dir = self.dir.join(&snap.id);
        if !snap_dir.exists() {
            anyhow::bail!("snapshot data for '{}' is missing from disk", id);
        }

        // Clear current upper/ then replace with snapshot
        if upper_dir.exists() {
            fs::remove_dir_all(upper_dir).context("clear upper dir for restore")?;
        }
        fs::create_dir_all(upper_dir).context("recreate upper dir")?;
        copy_dir_recursive(&snap_dir, upper_dir).context("copy snapshot into upper")?;

        Ok(())
    }

    /// Delete a snapshot.
    pub fn destroy(&self, id: &str) -> Result<SnapshotMeta> {
        let meta = self.get(id)?;
        let snap_dir = self.dir.join(&meta.id);
        if snap_dir.exists() {
            fs::remove_dir_all(&snap_dir).context("remove snapshot dir")?;
        }
        let mut index = self.read_index()?;
        index.retain(|s| s.id != meta.id);
        self.write_index(&index)?;
        Ok(meta)
    }

    /// Prune oldest auto-created snapshots, keeping at most `max_keep` total
    /// snapshots. Manual (non-auto) snapshots are never pruned.
    ///
    /// Returns the number removed.
    pub fn prune(&self, max_keep: usize) -> Result<usize> {
        let index = self.read_index()?;
        if index.len() <= max_keep {
            return Ok(0);
        }
        // Sort oldest first; remove excess auto snapshots
        let mut sorted = index.clone();
        sorted.sort_by_key(|s| s.timestamp);

        let excess = index.len().saturating_sub(max_keep);
        let to_remove: Vec<String> = sorted
            .iter()
            .filter(|s| s.auto)
            .take(excess)
            .map(|s| s.id.clone())
            .collect();

        for id in &to_remove {
            let snap_dir = self.dir.join(id);
            if snap_dir.exists() {
                let _ = fs::remove_dir_all(&snap_dir);
            }
        }

        if !to_remove.is_empty() {
            let mut updated = index;
            updated.retain(|s| !to_remove.contains(&s.id));
            self.write_index(&updated)?;
        }

        Ok(to_remove.len())
    }

    fn read_index(&self) -> Result<Vec<SnapshotMeta>> {
        if !self.index.exists() {
            return Ok(Vec::new());
        }
        let s = fs::read_to_string(&self.index).context("read snapshot index")?;
        serde_json::from_str(&s).context("parse snapshot index")
    }

    fn write_index(&self, index: &[SnapshotMeta]) -> Result<()> {
        self.ensure_dirs()?;
        let s = serde_json::to_string_pretty(index).context("serialize snapshot index")?;
        let tmp = self.index.with_extension("tmp");
        fs::write(&tmp, &s).context("write snapshot index tmp")?;
        fs::rename(&tmp, &self.index).context("rename snapshot index")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Recursively copy a directory, returning (file_count, total_size_bytes).
/// Symlinks are re-created. Char-device whiteout files are skipped (they
/// represent overlayfs deletions and are re-created on restore if needed).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(usize, u64)> {
    if !src.exists() {
        return Ok((0, 0));
    }
    fs::create_dir_all(dst)?;
    let mut file_count = 0usize;
    let mut total_size = 0u64;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ft = entry.file_type()?;

        if ft.is_dir() {
            let (fc, sz) = copy_dir_recursive(&src_path, &dst_path)?;
            file_count += fc;
            total_size += sz;
        } else if ft.is_file() {
            fs::copy(&src_path, &dst_path)?;
            file_count += 1;
            total_size += entry.metadata()?.len();
        } else if ft.is_symlink() {
            let target = fs::read_link(&src_path)?;
            if dst_path.symlink_metadata().is_ok() {
                let _ = fs::remove_file(&dst_path);
            }
            std::os::unix::fs::symlink(&target, &dst_path)?;
            file_count += 1;
        }
        // Skip char-device whiteout entries — not meaningful outside an active overlay
    }

    Ok((file_count, total_size))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_upper(td: &TempDir) -> PathBuf {
        let upper = td.path().join("upper");
        fs::create_dir_all(upper.join("sub")).unwrap();
        fs::write(upper.join("file.txt"), "hello").unwrap();
        fs::write(upper.join("sub/nested.txt"), "world").unwrap();
        upper
    }

    #[test]
    fn create_and_list() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, Some("my snap"), false).unwrap();
        assert_eq!(snap.name.as_deref(), Some("my snap"));
        assert_eq!(snap.file_count, 2);
        assert!(!snap.auto);

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, snap.id);
    }

    #[test]
    fn create_auto() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, None, true).unwrap();
        assert!(snap.auto);
        assert_eq!(snap.display_name(), "auto");
    }

    #[test]
    fn restore_replaces_upper() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, None, false).unwrap();

        // Modify upper
        fs::write(upper.join("file.txt"), "modified").unwrap();
        fs::write(upper.join("extra.txt"), "new file").unwrap();

        // Restore
        store.restore(&upper, &snap.id).unwrap();
        assert_eq!(fs::read_to_string(upper.join("file.txt")).unwrap(), "hello");
        assert!(!upper.join("extra.txt").exists());
        assert!(upper.join("sub/nested.txt").exists());
    }

    #[test]
    fn destroy_removes_snapshot() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, None, false).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        store.destroy(&snap.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
    }

    #[test]
    fn prune_removes_oldest_auto_only() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        for _ in 0..3 {
            store.create(&upper, None, true).unwrap();
        }
        let manual = store.create(&upper, Some("keep"), false).unwrap();

        let removed = store.prune(2).unwrap();
        assert_eq!(removed, 2);

        let remaining = store.list().unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().any(|s| s.id == manual.id));
    }

    #[test]
    fn get_by_prefix() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, None, false).unwrap();
        let found = store.get(&snap.id[..4]).unwrap();
        assert_eq!(found.id, snap.id);
    }

    #[test]
    fn empty_upper_snapshot() {
        let td = TempDir::new().unwrap();
        let upper = td.path().join("upper");
        fs::create_dir_all(&upper).unwrap();
        let store = SnapshotStore::new(td.path());

        let snap = store.create(&upper, None, false).unwrap();
        assert_eq!(snap.file_count, 0);
        assert_eq!(snap.size_bytes, 0);
    }

    #[test]
    fn prune_respects_manual_snapshots() {
        let td = TempDir::new().unwrap();
        let upper = make_upper(&td);
        let store = SnapshotStore::new(td.path());

        // 5 manual snapshots, no auto
        for i in 0..5 {
            store.create(&upper, Some(&format!("manual-{i}")), false).unwrap();
        }
        // Prune with max_keep=2 — none removed (all manual)
        let removed = store.prune(2).unwrap();
        assert_eq!(removed, 0);
        assert_eq!(store.list().unwrap().len(), 5);
    }
}
