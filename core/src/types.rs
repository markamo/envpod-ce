// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Pod identity & lifecycle
// ---------------------------------------------------------------------------

/// Opaque handle returned by the backend after creating a pod environment.
/// The backend stores its own internal state; the governance layer only holds this handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodHandle {
    pub id: Uuid,
    pub name: String,
    pub backend: String,
    pub created_at: DateTime<Utc>,
    /// Backend-specific opaque state (e.g., container ID, namespace paths).
    /// Serialized so pods survive daemon restarts.
    pub backend_state: serde_json::Value,
}

/// Handle to a running process inside a pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessHandle {
    pub pid: u32,
    pub pod_id: Uuid,
    pub command: Vec<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PodStatus {
    /// Created but no process running.
    Created,
    /// Agent process is running.
    Running,
    /// Frozen (paused) — state preserved.
    Frozen,
    /// Terminated — cleanup pending or complete.
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodInfo {
    pub handle: PodHandle,
    pub status: PodStatus,
    pub process: Option<ProcessHandle>,
    pub resource_usage: ResourceUsage,
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountPermission {
    ReadOnly,
    ReadWrite,
}

/// A single filesystem mount exposed to the pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    /// Host path to mount into the pod.
    pub host_path: PathBuf,
    /// Path as seen inside the pod. Defaults to `host_path` if None.
    pub pod_path: Option<PathBuf>,
    pub permission: MountPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    Added,
    Modified,
    Deleted,
}

/// A single file-level change in the overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: PathBuf,
    pub kind: DiffKind,
    /// Size in bytes (after change, or 0 for deletes).
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkMode {
    /// No network access (default).
    Isolated,
    /// Network access with monitoring and policy enforcement.
    Monitored,
    /// Unrestricted host network (requires explicit ack).
    Unsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DnsMode {
    /// Only explicitly allowed domains resolve.
    #[serde(alias = "Whitelist", alias = "whitelist", alias = "allowlist")]
    Allowlist,
    /// All domains resolve except explicitly blocked.
    #[serde(alias = "Blacklist", alias = "blacklist", alias = "denylist")]
    Denylist,
    /// All domains resolve, queries are logged.
    #[serde(alias = "monitor")]
    Monitor,
}

/// Per-domain DNS rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsRule {
    pub domain: String,
    /// If Some, remap this domain to a different address.
    pub remap: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub mode: NetworkMode,
    pub dns_mode: DnsMode,
    /// Allowed domains (whitelist mode) or blocked domains (blacklist mode).
    pub dns_rules: Vec<DnsRule>,
    /// Max requests per minute. None = unlimited.
    pub rate_limit: Option<u32>,
    /// Max bandwidth in bytes per session. None = unlimited.
    pub bandwidth_cap: Option<u64>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Isolated,
            dns_mode: DnsMode::Allowlist,
            dns_rules: Vec::new(),
            rate_limit: None,
            bandwidth_cap: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Number of CPU cores (fractional allowed, e.g. 0.5).
    pub cpu_cores: Option<f64>,
    /// Memory limit in bytes.
    pub memory_bytes: Option<u64>,
    /// Max disk usage in bytes for the overlay.
    pub disk_bytes: Option<u64>,
    /// Max number of PIDs inside the pod.
    pub max_pids: Option<u32>,
    /// cpuset.cpus value (e.g. "0-1" or "0,2"). Pins pod to specific cores.
    pub cpuset_cpus: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Current CPU usage as fraction of allocated cores.
    pub cpu_percent: f64,
    /// Current memory usage in bytes.
    pub memory_bytes: u64,
    /// Current overlay disk usage in bytes.
    pub disk_bytes: u64,
    /// Current number of processes.
    pub pid_count: u32,
}
