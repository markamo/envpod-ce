// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! State reading helpers for the dashboard.
//!
//! Wraps core functions to read pod state from the filesystem.
//! No database — everything comes from existing files.

use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use envpod_core::audit::{AuditEntry, AuditLog};
use envpod_core::backend::native::state::NativeState;
use envpod_core::backend::native::filter_diff;
use envpod_core::backend::create_backend;
use envpod_core::config::PodConfig;
use envpod_core::store::PodStore;
use envpod_core::types::PodHandle;

/// Summary of a pod for fleet overview.
#[derive(Debug, Serialize)]
pub struct PodSummary {
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub backend: String,
    pub pod_ip: Option<String>,
}

/// Live resource stats from cgroup.
#[derive(Debug, Serialize)]
pub struct ResourceStats {
    pub cpu_usage_usec: u64,
    pub memory_bytes: u64,
    pub memory_limit: Option<u64>,
    pub pids_current: u32,
    pub pids_limit: Option<u32>,
}

/// Detailed pod info for the detail view.
#[derive(Debug, Serialize)]
pub struct PodDetail {
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub backend: String,
    pub pod_dir: String,
    pub pod_ip: Option<String>,
    pub config: Option<PodConfig>,
    pub resources: Option<ResourceStats>,
    pub diff_count: usize,
    pub vault_keys: Vec<String>,
}

/// A filesystem diff entry for the dashboard.
#[derive(Debug, Serialize)]
pub struct DiffEntry {
    pub path: String,
    pub kind: String,
    pub size: u64,
}

/// A single line in a file diff.
#[derive(Debug, Serialize)]
pub struct DiffLine {
    pub kind: String,       // "add" | "remove" | "context" | "separator"
    pub text: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

/// Full inline diff for one file.
#[derive(Debug, Serialize)]
pub struct FileDiffResult {
    pub path: String,
    pub kind: String,
    pub binary: bool,
    pub lines: Vec<DiffLine>,
}

/// List all pods with lightweight summary (no diff count, no resource stats).
/// Designed for fast fleet view — no expensive filesystem walks.
pub fn list_pods(store: &PodStore, _base_dir: &Path) -> Result<Vec<PodSummary>> {
    let handles = store.list()?;
    let mut summaries = Vec::new();

    for handle in handles {
        let state = match NativeState::from_handle(&handle) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let pod_ip = state.network.as_ref().map(|n| n.pod_ip.clone());

        summaries.push(PodSummary {
            name: handle.name.clone(),
            status: format!("{:?}", state.status).to_lowercase(),
            created_at: handle.created_at.to_rfc3339(),
            backend: handle.backend.clone(),
            pod_ip,
        });
    }

    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

/// Get detailed info for a single pod.
pub fn pod_detail(store: &PodStore, base_dir: &Path, name: &str) -> Result<PodDetail> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let config = state.load_config()?.unwrap_or_default();
    let diff_count = count_diffs(&handle, &state, &config, base_dir);
    let resources = read_resources(&state);

    let vault_keys = envpod_core::vault::Vault::new(&state.pod_dir)
        .ok()
        .and_then(|v| v.list().ok())
        .unwrap_or_default();

    let pod_ip = state.network.as_ref().map(|n| n.pod_ip.clone());

    Ok(PodDetail {
        name: handle.name.clone(),
        status: format!("{:?}", state.status).to_lowercase(),
        created_at: handle.created_at.to_rfc3339(),
        backend: handle.backend.clone(),
        pod_dir: state.pod_dir.display().to_string(),
        pod_ip,
        config: Some(config.clone()),
        resources,
        diff_count,
        vault_keys,
    })
}

/// Read audit log entries with pagination.
pub fn read_audit(store: &PodStore, name: &str, offset: usize, limit: usize) -> Result<(Vec<AuditEntry>, usize)> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let log = AuditLog::new(&state.pod_dir);
    let (mut all_entries, _) = log.read_from(0)?;

    // Merge file upload audit entries from in-pod upload server
    let upload_log = state.pod_dir.join("upper/tmp/envpod-uploads.jsonl");
    if upload_log.exists() {
        if let Ok(upload_entries) = AuditLog::read_file(&upload_log) {
            all_entries.extend(upload_entries);
            all_entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        }
    }

    let total = all_entries.len();
    let start = offset.min(total);
    let end = (start + limit).min(total);
    let page = all_entries[start..end].to_vec();

    Ok((page, total))
}

/// Read filesystem diffs for a pod.
pub fn read_diff(store: &PodStore, base_dir: &Path, name: &str) -> Result<Vec<DiffEntry>> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let config = state.load_config()?.unwrap_or_default();

    let backend = create_backend(&handle.backend, base_dir)?;
    let all_diffs = backend.diff(&handle)?;
    let diffs = filter_diff(all_diffs, &config.filesystem.tracking);

    Ok(diffs
        .into_iter()
        .map(|d| DiffEntry {
            path: d.path.display().to_string(),
            kind: format!("{:?}", d.kind),
            size: d.size,
        })
        .collect())
}

