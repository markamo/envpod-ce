// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::backend::native::seccomp::SeccompProfile;
use crate::types::{DnsMode, MountPermission, NetworkMode, ResourceLimits};

/// Top-level pod configuration, parsed from pod.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PodConfig {
    pub name: String,

    #[serde(rename = "type")]
    pub pod_type: PodType,

    pub backend: String,

    pub filesystem: FilesystemConfig,
    pub network: PodNetworkConfig,
    pub processor: ProcessorConfig,
    pub budget: BudgetConfig,
    pub audit: AuditConfig,
    pub tools: ToolsConfig,
    pub security: SecurityConfig,
    pub devices: DevicesConfig,
    pub snapshots: SnapshotConfig,
    pub queue: QueueConfig,
    pub web_display: WebDisplayConfig,
    pub host_user: HostUserConfig,

    /// Default user to run commands as inside the pod.
    /// Defaults to "agent" (non-root, UID 60000) for full pod boundary protection.
    /// Set to "root" to run as root (reduces protection).
    #[serde(default = "default_user")]
    pub user: String,

    /// Shell commands to run inside the pod during setup.
    #[serde(default)]
    pub setup: Vec<String>,

    /// Path to a setup script on the host filesystem.
    /// The script is injected into the pod and executed after inline setup commands.
    #[serde(default)]
    pub setup_script: Option<String>,
}

fn default_user() -> String {
    "agent".into()
}

impl Default for PodConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            pod_type: PodType::Standard,
            backend: "native".into(),
            filesystem: FilesystemConfig::default(),
            network: PodNetworkConfig::default(),
            processor: ProcessorConfig::default(),
            budget: BudgetConfig::default(),
            audit: AuditConfig::default(),
            tools: ToolsConfig::default(),
            security: SecurityConfig::default(),
            devices: DevicesConfig::default(),
            snapshots: SnapshotConfig::default(),
            queue: QueueConfig::default(),
            web_display: WebDisplayConfig::default(),
            host_user: HostUserConfig::default(),
            user: default_user(),
            setup: Vec::new(),
            setup_script: None,
        }
    }
}

impl PodConfig {
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        Ok(Self::from_yaml(&contents)?)
    }

    /// Convert pod config into cgroup resource limits.
    pub fn to_resource_limits(&self) -> ResourceLimits {
        ResourceLimits {
            cpu_cores: self.processor.cores,
            memory_bytes: self.processor.memory.as_deref().and_then(parse_memory_string),
            disk_bytes: None,
            max_pids: self.processor.max_pids,
            cpuset_cpus: self.processor.cpu_affinity.clone(),
        }
    }
}

/// Parse a human-readable memory string (e.g. "512MB", "2GB", "4096") into bytes.
/// Returns None for unparseable input.
pub fn parse_memory_string(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num_str, multiplier) = if let Some(n) = s.strip_suffix("GB") {
        (n.trim(), 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n.trim(), 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n.trim(), 1024)
    } else {
        (s, 1u64)
    };
    num_str.parse::<f64>().ok().map(|n| (n * multiplier as f64) as u64)
}

/// Parse a human-readable duration string into seconds.
/// Supports: "30s", "5m", "2h", "1h30m", "90" (seconds).
pub fn parse_duration_string(s: &str) -> Option<u64> {
    let s = s.trim();

    // Try plain integer (seconds)
    if let Ok(secs) = s.parse::<u64>() {
        return Some(secs);
    }

    let mut total: u64 = 0;
    let mut num_buf = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else {
            let n: u64 = num_buf.parse().ok()?;
            num_buf.clear();
            match ch {
                'h' | 'H' => total += n * 3600,
                'm' | 'M' => total += n * 60,
                's' | 'S' => total += n,
                _ => return None,
            }
        }
    }

    if !num_buf.is_empty() {
        // Trailing number without unit — treat as seconds
        let n: u64 = num_buf.parse().ok()?;
        total += n;
    }

    if total > 0 { Some(total) } else { None }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PodType {
    #[default]
    Standard,
    Hardened,
    Ephemeral,
    Supervised,
    AirGapped,
}

