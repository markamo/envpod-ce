// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Backend-specific state stored in PodHandle.backend_state.
//! Serialized to JSON so pods survive daemon restarts.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::PodConfig;
use crate::types::PodHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeState {
    pub pod_dir: PathBuf,
    pub cgroup_path: Option<PathBuf>,
    pub init_pid: Option<u32>,
    pub status: NativeStatus,
    /// Lower directories for the overlay mount.
    /// Default: ["/"] (entire host root filesystem).
    pub lower_dirs: Vec<PathBuf>,
    /// Network namespace state. None = host network (Unsafe mode).
    #[serde(default)]
    pub network: Option<NetworkState>,
}

/// Persisted network namespace state for a pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkState {
    /// Name of the network namespace (e.g. "envpod-a1b2c3d4").
    pub netns_name: String,
    /// Path to the netns file (e.g. /run/netns/envpod-a1b2c3d4).
    pub netns_path: PathBuf,
    /// Host-side veth interface name.
    pub host_veth: String,
    /// Pod-side veth interface name.
    pub pod_veth: String,
    /// Host-side veth IP (e.g. "10.200.1.1").
    pub host_ip: String,
    /// Pod-side veth IP (e.g. "10.200.1.2").
    pub pod_ip: String,
    /// Allocated pod index (1..254) for subnet assignment.
    pub pod_index: u8,
    /// Host's default outbound interface (for NAT masquerade).
    pub host_interface: String,
    /// DNS filtering mode.
    pub dns_mode: String,
    /// Allowed domains (for whitelist mode).
    pub dns_allow: Vec<String>,
    /// Denied domains (for blacklist mode).
    pub dns_deny: Vec<String>,
    /// Domain remapping table.
    pub dns_remap: HashMap<String, String>,
    /// Subnet base (e.g. "10.200" or "10.201"). Default: "10.200".
    #[serde(default = "default_subnet_base")]
    pub subnet_base: String,
}

