pub mod native;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::PodConfig;
use crate::types::{
    FileDiff, MountConfig, NetworkConfig, PodHandle, PodInfo, ProcessHandle, ResourceLimits,
};

/// Every isolation backend must implement this trait.
/// The governance layer communicates ONLY through this interface.
///
/// Backends own the low-level isolation primitives (namespaces, containers, VMs)
/// but know nothing about policy, audit, or governance — that lives above.
///
/// All methods take `&self` — backends are stateless coordinators.
/// Pod state is tracked via `PodHandle` and the backend's own external state
/// (e.g., cgroup filesystem, container runtime).
pub trait IsolationBackend: Send + Sync {
    /// Human-readable name for this backend (e.g., "native", "docker", "vm").
    fn name(&self) -> &str;

    // -- Lifecycle --------------------------------------------------------

    /// Restore kernel state (netns, cgroups, veth) that was lost after a host reboot.
    ///
    /// Checks if the kernel resources described by the `PodHandle` still exist.
    /// If any are missing, recreates them from persisted on-disk state so the
    /// pod can be used again without re-creating it.
    ///
    /// Returns `Ok(true)` if restoration was needed and performed,
    /// `Ok(false)` if everything was already intact (no-op).
    fn restore(&self, _handle: &PodHandle) -> Result<bool> {
        Ok(false)
    }

    /// Create an isolated environment from a pod config.
    /// Sets up namespaces/container/VM, overlay filesystem, network namespace.
    /// Does NOT start any process — the pod is in `Created` state.
    fn create(&self, config: &PodConfig) -> Result<PodHandle>;

    /// Start the agent process inside the pod.
    /// The command inherits the pod's isolation (namespaces, cgroups, overlay, netns).
    /// If `user` is Some, the process drops to the specified user (name or numeric uid)
    /// after all privileged setup is complete.
    /// `extra_env` contains additional KEY=VALUE environment variables.
    fn start(&self, handle: &PodHandle, command: &[String], user: Option<&str>, extra_env: &[String], quiet_log: Option<&Path>) -> Result<ProcessHandle>;

    /// Freeze all processes in the pod (SIGSTOP / cgroup freezer).
    /// State is fully preserved — can be resumed with `resume`.
    fn freeze(&self, handle: &PodHandle) -> Result<()>;

    /// Resume a frozen pod.
    fn resume(&self, handle: &PodHandle) -> Result<()>;

    /// Terminate all processes and release resources.
    /// Overlay is NOT deleted — call `rollback` or `destroy` for that.
    fn stop(&self, handle: &PodHandle) -> Result<()>;

    /// Destroy the environment entirely: remove overlay, namespaces, cgroups.
    /// This is irreversible.
    fn destroy(&self, handle: &PodHandle) -> Result<()>;

    // -- Filesystem -------------------------------------------------------

    /// Mount a host path into the running pod's overlay.
    fn mount(&self, handle: &PodHandle, mount: &MountConfig) -> Result<()>;

    /// Unmount a path from the pod (live mutation).
    fn unmount(&self, handle: &PodHandle, path: &Path) -> Result<()>;

    /// Return all file-level changes in the overlay vs the lower (real) filesystem.
    fn diff(&self, handle: &PodHandle) -> Result<Vec<FileDiff>>;

    /// Commit overlay changes to the real filesystem.
    /// Merges upper layer into lower layer for each mount.
    /// If `paths` is `None`, commits all changes. If `Some`, commits only
    /// the specified files and leaves the rest in the overlay.
    /// If `output_dir` is `Some`, writes committed files there instead of the lower layer.
    fn commit(&self, handle: &PodHandle, paths: Option<&[PathBuf]>, output_dir: Option<&Path>) -> Result<()>;

    /// Discard all overlay changes (delete upper layer contents).
    fn rollback(&self, handle: &PodHandle) -> Result<()>;

    // -- Network ----------------------------------------------------------

    /// Apply network configuration to the pod's network namespace.
    /// Can be called multiple times for live mutation.
    fn configure_network(&self, handle: &PodHandle, config: &NetworkConfig) -> Result<()>;

    // -- Resources --------------------------------------------------------

    /// Apply or update resource limits (cgroups / VM config).
    /// Can be called multiple times for live mutation.
    fn set_limits(&self, handle: &PodHandle, limits: &ResourceLimits) -> Result<()>;

    // -- Introspection ----------------------------------------------------

    /// Get current pod status, running process info, and resource usage.
    fn info(&self, handle: &PodHandle) -> Result<PodInfo>;
}

/// Create a backend by name.
///
/// `base_dir` is the envpod root (e.g. `/var/lib/envpod`).
/// The native backend stores pod data under `{base_dir}/pods/`.
pub fn create_backend(name: &str, base_dir: &std::path::Path) -> Result<Box<dyn IsolationBackend>> {
    match name {
        "native" => Ok(Box::new(native::NativeBackend::new(base_dir)?)),
        other => anyhow::bail!("unknown isolation backend: {other}"),
    }
}