// -- Filesystem -----------------------------------------------------------

/// Controls how system directories (/usr, /bin, /sbin, /lib, /lib64) are
/// handled inside the pod.
///
/// - **Safe** (default): Read-only bind mounts on top of merged overlay.
///   Agents cannot write to system dirs at all.
/// - **Advanced**: Full COW via overlay (system dirs in lower layer).
///   Agents can write, but `envpod commit` blocks system changes unless
///   `--include-system` is passed.
/// - **Dangerous**: Full COW via overlay, and `envpod commit` warns but
///   allows system changes by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemAccess {
    #[default]
    Safe,
    Advanced,
    Dangerous,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesystemConfig {
    pub mounts: Vec<MountEntry>,
    pub workspace: Option<PathBuf>,
    pub tracking: TrackingConfig,
    pub system_access: SystemAccess,
    #[serde(default)]
    pub apps: Vec<String>,
    /// Mount the caller's working directory into the pod (read-only, COW overlay).
    /// When true in pod.yaml, the CWD at `envpod init` time is captured and
    /// mounted on every `envpod run`. Override at runtime with --mount-cwd / --no-mount-cwd.
    #[serde(default)]
    pub mount_cwd: bool,
    /// The captured CWD path (set automatically during `envpod init` when mount_cwd is true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_path: Option<PathBuf>,
}

/// Controls which paths appear in `envpod diff` and `envpod commit` by default.
///
/// When `watch` is non-empty, only changes under those prefixes are shown.
/// Paths matching any `ignore` prefix are always excluded.
/// Use `--all` on the CLI to bypass this filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackingConfig {
    /// Paths to watch for changes (absolute, prefix-matched).
    /// If non-empty, only changes under these paths appear in filtered diff.
    /// Empty means watch everything (same as --all).
    pub watch: Vec<String>,
    /// Paths to always ignore in diff/commit (even under watched paths).
    pub ignore: Vec<String>,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            watch: vec![
                "/home".into(),
                "/opt".into(),
                "/root".into(),
                "/srv".into(),
                "/workspace".into(),
            ],
            ignore: vec![
                "/var/lib/apt".into(),
                "/var/lib/dpkg".into(),
                "/var/cache".into(),
                "/var/log".into(),
                "/tmp".into(),
                "/run".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountEntry {
    pub path: PathBuf,
    #[serde(default = "default_mount_permission")]
    pub permissions: MountPermission,
}

fn default_mount_permission() -> MountPermission {
    MountPermission::ReadOnly
}

// -- Network --------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PodNetworkConfig {
    pub mode: NetworkMode,
    pub dns: DnsConfig,
    pub rate_limit: Option<String>,
    pub bandwidth_cap: Option<String>,
    /// Subnet base for pod IP assignment (e.g. "10.201"). Default: "10.200".
    /// Pods with the same subnet base share an IP range (future: inter-pod routing).
    pub subnet: Option<String>,
    /// Localhost-only port mappings: "host_port:container_port[/proto]".
    /// Only accessible from the host machine (127.0.0.1). Safe default.
    /// CLI equivalent: -p
    #[serde(default)]
    pub ports: Vec<String>,
    /// Network-wide port mappings: "host_port:container_port[/proto]".
    /// Accessible from all host network interfaces — other machines on the LAN can reach these.
    /// CLI equivalent: -P
    #[serde(default)]
    pub public_ports: Vec<String>,
    /// Pod-to-pod port mappings: "container_port[/proto]".
    /// Accessible only from other pods (10.200.0.0/16 subnet). No host port mapping.
    /// User must tell agents the target pod's IP explicitly — no auto-discovery.
    /// CLI equivalent: -i
    #[serde(default)]
    pub internal_ports: Vec<String>,
    /// Advertise this pod to others via `<name>.pods.local` DNS.
    /// When true, the pod registers its name→IP in the running registry.
    /// Other pods' DNS servers resolve `<name>.pods.local` automatically.
    /// Default: false (pod is invisible to peer DNS).
    #[serde(default)]
    pub allow_discovery: bool,
    /// Pod names this pod is permitted to discover via `<name>.pods.local`.
    /// The central envpod-dns daemon enforces this bilaterally: the target must
    /// also have `allow_discovery: true`. Use `["*"]` to allow all discoverable pods.
    /// Default: [] (cannot discover any other pod).
    #[serde(default)]
    pub allow_pods: Vec<String>,
}