/// Read live cgroup resource stats for a pod.
pub fn read_resources(state: &NativeState) -> Option<ResourceStats> {
    let cgroup = state.cgroup_path.as_ref()?;

    let cpu = read_cgroup_u64(&cgroup.join("cpu.stat"), "usage_usec").unwrap_or(0);
    let mem = read_cgroup_file_u64(&cgroup.join("memory.current"))
        .ok()
        .flatten()
        .unwrap_or(0);
    let mem_limit = read_cgroup_file_u64(&cgroup.join("memory.max")).ok().flatten();
    let pids = read_cgroup_file_u64(&cgroup.join("pids.current"))
        .ok()
        .flatten()
        .unwrap_or(0) as u32;
    let pids_limit = read_cgroup_file_u64(&cgroup.join("pids.max"))
        .ok()
        .flatten()
        .map(|v| v as u32);

    Some(ResourceStats {
        cpu_usage_usec: cpu,
        memory_bytes: mem,
        memory_limit: mem_limit,
        pids_current: pids,
        pids_limit,
    })
}

/// Get the NativeState for a pod (for use in action handlers).
pub fn get_state(store: &PodStore, name: &str) -> Result<(PodHandle, NativeState)> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    Ok((handle, state))
}

fn count_diffs(handle: &PodHandle, _state: &NativeState, config: &PodConfig, base_dir: &Path) -> usize {
    let backend = match create_backend(&handle.backend, base_dir) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let all_diffs = match backend.diff(handle) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    filter_diff(all_diffs, &config.filesystem.tracking).len()
}

/// Generate an inline git-style diff for a single file in the pod overlay.
///
/// For Added files:   reads upper/<path>, returns all lines as additions.
/// For Modified files: diffs upper/<path> vs host <path>, shows context-limited hunks.
/// For Deleted files:  reads host <path>, returns all lines as removals.
/// Binary files:       returns binary=true, empty lines.
pub fn read_file_diff(store: &PodStore, _base_dir: &Path, name: &str, file_path: &str) -> Result<FileDiffResult> {
    use similar::{ChangeTag, TextDiff};

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;

    // Normalise path: strip leading '/' for overlay-relative access
    let rel = file_path.trim_start_matches('/');
    let upper_file = state.pod_dir.join("upper").join(rel);
    let host_file = std::path::Path::new("/").join(rel);

    // Determine kind from overlay state
    let kind = if upper_file.exists() {
        let m = std::fs::metadata(&upper_file)?;
        if m.file_type().is_char_device() {
            // Whiteout entry — overlayfs deletion marker
            "Deleted"
        } else if host_file.exists() {
            "Modified"
        } else {
            "Added"
        }
    } else if host_file.exists() {
        "Deleted"
    } else {
        anyhow::bail!("file not found in overlay or host: {file_path}");
    };

    let read_text = |path: &std::path::Path| -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        // Treat as binary if > 512 KB or not valid UTF-8
        if bytes.len() > 512 * 1024 { return None; }
        String::from_utf8(bytes).ok()
    };

    let lines = match kind {
        "Added" => {
            let Some(content) = read_text(&upper_file) else {
                return Ok(FileDiffResult { path: file_path.into(), kind: kind.into(), binary: true, lines: vec![] });
            };
            content.lines().enumerate().map(|(i, line)| DiffLine {
                kind: "add".into(),
                text: line.to_string(),
                old_line: None,
                new_line: Some((i + 1) as u32),
            }).collect()
        }
        "Deleted" => {
            let Some(content) = read_text(&host_file) else {
                return Ok(FileDiffResult { path: file_path.into(), kind: kind.into(), binary: true, lines: vec![] });
            };
            content.lines().enumerate().map(|(i, line)| DiffLine {
                kind: "remove".into(),
                text: line.to_string(),
                old_line: Some((i + 1) as u32),
                new_line: None,
            }).collect()
        }
        "Modified" => {
            let (Some(old), Some(new)) = (read_text(&host_file), read_text(&upper_file)) else {
                return Ok(FileDiffResult { path: file_path.into(), kind: kind.into(), binary: true, lines: vec![] });
            };
            let diff = TextDiff::from_lines(old.as_str(), new.as_str());
            let mut result = Vec::new();
            for (group_idx, group) in diff.grouped_ops(3).iter().enumerate() {
                if group_idx > 0 {
                    result.push(DiffLine { kind: "separator".into(), text: "·····".into(), old_line: None, new_line: None });
                }
                for op in group {
                    for change in diff.iter_changes(op) {
                        let (line_kind, old_n, new_n) = match change.tag() {
                            ChangeTag::Delete => ("remove", change.old_index().map(|i| (i + 1) as u32), None),
                            ChangeTag::Insert => ("add",    None, change.new_index().map(|i| (i + 1) as u32)),
                            ChangeTag::Equal  => ("context", change.old_index().map(|i| (i + 1) as u32), change.new_index().map(|i| (i + 1) as u32)),
                        };
                        result.push(DiffLine {
                            kind: line_kind.into(),
                            text: change.value().trim_end_matches('\n').to_string(),
                            old_line: old_n,
                            new_line: new_n,
                        });
                    }
                }
            }
            result
        }
        _ => vec![],
    };

    Ok(FileDiffResult { path: file_path.into(), kind: kind.into(), binary: false, lines })
}

fn read_cgroup_file_u64(path: &Path) -> Result<Option<u64>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let s = s.trim();
            if s == "max" {
                Ok(None)
            } else {
                Ok(Some(s.parse().unwrap_or(0)))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_cgroup_u64(path: &Path, key: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim();
            return rest.parse().ok();
        }
    }
    None
}
