// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Native Linux isolation backend.
//!
//! Uses kernel primitives directly for pod isolation:
//! - Mount namespace + pivot_root  — filesystem isolation
//! - OverlayFS                     — copy-on-write (agent writes never touch host)
//! - cgroups v2                    — resource limits + freeze/thaw
//! - Network namespace + veth      — network isolation with DNS filtering
//! - PID namespace                 — process isolation (child is PID 1)
//! - seccomp-BPF                   — syscall allowlist filtering

pub(crate) mod cgroup;
pub(crate) mod dev_mask;
pub mod gc;
pub(crate) mod namespace;
pub(crate) mod netns;
pub(crate) mod overlay;
pub(crate) mod proc_mask;
pub(crate) mod seccomp;
pub mod state;

// Re-export for CLI use
pub use overlay::{filter_diff, is_protected, partition_protected, snapshot_base, has_base, resolve_base_name, destroy_base, PROTECTED_SYSTEM_PATHS};
pub use netns::{
    gc_iptables,
    setup_port_forwards, cleanup_port_forwards,
    setup_internal_ports, cleanup_internal_ports,
    add_port_forward, remove_port_forward,
    add_internal_port, remove_internal_port,
    read_active_ports,
};
pub use gc::{gc_all, GcResult};
pub use cgroup::{migrate_display_pids, has_guardian};

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use uuid::Uuid;

use super::IsolationBackend;
use crate::audit::{AuditAction, AuditEntry, AuditLog};
use crate::config::PodConfig;
use crate::error::EnvpodError;
use crate::types::{
    FileDiff, MountConfig, MountPermission, NetworkConfig, NetworkMode, PodHandle, PodInfo,
    PodStatus, ProcessHandle, ResourceLimits,
};

/// Expand `~` prefix to the current user's home directory.
pub fn expand_tilde(path: &Path) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    path.to_path_buf()
}

/// Resolve the real (non-root) UID, preferring SUDO_UID when running under sudo.
fn resolve_real_uid() -> u32 {
    if let Ok(uid) = std::env::var("SUDO_UID") {
        if let Ok(n) = uid.parse::<u32>() {
            return n;
        }
    }
    nix::unistd::getuid().as_raw()
}

/// Detect the Wayland compositor socket path for a given UID.
///
/// Checks `$WAYLAND_DISPLAY` first (may be "wayland-0" or an absolute path),
/// then falls back to the default `wayland-0` under `/run/user/{uid}/`.
fn detect_wayland_socket(uid: u32) -> Option<PathBuf> {
    let runtime_dir = format!("/run/user/{uid}");

    // Check WAYLAND_DISPLAY env var
    if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
        if display.starts_with('/') {
            // Absolute path
            let p = PathBuf::from(&display);
            if p.exists() {
                return Some(p);
            }
        } else {
            // Relative name (e.g. "wayland-0")
            let p = PathBuf::from(format!("{runtime_dir}/{display}"));
            if p.exists() {
                return Some(p);
            }
        }
    }

    // Default: wayland-0
    let default = PathBuf::from(format!("{runtime_dir}/wayland-0"));
    if default.exists() {
        return Some(default);
    }

    None
}

use state::{NativeState, NativeStatus, NetworkState};

/// Check that the process is running as root (euid 0).
///
/// The native backend requires root for namespaces, cgroups, overlayfs, and
/// network namespace setup. Returns a clear error with remediation advice.
pub fn check_privileges() -> Result<()> {
    if nix::unistd::geteuid().as_raw() != 0 {
        return Err(EnvpodError::PermissionDenied(
            "envpod native backend requires root privileges.\n  \
             Run with: sudo envpod <command>"
                .into(),
        )
        .into());
    }
    Ok(())
}

/// Native Linux isolation backend.
pub struct NativeBackend {
    /// Base directory for all envpod data (pods, state, netns indices).
    base_dir: PathBuf,
    /// Base directory for pod runtime data (overlays, state files).
    runtime_dir: PathBuf,
}

impl NativeBackend {
    /// Create a new native backend.
    ///
    /// `base_dir` is the envpod root (e.g. `/var/lib/envpod`).
    /// Pod data is stored under `{base_dir}/pods/`.
    pub fn new(base_dir: &std::path::Path) -> Result<Self> {
        let runtime_dir = base_dir.join("pods");
        std::fs::create_dir_all(&runtime_dir)
            .with_context(|| format!("create runtime dir: {}", runtime_dir.display()))?;
        Ok(Self {
            base_dir: base_dir.to_path_buf(),
            runtime_dir,
        })
    }

    /// Create with a custom runtime directory (used in tests).
    pub fn with_runtime_dir(runtime_dir: PathBuf) -> Self {
        Self {
            base_dir: runtime_dir.clone(),
            runtime_dir,
        }
    }

    fn pod_dir(&self, pod_id: &Uuid) -> PathBuf {
        self.runtime_dir.join(pod_id.to_string())
    }

    /// Emit an audit log entry. Failures are logged but never propagated.
    fn emit_audit(
        pod_dir: &Path,
        pod_name: &str,
        action: AuditAction,
        detail: String,
        success: bool,
    ) {
        let log = AuditLog::new(pod_dir);
        let entry = AuditEntry {
            timestamp: Utc::now(),
            pod_name: pod_name.into(),
            action,
            detail,
            success,
        };
        if let Err(e) = log.append(&entry) {
            tracing::warn!(error = %e, ?action, "audit log write failed");
        }
    }

    /// Destroy with full iptables cleanup (no deferred rules).
    pub fn destroy_full(&self, handle: &PodHandle) -> Result<()> {
        self.destroy_impl(handle, true)
    }

    /// Internal destroy implementation.
    fn destroy_impl(&self, handle: &PodHandle, full: bool) -> Result<()> {
        let state = NativeState::from_handle(handle)?;

        // Emit audit before destroying the pod directory (which contains the log)
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Destroy, String::new(), true);

        // Clean up network namespace and associated resources.
        // Tolerate missing kernel state (e.g. after host reboot).
        if let Some(ref net) = state.network {
            if netns::netns_exists(&net.netns_name) {
                if let Err(e) = netns::destroy_netns(net, full) {
                    tracing::warn!(error = %e, "network namespace cleanup failed");
                }
            } else {
                // Netns already gone (reboot) — still clean up host-side veth if present
                if netns::veth_exists(&net.host_veth) {
                    let _ = std::process::Command::new("ip")
                        .args(["link", "del", &net.host_veth])
                        .output();
                }
            }
            netns::release_pod_index(&self.base_dir, net.pod_index);
        }

