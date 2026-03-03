// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Pod handle persistence.
//!
//! Saves/loads `PodHandle` as JSON files in a state directory so pods
//! survive across CLI invocations. Each pod is stored as `{name}.json`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::types::PodHandle;

pub struct PodStore {
    state_dir: PathBuf,
}

impl PodStore {
    pub fn new(state_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("create state dir: {}", state_dir.display()))?;
        Ok(Self { state_dir })
    }

    fn handle_path(&self, name: &str) -> PathBuf {
        self.state_dir.join(format!("{name}.json"))
    }

    /// Save a pod handle to disk.
    pub fn save(&self, handle: &PodHandle) -> Result<()> {
        let path = self.handle_path(&handle.name);
        let json = serde_json::to_string_pretty(handle)
            .context("serialize pod handle")?;
        fs::write(&path, json)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Load a pod handle by name.
    pub fn load(&self, name: &str) -> Result<PodHandle> {
        let path = self.handle_path(name);
        let json = fs::read_to_string(&path)
            .with_context(|| format!("pod not found: {name}"))?;
        serde_json::from_str(&json).context("deserialize pod handle")
    }

    /// Check if a pod exists.
    pub fn exists(&self, name: &str) -> bool {
        self.handle_path(name).exists()
    }

    /// Remove a pod's persisted state.
    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.handle_path(name);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(())
    }

    /// List all persisted pod handles.
    pub fn list(&self) -> Result<Vec<PodHandle>> {
        let mut pods = Vec::new();
        let entries = match fs::read_dir(&self.state_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(pods),
            Err(e) => return Err(e.into()),
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(json) = fs::read_to_string(&path) {
                    if let Ok(handle) = serde_json::from_str::<PodHandle>(&json) {
                        pods.push(handle);
                    }
                }
            }
        }

        pods.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(pods)
    }

    /// Return the state directory path.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_handle(name: &str) -> PodHandle {
        PodHandle {
            id: Uuid::new_v4(),
            name: name.into(),
            backend: "native".into(),
            created_at: Utc::now(),
            backend_state: serde_json::json!({"status": "created"}),
        }
    }

    #[test]
    fn save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();

        let handle = make_handle("test-pod");
        store.save(&handle).unwrap();

        let loaded = store.load("test-pod").unwrap();
        assert_eq!(loaded.id, handle.id);
        assert_eq!(loaded.name, "test-pod");
    }

    #[test]
    fn load_nonexistent_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();
        assert!(store.load("nonexistent").is_err());
    }

    #[test]
    fn exists_check() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();

        assert!(!store.exists("test-pod"));
        store.save(&make_handle("test-pod")).unwrap();
        assert!(store.exists("test-pod"));
    }

    #[test]
    fn remove_deletes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();

        store.save(&make_handle("test-pod")).unwrap();
        assert!(store.exists("test-pod"));

        store.remove("test-pod").unwrap();
        assert!(!store.exists("test-pod"));
    }

    #[test]
    fn remove_nonexistent_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();
        store.remove("nonexistent").unwrap();
    }

    #[test]
    fn list_returns_all_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();

        store.save(&make_handle("charlie")).unwrap();
        store.save(&make_handle("alpha")).unwrap();
        store.save(&make_handle("bravo")).unwrap();

        let pods = store.list().unwrap();
        assert_eq!(pods.len(), 3);
        assert_eq!(pods[0].name, "alpha");
        assert_eq!(pods[1].name, "bravo");
        assert_eq!(pods[2].name, "charlie");
    }

    #[test]
    fn list_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PodStore::new(tmp.path().to_path_buf()).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