fn default_subnet_base() -> String {
    "10.200".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NativeStatus {
    Created,
    Running,
    Frozen,
    Stopped,
}

impl NativeState {
    pub fn from_handle(handle: &PodHandle) -> Result<Self> {
        serde_json::from_value(handle.backend_state.clone())
            .context("failed to deserialize native backend state")
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("NativeState serialization cannot fail")
    }

    pub fn upper_dir(&self) -> PathBuf {
        self.pod_dir.join("upper")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.pod_dir.join("work")
    }

    pub fn merged_dir(&self) -> PathBuf {
        self.pod_dir.join("merged")
    }

    /// Upper dir for per-system-dir COW overlays (advanced/dangerous mode).
    pub fn sys_upper_dir(&self) -> PathBuf {
        self.pod_dir.join("sys_upper")
    }

    /// Work dir for per-system-dir COW overlays (advanced/dangerous mode).
    pub fn sys_work_dir(&self) -> PathBuf {
        self.pod_dir.join("sys_work")
    }

    /// Path to the minimal rootfs used as overlay lower layer.
    /// Contains only system essentials — not the entire host filesystem.
    pub fn rootfs_dir(&self) -> PathBuf {
        self.pod_dir.join("rootfs")
    }

    /// Path to the network namespace file, if network isolation is active.
    pub fn netns_path(&self) -> Option<PathBuf> {
        self.network.as_ref().map(|n| n.netns_path.clone())
    }

    /// Path to the pod's stdout/stderr log file.
    pub fn log_path(&self) -> PathBuf {
        self.pod_dir.join("run.log")
    }

    /// Path to the persisted pod.yaml configuration.
    pub fn config_path(&self) -> PathBuf {
        self.pod_dir.join("pod.yaml")
    }

    /// Load the persisted pod configuration, if it exists.
    pub fn load_config(&self) -> Result<Option<PodConfig>> {
        let path = self.config_path();
        if path.exists() {
            let config = PodConfig::from_file(&path)?;
            Ok(Some(config))
        } else {
            Ok(None)
        }
    }
}

impl NetworkState {
    /// Persist network state to `{pod_dir}/network-state.json`.
    pub fn save(&self, pod_dir: &std::path::Path) -> Result<()> {
        let path = pod_dir.join("network-state.json");
        let json = serde_json::to_string_pretty(self).context("serialize network state")?;
        std::fs::write(&path, json)
            .with_context(|| format!("write network state: {}", path.display()))?;
        Ok(())
    }

    /// Load network state from `{pod_dir}/network-state.json`.
    /// Returns None if the file doesn't exist.
    pub fn load(pod_dir: &std::path::Path) -> Result<Option<Self>> {
        let path = pod_dir.join("network-state.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => {
                let state = serde_json::from_str(&json).context("parse network-state.json")?;
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("read network state: {}", path.display()))),
        }
    }

    /// Update DNS allow/deny lists in place.
    pub fn update_dns_lists(
        &mut self,
        add_allow: &[String],
        add_deny: &[String],
        remove_allow: &[String],
        remove_deny: &[String],
    ) {
        // Add new entries (avoid duplicates)
        for domain in add_allow {
            if !self.dns_allow.contains(domain) {
                self.dns_allow.push(domain.clone());
            }
        }
        for domain in add_deny {
            if !self.dns_deny.contains(domain) {
                self.dns_deny.push(domain.clone());
            }
        }

        // Remove entries
        self.dns_allow.retain(|d| !remove_allow.contains(d));
        self.dns_deny.retain(|d| !remove_deny.contains(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let state = NativeState {
            pod_dir: PathBuf::from("/var/lib/envpod/pods/abc"),
            cgroup_path: Some(PathBuf::from("/sys/fs/cgroup/envpod/abc")),
            init_pid: Some(12345),
            status: NativeStatus::Running,
            lower_dirs: vec![PathBuf::from("/")],
            network: None,
        };

        let json = state.to_json();
        let recovered: NativeState = serde_json::from_value(json).unwrap();

        assert_eq!(recovered.pod_dir, state.pod_dir);
        assert_eq!(recovered.cgroup_path, state.cgroup_path);
        assert_eq!(recovered.init_pid, Some(12345));
        assert_eq!(recovered.status, NativeStatus::Running);
        assert_eq!(recovered.lower_dirs, vec![PathBuf::from("/")]);
        assert!(recovered.network.is_none());
    }

    #[test]
    fn round_trips_with_network_state() {
        let state = NativeState {
            pod_dir: PathBuf::from("/var/lib/envpod/pods/abc"),
            cgroup_path: None,
            init_pid: None,
            status: NativeStatus::Created,
            lower_dirs: vec![PathBuf::from("/")],
            network: Some(NetworkState {
                netns_name: "envpod-abc".into(),
                netns_path: PathBuf::from("/run/netns/envpod-abc"),
                host_veth: "veth-abc-h".into(),
                pod_veth: "veth-abc-p".into(),
                host_ip: "10.200.1.1".into(),
                pod_ip: "10.200.1.2".into(),
                pod_index: 1,
                host_interface: "eth0".into(),
                dns_mode: "whitelist".into(),
                dns_allow: vec!["anthropic.com".into()],
                dns_deny: Vec::new(),
                dns_remap: std::collections::HashMap::new(),
                subnet_base: "10.200".into(),
            }),
        };

        let json = state.to_json();
        let recovered: NativeState = serde_json::from_value(json).unwrap();

        let net = recovered.network.unwrap();
        assert_eq!(net.netns_name, "envpod-abc");
        assert_eq!(net.host_ip, "10.200.1.1");
        assert_eq!(net.pod_index, 1);
        assert_eq!(net.dns_allow, vec!["anthropic.com"]);
        assert_eq!(net.subnet_base, "10.200");
    }

    #[test]
    fn backward_compat_without_network_field() {
        // Old state JSON without network field should deserialize fine
        let json = serde_json::json!({
            "pod_dir": "/var/lib/envpod/pods/old",
            "cgroup_path": null,
            "init_pid": null,
            "status": "created",
            "lower_dirs": ["/"]
        });

        let state: NativeState = serde_json::from_value(json).unwrap();
        assert!(state.network.is_none());
        assert_eq!(state.pod_dir, PathBuf::from("/var/lib/envpod/pods/old"));
    }

    #[test]
    fn network_state_save_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let net = NetworkState {
            netns_name: "envpod-test".into(),
            netns_path: PathBuf::from("/run/netns/envpod-test"),
            host_veth: "veth-test-h".into(),
            pod_veth: "veth-test-p".into(),
            host_ip: "10.200.1.1".into(),
            pod_ip: "10.200.1.2".into(),
            pod_index: 1,
            host_interface: "eth0".into(),
            dns_mode: "whitelist".into(),
            dns_allow: vec!["anthropic.com".into()],
            dns_deny: vec!["evil.com".into()],
            dns_remap: HashMap::new(),
            subnet_base: "10.200".into(),
        };

        net.save(tmp.path()).unwrap();
        let loaded = NetworkState::load(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.netns_name, "envpod-test");
        assert_eq!(loaded.dns_allow, vec!["anthropic.com"]);
        assert_eq!(loaded.dns_deny, vec!["evil.com"]);
        assert_eq!(loaded.subnet_base, "10.200");
    }

    #[test]
    fn network_state_load_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = NetworkState::load(tmp.path()).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn update_dns_lists_adds_and_removes() {
        let mut net = NetworkState {
            netns_name: "test".into(),
            netns_path: PathBuf::from("/run/netns/test"),
            host_veth: "vh".into(),
            pod_veth: "vp".into(),
            host_ip: "10.200.1.1".into(),
            pod_ip: "10.200.1.2".into(),
            pod_index: 1,
            host_interface: "eth0".into(),
            dns_mode: "whitelist".into(),
            dns_allow: vec!["existing.com".into()],
            dns_deny: vec!["old-deny.com".into()],
            dns_remap: HashMap::new(),
            subnet_base: "10.200".into(),
        };

        net.update_dns_lists(
            &["new-allow.com".into()],
            &["new-deny.com".into()],
            &[],
            &["old-deny.com".into()],
        );

        assert_eq!(net.dns_allow, vec!["existing.com", "new-allow.com"]);
        assert_eq!(net.dns_deny, vec!["new-deny.com"]);
    }

    #[test]
    fn update_dns_lists_avoids_duplicates() {
        let mut net = NetworkState {
            netns_name: "test".into(),
            netns_path: PathBuf::from("/run/netns/test"),
            host_veth: "vh".into(),
            pod_veth: "vp".into(),
            host_ip: "10.200.1.1".into(),
            pod_ip: "10.200.1.2".into(),
            pod_index: 1,
            host_interface: "eth0".into(),
            dns_mode: "whitelist".into(),
            dns_allow: vec!["anthropic.com".into()],
            dns_deny: Vec::new(),
            dns_remap: HashMap::new(),
            subnet_base: "10.200".into(),
        };

        net.update_dns_lists(&["anthropic.com".into()], &[], &[], &[]);
        assert_eq!(net.dns_allow.len(), 1, "should not add duplicate");
    }

    #[test]
    fn dir_helpers() {
        let state = NativeState {
            pod_dir: PathBuf::from("/var/lib/envpod/pods/abc"),
            cgroup_path: None,
            init_pid: None,
            status: NativeStatus::Created,
            lower_dirs: vec![PathBuf::from("/")],
            network: None,
        };

        assert_eq!(state.upper_dir(), PathBuf::from("/var/lib/envpod/pods/abc/upper"));
        assert_eq!(state.work_dir(), PathBuf::from("/var/lib/envpod/pods/abc/work"));
        assert_eq!(state.merged_dir(), PathBuf::from("/var/lib/envpod/pods/abc/merged"));
        assert_eq!(state.rootfs_dir(), PathBuf::from("/var/lib/envpod/pods/abc/rootfs"));
        assert!(state.netns_path().is_none());
    }
}