impl Default for PodNetworkConfig {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Isolated,
            dns: DnsConfig::default(),
            rate_limit: None,
            bandwidth_cap: None,
            subnet: None,
            ports: Vec::new(),
            public_ports: Vec::new(),
            internal_ports: Vec::new(),
            allow_discovery: false,
            allow_pods: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DnsConfig {
    pub mode: DnsMode,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub remap: std::collections::HashMap<String, String>,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            mode: DnsMode::Monitor,
            allow: Vec::new(),
            deny: Vec::new(),
            remap: std::collections::HashMap::new(),
        }
    }
}

// -- Processor ------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessorConfig {
    pub cores: Option<f64>,
    pub memory: Option<String>,
    /// Pin pod to specific CPUs (e.g. "0-1", "0,2"). Maps to cpuset.cpus.
    pub cpu_affinity: Option<String>,
    /// Maximum number of processes/threads the pod can create.
    pub max_pids: Option<u32>,
}

// -- Budget ---------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    pub max_duration: Option<String>,
    pub max_requests: Option<u64>,
    pub max_bandwidth: Option<String>,
}

// -- Tools ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Allowed commands (exact path or basename). Empty = allow all.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

// -- Devices --------------------------------------------------------------

/// Display protocol for pod passthrough.
///
/// - **Auto** (default): detect Wayland first, fall back to X11.
/// - **Wayland**: Wayland compositor socket only (secure — client isolation enforced).
/// - **X11**: X11 socket only (insecure — keylogging/screenshot possible).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayProtocol {
    #[default]
    Auto,
    Wayland,
    X11,
}

/// Audio protocol for pod passthrough.
///
/// - **Auto** (default): detect PipeWire first, fall back to PulseAudio.
/// - **Pipewire**: PipeWire socket only (finer-grained permissions).
/// - **Pulseaudio**: PulseAudio socket only (unrestricted microphone access).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioProtocol {
    #[default]
    Auto,
    Pipewire,
    Pulseaudio,
}

/// Desktop environment to install inside the pod.
///
/// - **None** (default): no desktop — CLI only.
/// - **Xfce**: XFCE4 + xfce4-terminal + dbus-x11 (~200MB). Familiar, lightweight.
/// - **Openbox**: Openbox + tint2 + xterm (~50MB). Ultra-minimal window manager.
/// - **Sway**: Sway + foot terminal (~150MB). Wayland-native compositor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopEnv {
    #[default]
    None,
    Xfce,
    Openbox,
    Sway,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicesConfig {
    /// Allow GPU access (NVIDIA + DRI devices). Default: false.
    pub gpu: bool,
    /// Auto-mount display socket. Default: false.
    pub display: bool,
    /// Auto-mount audio socket and /dev/snd. Default: false.
    pub audio: bool,
    /// Display protocol override (auto/wayland/x11). Default: auto.
    #[serde(default)]
    pub display_protocol: DisplayProtocol,
    /// Audio protocol override (auto/pipewire/pulseaudio). Default: auto.
    #[serde(default)]
    pub audio_protocol: AudioProtocol,
    /// Install a desktop environment during setup. Default: none (CLI only).
    /// Pairs with web_display (noVNC/WebRTC) or devices.display (host passthrough).
    #[serde(default)]
    pub desktop_env: DesktopEnv,
    /// Additional device paths to passthrough (e.g., "/dev/fuse").
    #[serde(default)]
    pub extra: Vec<String>,
}