        // Ensure all cgroup processes are dead before unmounting.
        // Multiple shells/sessions may be running — kill aggressively and wait.
        if let Some(ref cg) = state.cgroup_path {
            if cgroup::cgroup_exists(cg) && cgroup::has_processes(cg) {
                cgroup::kill_all(cg, nix::sys::signal::Signal::SIGKILL).ok();
                // Wait up to 5 seconds for all processes to exit
                for _ in 0..50 {
                    if !cgroup::has_processes(cg) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            cgroup::destroy(cg).ok();
        }

        // Unmount all overlays: system COW sub-mounts first, then main overlay.
        // System COW overlays (advanced/dangerous mode) mount inside merged/
        // at merged/usr, merged/bin, etc. They must be unmounted before the
        // main overlay at merged/ can be unmounted.
        let merged = state.pod_dir.join("merged");
        if merged.exists() {
            // Unmount system COW overlays (best-effort, may not exist in safe mode)
            let sys_dirs: Vec<&str> = {
                let mut v: Vec<&str> = vec!["usr"];
                v.extend(overlay::real_system_dirs());
                v
            };
            for dir in &sys_dirs {
                let mount_point = merged.join(dir);
                if mount_point.exists() {
                    overlay::unmount_overlay(&mount_point).ok();
                }
            }
            // Unmount other bind mounts inside merged (proc, dev, sys, etc.)
            for sub in &["proc", "sys", "dev", "dev/shm", "dev/pts", "tmp", "run"] {
                let mount_point = merged.join(sub);
                if mount_point.exists() {
                    overlay::unmount_overlay(&mount_point).ok();
                }
            }
            // Unmount the main overlay
            overlay::unmount_overlay(&merged).ok();
        }

        // Remove all overlay directories
        overlay::destroy(&state.pod_dir)
    }

    /// Set up network namespace, veth pair, NAT, and iptables for a pod.
    fn setup_network(&self, short_id: &str, config: &PodConfig) -> Result<NetworkState> {
        // 1. Detect host's default outbound interface (cached)
        let host_interface = netns::detect_host_interface_cached(Some(&self.base_dir))
            .context("detect host network interface")?;

        // 2. Create network namespace
        let netns_name = netns::create_netns(short_id)
            .context("create network namespace")?;

        // 3. Allocate a unique pod index for subnet assignment
        let subnet_base = config.network.subnet.as_deref()
            .unwrap_or(netns::DEFAULT_SUBNET_BASE);
        let pod_index = netns::allocate_pod_index(&self.base_dir, subnet_base)
            .context("allocate pod network index")?;

        // 4. Set up veth pair
        let veth_config = netns::VethConfig::from_index(pod_index, short_id, &netns_name, subnet_base);
        if let Err(e) = netns::setup_veth(&veth_config) {
            // Clean up on failure
            netns::release_pod_index(&self.base_dir, pod_index);
            let _ = std::process::Command::new("ip")
                .args(["netns", "del", &netns_name])
                .output();
            return Err(e.context("setup veth pair"));
        }

        // 5. Set up NAT masquerade + FORWARD rules on host
        if let Err(e) = netns::setup_host_nat(&host_interface, &veth_config.subnet, &veth_config.host_veth) {
            tracing::warn!(error = %e, "NAT setup failed — pod may not have internet access");
            eprintln!("warning: NAT setup failed — pod may not have internet access: {e}");
        }

        // 6. Set up iptables inside pod to restrict DNS
        if config.network.mode == NetworkMode::Isolated {
            netns::setup_pod_iptables(&netns_name, &veth_config.host_ip)
                .context("setup pod iptables")?;
        }

        let dns_mode = match config.network.dns.mode {
            crate::types::DnsMode::Whitelist => "whitelist",
            crate::types::DnsMode::Blacklist => "blacklist",
            crate::types::DnsMode::Monitor => "monitor",
        };

        Ok(NetworkState {
            netns_name,
            netns_path: PathBuf::from(format!("/run/netns/envpod-{short_id}")),
            host_veth: veth_config.host_veth,
            pod_veth: veth_config.pod_veth,
            host_ip: veth_config.host_ip,
            pod_ip: veth_config.pod_ip,
            pod_index,
            host_interface,
            dns_mode: dns_mode.to_string(),
            dns_allow: config.network.dns.allow.clone(),
            dns_deny: config.network.dns.deny.clone(),
            dns_remap: config.network.dns.remap.clone(),
            subnet_base: subnet_base.to_string(),
        })
    }

    /// Clone a pod's filesystem and kernel resources, creating a new independent pod.
    ///
    /// Two modes:
    /// - `use_current = false` — clone from base snapshot (post-init+setup, before agent changes)
    /// - `use_current = true` — clone from current state (includes agent modifications)
    ///
    /// Returns a new `PodHandle` for the cloned pod. The caller must persist it
    /// via `PodStore::save()`.
    /// Build a deferred NetworkState (kernel resources not yet created).
    /// `restore()` will create netns + veth on first `run`.
    fn deferred_network(&self, short_id: &str, config: &PodConfig) -> Option<NetworkState> {
        let host_interface = netns::detect_host_interface_cached(Some(&self.base_dir)).ok()?;
        let subnet_base = config.network.subnet.as_deref()
            .unwrap_or(netns::DEFAULT_SUBNET_BASE);
        let pod_index = netns::allocate_pod_index(&self.base_dir, subnet_base).ok()?;
        let veth_config = netns::VethConfig::from_index(pod_index, short_id, &format!("envpod-{short_id}"), subnet_base);

        let dns_mode = match config.network.dns.mode {
            crate::types::DnsMode::Whitelist => "whitelist",
            crate::types::DnsMode::Blacklist => "blacklist",
            crate::types::DnsMode::Monitor => "monitor",
        };

        Some(NetworkState {
            netns_name: format!("envpod-{short_id}"),
            netns_path: PathBuf::from(format!("/run/netns/envpod-{short_id}")),
            host_veth: veth_config.host_veth,
            pod_veth: veth_config.pod_veth,
            host_ip: veth_config.host_ip,
            pod_ip: veth_config.pod_ip,
            pod_index,
            host_interface,
            dns_mode: dns_mode.to_string(),
            dns_allow: config.network.dns.allow.clone(),
            dns_deny: config.network.dns.deny.clone(),
            dns_remap: config.network.dns.remap.clone(),
            subnet_base: subnet_base.to_string(),
        })
    }

    pub fn clone_pod(
        &self,
        source_handle: &PodHandle,
        new_name: &str,
        use_current: bool,
    ) -> Result<PodHandle> {
        check_privileges()?;

        let source_state = NativeState::from_handle(source_handle)?;
        let source_config = source_state
            .load_config()?
            .context("source pod has no pod.yaml")?;

        let new_id = Uuid::new_v4();
        let new_pod_dir = self.pod_dir(&new_id);
        let short_id = &new_id.to_string()[..8];

        // 1. Allocate pod network index first (so we can clean up on failure)
        let network = if source_state.network.is_some() {
            self.deferred_network(short_id, &source_config)
        } else {
            None
        };

        // 2. Clone filesystem (rootfs + upper + sys_upper + pod.yaml)
        if let Err(e) = overlay::clone_filesystem(&source_state.pod_dir, &new_pod_dir, use_current) {
            if let Some(ref net) = network {
                netns::release_pod_index(&self.base_dir, net.pod_index);
            }
            let _ = std::fs::remove_dir_all(&new_pod_dir);
            return Err(e.context("clone filesystem"));
        }

        // 3. Update pod.yaml with new name
        let mut cloned_config = source_config.clone();
        cloned_config.name = new_name.to_string();
        let yaml = serde_yaml::to_string(&cloned_config).context("serialize cloned pod.yaml")?;
        if let Err(e) = std::fs::write(new_pod_dir.join("pod.yaml"), &yaml) {
            if let Some(ref net) = network {
                netns::release_pod_index(&self.base_dir, net.pod_index);
            }
            let _ = std::fs::remove_dir_all(&new_pod_dir);
            return Err(anyhow::Error::from(e).context("write cloned pod.yaml"));
        }

        // 4. Defer cgroup creation to first run (restore() handles it).
        let cgroup_path = Some(
            PathBuf::from(cgroup::CGROUP_BASE)
                .join(cgroup::ENVPOD_SLICE)
                .join(new_id.to_string()),
        );

        // 5. Build state + handle
        let state = NativeState {
            pod_dir: new_pod_dir.clone(),
            cgroup_path,
            init_pid: None,
            status: NativeStatus::Created,
            lower_dirs: vec![PathBuf::from("/")],
            network,
        };

        // 5. Audit entry
        let mode = if use_current { "current" } else { "base" };
        Self::emit_audit(
            &new_pod_dir,
            new_name,
            AuditAction::Clone,
            format!("cloned from '{}' (mode={})", source_handle.name, mode),
            true,
        );

        Ok(PodHandle {
            id: new_id,
            name: new_name.to_string(),
            backend: "native".into(),
            created_at: Utc::now(),
            backend_state: state.to_json(),
        })
    }

    /// Clone a pod from a standalone base pod (no source pod handle needed).
    pub fn clone_from_base(
        &self,
        base_dir: &Path,
        base_name: &str,
        new_name: &str,
    ) -> Result<PodHandle> {
        check_privileges()?;

        let base_pod_dir = base_dir.join(base_name);
        if !base_pod_dir.join("rootfs").exists() {
            anyhow::bail!("base pod '{}' not found", base_name);
        }

        // Load config from the base's pod.yaml (copied during snapshot)
        let config_path = base_pod_dir.join("pod.yaml");
        let config: crate::config::PodConfig = if config_path.exists() {
            let yaml = std::fs::read_to_string(&config_path).context("read base pod.yaml")?;
            serde_yaml::from_str(&yaml).context("parse base pod.yaml")?
        } else {
            anyhow::bail!("base pod '{}' has no pod.yaml", base_name);
        };

        let new_id = Uuid::new_v4();
        let new_pod_dir = self.pod_dir(&new_id);
        let short_id = &new_id.to_string()[..8];

        // Allocate pod network index first (so we can clean up on failure)
        let network = self.deferred_network(short_id, &config);

        if let Err(e) = overlay::clone_filesystem(&base_pod_dir, &new_pod_dir, false) {
            if let Some(ref net) = network {
                netns::release_pod_index(&self.base_dir, net.pod_index);
            }
            let _ = std::fs::remove_dir_all(&new_pod_dir);
            return Err(e.context("clone filesystem from base"));
        }

        // Update pod.yaml with new name
        let mut cloned_config = config.clone();
        cloned_config.name = new_name.to_string();
        let yaml = serde_yaml::to_string(&cloned_config).context("serialize cloned pod.yaml")?;
        if let Err(e) = std::fs::write(new_pod_dir.join("pod.yaml"), &yaml) {
            if let Some(ref net) = network {
                netns::release_pod_index(&self.base_dir, net.pod_index);
            }
            let _ = std::fs::remove_dir_all(&new_pod_dir);
            return Err(anyhow::Error::from(e).context("write cloned pod.yaml"));
        }

        // Defer cgroup creation to first run
        let cgroup_path = Some(
            PathBuf::from(cgroup::CGROUP_BASE)
                .join(cgroup::ENVPOD_SLICE)
                .join(new_id.to_string()),
        );

        // Build state + handle
        let state = NativeState {
            pod_dir: new_pod_dir.clone(),
            cgroup_path,
            init_pid: None,
            status: NativeStatus::Created,
            lower_dirs: vec![PathBuf::from("/")],
            network,
        };

        // Audit entry
        Self::emit_audit(
            &new_pod_dir,
            new_name,
            AuditAction::Clone,
            format!("cloned from base '{}'", base_name),
            true,
        );

        Ok(PodHandle {
            id: new_id,
            name: new_name.to_string(),
            backend: "native".into(),
            created_at: Utc::now(),
            backend_state: state.to_json(),
        })
    }

    /// Start a process inside the pod, bypassing the allowed_commands check.
    /// Used for setup commands defined by the pod creator (trusted).
    pub fn start_setup(
        &self,
        handle: &PodHandle,
        command: &[String],
        quiet_log: Option<&Path>,
    ) -> Result<ProcessHandle> {
        check_privileges()?;
        let state = NativeState::from_handle(handle)?;
        let pod_config = state.load_config()?;
        self.start_inner(handle, command, &state, pod_config.as_ref(), quiet_log, None, &[])
    }

    fn start_inner(
        &self,
        handle: &PodHandle,
        command: &[String],
        state: &NativeState,
        pod_config: Option<&PodConfig>,
        quiet_log: Option<&Path>,
        user: Option<&str>,
        extra_env: &[String],
    ) -> Result<ProcessHandle> {
        let cgroup_procs = state.cgroup_path.as_ref().map(|p| cgroup::procs_path(p));

        // If network isolation is active, write resolv.conf into the upper
        // layer so the pod resolves DNS through our filtering server.
        // Written to upper (not rootfs) so rootfs stays immutable and can be
        // shared across cloned pods.
        if let Some(ref net) = state.network {
            netns::write_pod_resolv_conf(&state.upper_dir(), &net.host_ip)
                .context("write pod resolv.conf")?;
        }

        let netns_path = state.netns_path();
        let log_path = state.log_path();

        // Load vault secrets and merge with extra env vars from --env flags.
        // Also refresh vault_env file so the bind-mounted live file is current.
        let mut env_map = crate::vault::Vault::new(&state.pod_dir)
            .and_then(|v| {
                let _ = v.refresh_env_file(&state.pod_dir);
                v.load_all()
            })
            .unwrap_or_default();
        for entry in extra_env {
            if let Some((key, value)) = entry.split_once('=') {
                env_map.insert(key.to_string(), value.to_string());
            }
        }
        let vault_env = if env_map.is_empty() { None } else { Some(&env_map) };

        // Read security config (seccomp profile + /dev/shm size)
        let security = pod_config
            .map(|c| c.security.clone())
            .unwrap_or_default();
        let seccomp_profile = security.seccomp_profile();
        let shm_size = Some(security.shm_size_bytes().unwrap_or(67_108_864)); // 64MB default

        // Read devices config (GPU passthrough, extra devices)
        let devices = pod_config
            .map(|c| c.devices.clone())
            .unwrap_or_default();

        // Build mount entries from pod config (filesystem.mounts)
        let mut mount_entries: Vec<(PathBuf, PathBuf, bool)> = pod_config
            .map(|c| {
                c.filesystem
                    .mounts
                    .iter()
                    .map(|m| {
                        let host_path = expand_tilde(&m.path);
                        let pod_path = host_path.clone();
                        let readonly = m.permissions == MountPermission::ReadOnly;
                        (host_path, pod_path, readonly)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Mount CWD (filesystem.mount_cwd + cwd_path)
        if let Some(config) = pod_config {
            if config.filesystem.mount_cwd {
                if let Some(ref cwd) = config.filesystem.cwd_path {
                    if cwd.exists() && !mount_entries.iter().any(|(h, _, _)| h == cwd) {
                        mount_entries.push((cwd.clone(), cwd.clone(), true)); // read-only, COW
                        tracing::info!(path = %cwd.display(), "mounting CWD");
                    }
                }
            }
        }

        // Auto-mount host apps (filesystem.apps)
        if let Some(config) = pod_config {
            for app_name in &config.filesystem.apps {
                match crate::app_resolver::resolve_app(app_name) {
                    Ok(resolved) => {
                        for path in &resolved.paths {
                            if path.exists() && !mount_entries.iter().any(|(h, _, _)| h == path) {
                                mount_entries.push((path.clone(), path.clone(), true));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(app = %app_name, error = %e, "failed to resolve app");
                    }
                }
            }
        }

        // Clone host user directory mounts
        if let Some(config) = pod_config {
            if config.host_user.clone_host {
                if let Ok(user) = crate::user_clone::get_host_user() {
                    let user_mounts = crate::user_clone::user_dir_mounts(&user, &config.host_user);
                    for (host_path, pod_path, readonly) in user_mounts {
                        if host_path.exists() && !mount_entries.iter().any(|(h, _, _)| h == &host_path) {
                            mount_entries.push((host_path, pod_path, readonly));
                        }
                    }
                }
            }
        }

        // Append auto-mount entries from devices config (display/audio)
        if devices.display {
            use crate::config::DisplayProtocol;
            let protocol = devices.display_protocol;
            let real_uid = resolve_real_uid();
            match protocol {
                DisplayProtocol::Wayland => {
                    if let Some(socket) = detect_wayland_socket(real_uid) {
                        mount_entries.push((socket, PathBuf::from("/tmp/wayland-0"), true));
                    } else {
                        tracing::warn!("display_protocol is wayland but no Wayland socket found");
                    }
                }
                DisplayProtocol::X11 => {
                    let x11 = PathBuf::from("/tmp/.X11-unix");
                    if x11.exists() {
                        mount_entries.push((x11.clone(), x11, true));
                    }
                }
                DisplayProtocol::Auto => {
                    // Prefer Wayland, fall back to X11
                    if let Some(socket) = detect_wayland_socket(real_uid) {
                        mount_entries.push((socket, PathBuf::from("/tmp/wayland-0"), true));
                    } else {
                        let x11 = PathBuf::from("/tmp/.X11-unix");
                        if x11.exists() {
                            mount_entries.push((x11.clone(), x11, true));
                        }
                    }
                }
            }
        }
        if devices.audio {
            use crate::config::AudioProtocol;
            let protocol = devices.audio_protocol;
            let real_uid = resolve_real_uid();
            match protocol {
                AudioProtocol::Pipewire => {
                    let pipewire = PathBuf::from(format!("/run/user/{real_uid}/pipewire-0"));
                    if pipewire.exists() {
                        mount_entries.push((pipewire, PathBuf::from("/tmp/pipewire-0"), true));
                    } else {
                        tracing::warn!("audio_protocol is pipewire but no PipeWire socket found");
                    }
                }
                AudioProtocol::Pulseaudio => {
                    let pulse_native = PathBuf::from(format!("/run/user/{real_uid}/pulse/native"));
                    if pulse_native.exists() {
                        mount_entries.push((pulse_native, PathBuf::from("/tmp/pulse-native"), true));
                    } else {
                        // PipeWire's PulseAudio compat socket as fallback
                        let pipewire = PathBuf::from(format!("/run/user/{real_uid}/pipewire-0"));
                        if pipewire.exists() {
                            mount_entries.push((pipewire, PathBuf::from("/tmp/pulse-native"), true));
                        }
                    }
                }
                AudioProtocol::Auto => {
                    // Prefer PipeWire native, fall back to PulseAudio
                    let pipewire = PathBuf::from(format!("/run/user/{real_uid}/pipewire-0"));
                    let pulse_native = PathBuf::from(format!("/run/user/{real_uid}/pulse/native"));
                    if pipewire.exists() {
                        mount_entries.push((pipewire, PathBuf::from("/tmp/pipewire-0"), true));
                    } else if pulse_native.exists() {
                        mount_entries.push((pulse_native, PathBuf::from("/tmp/pulse-native"), true));
                    }
                }
            }
            // Cookie is copied (not mounted) by --enable-audio with 0644 perms (PulseAudio only).
            // D-Bus is intentionally not forwarded (doesn't work across namespaces).
        }

        let rootfs = state.rootfs_dir();

        // Read system access profile (safe/advanced/dangerous)
        let system_access = pod_config
            .map(|c| c.filesystem.system_access)
            .unwrap_or_default();

        // Resolve user name/uid to (uid, gid) from the pod's /etc/passwd
        let run_as = match user {
            Some(u) => Some(resolve_pod_user(&state.pod_dir, u)?),
            None => None,
        };

        let result = namespace::spawn_isolated(
            command,
            &state.lower_dirs,
            &state.upper_dir(),
            &state.work_dir(),
            &state.merged_dir(),
            cgroup_procs.as_deref(),
            netns_path.as_deref(),
            &handle.name,
            Some(log_path.as_path()),
            vault_env,
            seccomp_profile,
            shm_size,
            &rootfs,
            &mount_entries,
            devices,
            system_access,
            quiet_log.map(|p| p.to_path_buf()),
            run_as,
        )
        .context("start isolated process");

        let success = result.is_ok();
        let detail = match &result {
            Ok(pid) => format!("pid={pid}, cmd={}", command.join(" ")),
            Err(e) => format!("error: {e}"),
        };
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Start, detail, success);

        let pid = result?;

        Ok(ProcessHandle {
            pid,
            pod_id: handle.id,
            command: command.to_vec(),
            started_at: Utc::now(),
        })
    }
}

/// Resolve a username or numeric uid to (uid, gid) from the pod's `/etc/passwd`.
///
/// Accepts either a username (e.g. "dev") or a numeric uid string (e.g. "1000").
/// Reads from the overlay upper layer first (captures `useradd` etc.), then
/// falls back to the rootfs copy. The merged dir isn't mounted yet at this point.
fn resolve_pod_user(pod_dir: &Path, user: &str) -> Result<(u32, u32)> {
    // Upper layer has the latest version (e.g. after useradd)
    let upper_passwd = pod_dir.join("upper/etc/passwd");
    // Rootfs has the base copy from init
    let rootfs_passwd = pod_dir.join("rootfs/etc/passwd");

    let contents = if upper_passwd.exists() {
        std::fs::read_to_string(&upper_passwd)
            .with_context(|| format!("read pod /etc/passwd: {}", upper_passwd.display()))?
    } else {
        std::fs::read_to_string(&rootfs_passwd)
            .with_context(|| format!("read pod /etc/passwd: {}", rootfs_passwd.display()))?
    };

    // Check if user is numeric (uid)
    if let Ok(uid) = user.parse::<u32>() {
        for line in contents.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 4 {
                if let Ok(puid) = fields[2].parse::<u32>() {
                    if puid == uid {
                        let gid = fields[3].parse::<u32>()
                            .with_context(|| format!("parse gid for uid {uid}"))?;
                        return Ok((uid, gid));
                    }
                }
            }
        }
        anyhow::bail!("uid {uid} not found in pod /etc/passwd");
    }

    // Username lookup
    for line in contents.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 4 && fields[0] == user {
            let uid = fields[2].parse::<u32>()
                .with_context(|| format!("parse uid for user '{user}'"))?;
            let gid = fields[3].parse::<u32>()
                .with_context(|| format!("parse gid for user '{user}'"))?;
            return Ok((uid, gid));
        }
    }

    anyhow::bail!("user '{user}' not found in pod /etc/passwd");
}

impl IsolationBackend for NativeBackend {
    fn name(&self) -> &str {
        "native"
    }

    fn restore(&self, handle: &PodHandle) -> Result<bool> {
        let state = NativeState::from_handle(handle)?;
        let mut restored_something = false;

        // Check and restore cgroup
        if let Some(ref cg) = state.cgroup_path {
            if !cgroup::cgroup_exists(cg) {
                // Extract pod_id from the cgroup path (last component)
                let id_string = handle.id.to_string();
                let pod_id = cg
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&id_string);

                match cgroup::create(pod_id) {
                    Ok(new_cg) => {
                        // Re-apply resource limits from config
                        if let Ok(Some(config)) = state.load_config() {
                            let limits = config.to_resource_limits();
                            if let Err(e) = cgroup::set_limits(&new_cg, &limits) {
                                tracing::warn!(error = %e, "failed to restore cgroup limits");
                            }
                        }
                        restored_something = true;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "cgroup restoration failed");
                    }
                }
            }
        }

        // Check and restore network namespace
        if let Some(ref net) = state.network {
            if !netns::netns_exists(&net.netns_name) || !netns::veth_exists(&net.host_veth) {
                // Determine if we need isolated mode iptables
                let isolated_mode = state
                    .load_config()
                    .ok()
                    .flatten()
                    .map(|c| c.network.mode == NetworkMode::Isolated)
                    .unwrap_or(false);

                // Clean up any partial state before restoring
                if netns::netns_exists(&net.netns_name) {
                    // Netns exists but veth is gone — destroy and recreate cleanly
                    let _ = std::process::Command::new("ip")
                        .args(["netns", "del", &net.netns_name])
                        .output();
                }
                if netns::veth_exists(&net.host_veth) {
                    let _ = std::process::Command::new("ip")
                        .args(["link", "del", &net.host_veth])
                        .output();
                }

                netns::restore_network(net, &self.base_dir, isolated_mode)
                    .context("restore network namespace")?;

                restored_something = true;
            }
        }

        if restored_something {
            Self::emit_audit(
                &state.pod_dir,
                &handle.name,
                AuditAction::Restore,
                "post-reboot kernel state restoration".into(),
                true,
            );
        }

        Ok(restored_something)
    }

    fn create(&self, config: &PodConfig) -> Result<PodHandle> {
        let id = Uuid::new_v4();
        let pod_dir = self.pod_dir(&id);
        let short_id = &id.to_string()[..8];

        // Create overlay directory structure
        overlay::create_dirs(&pod_dir)
            .context("create overlay directories")?;

        // Create minimal rootfs (overlay lower layer — not the full host FS)
        overlay::create_rootfs(&pod_dir)
            .context("create rootfs")?;

        // Clone host user into rootfs if configured
        if config.host_user.clone_host {
            let rootfs = pod_dir.join("rootfs");
            match crate::user_clone::setup_cloned_user(&rootfs, &config.host_user) {
                Ok(user) => {
                    tracing::info!(username = %user.username, "cloned host user into pod");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to clone host user — falling back to agent");
                    eprintln!("warning: failed to clone host user: {e:#}");
                }
            }
        }

        // Create cgroup for resource control
        let cgroup_path = match cgroup::create(&id.to_string()) {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::warn!(error = %e, "cgroup creation failed — resource limits unavailable");
                eprintln!("warning: cgroup creation failed — resource limits unavailable: {e}");
                None
            }
        };

        // Apply resource limits from config (cpu cores, memory, cpuset)
        if let Some(ref cg) = cgroup_path {
            let limits = config.to_resource_limits();
            if let Err(e) = cgroup::set_limits(cg, &limits) {
                tracing::warn!(error = %e, "failed to set initial resource limits");
                eprintln!("warning: failed to set resource limits: {e}");
            }
        }

        // Set up network namespace if mode != Unsafe
        let network = if config.network.mode != NetworkMode::Unsafe {
            match self.setup_network(short_id, config) {
                Ok(net_state) => Some(net_state),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "network namespace setup failed, falling back to host network"
                    );
                    eprintln!("warning: network isolation failed — pod will use host network: {e}");
                    None
                }
            }
        } else {
            None
        };

