// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Garbage collection for orphaned pod resources.
//!
//! `envpod gc` cleans up resources left behind by destroyed pods:
//! - Stale iptables rules referencing dead veth interfaces
//! - Orphaned network namespaces (`envpod-*` in `/run/netns/`)
//! - Orphaned cgroups under `/sys/fs/cgroup/envpod/`
//! - Orphaned pod directories in `{base_dir}/pods/` with no state file
//! - Stale state files pointing to non-existent pod directories
//! - Stale netns index files for non-existent pods

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use super::{cgroup, netns, overlay};
use crate::store::PodStore;
use crate::backend::native::state::NativeState;

/// Results from a full garbage collection run.
#[derive(Debug, Default)]
pub struct GcResult {
    pub iptables_rules: usize,
    pub network_namespaces: usize,
    pub cgroups: usize,
    pub pod_directories: usize,
    pub state_files: usize,
    pub index_files: usize,
}

impl GcResult {
    pub fn total(&self) -> usize {
        self.iptables_rules
            + self.network_namespaces
            + self.cgroups
            + self.pod_directories
            + self.state_files
            + self.index_files
    }
}

/// Run full garbage collection. Returns counts of each resource type cleaned up.
pub fn gc_all(base_dir: &Path, store: &PodStore) -> Result<GcResult> {
    let mut result = GcResult::default();

    // Load all known pods to build sets of valid resources
    let pods = store.list().unwrap_or_default();

    let mut valid_netns: HashSet<String> = HashSet::new();
    let mut valid_cgroups: HashSet<String> = HashSet::new();
    let mut valid_pod_dirs: HashSet<String> = HashSet::new();
    let mut valid_indices: HashSet<u8> = HashSet::new();
    let mut valid_pod_ips: HashSet<String> = HashSet::new();

    for handle in &pods {
        if let Ok(state) = NativeState::from_handle(handle) {
            // Track valid pod directory (the UUID directory name)
            if let Some(dir_name) = state.pod_dir.file_name() {
                valid_pod_dirs.insert(dir_name.to_string_lossy().to_string());
            }

            // Track valid cgroup
            if let Some(ref cg) = state.cgroup_path {
                if let Some(cg_name) = cg.file_name() {
                    valid_cgroups.insert(cg_name.to_string_lossy().to_string());
                }
            }

            // Track valid network namespace, index, and pod IP
            if let Some(ref net) = state.network {
                valid_netns.insert(net.netns_name.clone());
                valid_indices.insert(net.pod_index);
                valid_pod_ips.insert(net.pod_ip.clone());
            }
        }
    }

    // 1. Stale iptables rules (dead veth interfaces + dead pod IPs)
    result.iptables_rules = netns::gc_iptables(&valid_pod_ips)?;

    // 2. Orphaned network namespaces
    result.network_namespaces = gc_network_namespaces(&valid_netns);

    // 3. Orphaned cgroups
    result.cgroups = gc_cgroups(&valid_cgroups);

    // 4. Orphaned pod directories
    result.pod_directories = gc_pod_directories(base_dir, &valid_pod_dirs);

    // 5. Stale state files
    result.state_files = gc_state_files(store, base_dir);

    // 6. Stale netns index files
    result.index_files = gc_index_files(base_dir, &valid_indices);

    // 7. Verify host network interface health
    let host_iface = netns::detect_host_interface_cached(Some(base_dir)).ok();
    if let Some(ref iface) = host_iface {
        netns::verify_host_interface(iface).ok();
    }

    Ok(result)
}

/// Remove network namespaces named `envpod-*` that don't belong to any known pod.
fn gc_network_namespaces(valid: &HashSet<String>) -> usize {
    let mut count = 0;
    let netns_dir = Path::new("/run/netns");

    let entries = match std::fs::read_dir(netns_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("envpod-") && !valid.contains(&name) {
            if std::process::Command::new("ip")
                .args(["netns", "del", &name])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok()
            {
                count += 1;
            }
        }
    }

    count
}

/// Remove envpod cgroup directories that don't belong to any known pod.
fn gc_cgroups(valid: &HashSet<String>) -> usize {
    let mut count = 0;
    let envpod_cgroup = Path::new(cgroup::CGROUP_BASE).join(cgroup::ENVPOD_SLICE);

    let entries = match std::fs::read_dir(&envpod_cgroup) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid.contains(&name) {
            // Kill any remaining processes before removing
            let cg_path = envpod_cgroup.join(&name);
            if cgroup::has_processes(&cg_path) {
                let _ = cgroup::kill_all(&cg_path, nix::sys::signal::Signal::SIGKILL);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if cgroup::destroy(&cg_path).is_ok() {
                count += 1;
            }
        }
    }

    count
}

/// Remove pod directories in `{base_dir}/pods/` that have no corresponding state file.
/// Unmounts any stale overlay/bind mounts first (the root cause of gc failures).
fn gc_pod_directories(base_dir: &Path, valid: &HashSet<String>) -> usize {
    let mut count = 0;
    let pods_dir = base_dir.join("pods");

    let entries = match std::fs::read_dir(&pods_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid.contains(&name) {
            let path = pods_dir.join(&name);
            if path.is_dir() {
                // Unmount all stale mounts under this pod directory first.
                // Without this, remove_dir_all silently fails on mounted dirs.
                overlay::unmount_all_under(&path);
                if std::fs::remove_dir_all(&path).is_ok() {
                    count += 1;
                }
            }
        }
    }

    count
}

/// Remove state files whose pod_dir no longer exists on disk.
fn gc_state_files(store: &PodStore, base_dir: &Path) -> usize {
    let mut count = 0;
    let state_dir = base_dir.join("state");

    let entries = match std::fs::read_dir(&state_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            if let Some(stem) = path.file_stem() {
                let name = stem.to_string_lossy().to_string();
                // Try to load the handle and check if pod_dir exists
                if let Ok(handle) = store.load(&name) {
                    if let Ok(state) = NativeState::from_handle(&handle) {
                        if !state.pod_dir.exists() {
                            if store.remove(&name).is_ok() {
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    count
}

/// Remove netns index files for indices not used by any known pod.
fn gc_index_files(base_dir: &Path, valid: &HashSet<u8>) -> usize {
    let mut count = 0;
    let index_dir = base_dir.join("netns_index");

    let entries = match std::fs::read_dir(&index_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(idx) = name.parse::<u8>() {
            if !valid.contains(&idx) {
                let path = index_dir.join(&name);
                if std::fs::remove_file(&path).is_ok() {
                    count += 1;
                }
            }
        }
    }

    count
}