// -- Security -------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Seccomp profile name: "default" or "browser". Empty/unknown → Default.
    #[serde(default)]
    pub seccomp_profile: String,
    /// Size of pod-private /dev/shm tmpfs (e.g. "256MB"). None → 64MB default.
    #[serde(default)]
    pub shm_size: Option<String>,
}

impl SecurityConfig {
    /// Convert the string profile name to a typed `SeccompProfile`.
    pub fn seccomp_profile(&self) -> SeccompProfile {
        match self.seccomp_profile.as_str() {
            "browser" => SeccompProfile::Browser,
            _ => SeccompProfile::Default,
        }
    }

    /// Parse `shm_size` into bytes (e.g. "256MB" → 268435456).
    pub fn shm_size_bytes(&self) -> Option<u64> {
        self.shm_size.as_deref().and_then(parse_memory_string)
    }
}

// -- Queue ----------------------------------------------------------------

/// Action queue configuration for the pod.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    /// Mount the queue Unix socket at /run/envpod/queue.sock inside the pod.
    /// Agents submit and poll actions via this socket — no env var needed.
    pub socket: bool,
    /// Require human approval before `envpod commit` executes.
    /// Submits a staged queue entry; `envpod approve <pod> <id>` executes it.
    pub require_commit_approval: bool,
    /// Require human approval before `envpod rollback` executes.
    pub require_rollback_approval: bool,
}

// -- Snapshots ------------------------------------------------------------

/// Snapshot configuration — automatic checkpoints of the overlay upper/.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotConfig {
    /// Automatically create a snapshot before each `envpod run`.
    /// The snapshot captures the overlay state just before the agent session,
    /// enabling "rollback to before last session" without manual checkpoints.
    pub auto_on_run: bool,
    /// Maximum total snapshots to keep. When exceeded, the oldest
    /// auto-created snapshots are pruned. Manual (named) snapshots are
    /// never pruned automatically. Default: 10.
    pub max_keep: usize,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self { auto_on_run: false, max_keep: 10 }
    }
}

// -- Audit ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub action_log: bool,
    pub system_trace: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            action_log: true,
            system_trace: false,
        }
    }
}

// -- Web Display --------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebDisplayType {
    #[default]
    None,
    Novnc,
    Webrtc,
}

/// Web display configuration — browser-based GUI access to pod desktops.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebDisplayConfig {
    /// Display type: none (default), novnc (CE), webrtc (Premium).
    #[serde(rename = "type")]
    pub display_type: WebDisplayType,
    /// Host port for browser access. Default: 6080.
    pub port: u16,
    /// Virtual display resolution (e.g. "1280x720"). Default: "1280x720".
    pub resolution: String,
    /// Enable audio streaming. noVNC: PulseAudio + Opus/WebM via WebSocket. WebRTC: GStreamer.
    pub audio: bool,
    /// Host port for audio WebSocket (noVNC audio only). Default: 6081.
    pub audio_port: u16,
    /// Enable file upload button in noVNC panel. Default: true.
    pub file_upload: bool,
    /// Host port for upload server (noVNC only). Default: 5080.
    pub upload_port: u16,
    /// Video codec (WebRTC only): vp8 or h264. Default: "vp8".
    pub codec: String,
}

impl Default for WebDisplayConfig {
    fn default() -> Self {
        Self {
            display_type: WebDisplayType::None,
            port: 6080,
            resolution: "1280x720".into(),
            audio: false,
            audio_port: 6081,
            file_upload: true,
            upload_port: 5080,
            codec: "vp8".into(),
        }
    }
}