        let state = NativeState {
            pod_dir: pod_dir.clone(),
            cgroup_path,
            init_pid: None,
            status: NativeStatus::Created,
            lower_dirs: vec![PathBuf::from("/")],
            network,
        };

        Self::emit_audit(
            &pod_dir,
            &config.name,
            AuditAction::Create,
            "backend=native".into(),
            true,
        );

        Ok(PodHandle {
            id,
            name: config.name.clone(),
            backend: "native".into(),
            created_at: Utc::now(),
            backend_state: state.to_json(),
        })
    }

    fn start(&self, handle: &PodHandle, command: &[String], user: Option<&str>, extra_env: &[String], quiet_log: Option<&Path>) -> Result<ProcessHandle> {
        check_privileges()?;

        let state = NativeState::from_handle(handle)?;
        let pod_config = state.load_config()?;

        // Tool security: check command against allowed_commands list
        if let Some(ref config) = pod_config {
            if !config.tools.allowed_commands.is_empty() {
                let cmd = &command[0];
                let cmd_basename = std::path::Path::new(cmd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(cmd);

                let allowed = config.tools.allowed_commands.iter().any(|a| {
                    a == cmd || a == cmd_basename
                });

                if !allowed {
                    Self::emit_audit(
                        &state.pod_dir,
                        &handle.name,
                        AuditAction::ToolBlocked,
                        format!("cmd={cmd}, allowed={:?}", config.tools.allowed_commands),
                        true,
                    );
                    anyhow::bail!(
                        "command '{}' is not in allowed_commands list",
                        cmd
                    );
                }
            }
        }

        self.start_inner(handle, command, &state, pod_config.as_ref(), quiet_log, user, extra_env)
    }

    fn freeze(&self, handle: &PodHandle) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        if let Some(ref cg) = state.cgroup_path {
            cgroup::freeze(cg)?;
        }
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Freeze, String::new(), true);
        Ok(())
    }

    fn resume(&self, handle: &PodHandle) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        if let Some(ref cg) = state.cgroup_path {
            cgroup::thaw(cg)?;
        }
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Resume, String::new(), true);
        Ok(())
    }

    fn stop(&self, handle: &PodHandle) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        if let Some(ref cg) = state.cgroup_path {
            if cgroup::cgroup_exists(cg) && cgroup::has_processes(cg) {
                cgroup::kill_all(cg, nix::sys::signal::Signal::SIGTERM)?;
                // Brief grace period for clean shutdown
                std::thread::sleep(std::time::Duration::from_millis(100));
                cgroup::kill_all(cg, nix::sys::signal::Signal::SIGKILL)?;
                // Wait for processes to actually exit (up to 2 seconds)
                for _ in 0..20 {
                    if !cgroup::has_processes(cg) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Stop, String::new(), true);
        Ok(())
    }

    fn destroy(&self, handle: &PodHandle) -> Result<()> {
        self.destroy_impl(handle, false)
    }

    fn mount(&self, handle: &PodHandle, mount_cfg: &MountConfig) -> Result<()> {
        let state = NativeState::from_handle(handle)?;

        // Determine target path in the merged overlay
        let rel_target = match &mount_cfg.pod_path {
            Some(p) => p.strip_prefix("/").unwrap_or(p).to_path_buf(),
            None => mount_cfg
                .host_path
                .strip_prefix("/")
                .unwrap_or(&mount_cfg.host_path)
                .to_path_buf(),
        };
        let target = state.merged_dir().join(&rel_target);

        std::fs::create_dir_all(&target)
            .with_context(|| format!("create mount target: {}", target.display()))?;

        // Bind-mount the host path
        nix::mount::mount(
            Some(&mount_cfg.host_path),
            &target,
            None::<&str>,
            nix::mount::MsFlags::MS_BIND | nix::mount::MsFlags::MS_REC,
            None::<&str>,
        )
        .with_context(|| {
            format!(
                "bind mount {} → {}",
                mount_cfg.host_path.display(),
                target.display()
            )
        })?;

        // Remount read-only if specified
        if mount_cfg.permission == MountPermission::ReadOnly {
            nix::mount::mount(
                None::<&str>,
                &target,
                None::<&str>,
                nix::mount::MsFlags::MS_BIND
                    | nix::mount::MsFlags::MS_REMOUNT
                    | nix::mount::MsFlags::MS_RDONLY,
                None::<&str>,
            )
            .context("read-only remount")?;
        }

        Ok(())
    }

    fn unmount(&self, handle: &PodHandle, path: &Path) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        let rel = path.strip_prefix("/").unwrap_or(path);
        let target = state.merged_dir().join(rel);

        nix::mount::umount2(&target, nix::mount::MntFlags::MNT_DETACH)
            .with_context(|| format!("unmount {}", target.display()))?;
        Ok(())
    }

    fn diff(&self, handle: &PodHandle) -> Result<Vec<FileDiff>> {
        let state = NativeState::from_handle(handle)?;
        let mut result = overlay::diff(&state.upper_dir(), &state.lower_dirs);

        // Also scan per-system-dir overlay uppers (advanced/dangerous mode)
        let sys_upper = state.sys_upper_dir();
        if sys_upper.exists() {
            if let Ok(ref mut diffs) = result {
                let sys_diffs = overlay::diff_sys_upper(&sys_upper)?;
                diffs.extend(sys_diffs);
            }
        }

        let detail = match &result {
            Ok(diffs) => format!("{} change(s)", diffs.len()),
            Err(e) => format!("error: {e}"),
        };
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Diff, detail, result.is_ok());
        result
    }

    fn commit(&self, handle: &PodHandle, paths: Option<&[PathBuf]>, output_dir: Option<&Path>) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        let target = output_dir.unwrap_or(&state.lower_dirs[0]);
        if let Some(dir) = output_dir {
            std::fs::create_dir_all(dir)?;
        }
        let result = match paths {
            None => overlay::commit(&state.upper_dir(), target),
            Some(p) => overlay::commit_selective(&state.upper_dir(), target, p),
        };

        // Also commit per-system-dir overlay changes (advanced/dangerous mode).
        // For system dirs, target is always "/" (the host root) since system
        // overlay lowers are host dirs like /usr, /bin, etc.
        let sys_upper = state.sys_upper_dir();
        if result.is_ok() && sys_upper.exists() {
            let sys_target = output_dir.unwrap_or(Path::new("/"));
            overlay::commit_sys_upper(&sys_upper, sys_target)?;
        }

        let detail = match (&result, paths) {
            (Ok(()), None) => "all".to_string(),
            (Ok(()), Some(p)) => format!("{} file(s)", p.len()),
            (Err(e), _) => format!("error: {e}"),
        };
        let success = result.is_ok();
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Commit, detail, success);
        result
    }

    fn rollback(&self, handle: &PodHandle) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        let result = overlay::rollback(&state.upper_dir(), &state.work_dir());

        // Also clear per-system-dir overlay uppers (advanced/dangerous mode)
        if result.is_ok() {
            let sys_upper = state.sys_upper_dir();
            let sys_work = state.sys_work_dir();
            if sys_upper.exists() {
                overlay::rollback(&sys_upper, &sys_work)?;
            }
        }

        let (detail, success) = match &result {
            Ok(()) => (String::new(), true),
            Err(e) => (format!("error: {e}"), false),
        };
        Self::emit_audit(&state.pod_dir, &handle.name, AuditAction::Rollback, detail, success);
        result
    }

    fn configure_network(&self, handle: &PodHandle, config: &NetworkConfig) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        if state.network.is_none() {
            tracing::warn!("configure_network called but pod has no network namespace");
            return Ok(());
        }

        // Load persisted network state, falling back to the in-handle state
        let mut persisted = NetworkState::load(&state.pod_dir)?
            .or_else(|| state.network.clone())
            .context("no network state available")?;

        // Extract allow/deny from the dns_rules
        let domains: Vec<String> = config.dns_rules.iter().map(|r| r.domain.clone()).collect();
        let add_allow = if config.dns_mode == crate::types::DnsMode::Whitelist {
            &domains
        } else {
            &Vec::new()
        };
        let add_deny = if config.dns_mode == crate::types::DnsMode::Blacklist {
            &domains
        } else {
            &Vec::new()
        };

        persisted.update_dns_lists(add_allow, add_deny, &[], &[]);
        persisted.save(&state.pod_dir)?;

        Self::emit_audit(
            &state.pod_dir,
            &handle.name,
            AuditAction::SetLimits,
            format!(
                "dns updated: allow={}, deny={}",
                persisted.dns_allow.len(),
                persisted.dns_deny.len(),
            ),
            true,
        );

        Ok(())
    }

    fn set_limits(&self, handle: &PodHandle, limits: &ResourceLimits) -> Result<()> {
        let state = NativeState::from_handle(handle)?;
        if let Some(ref cg) = state.cgroup_path {
            cgroup::set_limits(cg, limits)?;
        }
        Ok(())
    }

    fn info(&self, handle: &PodHandle) -> Result<PodInfo> {
        let state = NativeState::from_handle(handle)?;

        let resource_usage = state
            .cgroup_path
            .as_ref()
            .and_then(|cg| cgroup::read_usage(cg).ok())
            .unwrap_or_default();

        // Determine live status: check if the process is still alive
        let status = match state.status {
            NativeStatus::Created => {
                // Check if a supervisor is running (e.g. `envpod start` launched
                // but state file hasn't been updated yet, or background run)
                let supervisor_pid_path = state.pod_dir.join("supervisor.pid");
                if let Ok(pid_str) = std::fs::read_to_string(&supervisor_pid_path) {
                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                        let nix_pid = nix::unistd::Pid::from_raw(pid);
                        if nix::sys::signal::kill(nix_pid, None).is_ok() {
                            PodStatus::Running
                        } else {
                            PodStatus::Created
                        }
                    } else {
                        PodStatus::Created
                    }
                } else {
                    PodStatus::Created
                }
            }
            NativeStatus::Running => {
                if let Some(pid) = state.init_pid {
                    let pid = nix::unistd::Pid::from_raw(pid as i32);
                    match nix::sys::signal::kill(pid, None) {
                        Ok(()) => PodStatus::Running,
                        Err(_) => PodStatus::Stopped,
                    }
                } else {
                    // No init_pid but status says Running — check supervisor
                    let supervisor_pid_path = state.pod_dir.join("supervisor.pid");
                    if let Ok(pid_str) = std::fs::read_to_string(&supervisor_pid_path) {
                        if let Ok(pid) = pid_str.trim().parse::<i32>() {
                            let nix_pid = nix::unistd::Pid::from_raw(pid);
                            if nix::sys::signal::kill(nix_pid, None).is_ok() {
                                PodStatus::Running
                            } else {
                                PodStatus::Stopped
                            }
                        } else {
                            PodStatus::Created
                        }
                    } else {
                        PodStatus::Created
                    }
                }
            }
            NativeStatus::Frozen => PodStatus::Frozen,
            NativeStatus::Stopped => PodStatus::Stopped,
        };

        Ok(PodInfo {
            handle: handle.clone(),
            status,
            process: state.init_pid.map(|pid| ProcessHandle {
                pid,
                pod_id: handle.id,
                command: Vec::new(),
                started_at: handle.created_at,
            }),
            resource_usage,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — non-root tests that verify wiring, serialization, and overlay logic
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PodConfig;
    use crate::types::DiffKind;

    fn test_config() -> PodConfig {
        PodConfig {
            name: "test-pod".into(),
            ..Default::default()
        }
    }

    #[test]
    fn check_privileges_returns_error_for_non_root() {
        // In normal `cargo test` we run as a regular user, so this should fail
        if nix::unistd::geteuid().as_raw() != 0 {
            let result = check_privileges();
            assert!(result.is_err());
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("root privileges"),
                "error should mention root: {msg}"
            );
            assert!(
                msg.contains("sudo envpod"),
                "error should suggest sudo: {msg}"
            );
        }
    }

    #[test]
    fn backend_name() {
        let backend = NativeBackend::with_runtime_dir(PathBuf::from("/tmp/envpod-test"));
        assert_eq!(backend.name(), "native");
    }

    #[test]
    fn pod_dir_includes_uuid() {
        let backend = NativeBackend::with_runtime_dir(PathBuf::from("/var/lib/envpod/pods"));
        let id = Uuid::nil();
        assert_eq!(
            backend.pod_dir(&id).to_string_lossy(),
            "/var/lib/envpod/pods/00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn create_sets_up_overlay_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();
        assert!(state.upper_dir().is_dir());
        assert!(state.work_dir().is_dir());
        assert!(state.merged_dir().is_dir());
        assert_eq!(state.status, NativeStatus::Created);
        assert!(state.init_pid.is_none());
    }

    #[test]
    fn create_preserves_pod_name() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let mut config = test_config();
        config.name = "my-agent".into();
        let handle = backend.create(&config).unwrap();

        assert_eq!(handle.name, "my-agent");
        assert_eq!(handle.backend, "native");
    }

    #[test]
    fn destroy_removes_all_state() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();
        assert!(state.pod_dir.exists());

        backend.destroy(&handle).unwrap();
        assert!(!state.pod_dir.exists());
    }

    #[test]
    fn rollback_clears_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();

        // Simulate agent writes
        std::fs::write(state.upper_dir().join("agent_output.txt"), "data").unwrap();
        std::fs::create_dir_all(state.upper_dir().join("subdir")).unwrap();
        std::fs::write(state.upper_dir().join("subdir/nested.txt"), "nested").unwrap();

        backend.rollback(&handle).unwrap();

        // Upper and work should exist but be empty
        assert!(state.upper_dir().is_dir());
        assert!(state.work_dir().is_dir());
        assert_eq!(std::fs::read_dir(state.upper_dir()).unwrap().count(), 0);
    }

    #[test]
    fn diff_detects_added_files() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();

        // A file that doesn't exist anywhere on the host
        let unique = format!("envpod_test_{}", Uuid::new_v4());
        std::fs::write(state.upper_dir().join(&unique), "new content").unwrap();

        let diffs = backend.diff(&handle).unwrap();
        let found = diffs
            .iter()
            .find(|d| d.path.to_string_lossy().contains(&unique));

        assert!(found.is_some(), "should detect added file");
        assert_eq!(found.unwrap().kind, DiffKind::Added);
        assert!(found.unwrap().size > 0);
    }

    #[test]
    fn diff_detects_modified_files() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();

        // /etc/passwd exists on all Linux systems — simulate modification
        std::fs::create_dir_all(state.upper_dir().join("etc")).unwrap();
        std::fs::write(state.upper_dir().join("etc/passwd"), "modified").unwrap();

        let diffs = backend.diff(&handle).unwrap();
        let found = diffs
            .iter()
            .find(|d| d.path == PathBuf::from("/etc/passwd"));

        assert!(found.is_some(), "should detect modified /etc/passwd");
        assert_eq!(found.unwrap().kind, DiffKind::Modified);
    }

    #[test]
    fn diff_recurses_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();

        let unique = format!("envpod_test_{}", Uuid::new_v4());
        std::fs::create_dir_all(state.upper_dir().join("a/b/c")).unwrap();
        std::fs::write(
            state.upper_dir().join(format!("a/b/c/{unique}")),
            "deep",
        )
        .unwrap();

        let diffs = backend.diff(&handle).unwrap();
        let found = diffs
            .iter()
            .find(|d| d.path.to_string_lossy().contains(&unique));

        assert!(found.is_some(), "should find deeply nested file");
        assert_eq!(found.unwrap().kind, DiffKind::Added);
    }

    #[test]
    fn commit_copies_to_lower() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("fake_lower");
        std::fs::create_dir_all(&fake_lower).unwrap();

        // Create a pod with fake_lower as the lower dir
        let pod_dir = tmp.path().join("pod");
        overlay::create_dirs(&pod_dir).unwrap();

        // Simulate agent writes
        std::fs::create_dir_all(pod_dir.join("upper/subdir")).unwrap();
        std::fs::write(pod_dir.join("upper/subdir/file.txt"), "committed").unwrap();

        // Commit directly via overlay module (using fake lower)
        overlay::commit(&pod_dir.join("upper"), &fake_lower).unwrap();

        let target = fake_lower.join("subdir/file.txt");
        assert!(target.exists());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "committed");
    }

    #[test]
    fn info_returns_created_for_new_pod() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let info = backend.info(&handle).unwrap();
        assert_eq!(info.status, PodStatus::Created);
        assert!(info.process.is_none());
    }

    #[test]
    fn state_round_trips_through_handle() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        let state = NativeState::from_handle(&handle).unwrap();
        assert_eq!(state.lower_dirs, vec![PathBuf::from("/")]);
        assert_eq!(state.status, NativeStatus::Created);
        assert!(state.pod_dir.starts_with(tmp.path()));
    }

    // -----------------------------------------------------------------------
    // Root-only integration tests
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn full_lifecycle_with_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        // Write to /opt (NOT /tmp — /tmp is a fresh tmpfs that bypasses the overlay)
        let test_path = format!("/opt/envpod_lifecycle_{}", Uuid::new_v4());
        let proc_handle = backend
            .start(
                &handle,
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("echo 'agent wrote this' > {test_path}"),
                ],
                None,
                &[],
                None,
            )
            .unwrap();

        assert!(proc_handle.pid > 0);

        // Wait for completion
        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(proc_handle.pid as i32),
            None,
        )
        .unwrap();

        // File should NOT be on host
        assert!(
            !Path::new(&test_path).exists(),
            "write should not leak to host"
        );

        // File should show up in diff
        let diffs = backend.diff(&handle).unwrap();
        assert!(
            !diffs.is_empty(),
            "diff should show the written file"
        );

        // Rollback should clear
        backend.rollback(&handle).unwrap();
        let diffs_after = backend.diff(&handle).unwrap();
        assert!(
            diffs_after.is_empty(),
            "diff should be empty after rollback"
        );

        // Cleanup
        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn proc_isolation_hides_host_pids() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        // Write to /opt (NOT /tmp — /tmp is a fresh tmpfs that bypasses the overlay)
        let output_file = format!("/opt/envpod_proc_test_{}", Uuid::new_v4());
        let proc_handle = backend
            .start(
                &handle,
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("ls /proc > {output_file}"),
                ],
                None,
                &[],
                None,
            )
            .unwrap();

        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(proc_handle.pid as i32),
            None,
        )
        .unwrap();

        // Read the captured output from the overlay upper layer
        let upper_output = tmp
            .path()
            .join(handle.id.to_string())
            .join("upper")
            .join(output_file.trim_start_matches('/'));

        let contents = std::fs::read_to_string(&upper_output)
            .unwrap_or_else(|_| panic!("should find proc listing at {}", upper_output.display()));

        // With a fresh procfs mount, the pod should see a very limited set of
        // numeric PIDs. Host typically has hundreds. The pod should have very few
        // (the shell + ls, plus PID 1 from the namespace perspective).
        let numeric_pids: Vec<&str> = contents
            .lines()
            .filter(|line| line.trim().parse::<u32>().is_ok())
            .collect();

        // Sanity: should see at least PID 1 (self)
        assert!(
            !numeric_pids.is_empty(),
            "pod should see at least its own PID"
        );

        // The pod should NOT see hundreds of host PIDs
        assert!(
            numeric_pids.len() < 20,
            "fresh procfs should show very few PIDs, got {}: {:?}",
            numeric_pids.len(),
            numeric_pids
        );

        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn rootfs_hides_host_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();

        // List /home and /var inside the pod — should be empty with rootfs isolation
        let output_file = format!("/opt/envpod_rootfs_test_{}", Uuid::new_v4());
        let proc_handle = backend
            .start(
                &handle,
                &[
                    "/bin/sh".into(),
                    "-c".into(),
                    format!(
                        "echo HOME_CONTENTS: > {output_file} && \
                         ls /home >> {output_file} 2>&1 && \
                         echo VAR_CONTENTS: >> {output_file} && \
                         ls /var >> {output_file} 2>&1"
                    ),
                ],
                None,
                &[],
                None,
            )
            .unwrap();

        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(proc_handle.pid as i32),
            None,
        )
        .unwrap();

        let upper_output = tmp
            .path()
            .join(handle.id.to_string())
            .join("upper")
            .join(output_file.trim_start_matches('/'));

        let contents = std::fs::read_to_string(&upper_output)
            .unwrap_or_else(|_| panic!("should find rootfs test at {}", upper_output.display()));

        // /home should be empty — no host home directories visible
        let home_section = contents
            .split("HOME_CONTENTS:")
            .nth(1)
            .and_then(|s| s.split("VAR_CONTENTS:").next())
            .unwrap_or("")
            .trim();

        assert!(
            home_section.is_empty(),
            "pod /home should be EMPTY (rootfs isolation), but found: {home_section}"
        );

        // /var should be empty — no host /var content visible
        let var_section = contents
            .split("VAR_CONTENTS:")
            .nth(1)
            .unwrap_or("")
            .trim();

        assert!(
            var_section.is_empty(),
            "pod /var should be EMPTY (rootfs isolation), but found: {var_section}"
        );

        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn audit_log_records_lifecycle() {
        use crate::audit::{AuditAction, AuditLog};

        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_runtime_dir(tmp.path().to_path_buf());
        let handle = backend.create(&test_config()).unwrap();
        let state = NativeState::from_handle(&handle).unwrap();

        // Start a simple echo command
        let proc_handle = backend
            .start(&handle, &["/bin/echo".into(), "audit-test".into()], None, &[], None)
            .unwrap();

        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(proc_handle.pid as i32),
            None,
        )
        .unwrap();

        // Trigger diff, rollback
        let _diffs = backend.diff(&handle).unwrap();
        backend.rollback(&handle).unwrap();

        // Read audit log before destroy (destroy removes the pod dir)
        let log = AuditLog::new(&state.pod_dir);
        let entries = log.read_all().unwrap();

        // Should have: create, start, diff, rollback (in order)
        assert!(
            entries.len() >= 4,
            "expected at least 4 audit entries, got {}: {:?}",
            entries.len(),
            entries.iter().map(|e| format!("{}", e.action)).collect::<Vec<_>>()
        );

        let actions: Vec<AuditAction> = entries.iter().map(|e| e.action).collect();
        assert_eq!(actions[0], AuditAction::Create);
        assert_eq!(actions[1], AuditAction::Start);
        assert_eq!(actions[2], AuditAction::Diff);
        assert_eq!(actions[3], AuditAction::Rollback);

        // All should be successful
        assert!(entries.iter().all(|e| e.success));

        // All should reference the pod name
        assert!(entries.iter().all(|e| e.pod_name == "test-pod"));

        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn network_namespace_creates_veth() {
        use crate::config::{DnsConfig, PodNetworkConfig};

        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::new(tmp.path()).unwrap();

        let mut config = test_config();
        config.network = PodNetworkConfig {
            mode: NetworkMode::Isolated,
            dns: DnsConfig {
                mode: crate::types::DnsMode::Whitelist,
                allow: vec!["anthropic.com".into()],
                deny: Vec::new(),
                remap: std::collections::HashMap::new(),
            },
            ..Default::default()
        };

        let handle = backend.create(&config).unwrap();
        let state = NativeState::from_handle(&handle).unwrap();

        // Should have network state
        let net = state.network.as_ref().expect("should have network state");

        // Verify netns file exists
        assert!(
            net.netns_path.exists(),
            "netns file should exist at {}",
            net.netns_path.display()
        );

        // Verify host veth interface exists
        let output = std::process::Command::new("ip")
            .args(["link", "show", &net.host_veth])
            .output()
            .expect("ip link show should run");
        assert!(
            output.status.success(),
            "host veth {} should exist",
            net.host_veth
        );

        // Destroy and verify cleanup
        backend.destroy(&handle).unwrap();

        // Netns should be gone
        assert!(
            !net.netns_path.exists(),
            "netns file should be cleaned up"
        );

        // Host veth should be gone
        let output = std::process::Command::new("ip")
            .args(["link", "show", &net.host_veth])
            .output()
            .expect("ip link show should run");
        assert!(
            !output.status.success(),
            "host veth should be cleaned up after destroy"
        );
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn restore_recreates_netns_and_cgroup_after_deletion() {
        use crate::config::{DnsConfig, PodNetworkConfig};

        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::new(tmp.path()).unwrap();

        let mut config = test_config();
        config.network = PodNetworkConfig {
            mode: NetworkMode::Isolated,
            dns: DnsConfig {
                mode: crate::types::DnsMode::Whitelist,
                allow: vec!["anthropic.com".into()],
                deny: Vec::new(),
                remap: std::collections::HashMap::new(),
            },
            ..Default::default()
        };

        let handle = backend.create(&config).unwrap();
        let state = NativeState::from_handle(&handle).unwrap();
        let net = state.network.as_ref().expect("should have network state");

        // Save pod.yaml so restore() can read config
        let yaml = serde_yaml::to_string(&config).unwrap();
        std::fs::write(state.config_path(), yaml).unwrap();

        // Verify kernel state exists
        assert!(netns::netns_exists(&net.netns_name));
        assert!(netns::veth_exists(&net.host_veth));

        // Simulate reboot: delete netns and cgroup
        let _ = std::process::Command::new("ip")
            .args(["link", "del", &net.host_veth])
            .output();
        let _ = std::process::Command::new("ip")
            .args(["netns", "del", &net.netns_name])
            .output();
        if let Some(ref cg) = state.cgroup_path {
            cgroup::destroy(cg).ok();
        }

        // Verify they're gone
        assert!(!netns::netns_exists(&net.netns_name));
        assert!(!netns::veth_exists(&net.host_veth));

        // Restore should recreate them
        let restored = backend.restore(&handle).unwrap();
        assert!(restored, "restore should return true after recreating state");

        // Verify kernel state is back
        assert!(
            netns::netns_exists(&net.netns_name),
            "netns should be restored"
        );
        assert!(
            netns::veth_exists(&net.host_veth),
            "host veth should be restored"
        );
        if let Some(ref cg) = state.cgroup_path {
            assert!(
                cgroup::cgroup_exists(cg),
                "cgroup should be restored"
            );
        }

        // Cleanup
        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn restore_is_noop_when_state_intact() {
        use crate::config::{DnsConfig, PodNetworkConfig};

        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::new(tmp.path()).unwrap();

        let mut config = test_config();
        config.network = PodNetworkConfig {
            mode: NetworkMode::Isolated,
            dns: DnsConfig {
                mode: crate::types::DnsMode::Whitelist,
                allow: vec!["anthropic.com".into()],
                deny: Vec::new(),
                remap: std::collections::HashMap::new(),
            },
            ..Default::default()
        };

        let handle = backend.create(&config).unwrap();

        // Restore should be a no-op (returns false)
        let restored = backend.restore(&handle).unwrap();
        assert!(!restored, "restore should return false when state is intact");

        backend.destroy(&handle).unwrap();
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn destroy_succeeds_with_missing_netns() {
        use crate::config::{DnsConfig, PodNetworkConfig};

        let tmp = tempfile::tempdir().unwrap();
        let backend = NativeBackend::new(tmp.path()).unwrap();

        let mut config = test_config();
        config.network = PodNetworkConfig {
            mode: NetworkMode::Isolated,
            dns: DnsConfig {
                mode: crate::types::DnsMode::Whitelist,
                allow: vec!["anthropic.com".into()],
                deny: Vec::new(),
                remap: std::collections::HashMap::new(),
            },
            ..Default::default()
        };

        let handle = backend.create(&config).unwrap();
        let state = NativeState::from_handle(&handle).unwrap();
        let net = state.network.as_ref().expect("should have network state");

        // Simulate reboot: delete netns and cgroup
        let _ = std::process::Command::new("ip")
            .args(["link", "del", &net.host_veth])
            .output();
        let _ = std::process::Command::new("ip")
            .args(["netns", "del", &net.netns_name])
            .output();
        if let Some(ref cg) = state.cgroup_path {
            cgroup::destroy(cg).ok();
        }

        // Destroy should still succeed
        backend.destroy(&handle).unwrap();
        assert!(!state.pod_dir.exists(), "pod dir should be removed");
    }
}