// -- Host User ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostUserConfig {
    pub clone_host: bool,
    pub dirs: Vec<String>,
    pub exclude: Vec<String>,
    pub include_dotfiles: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_yaml() {
        let yaml = "name: test-agent\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.pod_type, PodType::Standard);
        assert_eq!(config.backend, "native");
    }

    #[test]
    fn parse_full_yaml() {
        let yaml = r#"
name: tax-agent
type: hardened
backend: native

filesystem:
  mounts:
    - path: ~/Documents/taxes
      permissions: ReadWrite
    - path: ~/Documents/receipts
      permissions: ReadOnly

network:
  mode: Monitored
  dns:
    mode: Whitelist
    allow:
      - api.anthropic.com
      - api.openai.com

processor:
  cores: 2.0
  memory: "2GB"

budget:
  max_duration: "2h"
  max_requests: 1000

audit:
  action_log: true
  system_trace: true
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.name, "tax-agent");
        assert_eq!(config.pod_type, PodType::Hardened);
        assert_eq!(config.filesystem.mounts.len(), 2);
        assert_eq!(
            config.filesystem.mounts[0].permissions,
            MountPermission::ReadWrite
        );
        assert_eq!(config.network.mode, NetworkMode::Monitored);
        assert_eq!(config.network.dns.allow.len(), 2);
        assert_eq!(config.processor.cores, Some(2.0));
        assert!(config.audit.system_trace);
    }

    #[test]
    fn default_config_is_secure() {
        let config = PodConfig::default();
        assert_eq!(config.network.mode, NetworkMode::Isolated);
        assert_eq!(config.network.dns.mode, DnsMode::Monitor);
        assert!(config.network.dns.allow.is_empty());
        assert!(config.filesystem.mounts.is_empty());
        assert!(config.audit.action_log);
        assert!(config.tools.allowed_commands.is_empty());
        assert!(config.network.subnet.is_none());
    }

    #[test]
    fn parse_yaml_with_custom_subnet() {
        let yaml = r#"
name: grouped-agent
network:
  mode: Isolated
  subnet: "10.201"
  dns:
    mode: Whitelist
    allow: [api.anthropic.com]
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.network.subnet, Some("10.201".into()));
        assert_eq!(config.network.mode, NetworkMode::Isolated);
    }

    #[test]
    fn parse_yaml_without_subnet_defaults_to_none() {
        let yaml = r#"
name: standard-agent
network:
  mode: Isolated
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.network.subnet.is_none());
    }

    // -- parse_memory_string ------------------------------------------------

    #[test]
    fn parse_memory_gigabytes() {
        assert_eq!(parse_memory_string("2GB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory_string("4GB"), Some(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parse_memory_megabytes() {
        assert_eq!(parse_memory_string("512MB"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory_string("1MB"), Some(1024 * 1024));
    }

    #[test]
    fn parse_memory_kilobytes() {
        assert_eq!(parse_memory_string("1024KB"), Some(1024 * 1024));
    }

    #[test]
    fn parse_memory_plain_bytes() {
        assert_eq!(parse_memory_string("65536"), Some(65536));
    }

    #[test]
    fn parse_memory_fractional() {
        assert_eq!(parse_memory_string("1.5GB"), Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
    }

    #[test]
    fn parse_memory_with_whitespace() {
        assert_eq!(parse_memory_string("  4GB  "), Some(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parse_memory_invalid() {
        assert_eq!(parse_memory_string("abc"), None);
        assert_eq!(parse_memory_string(""), None);
        assert_eq!(parse_memory_string("GB"), None);
    }

    // -- parse_duration_string ----------------------------------------------

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration_string("30s"), Some(30));
        assert_eq!(parse_duration_string("90s"), Some(90));
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration_string("5m"), Some(300));
        assert_eq!(parse_duration_string("1m"), Some(60));
    }

    #[test]
    fn parse_duration_hours() {
        assert_eq!(parse_duration_string("2h"), Some(7200));
        assert_eq!(parse_duration_string("1h"), Some(3600));
    }

    #[test]
    fn parse_duration_compound() {
        assert_eq!(parse_duration_string("1h30m"), Some(5400));
        assert_eq!(parse_duration_string("2h15m30s"), Some(2 * 3600 + 15 * 60 + 30));
    }

    #[test]
    fn parse_duration_plain_integer_as_seconds() {
        assert_eq!(parse_duration_string("90"), Some(90));
        assert_eq!(parse_duration_string("3600"), Some(3600));
    }

    #[test]
    fn parse_duration_with_whitespace() {
        assert_eq!(parse_duration_string("  2h  "), Some(7200));
    }

    #[test]
    fn parse_duration_invalid() {
        assert_eq!(parse_duration_string("abc"), None);
        assert_eq!(parse_duration_string(""), None);
        assert_eq!(parse_duration_string("5x"), None);
    }

    #[test]
    fn parse_duration_zero_returns_none() {
        // "0s" and "0m" parse to total=0 which returns None (no useful duration)
        assert_eq!(parse_duration_string("0s"), None);
        assert_eq!(parse_duration_string("0m"), None);
        // Plain "0" hits the u64 parse path and returns Some(0)
        assert_eq!(parse_duration_string("0"), Some(0));
    }

    // -- to_resource_limits -------------------------------------------------

    #[test]
    fn to_resource_limits_maps_cores_and_memory() {
        let config = PodConfig {
            processor: ProcessorConfig {
                cores: Some(2.0),
                memory: Some("4GB".into()),
                cpu_affinity: None,
                max_pids: None,
            },
            ..Default::default()
        };

        let limits = config.to_resource_limits();
        assert_eq!(limits.cpu_cores, Some(2.0));
        assert_eq!(limits.memory_bytes, Some(4 * 1024 * 1024 * 1024));
        assert!(limits.cpuset_cpus.is_none());
    }

    #[test]
    fn to_resource_limits_maps_cpu_affinity() {
        let config = PodConfig {
            processor: ProcessorConfig {
                cores: None,
                memory: None,
                cpu_affinity: Some("0-3".into()),
                max_pids: None,
            },
            ..Default::default()
        };

        let limits = config.to_resource_limits();
        assert_eq!(limits.cpuset_cpus, Some("0-3".into()));
        assert!(limits.cpu_cores.is_none());
        assert!(limits.memory_bytes.is_none());
    }

    #[test]
    fn to_resource_limits_empty_config() {
        let config = PodConfig::default();
        let limits = config.to_resource_limits();
        assert!(limits.cpu_cores.is_none());
        assert!(limits.memory_bytes.is_none());
        assert!(limits.cpuset_cpus.is_none());
    }

    // -- YAML round-trip for new fields -------------------------------------

    // -- SecurityConfig -----------------------------------------------------

    #[test]
    fn security_config_defaults() {
        let sec = SecurityConfig::default();
        assert_eq!(sec.seccomp_profile(), SeccompProfile::Default);
        assert!(sec.shm_size_bytes().is_none());
    }

    #[test]
    fn security_config_browser_profile() {
        let sec = SecurityConfig {
            seccomp_profile: "browser".into(),
            shm_size: Some("256MB".into()),
        };
        assert_eq!(sec.seccomp_profile(), SeccompProfile::Browser);
        assert_eq!(sec.shm_size_bytes(), Some(256 * 1024 * 1024));
    }

    #[test]
    fn security_config_unknown_profile_falls_back_to_default() {
        let sec = SecurityConfig {
            seccomp_profile: "unknown-thing".into(),
            shm_size: None,
        };
        assert_eq!(sec.seccomp_profile(), SeccompProfile::Default);
    }

    #[test]
    fn pod_config_deserialize_without_security() {
        // Backward compat: YAML without security section should deserialize fine
        let yaml = "name: old-agent\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.security.seccomp_profile(), SeccompProfile::Default);
        assert!(config.security.shm_size.is_none());
    }

    #[test]
    fn pod_config_deserialize_with_security() {
        let yaml = r#"
name: browser-agent
security:
  seccomp_profile: browser
  shm_size: "512MB"
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.security.seccomp_profile(), SeccompProfile::Browser);
        assert_eq!(config.security.shm_size_bytes(), Some(512 * 1024 * 1024));
    }

    #[test]
    fn parse_yaml_with_tools_and_affinity() {
        let yaml = r#"
name: secure-agent
processor:
  cores: 4.0
  memory: "8GB"
  cpu_affinity: "0-3"
tools:
  allowed_commands:
    - /bin/bash
    - /usr/bin/git
budget:
  max_duration: "1h30m"
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.processor.cpu_affinity, Some("0-3".into()));
        assert_eq!(config.tools.allowed_commands, vec!["/bin/bash", "/usr/bin/git"]);
        assert_eq!(config.budget.max_duration, Some("1h30m".into()));
    }

    // -- Setup commands -----------------------------------------------------

    #[test]
    fn parse_yaml_with_setup_commands() {
        let yaml = r#"
name: dev-agent
setup:
  - "apt-get update && apt-get install -y python3"
  - "pip install aider-chat"
  - "git clone https://github.com/example/repo.git /workspace"
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.setup.len(), 3);
        assert_eq!(config.setup[0], "apt-get update && apt-get install -y python3");
        assert_eq!(config.setup[2], "git clone https://github.com/example/repo.git /workspace");
    }

    #[test]
    fn setup_defaults_to_empty() {
        let yaml = "name: minimal\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.setup.is_empty());
    }

    // -- DevicesConfig ------------------------------------------------------

    #[test]
    fn devices_config_defaults_to_no_gpu() {
        let config = DevicesConfig::default();
        assert!(!config.gpu);
        assert!(!config.display);
        assert!(!config.audio);
        assert!(config.extra.is_empty());
    }

    #[test]
    fn pod_config_deserialize_without_devices() {
        // Backward compat: YAML without devices section should deserialize fine
        let yaml = "name: old-agent\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(!config.devices.gpu);
        assert!(!config.devices.display);
        assert!(!config.devices.audio);
        assert!(config.devices.extra.is_empty());
    }

    #[test]
    fn pod_config_deserialize_with_gpu() {
        let yaml = r#"
name: gpu-agent
devices:
  gpu: true
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.devices.gpu);
    }

    // -- TrackingConfig -------------------------------------------------------

    #[test]
    fn tracking_config_defaults() {
        let tracking = TrackingConfig::default();
        assert!(tracking.watch.contains(&"/home".to_string()));
        assert!(tracking.watch.contains(&"/workspace".to_string()));
        assert!(tracking.ignore.contains(&"/var/cache".to_string()));
        assert!(tracking.ignore.contains(&"/tmp".to_string()));
    }

    #[test]
    fn parse_yaml_with_tracking() {
        let yaml = r#"
name: tracked-agent
filesystem:
  tracking:
    watch:
      - /home
      - /workspace
      - /opt/myapp
    ignore:
      - /var/cache
      - /tmp
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.filesystem.tracking.watch, vec!["/home", "/workspace", "/opt/myapp"]);
        assert_eq!(config.filesystem.tracking.ignore, vec!["/var/cache", "/tmp"]);
    }

    #[test]
    fn parse_yaml_without_tracking_gets_defaults() {
        let yaml = "name: no-tracking\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(!config.filesystem.tracking.watch.is_empty());
        assert!(!config.filesystem.tracking.ignore.is_empty());
    }

    // -- SystemAccess ---------------------------------------------------------

    #[test]
    fn system_access_defaults_to_safe() {
        let yaml = "name: default-agent\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.filesystem.system_access, SystemAccess::Safe);
    }

    #[test]
    fn parse_yaml_with_system_access_advanced() {
        let yaml = r#"
name: advanced-agent
filesystem:
  system_access: advanced
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.filesystem.system_access, SystemAccess::Advanced);
    }

    #[test]
    fn parse_yaml_with_system_access_dangerous() {
        let yaml = r#"
name: dangerous-agent
filesystem:
  system_access: dangerous
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.filesystem.system_access, SystemAccess::Dangerous);
    }

    #[test]
    fn pod_config_deserialize_with_display_and_audio() {
        let yaml = r#"
name: media-agent
devices:
  display: true
  audio: true
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(!config.devices.gpu);
        assert!(config.devices.display);
        assert!(config.devices.audio);
        // Backward compat: protocol defaults to Auto when not specified
        assert_eq!(config.devices.display_protocol, DisplayProtocol::Auto);
        assert_eq!(config.devices.audio_protocol, AudioProtocol::Auto);
    }

    #[test]
    fn pod_config_deserialize_with_extra_devices() {
        let yaml = r#"
name: fuse-agent
devices:
  extra:
    - "/dev/fuse"
    - "/dev/kvm"
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(!config.devices.gpu);
        assert_eq!(config.devices.extra, vec!["/dev/fuse", "/dev/kvm"]);
    }

    // -- DisplayProtocol / AudioProtocol ------------------------------------

    #[test]
    fn display_protocol_defaults_to_auto() {
        let yaml = "name: test\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.display_protocol, DisplayProtocol::Auto);
    }

    #[test]
    fn parse_display_protocol_wayland() {
        let yaml = r#"
name: wayland-agent
devices:
  display: true
  display_protocol: wayland
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.devices.display);
        assert_eq!(config.devices.display_protocol, DisplayProtocol::Wayland);
    }

    #[test]
    fn parse_display_protocol_x11() {
        let yaml = r#"
name: x11-agent
devices:
  display: true
  display_protocol: x11
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.display_protocol, DisplayProtocol::X11);
    }

    #[test]
    fn audio_protocol_defaults_to_auto() {
        let yaml = "name: test\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.audio_protocol, AudioProtocol::Auto);
    }

    #[test]
    fn parse_audio_protocol_pipewire() {
        let yaml = r#"
name: pw-agent
devices:
  audio: true
  audio_protocol: pipewire
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.devices.audio);
        assert_eq!(config.devices.audio_protocol, AudioProtocol::Pipewire);
    }

    #[test]
    fn parse_audio_protocol_pulseaudio() {
        let yaml = r#"
name: pa-agent
devices:
  audio: true
  audio_protocol: pulseaudio
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.audio_protocol, AudioProtocol::Pulseaudio);
    }

    // -- DesktopEnv ----------------------------------------------------------

    #[test]
    fn desktop_env_defaults_to_none() {
        let yaml = "name: test\n";
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.desktop_env, DesktopEnv::None);
    }

    #[test]
    fn parse_desktop_env_xfce() {
        let yaml = r#"
name: desktop-agent
devices:
  desktop_env: xfce
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.desktop_env, DesktopEnv::Xfce);
    }

    #[test]
    fn parse_desktop_env_openbox() {
        let yaml = r#"
name: desktop-agent
devices:
  desktop_env: openbox
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.desktop_env, DesktopEnv::Openbox);
    }

    #[test]
    fn parse_desktop_env_sway() {
        let yaml = r#"
name: desktop-agent
devices:
  desktop_env: sway
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.devices.desktop_env, DesktopEnv::Sway);
    }

    #[test]
    fn backward_compat_no_protocol_fields() {
        // Existing pod.yaml files without display_protocol/audio_protocol must still parse
        let yaml = r#"
name: legacy-agent
devices:
  gpu: true
  display: true
  audio: true
  extra:
    - "/dev/fuse"
"#;
        let config = PodConfig::from_yaml(yaml).unwrap();
        assert!(config.devices.gpu);
        assert!(config.devices.display);
        assert!(config.devices.audio);
        assert_eq!(config.devices.display_protocol, DisplayProtocol::Auto);
        assert_eq!(config.devices.audio_protocol, AudioProtocol::Auto);
        assert_eq!(config.devices.extra, vec!["/dev/fuse"]);
    }
}
