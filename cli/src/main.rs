// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

use std::io::Seek;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

mod dashboard;
mod presets;
mod wizard;

use envpod_core::audit::{AuditAction, AuditEntry, AuditLog};
use envpod_core::backend::{create_backend, IsolationBackend};
use envpod_core::backend::native::NativeBackend;
use envpod_core::backend::native::{snapshot_base, has_base, resolve_base_name, destroy_base, gc_all};
use envpod_core::backend::native::state::{NativeState, NativeStatus};
use envpod_core::config::{self, PodConfig};
use envpod_core::monitor::{MonitorAgent, MonitorPolicy};
use envpod_core::queue::{ActionQueue, ActionTier, QueueExecutor};
use envpod_core::remote::ControlServer;
use envpod_core::store::PodStore;
use envpod_core::types::{DiffKind, MountConfig, MountPermission};
use envpod_core::undo::{UndoMechanism, UndoRegistry};
use envpod_dns::resolver::{DnsPolicy, DnsPolicyMode};
use envpod_dns::server::{self, DnsServer};

// ---------------------------------------------------------------------------
// ANSI color helpers (no external crate)
// ---------------------------------------------------------------------------

mod color {
    use std::io::IsTerminal;

    fn use_color() -> bool {
        std::io::stdout().is_terminal()
    }

    pub fn red(s: &str) -> String {
        if use_color() { format!("\x1b[31m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn green(s: &str) -> String {
        if use_color() { format!("\x1b[32m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn yellow(s: &str) -> String {
        if use_color() { format!("\x1b[33m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn cyan(s: &str) -> String {
        if use_color() { format!("\x1b[36m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn bold(s: &str) -> String {
        if use_color() { format!("\x1b[1m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn dim(s: &str) -> String {
        if use_color() { format!("\x1b[2m{s}\x1b[0m") } else { s.to_string() }
    }
}

/// Default base directory for all envpod state.
const DEFAULT_BASE_DIR: &str = "/var/lib/envpod";

#[derive(Parser)]
#[command(name = "envpod")]
#[command(about = "Zero-trust governance environments for AI agents (CE)")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " (CE) ", env!("ENVPOD_BUILD_HASH"), " ", env!("ENVPOD_BUILD_TIME")))]
struct Cli {
    /// Base directory for envpod state and pod data
    #[arg(long, env = "ENVPOD_DIR", default_value = DEFAULT_BASE_DIR, global = true)]
    dir: PathBuf,

    /// Show envpod version, license, and project info
    #[arg(long)]
    about: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new pod
    Init {
        /// Pod name
        name: String,
        /// Isolation backend
        #[arg(long, default_value = "native")]
        backend: String,
        /// Path to pod.yaml configuration file
        #[arg(short, long, conflicts_with = "preset")]
        config: Option<PathBuf>,
        /// Use a built-in preset (run `envpod presets` to list)
        #[arg(short = 'p', long, conflicts_with = "config")]
        preset: Option<String>,
        /// Create a base pod after successful setup (for fast cloning).
        /// Optionally specify a name (default: pod name).
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        create_base: Option<String>,
        /// Show live output from setup commands
        #[arg(short, long)]
        verbose: bool,
    },
    /// List available presets for `envpod init --preset`
    Presets,
    /// Run a command inside a pod (Ctrl+Z to detach, `envpod fg` to reattach)
    Run {
        /// Pod name
        name: String,
        /// Run as root inside the pod (default is non-root 'agent' user)
        #[arg(long)]
        root: bool,
        /// Run as this user (name or numeric uid) inside the pod
        #[arg(short, long)]
        user: Option<String>,
        /// Set environment variables (KEY=VALUE), can be repeated
        #[arg(short, long = "env")]
        env_vars: Vec<String>,
        /// Enable display forwarding (Wayland preferred, X11 fallback; override with display_protocol in pod.yaml)
        #[arg(short = 'd', long)]
        enable_display: bool,
        /// Enable audio forwarding (PipeWire preferred, PulseAudio fallback; override with audio_protocol in pod.yaml)
        #[arg(short = 'a', long)]
        enable_audio: bool,
        /// Run in background (use `envpod fg <pod>` to bring to foreground)
        #[arg(short = 'b', long)]
        background: bool,
        /// Publish port to localhost only: host_port:container_port[/proto] (e.g. -p 8080:3000)
        #[arg(short = 'p', long = "publish")]
        ports: Vec<String>,
        /// Publish port to all interfaces: host_port:container_port[/proto] (e.g. -P 8080:3000)
        #[arg(short = 'P', long = "publish-all")]
        public_ports: Vec<String>,
        /// Open port to other pods only: container_port[/proto] (e.g. -i 3000)
        #[arg(short = 'i', long = "internal")]
        internal_ports: Vec<String>,
        /// Mount the init-time working directory into the pod (overrides pod.yaml mount_cwd)
        #[arg(short = 'w', long)]
        mount_cwd: bool,
        /// Do not mount the working directory even if pod.yaml has mount_cwd: true
        #[arg(long, conflicts_with = "mount_cwd")]
        no_mount_cwd: bool,
        /// Command and arguments to execute
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Start a pod in the background (services auto-start, connect via noVNC or `envpod run <pod> -- bash`)
    Start {
        /// Pod name(s) to start
        #[arg(required_unless_present = "all")]
        names: Vec<String>,
        /// Start all stopped pods
        #[arg(long)]
        all: bool,
        /// Run as root inside the pod (default is non-root 'agent' user)
        #[arg(long)]
        root: bool,
        /// Run as this user (name or numeric uid) inside the pod
        #[arg(short, long)]
        user: Option<String>,
        /// Set environment variables (KEY=VALUE), can be repeated
        #[arg(short, long = "env")]
        env_vars: Vec<String>,
        /// Enable display forwarding
        #[arg(short = 'd', long, conflicts_with = "all")]
        enable_display: bool,
        /// Enable audio forwarding
        #[arg(short = 'a', long, conflicts_with = "all")]
        enable_audio: bool,
        /// Publish port to localhost only: host_port:container_port[/proto]
        #[arg(short = 'p', long = "publish", conflicts_with = "all")]
        ports: Vec<String>,
        /// Publish port to all interfaces: host_port:container_port[/proto]
        #[arg(short = 'P', long = "publish-all", conflicts_with = "all")]
        public_ports: Vec<String>,
        /// Open port to other pods only: container_port[/proto]
        #[arg(short = 'i', long = "internal", conflicts_with = "all")]
        internal_ports: Vec<String>,
        /// Command to run (overrides start_command from pod.yaml)
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Stop a running pod (preserves data, can `start` again)
    Stop {
        /// Pod name(s) to stop
        #[arg(required_unless_present = "all")]
        names: Vec<String>,
        /// Stop all running pods
        #[arg(long)]
        all: bool,
    },
    /// Restart a running pod (stop then start with same options)
    Restart {
        /// Pod name(s) to restart
        #[arg(required_unless_present = "all")]
        names: Vec<String>,
        /// Restart all running pods
        #[arg(long)]
        all: bool,
    },
    /// Bring a background pod to the foreground (attach to running process)
    Fg {
        /// Pod name
        name: String,
    },
    /// Show filesystem changes in a pod's overlay
    Diff {
        /// Pod name
        name: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Show all changes including system/ignored paths
        #[arg(long)]
        all: bool,
    },
    /// Commit overlay changes to the real filesystem
    Commit {
        /// Pod name
        name: String,
        /// File paths to commit (from diff output). Commits all if omitted.
        paths: Vec<String>,
        /// Commit all changes EXCEPT these paths
        #[arg(long)]
        exclude: Vec<String>,
        /// Export committed files to this directory instead of the host filesystem
        #[arg(long, short = 'o')]
        output: Option<String>,
        /// Commit all changes including system/ignored paths
        #[arg(long)]
        all: bool,
        /// Include system directory changes (/usr, /bin, /sbin, /lib, /lib64)
        #[arg(long)]
        include_system: bool,
        /// Rollback all uncommitted changes after selective commit
        #[arg(long)]
        rollback_rest: bool,
    },
    /// Discard overlay changes
    Rollback {
        /// Pod name
        name: String,
    },
    /// Show audit log for a pod
    Audit {
        /// Pod name (required for audit log; optional with --security)
        name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Static security audit of pod configuration
        #[arg(long)]
        security: bool,
        /// Path to pod.yaml (for --security without a created pod)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Freeze a pod (preserve state)
    Lock {
        /// Pod name (omit with --all for building-wide lockdown)
        name: Option<String>,
        /// Lock all pods
        #[arg(long)]
        all: bool,
    },
    /// Resume a frozen pod
    Unlock {
        /// Pod name
        name: String,
    },
    /// Terminate a pod's processes and rollback changes
    Kill {
        /// Pod name
        name: String,
    },
    /// Remove a pod entirely (overlay, cgroup, state)
    Destroy {
        /// Pod name(s) to destroy
        #[arg(required = true)]
        names: Vec<String>,
        /// Also remove the base pod (rootfs shared by clones)
        #[arg(long)]
        base: bool,
        /// Full cleanup: also remove iptables rules immediately (slower, no gc needed)
        #[arg(long)]
        full: bool,
    },
    /// Manage the action staging queue for a pod
    Queue {
        /// Pod name
        name: String,
        /// Output as JSON (when listing)
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<QueueAction>,
    },
    /// Approve a queued action
    Approve {
        /// Pod name
        name: String,
        /// Action ID (full UUID or unique 8-char prefix)
        id: String,
    },
    /// Cancel a queued action
    Cancel {
        /// Pod name
        name: String,
        /// Action ID (full UUID or unique 8-char prefix)
        id: String,
    },
    /// Show pod status and resource usage
    Status {
        /// Pod name
        name: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show pod output logs
    Logs {
        /// Pod name
        name: String,
        /// Follow log output (poll for new lines)
        #[arg(long, short)]
        follow: bool,
        /// Number of lines to show from the end (0 = all)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
    /// List all pods
    Ls {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage pod credential vault
    Vault {
        /// Pod name
        name: String,
        #[command(subcommand)]
        action: VaultAction,
    },
    /// Mount a host path into a pod's overlay
    Mount {
        /// Pod name
        name: String,
        /// Host path to mount
        host_path: PathBuf,
        /// Target path inside the pod (defaults to host_path)
        #[arg(long)]
        target: Option<PathBuf>,
        /// Mount as read-only
        #[arg(long)]
        readonly: bool,
    },
    /// Unmount a path from a pod
    Unmount {
        /// Pod name
        name: String,
        /// Path to unmount
        path: PathBuf,
    },
    /// Undo reversible actions on a pod
    Undo {
        /// Pod name
        name: String,
        /// Action ID to undo (full UUID or 8-char prefix). Omit to list undo-able actions.
        id: Option<String>,
        /// Undo all pending actions
        #[arg(long)]
        all: bool,
    },
    /// Send a remote control command to a running pod
    Remote {
        /// Pod name
        name: String,
        /// Command: freeze, resume, kill, restrict, status, alerts
        cmd: String,
        /// JSON payload (for restrict command)
        #[arg(long)]
        payload: Option<String>,
    },
    /// Manage monitoring policy for a pod
    Monitor {
        /// Pod name
        name: String,
        #[command(subcommand)]
        action: MonitorAction,
    },
    /// Update DNS policy on a running pod
    Dns {
        /// Pod name
        name: String,
        /// Add domain(s) to allow list
        #[arg(long)]
        allow: Vec<String>,
        /// Add domain(s) to deny list
        #[arg(long)]
        deny: Vec<String>,
        /// Remove domain(s) from allow list
        #[arg(long)]
        remove_allow: Vec<String>,
        /// Remove domain(s) from deny list
        #[arg(long)]
        remove_deny: Vec<String>,
    },
    /// Run setup commands for a pod (from pod.yaml setup section)
    Setup {
        /// Pod name
        name: String,
        /// Create a base pod after successful setup (for fast cloning).
        /// Optionally specify a name (default: pod name).
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        create_base: Option<String>,
        /// Show live output from setup commands
        #[arg(short, long)]
        verbose: bool,
    },
    /// Clone an existing pod (fast — skips rootfs rebuild and setup)
    Clone {
        /// Source pod name to clone from
        source: String,
        /// Name for the new cloned pod
        name: String,
        /// Clone from current state (includes agent modifications) instead of base snapshot
        #[arg(long)]
        current: bool,
    },
    /// Manage base pods (reusable rootfs snapshots for fast cloning)
    Base {
        #[command(subcommand)]
        action: BaseAction,
    },
    /// Manage the action catalog for a pod (host-defined tool menu for agents)
    Actions {
        /// Pod name
        name: String,
        #[command(subcommand)]
        action: ActionsSubcmd,
    },
    /// Start the web dashboard (fleet overview, pod detail, audit, diff)
    Dashboard {
        /// Port to listen on
        #[arg(long, default_value = "9090")]
        port: u16,
        /// Don't open the browser automatically
        #[arg(long)]
        no_open: bool,
        /// Run dashboard as a background daemon
        #[arg(long, short = 'd')]
        daemon: bool,
        /// Stop a running dashboard daemon
        #[arg(long, conflicts_with_all = ["daemon", "port", "no_open"])]
        stop: bool,
    },
    /// View or mutate port forwarding rules on a running pod (no restart required)
    Ports {
        /// Pod name
        name: String,
        /// Add a localhost-only port forward: host_port:container_port[/proto] (like -p)
        #[arg(long = "publish", short = 'p')]
        add_publish: Vec<String>,
        /// Add a public (all-interfaces) port forward: host_port:container_port[/proto] (like -P)
        #[arg(long = "publish-all", short = 'P')]
        add_publish_all: Vec<String>,
        /// Add a pod-to-pod internal port: container_port[/proto] (like -i)
        #[arg(long = "internal", short = 'i')]
        add_internal: Vec<String>,
        /// Remove a port forward by host port: port[/proto] (e.g. 8080 or 8080/udp)
        #[arg(long = "remove")]
        remove: Vec<String>,
        /// Remove an internal port by container port: port[/proto] (e.g. 3000)
        #[arg(long = "remove-internal")]
        remove_internal: Vec<String>,
    },
    /// View or mutate discovery settings on a running pod (no restart required)
    Discover {
        /// Pod name
        name: String,
        /// Enable discovery: make this pod resolvable as <name>.pods.local
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Disable discovery: remove this pod from the DNS registry
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Add a pod name to this pod's allow_pods list
        #[arg(long = "add-pod")]
        add_pods: Vec<String>,
        /// Remove a pod name from this pod's allow_pods list (use '*' to clear all)
        #[arg(long = "remove-pod")]
        remove_pods: Vec<String>,
    },
    /// Start the central pod discovery DNS daemon (required for allow_discovery / allow_pods)
    DnsDaemon {
        /// Unix socket path (default: /var/lib/envpod/dns.sock)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Remove all stopped pods (preserves running/frozen pods and base pods)
    Prune {
        /// Also prune unused base pods
        #[arg(long)]
        bases: bool,
    },
    /// Clean up stale iptables rules from destroyed pods
    Gc,
    /// Manage pod overlay snapshots (checkpoints of the filesystem state)
    Snapshot {
        /// Pod name
        name: String,
        #[command(subcommand)]
        action: SnapshotAction,
    },
    /// Resize CPU, memory, or disk limits on a pod (live if running, persisted to config)
    Resize {
        /// Pod name
        name: String,
        /// CPU cores (fractional allowed, e.g., 0.5, 2, 4)
        #[arg(long)]
        cpus: Option<f64>,
        /// Memory limit (e.g., "512MB", "2GB", "16GB")
        #[arg(long)]
        memory: Option<String>,
        /// /tmp tmpfs size (e.g., "512MB", "2GB")
        #[arg(long)]
        tmp_size: Option<String>,
        /// Max disk size for overlay (e.g., "10GB")
        #[arg(long)]
        disk_size: Option<String>,
        /// Max processes/threads
        #[arg(long)]
        max_pids: Option<u32>,
        /// CPU affinity (e.g., "0-1", "0,2")
        #[arg(long)]
        cpu_affinity: Option<String>,
        /// Enable GPU passthrough (stopped pods only)
        #[arg(long)]
        gpu: Option<bool>,
        /// Enable display forwarding (stopped pods only)
        #[arg(long)]
        display: Option<bool>,
        /// Enable audio forwarding (stopped pods only)
        #[arg(long)]
        audio: Option<bool>,
        /// Desktop environment: none, xfce, openbox, sway (stopped pods only)
        #[arg(long)]
        desktop: Option<String>,
        /// Web display type: none, novnc (stopped pods only)
        #[arg(long)]
        web_display: Option<String>,
    },
    /// Generate shell tab completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Screen text or files for prompt injection, credential exposure, PII, and exfiltration
    Screen {
        /// Text to screen (or use --file)
        text: Vec<String>,
        /// Screen a file's contents
        #[arg(long, short)]
        file: Option<String>,
        /// Screen as an API request body (parses JSON for message content)
        #[arg(long)]
        api: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show envpod version, license, and project info
    About,
}

#[derive(Subcommand)]
enum SnapshotAction {
    /// Create a snapshot of the current overlay state
    Create {
        /// Optional human-readable label
        #[arg(long, short = 'n')]
        name: Option<String>,
    },
    /// List all snapshots for a pod
    Ls,
    /// Restore the overlay to a snapshot (pod must be stopped)
    Restore {
        /// Snapshot id or unique prefix
        id: String,
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Delete a snapshot
    Destroy {
        /// Snapshot id or unique prefix
        id: String,
    },
    /// Prune oldest auto-snapshots, keeping at most max_keep (from pod.yaml)
    Prune,
    /// Promote a snapshot to a new clonable base pod
    Promote {
        /// Snapshot id or unique prefix
        id: String,
        /// Name for the new base pod
        base_name: String,
    },
}

#[derive(Subcommand)]
enum VaultAction {
    /// Set a secret (reads value from stdin)
    Set {
        /// Secret key name (alphanumeric + underscore)
        key: String,
    },
    /// Get a secret value
    Get {
        /// Secret key name
        key: String,
    },
    /// List all secret keys
    List,
    /// Remove a secret
    Rm {
        /// Secret key name
        key: String,
    },
    /// Import secrets from a .env file (KEY=value lines)
    Import {
        /// Path to .env file
        path: PathBuf,
        /// Overwrite existing keys (default: skip conflicts)
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Subcommand)]
enum MonitorAction {
    /// Validate and install a monitoring policy
    SetPolicy {
        /// Path to monitoring-policy.yaml
        path: PathBuf,
    },
    /// Show monitor alerts from the audit log
    Alerts {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BaseAction {
    /// Create a base pod (init + setup + snapshot, then remove the temporary pod)
    Create {
        /// Name for the base pod
        name: String,
        /// Path to pod.yaml config
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Show live output from setup commands
        #[arg(short, long)]
        verbose: bool,
    },
    /// List all base pods
    Ls {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove one or more base pods
    Destroy {
        /// Base pod name(s)
        #[arg(required = true)]
        names: Vec<String>,
        /// Force removal even if pods still reference this base
        #[arg(long)]
        force: bool,
    },
    /// Remove all base pods not used by any pod
    Prune,
    /// Resize a base pod's config (affects future clones)
    Resize {
        /// Base pod name
        name: String,
        /// CPU cores (fractional allowed, e.g., 0.5, 2, 4)
        #[arg(long)]
        cpus: Option<f64>,
        /// Memory limit (e.g., "512MB", "2GB", "16GB")
        #[arg(long)]
        memory: Option<String>,
        /// /tmp tmpfs size (e.g., "512MB", "2GB")
        #[arg(long)]
        tmp_size: Option<String>,
        /// Max disk size for overlay (e.g., "10GB")
        #[arg(long)]
        disk_size: Option<String>,
        /// Max processes/threads
        #[arg(long)]
        max_pids: Option<u32>,
        /// CPU affinity (e.g., "0-1", "0,2")
        #[arg(long)]
        cpu_affinity: Option<String>,
        /// Enable GPU passthrough
        #[arg(long)]
        gpu: Option<bool>,
        /// Enable display forwarding
        #[arg(long)]
        display: Option<bool>,
        /// Enable audio forwarding
        #[arg(long)]
        audio: Option<bool>,
        /// Desktop environment: none, xfce, openbox, sway
        #[arg(long)]
        desktop: Option<String>,
        /// Web display type: none, novnc
        #[arg(long)]
        web_display: Option<String>,
    },
}

#[derive(Subcommand)]
enum QueueAction {
    /// Add an action to the queue
    Add {
        /// Reversibility tier (delayed, staged, blocked)
        #[arg(long)]
        tier: String,
        /// Description of the action
        #[arg(long)]
        description: String,
        /// Delay in seconds before auto-execution (delayed tier only, default: 30)
        #[arg(long)]
        delay: Option<u64>,
    },
}

#[derive(Subcommand)]
enum ActionsSubcmd {
    /// List all actions in the catalog
    Ls,
    /// Add or replace an action in the catalog
    Add {
        /// Action name (e.g. send_email)
        name: String,
        /// Description shown to agents
        #[arg(long)]
        description: String,
        /// Reversibility tier: immediate, delayed, staged, blocked (default: staged)
        #[arg(long, default_value = "staged")]
        tier: String,
        /// Add a parameter (format: "name[:required]", e.g. "to:required" or "body")
        /// Use multiple times: --param to:required --param subject:required --param body
        #[arg(long = "param")]
        params: Vec<String>,
    },
    /// Remove an action from the catalog
    Remove {
        /// Action name
        name: String,
    },
    /// Change the tier of an existing action (runtime — no pod restart needed)
    SetTier {
        /// Action name
        name: String,
        /// New tier: immediate, delayed, staged, blocked
        tier: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env(),
        )
        .init();

    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let base_dir = &cli.dir;

    // --about flag or no subcommand → show about info
    let command = if cli.about {
        Commands::About
    } else {
        match cli.command {
            Some(cmd) => cmd,
            None => {
                Cli::command().print_help()?;
                return Ok(());
            }
        }
    };

    let store = PodStore::new(base_dir.join("state"))?;

    match command {
        Commands::Init {
            name,
            backend,
            config,
            preset,
            create_base,
            verbose,
        } => cmd_init(&store, base_dir, &name, &backend, config.as_deref(), preset.as_deref(), create_base.as_deref(), verbose).await,
        Commands::Presets => {
            eprint!("{}", presets::format_table());
            Ok(())
        }
        Commands::Run { name, root, user, env_vars, enable_display, enable_audio, background, ports, public_ports, internal_ports, mount_cwd, no_mount_cwd, command } => {
            if background {
                cmd_run_background(base_dir, &name, &command, root, user.as_deref(), &env_vars, enable_display, enable_audio, &ports, &public_ports, &internal_ports)
            } else {
                cmd_run(&store, base_dir, &name, &command, root, user.as_deref(), &env_vars, enable_display, enable_audio, &ports, &public_ports, &internal_ports, mount_cwd, no_mount_cwd).await
            }
        }
        Commands::Start { names, all, root, user, env_vars, enable_display, enable_audio, ports, public_ports, internal_ports, command } => {
            let targets = if all {
                stopped_pod_names(&store)?
            } else {
                names
            };
            if targets.is_empty() {
                eprintln!("no stopped pods to start");
                return Ok(());
            }
            for name in &targets {
                // Resolve start command: CLI override > pod.yaml start_command > sleep infinity
                let start_cmd = if !command.is_empty() {
                    command.clone()
                } else {
                    resolve_start_command(&store, name)
                };
                cmd_run_background(base_dir, name, &start_cmd, root, user.as_deref(), &env_vars, enable_display, enable_audio, &ports, &public_ports, &internal_ports)?;
            }
            Ok(())
        }
        Commands::Stop { names, all } => {
            let targets = if all {
                running_pod_names(&store)?
            } else {
                names
            };
            if targets.is_empty() {
                eprintln!("no running pods to stop");
                return Ok(());
            }
            for name in &targets {
                cmd_stop(&store, base_dir, name).await?;
            }
            Ok(())
        }
        Commands::Restart { names, all } => {
            let targets = if all {
                running_pod_names(&store)?
            } else {
                names
            };
            if targets.is_empty() {
                eprintln!("no running pods to restart");
                return Ok(());
            }
            for name in &targets {
                eprintln!("restarting pod '{}'...", name);
                cmd_stop(&store, base_dir, name).await?;
                let start_cmd = resolve_start_command(&store, name);
                cmd_run_background(base_dir, name, &start_cmd, false, None, &[], false, false, &[], &[], &[])?;
            }
            Ok(())
        }
        Commands::Fg { name } => cmd_fg(&store, base_dir, &name).await,
        Commands::Diff { name, json, all } => cmd_diff(&store, base_dir, &name, json, all),
        Commands::Commit { name, paths, exclude, output, all, include_system, rollback_rest } => cmd_commit(&store, base_dir, &name, &paths, &exclude, output.as_deref(), all, include_system, rollback_rest),
        Commands::Rollback { name } => cmd_rollback(&store, base_dir, &name),
        Commands::Audit { name, json, security, config } => {
            if security {
                cmd_security_audit(&store, name.as_deref(), config.as_deref(), json)
            } else {
                let name = name.as_deref().unwrap_or_else(|| {
                    eprintln!("error: pod name is required for audit log (use --security for config audit)");
                    std::process::exit(1);
                });
                cmd_audit(&store, base_dir, name, json)
            }
        }
        Commands::Lock { name, all } => cmd_lock(&store, base_dir, name.as_deref(), all),
        Commands::Unlock { name } => cmd_unlock(&store, base_dir, &name),
        Commands::Kill { name } => cmd_kill(&store, base_dir, &name),
        Commands::Destroy { names, base, full } => {
            if base && names.len() > 1 {
                anyhow::bail!("--base cannot be used with multiple pod names");
            }
            for name in &names {
                cmd_destroy(&store, base_dir, name, base, full).await?;
            }
            Ok(())
        }
        Commands::Actions { name, action } => cmd_actions(&store, &name, action),
        Commands::Queue { name, json, action } => cmd_queue(&store, &name, json, action),
        Commands::Approve { name, id } => cmd_approve(&store, base_dir, &name, &id).await,
        Commands::Cancel { name, id } => cmd_cancel(&store, &name, &id),
        Commands::Status { name, json } => cmd_status(&store, base_dir, &name, json),
        Commands::Logs { name, follow, lines } => cmd_logs(&store, &name, follow, lines),
        Commands::Mount {
            name,
            host_path,
            target,
            readonly,
        } => cmd_mount(&store, base_dir, &name, &host_path, target.as_deref(), readonly),
        Commands::Unmount { name, path } => cmd_unmount(&store, base_dir, &name, &path),
        Commands::Undo { name, id, all } => cmd_undo(&store, base_dir, &name, id.as_deref(), all),
        Commands::Ls { json } => cmd_ls(&store, base_dir, json),
        Commands::Vault { name, action } => cmd_vault(&store, &name, action),
        Commands::Remote { name, cmd, payload } => {
            cmd_remote(&store, &name, &cmd, payload.as_deref()).await
        }
        Commands::Monitor { name, action } => cmd_monitor(&store, &name, action),
        Commands::Dns {
            name,
            allow,
            deny,
            remove_allow,
            remove_deny,
        } => cmd_dns(&store, &name, &allow, &deny, &remove_allow, &remove_deny).await,
        Commands::Setup { name, create_base, verbose } => cmd_setup(&store, base_dir, &name, create_base.as_deref(), verbose).await,
        Commands::Clone { source, name, current } => cmd_clone(&store, base_dir, &source, &name, current),
        Commands::Base { action } => cmd_base(&store, base_dir, action).await,
        Commands::Dashboard { port, no_open, daemon, stop } => dashboard::run(base_dir.clone(), port, no_open, daemon, stop).await,
        Commands::Ports { name, add_publish, add_publish_all, add_internal, remove, remove_internal } =>
            cmd_ports(&store, &name, &add_publish, &add_publish_all, &add_internal, &remove, &remove_internal),
        Commands::Discover { name, on, off, add_pods, remove_pods } =>
            cmd_discover(&store, base_dir, &name, on, off, &add_pods, &remove_pods).await,
        Commands::DnsDaemon { socket } => cmd_dns_daemon(base_dir, socket).await,
        Commands::Snapshot { name, action } => cmd_snapshot(&store, base_dir, &name, action),
        Commands::Resize { name, cpus, memory, tmp_size, disk_size, max_pids, cpu_affinity, gpu, display, audio, desktop, web_display } =>
            cmd_resize(&store, base_dir, &name, cpus, memory.as_deref(), tmp_size.as_deref(), disk_size.as_deref(), max_pids, cpu_affinity.as_deref(), gpu, display, audio, desktop.as_deref(), web_display.as_deref()),
        Commands::Prune { bases } => cmd_prune(&store, base_dir, bases).await,
        Commands::Gc => {
            let result = gc_all(&base_dir, &store)?;
            if result.total() == 0 {
                println!("Nothing to clean up");
            } else {
                if result.iptables_rules > 0 {
                    println!("Removed {} stale iptables rule{}", result.iptables_rules, if result.iptables_rules == 1 { "" } else { "s" });
                }
                if result.network_namespaces > 0 {
                    println!("Removed {} orphaned network namespace{}", result.network_namespaces, if result.network_namespaces == 1 { "" } else { "s" });
                }
                if result.cgroups > 0 {
                    println!("Removed {} orphaned cgroup{}", result.cgroups, if result.cgroups == 1 { "" } else { "s" });
                }
                if result.pod_directories > 0 {
                    println!("Removed {} orphaned pod director{}", result.pod_directories, if result.pod_directories == 1 { "y" } else { "ies" });
                }
                if result.state_files > 0 {
                    println!("Removed {} stale state file{}", result.state_files, if result.state_files == 1 { "" } else { "s" });
                }
                if result.index_files > 0 {
                    println!("Removed {} stale index file{}", result.index_files, if result.index_files == 1 { "" } else { "s" });
                }
            }
            Ok(())
        }
        Commands::Screen { text, file, api, json } => {
            let rules = envpod_core::screening::ScreeningRules::load_default();
            let content = if let Some(path) = file {
                std::fs::read_to_string(&path)
                    .with_context(|| format!("read file: {path}"))?
            } else if !text.is_empty() {
                text.join(" ")
            } else {
                // Read from stdin
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                buf
            };

            let result = if api {
                rules.screen_api_request(&content)
            } else {
                rules.screen(&content)
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if result.matched {
                eprintln!(
                    "  {} [{}] {}",
                    color::red("BLOCKED"),
                    result.category.as_deref().unwrap_or("unknown"),
                    result.pattern.as_deref().unwrap_or("")
                );
                if let Some(ref frag) = result.fragment {
                    eprintln!("  Context: {}", color::dim(frag));
                }
                std::process::exit(1);
            } else {
                eprintln!("  {} No issues detected", color::green("CLEAN"));
            }
            Ok(())
        }
        Commands::Completions { shell } => {
            print_completions(shell, base_dir);
            Ok(())
        }
        Commands::About => {
            let version = concat!(env!("CARGO_PKG_VERSION"), " (CE)");
            let build = concat!(env!("ENVPOD_BUILD_HASH"), " ", env!("ENVPOD_BUILD_TIME"));
            println!("{}", color::bold("envpod"));
            println!("Zero-trust governance environments for AI agents");
            println!();
            println!("  Version:  {version}");
            println!("  Build:    {build}");
            println!("  License:  BSL 1.1 (converts to AGPL-3.0 on 2030-03-07)");
            println!("  Source:   https://github.com/markamo/envpod-ce");
            println!("  Docs:     https://envpod.dev");
            println!();
            println!("  {}  Mark Amo-Boateng, PhD / Xtellix Inc.", color::dim("Author:"));
            println!();
            println!("{}", color::dim("Docker isolates. Envpod governs."));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

async fn cmd_init(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    backend_name: &str,
    config_path: Option<&std::path::Path>,
    preset_name: Option<&str>,
    create_base: Option<&str>,
    verbose: bool,
) -> Result<()> {
    if store.exists(name) {
        anyhow::bail!("pod '{name}' already exists");
    }

    // Install default screening rules (first init only, non-fatal)
    envpod_core::screening::install_default_rules(base_dir).ok();

    let mut config = if let Some(path) = config_path {
        // Explicit config file
        PodConfig::from_file(path)
            .with_context(|| format!("load config: {}", path.display()))?
    } else if let Some(pname) = preset_name {
        // --preset flag
        let preset = presets::get(pname).ok_or_else(|| {
            let available: Vec<&str> = presets::list().iter().map(|p| p.name).collect();
            anyhow::anyhow!(
                "unknown preset '{pname}'. Available presets:\n  {}",
                available.join(", ")
            )
        })?;
        serde_yaml::from_str(preset.yaml)
            .with_context(|| format!("parse preset '{pname}'"))?
    } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // Interactive wizard (stdin is a terminal)
        match wizard::run_interactive()? {
            Some(cfg) => cfg,
            None => PodConfig::default(), // user picked "custom"
        }
    } else {
        // Non-interactive (piped/scripted) — use defaults
        PodConfig::default()
    };
    config.name = name.to_string();
    config.backend = backend_name.to_string();

    // Capture CWD at init time when mount_cwd is enabled
    if config.filesystem.mount_cwd && config.filesystem.cwd_path.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            config.filesystem.cwd_path = Some(cwd);
        }
    }

    let init_start = std::time::Instant::now();
    let divider = color::dim("  ────────────────────────────────────────");

    // ── Banner ──
    let version = env!("CARGO_PKG_VERSION");
    let tag = format!("envpod CE v{version} — Zero-trust governance for AI agents");
    // Use char count (not byte len) so multi-byte chars like — don't widen the box
    let display_width = tag.chars().count();
    let inner_width = display_width + 2; // 1 space padding each side
    eprintln!();
    eprintln!("  {}┌{}┐{}", "\x1b[1m", "─".repeat(inner_width), "\x1b[0m");
    eprintln!("  {}│ {tag} │{}", "\x1b[1m", "\x1b[0m");
    eprintln!("  {}└{}┘{}", "\x1b[1m", "─".repeat(inner_width), "\x1b[0m");
    eprintln!();

    let create_start = std::time::Instant::now();
    let backend = create_backend(backend_name, base_dir)?;
    let handle = backend.create(&config)?;
    let create_elapsed = create_start.elapsed();

    store.save(&handle)?;

    // Persist pod.yaml into pod_dir so runtime features (budget, tools, vault)
    // can access the config without the original file path.
    if let Ok(state) = NativeState::from_handle(&handle) {
        let yaml = serde_yaml::to_string(&config).context("serialize pod config")?;
        std::fs::write(state.config_path(), yaml).context("persist pod.yaml")?;
    }

    // Write web display scripts to overlay (daemon + wrapper)
    if config.web_display.display_type != envpod_core::config::WebDisplayType::None {
        if let Ok(state) = NativeState::from_handle(&handle) {
            let wrapper = envpod_core::web_display::generate_supervisor_script(&config.web_display);
            let daemon = envpod_core::web_display::generate_services_daemon(&config.web_display, config.devices.desktop_env);
            // With advanced/dangerous system_access, /usr gets its own COW overlay
            // backed by sys_upper/usr/, so we must write there instead of upper/
            let script_dir = if matches!(config.filesystem.system_access, envpod_core::config::SystemAccess::Advanced | envpod_core::config::SystemAccess::Dangerous) {
                state.sys_upper_dir().join("usr/local/bin")
            } else {
                state.upper_dir().join("usr/local/bin")
            };
            std::fs::create_dir_all(&script_dir).ok();
            use std::os::unix::fs::PermissionsExt;
            // Write wrapper (envpod-display-start) — entry point for user commands
            let script_path = script_dir.join("envpod-display-start");
            std::fs::write(&script_path, &wrapper).context("write display wrapper script")?;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
            // Write daemon (envpod-display-services) — background service manager
            let daemon_path = script_dir.join("envpod-display-services");
            std::fs::write(&daemon_path, &daemon).context("write display services daemon")?;
            std::fs::set_permissions(&daemon_path, std::fs::Permissions::from_mode(0o755))?;

            // Inject audio overlay files (audio-proxy.sh, audio-plugin.js)
            for (pod_path, content, executable) in envpod_core::web_display::audio_overlay_files(&config.web_display) {
                let rel = pod_path.trim_start_matches('/');
                // For advanced/dangerous, /usr paths go to sys_upper
                let dest = if rel.starts_with("usr/") && matches!(config.filesystem.system_access, envpod_core::config::SystemAccess::Advanced | envpod_core::config::SystemAccess::Dangerous) {
                    state.sys_upper_dir().join(rel)
                } else {
                    state.upper_dir().join(rel)
                };
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&dest, &content)
                    .with_context(|| format!("write audio file: {pod_path}"))?;
                if executable {
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                }
            }

            // Inject upload overlay files (upload-server.py, upload-plugin.js)
            for (pod_path, content, executable) in envpod_core::web_display::upload_overlay_files(&config.web_display) {
                let rel = pod_path.trim_start_matches('/');
                let dest = if rel.starts_with("usr/") && matches!(config.filesystem.system_access, envpod_core::config::SystemAccess::Advanced | envpod_core::config::SystemAccess::Dangerous) {
                    state.sys_upper_dir().join(rel)
                } else {
                    state.upper_dir().join(rel)
                };
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&dest, &content)
                    .with_context(|| format!("write upload file: {pod_path}"))?;
                if executable {
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                }
            }

            // Inject clipboard sync plugin (clipboard-plugin.js)
            for (pod_path, content, executable) in envpod_core::web_display::clipboard_overlay_files(&config.web_display) {
                let rel = pod_path.trim_start_matches('/');
                let dest = if rel.starts_with("usr/") && matches!(config.filesystem.system_access, envpod_core::config::SystemAccess::Advanced | envpod_core::config::SystemAccess::Dangerous) {
                    state.sys_upper_dir().join(rel)
                } else {
                    state.upper_dir().join(rel)
                };
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&dest, &content)
                    .with_context(|| format!("write clipboard file: {pod_path}"))?;
                if executable {
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                }
            }

            // Inject info plugin (info-plugin.js — about/branding button)
            {
                let version = concat!(env!("CARGO_PKG_VERSION"), " (CE)");
                for (pod_path, content, executable) in envpod_core::web_display::info_overlay_files(&config.web_display, &name, version) {
                    let rel = pod_path.trim_start_matches('/');
                    let dest = if rel.starts_with("usr/") && matches!(config.filesystem.system_access, envpod_core::config::SystemAccess::Advanced | envpod_core::config::SystemAccess::Dangerous) {
                        state.sys_upper_dir().join(rel)
                    } else {
                        state.upper_dir().join(rel)
                    };
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    std::fs::write(&dest, &content)
                        .with_context(|| format!("write info file: {pod_path}"))?;
                    if executable {
                        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                    }
                }
            }

            // Patch vnc.html after apt installs the novnc package:
            // - Replace both noVNC logos (side panel + connect dialog) with envpod
            // - Update page title to "envpod — <pod-name>"
            // - Replace favicon with envpod SVG icon
            // - Auto-connect (skip the connect dialog)
            // - Load audio/upload plugin scripts (if enabled)
            if config.web_display.display_type == envpod_core::config::WebDisplayType::Novnc {
                let favicon_svg = r#"%3Csvg xmlns=%27http://www.w3.org/2000/svg%27 viewBox=%270 0 32 32%27%3E%3Crect width=%2732%27 height=%2732%27 rx=%276%27 fill=%27%2324292e%27/%3E%3Ctext x=%2716%27 y=%2722%27 text-anchor=%27middle%27 font-family=%27monospace%27 font-weight=%27bold%27 font-size=%2714%27 fill=%27%2358a6ff%27%3Eep%3C/text%3E%3C/svg%3E"#;
                let mut patches = vec![
                    // Disable browser cache so patched vnc.html is always fresh
                    r#"sed -i 's|</head>|<meta http-equiv="Cache-Control" content="no-cache, no-store, must-revalidate"><meta http-equiv="Pragma" content="no-cache"></head>|' /usr/share/novnc/vnc.html"#.to_string(),
                    // Side panel logo: "noVNC" → "envpod"
                    r#"sed -i 's|<h1 class="noVNC_logo"[^>]*><span>no</span><br>VNC</h1>|<h1 class="noVNC_logo" translate="no"><span>env</span><br>pod</h1>|' /usr/share/novnc/vnc.html"#.to_string(),
                    // Connect dialog logo: "noVNC" → "envpod"
                    r#"sed -i 's|<div class="noVNC_logo"[^>]*><span>no</span>VNC</div>|<div class="noVNC_logo" translate="no"><span>env</span>pod</div>|' /usr/share/novnc/vnc.html"#.to_string(),
                    // Page title
                    format!(r#"sed -i 's|<title>noVNC</title>|<title>envpod — {name}</title>|' /usr/share/novnc/vnc.html"#),
                    // Favicon
                    format!(
                        r#"sed -i 's|<link rel="icon"[^>]*>||g; s|<link rel="apple-touch-icon"[^>]*>||g; s|</head>|<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,{favicon_svg}"></head>|' /usr/share/novnc/vnc.html"#
                    ),
                    // Auto-connect: inject autoconnect=true default so users skip the connect dialog
                    r#"sed -i 's|</body>|<script>if(!location.search.includes("autoconnect=false"))window.addEventListener("load",function(){setTimeout(function(){var b=document.getElementById("noVNC_connect_button");if(b)b.click();},500);});</script></body>|' /usr/share/novnc/vnc.html"#.to_string(),
                    // Expose UI.rfb to window so clipboard-plugin.js can find it
                    // (noVNC 1.3 uses ES6 modules — module scope is not on window)
                    r#"sed -i 's|UI\.rfb\.addEventListener("connect", UI\.connectFinished);|UI.rfb.addEventListener("connect", UI.connectFinished); window.rfb = UI.rfb;|' /usr/share/novnc/app/ui.js"#.to_string(),
                    // Clipboard sync: host↔pod paste via Clipboard API + noVNC RFB
                    r#"sed -i 's|</body>|<script src="clipboard-plugin.js"></script></body>|' /usr/share/novnc/vnc.html"#.to_string(),
                    // Info/about button
                    r#"sed -i 's|</body>|<script src="info-plugin.js"></script></body>|' /usr/share/novnc/vnc.html"#.to_string(),
                ];
                if config.web_display.file_upload {
                    patches.push(
                        r#"sed -i 's|</head>|<script src="upload-plugin.js"></script></head>|' /usr/share/novnc/vnc.html"#.to_string(),
                    );
                }
                if config.web_display.audio {
                    patches.push(
                        r#"sed -i 's|</head>|<script src="audio-plugin.js"></script></head>|' /usr/share/novnc/vnc.html"#.to_string(),
                    );
                }
                let patch_cmd = patches.join(" && ") + " 2>/dev/null; true";
                config.setup.push(patch_cmd);
            }

            // Prepend web display setup commands to config.setup
            let display_cmds = envpod_core::web_display::generate_setup_commands(&config.web_display);
            let mut new_setup = display_cmds;
            new_setup.extend(config.setup.clone());
            config.setup = new_setup;
        }
    }

    // Prepend desktop environment setup commands (xfce, openbox, sway)
    if config.devices.desktop_env != envpod_core::config::DesktopEnv::None {
        let desktop_cmds = envpod_core::desktop::generate_setup_commands(config.devices.desktop_env);
        let mut new_setup = desktop_cmds;
        new_setup.extend(config.setup.clone());
        config.setup = new_setup;
        eprintln!("  Desktop environment: {:?}", config.devices.desktop_env);
    }

    // Re-persist pod.yaml if setup commands were modified
    if config.web_display.display_type != envpod_core::config::WebDisplayType::None
        || config.devices.desktop_env != envpod_core::config::DesktopEnv::None
    {
        if let Ok(state) = NativeState::from_handle(&handle) {
            let yaml = serde_yaml::to_string(&config).context("serialize updated pod config")?;
            std::fs::write(state.config_path(), yaml).context("persist updated pod.yaml")?;
        }
    }

    let state_opt = NativeState::from_handle(&handle).ok();
    eprintln!("{divider}");
    eprintln!(
        "  {} {} {}  {}",
        color::bold("Pod Created"),
        color::dim("·"),
        color::cyan(name),
        color::dim(&fmt_duration(create_elapsed)),
    );
    eprintln!("{divider}");
    eprintln!();
    print_pod_info(name, &config, state_opt.as_ref());
    eprintln!();

    // ── Stage 2: Setup ──
    let setup_result = if !config.setup.is_empty() || config.setup_script.is_some() {
        eprintln!("{divider}");
        match run_setup_commands(store, base_dir, name, verbose).await {
            Ok((completed, total, success, log_path)) => {
                if !success {
                    eprintln!();
                    eprintln!(
                        "  Pod created but setup incomplete. Fix and re-run:"
                    );
                    eprintln!("  sudo envpod setup {name}");
                } else if let Some(base_arg) = create_base {
                    let base_name = if base_arg.is_empty() { name } else { base_arg };
                    snapshot_base_quiet(&handle, base_dir, base_name);
                }
                Some((completed, total, success, log_path))
            }
            Err(e) => {
                eprintln!();
                eprintln!(
                    "  {} setup failed: {e:#}",
                    color::red("warning:"),
                );
                eprintln!("  Pod created but setup incomplete. Fix and re-run:");
                eprintln!("  sudo envpod setup {name}");
                None
            }
        }
    } else if let Some(base_arg) = create_base {
        // No setup commands — snapshot immediately after create
        let base_name = if base_arg.is_empty() { name } else { base_arg };
        snapshot_base_quiet(&handle, base_dir, base_name);
        None
    } else {
        None
    };

    // ── Stage 3: Summary ──
    let total_elapsed = init_start.elapsed();
    eprintln!();
    eprintln!("{divider}");
    let success = setup_result.as_ref().map_or(true, |(_, _, s, _)| *s);
    if success {
        eprintln!("  {}  {}", color::green("Ready"), color::dim(&fmt_duration(total_elapsed)));
    } else {
        eprintln!("  {}  {}", color::red("Setup Incomplete"), color::dim(&fmt_duration(total_elapsed)));
    }
    eprintln!("{divider}");
    eprintln!();
    eprintln!("  {}  sudo envpod run {name} -- bash", color::dim("Run"));
    let sec_findings = security_findings(&config);
    if !sec_findings.is_empty() {
        let note_word = if sec_findings.len() == 1 { "note" } else { "notes" };
        eprintln!(
            "  {} {} {} — run: sudo envpod audit {name} --security",
            color::dim("Security"),
            sec_findings.len(),
            note_word,
        );
    }
    if let Some((_, _, _, ref log_path)) = setup_result {
        eprintln!("  {}  {}", color::dim("Log"), log_path.display());
    }
    eprintln!();

    Ok(())
}

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

async fn cmd_setup(store: &PodStore, base_dir: &std::path::Path, name: &str, create_base: Option<&str>, verbose: bool) -> Result<()> {
    let divider = color::dim("  ────────────────────────────────────────");
    let setup_start = std::time::Instant::now();

    eprintln!();
    eprintln!("{divider}");

    let (completed, total, success, log_path) =
        run_setup_commands(store, base_dir, name, verbose).await?;

    let elapsed = color::dim(&fmt_duration(setup_start.elapsed()));
    eprintln!();
    eprintln!("{divider}");
    if success {
        eprintln!(
            "  {} ({completed}/{total} steps {})  {elapsed}",
            color::bold("Setup complete"),
            color::green("✓"),
        );
        if let Some(base_arg) = create_base {
            let base_name = if base_arg.is_empty() { name } else { base_arg };
            let handle = store.load(name)?;
            snapshot_base_quiet(&handle, base_dir, base_name);
        }
    } else {
        eprintln!(
            "  {} (failed at step {completed}/{total} {})  {elapsed}",
            color::bold("Setup failed"),
            color::red("✗"),
        );
    }
    eprintln!("{divider}");
    eprintln!();
    eprintln!("  {}  {}", color::dim("Log"), log_path.display());

    // Show web display URL hint after setup if configured
    if success {
        let handle = store.load(name)?;
        if let Ok(state) = NativeState::from_handle(&handle) {
            if let Ok(yaml) = std::fs::read_to_string(state.config_path()) {
                if let Ok(cfg) = serde_yaml::from_str::<envpod_core::config::PodConfig>(&yaml) {
                    if cfg.web_display.display_type != envpod_core::config::WebDisplayType::None {
                        let label = match cfg.web_display.display_type {
                            envpod_core::config::WebDisplayType::Novnc => "noVNC",
                            envpod_core::config::WebDisplayType::Webrtc => "WebRTC",
                            _ => "Display",
                        };
                        let pod_ip = state.network.as_ref().map(|n| n.pod_ip.as_str()).unwrap_or("localhost");
                        eprintln!("  {}  {} → http://{}:{}/vnc.html", color::dim("Display"), label, pod_ip, cfg.web_display.port);
                    }
                }
            }
        }
    }

    eprintln!();
    Ok(())
}

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

/// Format a duration as a human-friendly string: "120ms", "3.2s", "1m 12s".
fn fmt_duration(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        let secs = d.as_secs();
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 { format!("{m}m") } else { format!("{m}m {s}s") }
    }
}

/// Run setup commands with clean one-liner-per-step output.
///
/// Returns (completed_steps, total_steps, success, log_path).
async fn run_setup_commands(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    verbose: bool,
) -> Result<(usize, usize, bool, PathBuf)> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let mut config = state
        .load_config()?
        .context("no pod.yaml found — cannot run setup")?;

    let setup_log = state.pod_dir.join("setup.log");

    if config.setup.is_empty() && config.setup_script.is_none() {
        eprintln!("  No setup commands defined for pod '{name}'.");
        return Ok((0, 0, true, setup_log));
    }

    let backend = NativeBackend::new(base_dir)?;

    // Restore kernel state if stale (e.g. after host reboot)
    backend.restore(&handle).ok();

    // Start temporary DNS server for setup (so pip/apt/curl can resolve)
    let _dns_handle = if let Some(ref net) = state.network {
        let upstream = server::parse_host_resolv_conf();
        let bind_ip: std::net::Ipv4Addr = net.host_ip.parse()
            .with_context(|| format!("parse DNS bind IP: {}", net.host_ip))?;
        let policy = build_dns_policy(net);
        let dns_server = DnsServer::new(bind_ip, policy, upstream, name.to_string())
            .with_filter_aaaa(true);
        let h = dns_server.spawn().await
            .context("DNS server failed to start (setup phase)")?;
        Some(h)
    } else {
        None
    };

    // Truncate previous setup log
    std::fs::write(&setup_log, "").ok();

    // Pre-inject setup_script into overlay so inline setup commands can reference it
    if let Some(ref script_path) = config.setup_script {
        inject_setup_script(&state, script_path, &config.filesystem.system_access)?;
    }

    // Silent pre-setup: fix common issues before user's setup commands.
    // These are idempotent, fast, and run without output (unless --verbose).
    let mut pre_setup_cmds: Vec<&str> = vec![
        // PEP 668: Ubuntu 24.04 blocks system pip installs
        "rm -f /usr/lib/python*/EXTERNALLY-MANAGED 2>/dev/null; true",
        // Stale apt lists in overlay cause "Unable to locate package"
        "rm -rf /var/lib/apt/lists/* 2>/dev/null; true",
        // Disable 3rd-party apt sources that may not resolve through DNS whitelist
        "cd /etc/apt/sources.list.d 2>/dev/null && for f in *.list *.sources; do [ -f \"$f\" ] && sed -i 's/^deb /# deb /' \"$f\"; done; true",
        // Remove broken dist-info dirs (no RECORD file) so pip doesn't choke on debian-installed packages
        "find /usr/lib/python3/dist-packages -maxdepth 1 -name '*.dist-info' -exec sh -c 'test ! -f \"$1/RECORD\" && rm -rf \"$1\"' _ {} \\; 2>/dev/null; true",
    ];
    let setup_text = config.setup.join(" ");

    // Auto-prepend apt-get update if setup uses apt-get install (visible step)
    let needs_apt = setup_text.contains("apt-get install") || setup_text.contains("apt install");
    if needs_apt && !setup_text.contains("apt-get update") && !setup_text.contains("apt update") {
        config.setup.insert(0, "DEBIAN_FRONTEND=noninteractive apt-get update -qq".to_string());
    }

    // Auto-install nvm + Node.js if setup commands use npm/npx/node
    let setup_text = config.setup.join(" ");
    let needs_node = setup_text.contains("npm ") || setup_text.contains("npx ")
        || setup_text.contains("npm\"") || setup_text.contains("npx\"")
        || setup_text.contains("node ") || setup_text.contains("nvm ");
    if needs_node {
        pre_setup_cmds.push(concat!(
            "command -v node >/dev/null 2>&1 && exit 0; ",
            "export NVM_DIR=/opt/nvm; ",
            "mkdir -p $NVM_DIR; ",
            "curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash; ",
            ". $NVM_DIR/nvm.sh; ",
            "nvm install 22; ",
            "ln -sf $(which node) /usr/local/bin/node; ",
            "ln -sf $(which npm) /usr/local/bin/npm; ",
            "ln -sf $(which npx) /usr/local/bin/npx; ",
            "true"
        ));
    }

    for pre_cmd in &pre_setup_cmds {
        let args = vec!["sh".to_string(), "-c".to_string(), pre_cmd.to_string()];
        let quiet = Some(setup_log.as_path());
        if let Ok(proc_handle) = backend.start_setup(&handle, &args, quiet) {
            nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(proc_handle.pid as i32),
                None,
            ).ok();
        }
    }

    let has_script = config.setup_script.is_some();
    let total = config.setup.len() + if has_script { 1 } else { 0 };

    eprintln!("  {} ({total} steps)", color::bold("Setup"));
    eprintln!("{}", color::dim("  ────────────────────────────────────────"));
    eprintln!();

    for (i, cmd) in config.setup.iter().enumerate() {
        let step = i + 1;
        let display_cmd = truncate_setup_cmd(cmd, 50);

        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            cmd.clone(),
        ];

        // verbose: show output on terminal; default: pipe to log file
        let quiet = if verbose { None } else { Some(setup_log.as_path()) };
        let step_start = std::time::Instant::now();
        let proc_handle = backend.start_setup(&handle, &args, quiet)?;

        let status = nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(proc_handle.pid as i32),
            None,
        );
        let step_time = color::dim(&fmt_duration(step_start.elapsed()));
        match status {
            Ok(nix::sys::wait::WaitStatus::Exited(_, 0)) => {
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::green("✓"));
            }
            Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::red(&format!("✗ (exit {code})")));
                eprintln!(
                    "  Setup failed at step {step}/{total} (log: {})",
                    setup_log.display()
                );
                return Ok((step, total, false, setup_log));
            }
            Ok(other) => {
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::red("✗ (signal)"));
                eprintln!(
                    "  Setup failed at step {step}/{total}: {other:?} (log: {})",
                    setup_log.display()
                );
                return Ok((step, total, false, setup_log));
            }
            Err(e) => {
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::red("✗ (error)"));
                eprintln!(
                    "  Setup failed at step {step}/{total}: {e} (log: {})",
                    setup_log.display()
                );
                return Ok((step, total, false, setup_log));
            }
        }
    }

    // Run setup_script if specified
    if let Some(ref script_path) = config.setup_script {
        let step = total; // last step
        let display_cmd = truncate_setup_cmd(script_path, 50);
        let quiet = if verbose { None } else { Some(setup_log.as_path()) };
        let step_start = std::time::Instant::now();
        match run_setup_script(&handle, &state, &backend, script_path, quiet, &config.filesystem.system_access) {
            Ok(()) => {
                let step_time = color::dim(&fmt_duration(step_start.elapsed()));
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::green("✓"));
            }
            Err(e) => {
                let step_time = color::dim(&fmt_duration(step_start.elapsed()));
                eprintln!("  [{step}/{total}] {display_cmd}  {}  {step_time}", color::red("✗"));
                eprintln!(
                    "  Setup failed at step {step}/{total}: {e:#} (log: {})",
                    setup_log.display()
                );
                return Ok((step, total, false, setup_log));
            }
        }
    }

    // Fix ownership of agent home directory in the upper layer.
    // Setup runs as root inside the pod, so files created under /home/agent/
    // are owned by root in the overlay upper. The agent user (uid 60000) needs
    // write access for config files, OAuth tokens, etc.
    let agent_home = state.upper_dir().join("home/agent");
    if agent_home.exists() {
        fix_upper_ownership(&agent_home, 60000, 60000);
    }

    Ok((total, total, true, setup_log))
}

/// Recursively chown files in the overlay upper layer to the given uid:gid.
/// This fixes ownership for files created by root during setup that the
/// non-root agent user needs to write to.
fn fix_upper_ownership(path: &std::path::Path, uid: u32, gid: u32) {
    use std::os::unix::fs::MetadataExt;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.uid() == 0 {
            nix::unistd::chown(path, Some(nix::unistd::Uid::from_raw(uid)), Some(nix::unistd::Gid::from_raw(gid))).ok();
        }
    }
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                fix_upper_ownership(&entry.path(), uid, gid);
            }
        }
    }
}

/// Truncate a setup command for display. Takes the first line only
/// (for multi-line `|` blocks) and truncates to `max` chars with `…`.
fn truncate_setup_cmd(cmd: &str, max: usize) -> String {
    let first_line = cmd.lines().next().unwrap_or(cmd).trim();
    if first_line.chars().count() <= max {
        first_line.to_string()
    } else {
        format!("{}…", first_line.chars().take(max).collect::<String>())
    }
}

/// Pre-inject setup_script into the pod overlay so inline setup commands can call it.
/// Copies to both /opt/.envpod-setup.sh and /usr/local/bin/<basename> for convenience.
fn inject_setup_script(state: &NativeState, script_path: &str, system_access: &config::SystemAccess) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use envpod_core::backend::native::expand_tilde;

    let host_path = expand_tilde(std::path::Path::new(script_path));
    if !host_path.exists() {
        anyhow::bail!(
            "setup_script not found on host: {} (resolved: {})",
            script_path,
            host_path.display()
        );
    }

    let script_content = std::fs::read_to_string(&host_path)
        .with_context(|| format!("read setup script: {}", host_path.display()))?;

    let upper_dir = state.upper_dir();

    // Inject at /opt/.envpod-setup.sh (canonical path — /opt is always in the main upper)
    let opt_dir = upper_dir.join("opt");
    std::fs::create_dir_all(&opt_dir).context("create opt dir in upper layer")?;
    let canonical = opt_dir.join(".envpod-setup.sh");
    std::fs::write(&canonical, &script_content).context("write setup script to upper layer")?;
    std::fs::set_permissions(&canonical, std::fs::Permissions::from_mode(0o755))?;

    // Also inject at /usr/local/bin/<basename> so setup commands can call it by name.
    // In advanced/dangerous mode, /usr has its own overlay backed by sys_upper.
    if let Some(basename) = std::path::Path::new(script_path).file_name() {
        let bin_dir = if matches!(system_access, config::SystemAccess::Advanced | config::SystemAccess::Dangerous) {
            state.sys_upper_dir().join("usr/local/bin")
        } else {
            upper_dir.join("usr/local/bin")
        };
        std::fs::create_dir_all(&bin_dir).context("create usr/local/bin in upper layer")?;
        let named_path = bin_dir.join(basename);
        std::fs::write(&named_path, &script_content).context("write named setup script")?;
        std::fs::set_permissions(&named_path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

fn run_setup_script(
    handle: &envpod_core::types::PodHandle,
    state: &NativeState,
    backend: &NativeBackend,
    script_path: &str,
    quiet_log: Option<&Path>,
    system_access: &config::SystemAccess,
) -> Result<()> {
    // Script is already injected by inject_setup_script() — just verify it exists
    let upper_dir = state.upper_dir();
    let injected_path = upper_dir.join("opt/.envpod-setup.sh");
    if !injected_path.exists() {
        // Fallback: inject now (in case called without prior injection)
        inject_setup_script(state, script_path, system_access)?;
    }

    // Execute the script inside the pod
    let args = vec![
        "bash".to_string(),
        "/opt/.envpod-setup.sh".to_string(),
    ];
    let proc_handle = backend.start_setup(handle, &args, quiet_log)?;

    let status = nix::sys::wait::waitpid(
        nix::unistd::Pid::from_raw(proc_handle.pid as i32),
        None,
    );
    match status {
        Ok(nix::sys::wait::WaitStatus::Exited(_, 0)) => {}
        Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
            anyhow::bail!("setup_script failed (exit code {code}): {script_path}");
        }
        Ok(other) => {
            anyhow::bail!("setup_script terminated unexpectedly ({other:?}): {script_path}");
        }
        Err(e) => {
            anyhow::bail!("setup_script wait error: {e}: {script_path}");
        }
    }

    // Clean up: remove script from upper layer (EXCLUDED_PATHS is the safety net)
    if let Err(e) = std::fs::remove_file(&injected_path) {
        eprintln!("warning: could not clean up setup script from overlay: {e}");
    }

    Ok(())
}

/// Save a base pod (rootfs + upper snapshot) for fast cloning.
/// If a base with the same name exists, auto-increments (e.g. my-agent-2, my-agent-3).
/// Best-effort — logs warnings on failure but never propagates errors.
fn snapshot_base_quiet(handle: &envpod_core::types::PodHandle, base_dir: &std::path::Path, requested_name: &str) {
    if let Ok(state) = NativeState::from_handle(handle) {
        let bases_dir = base_dir.join("bases");
        let base_name = if bases_dir.join(requested_name).exists() {
            // Auto-increment: name-2, name-3, ...
            let mut n = 2u32;
            loop {
                let candidate = format!("{requested_name}-{n}");
                if !bases_dir.join(&candidate).exists() {
                    break candidate;
                }
                n += 1;
            }
        } else {
            requested_name.to_string()
        };
        match snapshot_base(&state.pod_dir, &bases_dir, &base_name) {
            Ok(()) => {
                eprintln!(
                    "  {} Base '{}' created  (clone: envpod clone {} <name>)",
                    color::green("✓"),
                    color::bold(&base_name),
                    color::dim(&base_name),
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "base pod save failed");
                eprintln!("  {} Base creation failed: {e}", color::red("✗"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// clone
// ---------------------------------------------------------------------------

fn cmd_clone(
    store: &PodStore,
    base_dir: &std::path::Path,
    source: &str,
    new_name: &str,
    use_current: bool,
) -> Result<()> {
    if store.exists(new_name) {
        anyhow::bail!("pod '{new_name}' already exists");
    }

    let backend = NativeBackend::new(base_dir)?;
    let clone_start = std::time::Instant::now();

    // Try loading as a pod first, fall back to standalone base pod
    let handle = if let Ok(source_handle) = store.load(source) {
        let mode_str = if use_current { "current state" } else { "base snapshot" };
        eprintln!();
        eprintln!(
            "  {} '{}' → '{}' (from {})",
            color::bold("Cloning"),
            source,
            new_name,
            mode_str,
        );

        backend
            .clone_pod(&source_handle, new_name, use_current)
            .with_context(|| format!("clone pod '{source}' → '{new_name}'"))?
    } else {
        // Check if it's a standalone base pod
        let bases_dir = base_dir.join("bases");
        if !has_base(&bases_dir, source) {
            anyhow::bail!("no pod or base pod named '{source}'");
        }
        if use_current {
            anyhow::bail!("--current cannot be used with a base pod (no live state)");
        }

        eprintln!();
        eprintln!(
            "  {} base '{}' → '{}' (from base pod)",
            color::bold("Cloning"),
            source,
            new_name,
        );

        backend
            .clone_from_base(&bases_dir, source, new_name)
            .with_context(|| format!("clone base '{source}' → '{new_name}'"))?
    };

    store.save(&handle)?;

    let elapsed = clone_start.elapsed();
    eprintln!();
    eprintln!(
        "  {} '{}' cloned in {}",
        color::green("✓"),
        new_name,
        fmt_duration(elapsed),
    );
    eprintln!();
    eprintln!("  {}  sudo envpod run {new_name} -- bash", color::dim("Run"));
    eprintln!();

    Ok(())
}

// ---------------------------------------------------------------------------
// base
// ---------------------------------------------------------------------------

async fn cmd_base(store: &PodStore, base_dir: &std::path::Path, action: BaseAction) -> Result<()> {
    let bases_dir = base_dir.join("bases");

    match action {
        BaseAction::Create { name, config, verbose } => {
            if has_base(&bases_dir, &name) {
                anyhow::bail!("base pod '{name}' already exists");
            }

            // Use a temporary pod name that won't collide
            let tmp_pod = format!("__base_tmp_{name}");
            if store.exists(&tmp_pod) {
                // Clean up stale temp pod from a previous failed run
                let h = store.load(&tmp_pod)?;
                let b = create_backend(&h.backend, base_dir)?;
                b.stop(&h).ok();
                b.destroy(&h)?;
                store.remove(&tmp_pod)?;
            }

            // Init a temporary pod
            cmd_init(store, base_dir, &tmp_pod, "native", config.as_deref(), None, None, verbose).await?;

            // Snapshot it as a base pod
            let handle = store.load(&tmp_pod)?;
            let state = NativeState::from_handle(&handle)?;
            snapshot_base(&state.pod_dir, &bases_dir, &name)
                .with_context(|| format!("snapshot base pod '{name}'"))?;

            // Destroy the temporary pod (base rootfs is now independent)
            let backend = create_backend(&handle.backend, base_dir)?;
            backend.stop(&handle).ok();
            backend.destroy(&handle)?;
            store.remove(&tmp_pod)?;

            eprintln!();
            eprintln!(
                "  {} Base pod '{}' created",
                color::green("✓"),
                name,
            );
            eprintln!();
            eprintln!("  {}  sudo envpod clone {name} <pod-name>", color::dim("Use"));
            eprintln!();
        }
        BaseAction::Ls { json } => {
            // List all directories under bases/
            let entries = if bases_dir.exists() {
                let mut entries: Vec<String> = std::fs::read_dir(&bases_dir)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().join("rootfs").exists())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                entries.sort();
                entries
            } else {
                Vec::new()
            };

            if json {
                let json_bases: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|name| {
                        let users = find_base_users(store, &bases_dir, name);
                        serde_json::json!({
                            "name": name,
                            "pods": users,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_bases)?);
            } else if entries.is_empty() {
                println!("No base pods");
            } else {
                println!("{:<20} {}", "BASE", "PODS");
                println!("{}", "-".repeat(40));
                for name in &entries {
                    let users = find_base_users(store, &bases_dir, name);
                    let pods_display = if users.is_empty() {
                        "-".to_string()
                    } else {
                        users.join(", ")
                    };
                    println!("{:<20} {}", name, pods_display);
                }
                println!("\n{} base(s)", entries.len());
            }
        }
        BaseAction::Destroy { names, force } => {
            for name in &names {
                if !has_base(&bases_dir, name) {
                    anyhow::bail!("base pod '{name}' not found");
                }

                let users = find_base_users(store, &bases_dir, name);
                if !users.is_empty() && !force {
                    let list = users.join(", ");
                    anyhow::bail!(
                        "base pod '{name}' still used by: {list}\n  Use --force to remove anyway (pods will break)"
                    );
                }

                destroy_base(&bases_dir, name)?;
                println!("Destroyed base pod '{name}'");
            }
        }
        BaseAction::Resize { name, cpus, memory, tmp_size, disk_size, max_pids, cpu_affinity, gpu, display, audio, desktop, web_display } => {
            if !has_base(&bases_dir, &name) {
                anyhow::bail!("base pod '{name}' not found");
            }
            let config_path = bases_dir.join(&name).join("pod.yaml");
            if !config_path.exists() {
                anyhow::bail!("base pod '{name}' has no pod.yaml");
            }
            apply_config_resize(
                &config_path, cpus, memory.as_deref(), tmp_size.as_deref(),
                disk_size.as_deref(), max_pids, cpu_affinity.as_deref(),
                gpu, display, audio, desktop.as_deref(), web_display.as_deref(),
            )?;
            eprintln!("  {} base pod '{}' config updated", color::green("✓"), name);
            eprintln!("  {} future clones will inherit these settings", color::dim("i"));
        }
        BaseAction::Prune => {
            let entries = if bases_dir.exists() {
                std::fs::read_dir(&bases_dir)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().join("rootfs").exists())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };

            let mut removed = 0;
            for name in &entries {
                let users = find_base_users(store, &bases_dir, name);
                if users.is_empty() {
                    destroy_base(&bases_dir, name)?;
                    println!("Pruned base pod '{name}'");
                    removed += 1;
                }
            }

            if removed == 0 {
                println!("No unused base pods to prune");
            } else {
                println!("\nPruned {} base pod{}", removed, if removed == 1 { "" } else { "s" });
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Scan /tmp/.X11-unix/ for socket files (X0, X1, ...) and return ":N" for the first found.
fn detect_x11_display() -> Result<String> {
    let dir = std::path::Path::new("/tmp/.X11-unix");
    if !dir.exists() {
        anyhow::bail!("no X11 sockets found — /tmp/.X11-unix does not exist");
    }
    let mut entries: Vec<String> = std::fs::read_dir(dir)
        .context("failed to read /tmp/.X11-unix")?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('X') {
                Some(name[1..].to_string())
            } else {
                None
            }
        })
        .collect();
    entries.sort();
    match entries.first() {
        Some(n) => Ok(format!(":{n}")),
        None => anyhow::bail!("no X11 sockets found in /tmp/.X11-unix"),
    }
}

/// Detect the effective display protocol by checking host socket availability.
fn detect_display_protocol() -> config::DisplayProtocol {
    let uid = resolve_real_sudo_uid();
    let runtime_dir = format!("/run/user/{uid}");

    // Check Wayland first
    if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
        let path = if display.starts_with('/') {
            PathBuf::from(&display)
        } else {
            PathBuf::from(format!("{runtime_dir}/{display}"))
        };
        if path.exists() {
            return config::DisplayProtocol::Wayland;
        }
    }
    if PathBuf::from(format!("{runtime_dir}/wayland-0")).exists() {
        return config::DisplayProtocol::Wayland;
    }

    // Fall back to X11
    if Path::new("/tmp/.X11-unix").exists() {
        return config::DisplayProtocol::X11;
    }

    config::DisplayProtocol::X11
}

/// Detect the effective audio protocol by checking host socket availability.
fn detect_audio_protocol() -> config::AudioProtocol {
    let uid = resolve_real_sudo_uid();
    let runtime_dir = format!("/run/user/{uid}");

    // Check PipeWire first
    if PathBuf::from(format!("{runtime_dir}/pipewire-0")).exists() {
        return config::AudioProtocol::Pipewire;
    }

    // Fall back to PulseAudio
    if PathBuf::from(format!("{runtime_dir}/pulse/native")).exists() {
        return config::AudioProtocol::Pulseaudio;
    }

    config::AudioProtocol::Pulseaudio
}

/// Resolve the real (non-root) UID, preferring SUDO_UID when running under sudo.
fn resolve_real_sudo_uid() -> u32 {
    if let Ok(uid) = std::env::var("SUDO_UID") {
        if let Ok(n) = uid.parse::<u32>() {
            return n;
        }
    }
    nix::unistd::getuid().as_raw()
}

/// Run `xhost +local:` as the original (non-root) user to allow pod X11 connections.
/// Best-effort — failures are logged but not fatal.
fn run_xhost_allow() {
    let sudo_user = std::env::var("SUDO_USER").unwrap_or_default();
    if sudo_user.is_empty() {
        // Not running under sudo — try xhost directly
        match std::process::Command::new("xhost")
            .arg("+local:")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("warning: xhost +local: exited with {s}"),
            Err(e) => eprintln!("warning: failed to run xhost: {e}"),
        }
        return;
    }
    // Run as the original user so it targets their X session
    match std::process::Command::new("su")
        .args([&sudo_user, "-c", "xhost +local:"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("warning: xhost +local: exited with {s}"),
        Err(e) => eprintln!("warning: failed to run xhost: {e}"),
    }
}

// ---------------------------------------------------------------------------
// PulseAudio helpers
// ---------------------------------------------------------------------------

/// Detect the PulseAudio auth cookie for the original (non-root) user.
fn detect_pulse_cookie() -> Option<String> {
    let home = if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        // Look up home dir from /etc/passwd
        get_home_for_user(&sudo_user).unwrap_or_else(|| format!("/home/{sudo_user}"))
    } else {
        std::env::var("HOME").unwrap_or_default()
    };
    if home.is_empty() {
        return None;
    }

    // Standard location
    let cookie = format!("{home}/.config/pulse/cookie");
    if std::path::Path::new(&cookie).exists() {
        return Some(cookie);
    }

    // Legacy location
    let legacy = format!("{home}/.pulse-cookie");
    if std::path::Path::new(&legacy).exists() {
        return Some(legacy);
    }

    None
}

/// Look up a user's home directory from /etc/passwd.
fn get_home_for_user(username: &str) -> Option<String> {
    let contents = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in contents.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 6 && fields[0] == username {
            return Some(fields[5].to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// run (background wrapper)
// ---------------------------------------------------------------------------

fn cmd_run_background(base_dir: &std::path::Path, name: &str, command: &[String], root: bool, user: Option<&str>, env_vars: &[String], enable_display: bool, enable_audio: bool, cli_ports: &[String], cli_public_ports: &[String], cli_internal_ports: &[String]) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("no command specified — usage: envpod run {name} -b -- <command>");
    }

    // Resolve pod dir for the log file
    let store = PodStore::new(base_dir.join("state"))?;
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let run_log_path = state.pod_dir.join("run.log");

    // Truncate old log
    std::fs::write(&run_log_path, "")?;

    // Build the equivalent `envpod run` command WITHOUT -b
    let exe = std::env::current_exe().context("cannot find envpod binary")?;
    let mut args: Vec<String> = vec![
        "--dir".into(), base_dir.to_string_lossy().into_owned(),
        "run".into(),
    ];
    if root { args.push("--root".into()); }
    if let Some(u) = user { args.push("--user".into()); args.push(u.into()); }
    for e in env_vars { args.push("--env".into()); args.push(e.clone()); }
    if enable_display { args.push("--enable-display".into()); }
    if enable_audio { args.push("--enable-audio".into()); }
    for p in cli_ports { args.push("-p".into()); args.push(p.clone()); }
    for p in cli_public_ports { args.push("-P".into()); args.push(p.clone()); }
    for p in cli_internal_ports { args.push("-i".into()); args.push(p.clone()); }
    args.push(name.into());
    args.push("--".into());
    args.extend(command.iter().cloned());

    // Fork a daemon process
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Parent { child }) => {
            // Write supervisor PID
            let supervisor_pid_path = state.pod_dir.join("supervisor.pid");
            std::fs::write(&supervisor_pid_path, child.as_raw().to_string())?;

            eprintln!("  {}  supervisor PID {}", color::dim("Background"), child);
            eprintln!("  {}  {}", color::dim("Log     "), run_log_path.display());
            eprintln!();
            eprintln!("  Use {} to attach, {} to view logs",
                color::green(&format!("envpod fg {name}")),
                color::dim(&format!("tail -f {}", run_log_path.display())));
            Ok(())
        }
        Ok(nix::unistd::ForkResult::Child) => {
            // Detach from terminal
            nix::unistd::setsid().ok();

            // Redirect stdout/stderr to /dev/null (daemon banner text not needed).
            // The actual child process output goes to run.log via ENVPOD_QUIET_LOG.
            let devnull = std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .expect("failed to open /dev/null");
            let null_fd = std::os::unix::io::AsRawFd::as_raw_fd(&devnull);
            nix::unistd::dup2(null_fd, 1).ok();
            nix::unistd::dup2(null_fd, 2).ok();
            // Redirect stdin from /dev/null (don't just close it)
            let devnull_r = std::fs::File::open("/dev/null")
                .expect("failed to open /dev/null for read");
            let null_r_fd = std::os::unix::io::AsRawFd::as_raw_fd(&devnull_r);
            nix::unistd::dup2(null_r_fd, 0).ok();

            // Tell the re-exec'd envpod run to write child output to run.log
            std::env::set_var("ENVPOD_QUIET_LOG", &run_log_path);

            // Exec envpod run (fresh process, fresh tokio runtime)
            let c_exe = std::ffi::CString::new(exe.to_string_lossy().as_bytes()).unwrap();
            let c_args: Vec<std::ffi::CString> = std::iter::once(c_exe.clone())
                .chain(args.iter().map(|a| std::ffi::CString::new(a.as_bytes()).unwrap()))
                .collect();
            nix::unistd::execv(&c_exe, &c_args).expect("execv failed");
            #[allow(unreachable_code)]
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("failed to fork background daemon: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// stop / restart helpers
// ---------------------------------------------------------------------------

/// Return names of all currently running pods.
fn running_pod_names(store: &PodStore) -> Result<Vec<String>> {
    use envpod_core::backend::native::state::{NativeState, NativeStatus};
    let pods = store.list()?;
    let mut names = Vec::new();
    for handle in &pods {
        if let Ok(state) = NativeState::from_handle(handle) {
            if state.status == NativeStatus::Running {
                names.push(handle.name.clone());
            }
        }
    }
    Ok(names)
}

/// Resolve the start command for a pod: config start_command > sleep infinity.
fn resolve_start_command(store: &PodStore, name: &str) -> Vec<String> {
    use envpod_core::backend::native::state::NativeState;
    if let Ok(handle) = store.load(name) {
        if let Ok(state) = NativeState::from_handle(&handle) {
            if let Ok(Some(config)) = state.load_config() {
                if !config.start_command.is_empty() {
                    return config.start_command;
                }
            }
        }
    }
    vec!["sleep".into(), "infinity".into()]
}

/// Return names of all pods that are not currently running (Created or Stopped).
fn stopped_pod_names(store: &PodStore) -> Result<Vec<String>> {
    use envpod_core::backend::native::state::{NativeState, NativeStatus};
    let pods = store.list()?;
    let mut names = Vec::new();
    for handle in &pods {
        if let Ok(state) = NativeState::from_handle(handle) {
            if matches!(state.status, NativeStatus::Created | NativeStatus::Stopped) {
                names.push(handle.name.clone());
            }
        }
    }
    Ok(names)
}

async fn cmd_stop(store: &PodStore, base_dir: &std::path::Path, name: &str) -> Result<()> {
    let handle = store.load(name)?;
    let native_state = NativeState::from_handle(&handle).ok();

    // Clean up port forwards
    if let Some(ref s) = native_state {
        envpod_core::backend::native::cleanup_port_forwards(&s.pod_dir);
        envpod_core::backend::native::cleanup_internal_ports(&s.pod_dir);
    }

    // Unregister from DNS daemon
    {
        let daemon = envpod_core::dns_daemon::DaemonClient::new(
            envpod_core::dns_daemon::DaemonClient::default_path()
        );
        daemon.unregister(name).await.ok();
    }

    // Kill the supervisor process (background `envpod run -b` / `envpod start`)
    if let Some(ref s) = native_state {
        let supervisor_pid_path = s.pod_dir.join("supervisor.pid");
        if let Ok(pid_str) = std::fs::read_to_string(&supervisor_pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                let nix_pid = nix::unistd::Pid::from_raw(pid);
                nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM).ok();
                for _ in 0..20 {
                    if nix::sys::signal::kill(nix_pid, None).is_err() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                // Force kill if still alive
                nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGKILL).ok();
            }
        }
        std::fs::remove_file(s.pod_dir.join("supervisor.pid")).ok();
    }

    // Stop processes inside the cgroup
    let backend = create_backend(&handle.backend, base_dir)?;
    backend.stop(&handle).ok();

    // Update status in store
    if let Ok(mut updated_state) = NativeState::from_handle(&handle) {
        updated_state.status = NativeStatus::Stopped;
        updated_state.init_pid = None;
        let mut updated_handle = handle.clone();
        updated_handle.backend_state = updated_state.to_json();
        store.save(&updated_handle).ok();
    }

    println!("Stopped pod '{name}'");
    Ok(())
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

async fn cmd_run(store: &PodStore, base_dir: &std::path::Path, name: &str, command: &[String], root: bool, user: Option<&str>, env_vars: &[String], enable_display: bool, enable_audio: bool, cli_ports: &[String], cli_public_ports: &[String], cli_internal_ports: &[String], cli_mount_cwd: bool, cli_no_mount_cwd: bool) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("no command specified — usage: envpod run {name} -- <command>");
    }

    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;

    // Restore kernel state if stale (e.g. after host reboot)
    match backend.restore(&handle) {
        Ok(true) => eprintln!("Restored pod kernel state (post-reboot recovery)"),
        Ok(false) => {}
        Err(e) => eprintln!("warning: kernel state restoration failed: {e:#}"),
    }

    let state = NativeState::from_handle(&handle)?;
    let mut pod_config = state.load_config()?;

    // Resolve effective display/audio protocols from pod config
    let devices = pod_config.as_ref()
        .map(|c| &c.devices)
        .cloned()
        .unwrap_or_default();

    let mut extra_env: Vec<String> = env_vars.to_vec();
    extra_env.push(format!("ENVPOD_POD_NAME={name}"));

    // Ensure TERM is set inside the pod (needed for TUI apps like vim, claude).
    // Inherit from host if available, otherwise default to xterm-256color.
    if !extra_env.iter().any(|e| e.starts_with("TERM=")) {
        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());
        extra_env.push(format!("TERM={term}"));
    }

    // Handle --enable-display: protocol-aware env vars
    if enable_display {
        let effective = match devices.display_protocol {
            config::DisplayProtocol::Auto => detect_display_protocol(),
            other => other,
        };
        match effective {
            config::DisplayProtocol::Wayland | config::DisplayProtocol::Auto => {
                eprintln!("Display forwarding: WAYLAND_DISPLAY=/tmp/wayland-0 (Wayland)");
                extra_env.push("WAYLAND_DISPLAY=/tmp/wayland-0".to_string());
                // No xhost needed for Wayland
            }
            config::DisplayProtocol::X11 => {
                let display = detect_x11_display()?;
                eprintln!("Display forwarding: DISPLAY={display} (X11)");
                extra_env.push(format!("DISPLAY={display}"));
                run_xhost_allow();
            }
        }
        // Cursor theme fallback for X11 cursor library (used by some toolkits).
        // Don't set GSETTINGS_BACKEND=memory — it makes GSettings return values
        // instantly, which triggers GDK cursor loading BEFORE the Wayland roundtrip
        // populates wl_shm, causing a NULL shm crash.
        extra_env.push("XCURSOR_THEME=Adwaita".to_string());
        extra_env.push("XCURSOR_SIZE=24".to_string());
        // XDG_RUNTIME_DIR — GDK, PipeWire, and other libraries expect this to
        // exist and be writable. The host's /run/user/{uid} isn't available in
        // the pod, so point to /tmp which is a fresh writable tmpfs.
        extra_env.push("XDG_RUNTIME_DIR=/tmp".to_string());
    }

    // Handle --enable-audio: protocol-aware env vars
    if enable_audio {
        let effective = match devices.audio_protocol {
            config::AudioProtocol::Auto => detect_audio_protocol(),
            other => other,
        };
        match effective {
            config::AudioProtocol::Pipewire | config::AudioProtocol::Auto => {
                extra_env.push("PIPEWIRE_RUNTIME_DIR=/tmp".to_string());
                eprintln!("Audio forwarding: PIPEWIRE_RUNTIME_DIR=/tmp (PipeWire)");
                // Suppress D-Bus errors — doesn't work across namespaces
                extra_env.push("DBUS_SESSION_BUS_ADDRESS=disabled:".to_string());
            }
            config::AudioProtocol::Pulseaudio => {
                extra_env.push("PULSE_SERVER=unix:/tmp/pulse-native".to_string());
                eprintln!("Audio forwarding: PULSE_SERVER=unix:/tmp/pulse-native (PulseAudio)");
                extra_env.push("DBUS_SESSION_BUS_ADDRESS=disabled:".to_string());
            }
        }
    }

    // Copy PulseAudio cookie to pod overlay with world-readable permissions.
    // Only needed for PulseAudio protocol — PipeWire doesn't use cookies.
    if enable_audio {
        let effective = match devices.audio_protocol {
            config::AudioProtocol::Auto => detect_audio_protocol(),
            other => other,
        };
        if matches!(effective, config::AudioProtocol::Pulseaudio) {
            if let Some(cookie_host_path) = detect_pulse_cookie() {
                let cookie_dest = state.upper_dir().join("tmp/pulse-cookie");
                if let Some(parent) = cookie_dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&cookie_host_path, &cookie_dest)?;
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&cookie_dest, std::fs::Permissions::from_mode(0o644))?;
                }
                extra_env.push("PULSE_COOKIE=/tmp/pulse-cookie".to_string());
                eprintln!("Audio cookie: copied to pod (world-readable)");
            } else {
                eprintln!("warning: PulseAudio cookie not found — authentication may fail");
            }
        }
    }
    // Web display: set DISPLAY=:99 and wrap command with supervisor
    let web_display_type = pod_config.as_ref()
        .map(|c| c.web_display.display_type)
        .unwrap_or(envpod_core::config::WebDisplayType::None);
    let web_display_port = pod_config.as_ref()
        .map(|c| c.web_display.port)
        .unwrap_or(6080);

    if web_display_type != envpod_core::config::WebDisplayType::None {
        extra_env.push("DISPLAY=:99".to_string());
        extra_env.push("XDG_RUNTIME_DIR=/tmp".to_string());
    }

    // Build effective command: wrap with display wrapper if web_display is enabled.
    // The wrapper script is lightweight — it starts the services daemon in background
    // if not already running, then exec's the user command immediately.
    // Multiple runs get independent shells; services are shared.
    let effective_command: Vec<String> = if web_display_type != envpod_core::config::WebDisplayType::None {
        let mut cmd = vec!["/usr/local/bin/envpod-display-start".to_string()];
        cmd.extend_from_slice(command);
        cmd
    } else {
        command.to_vec()
    };
    let command = &effective_command;

    let shared_dns_policy: Option<std::sync::Arc<std::sync::RwLock<DnsPolicy>>> =
        if let Some(ref net) = state.network {
            // Persist initial network state for dns-reload support
            net.save(&state.pod_dir).ok();
            Some(std::sync::Arc::new(std::sync::RwLock::new(build_dns_policy(net))))
        } else {
            None
        };

    let dns_handle = if let Some(ref net) = state.network {
        let upstream = server::parse_host_resolv_conf();
        let bind_ip: std::net::Ipv4Addr = net.host_ip.parse()
            .with_context(|| format!("parse DNS bind IP: {}", net.host_ip))?;

        let audit_path = state.pod_dir.join("audit.jsonl");
        let daemon_sock = envpod_core::dns_daemon::DaemonClient::default_path();
        let dns_server = DnsServer::new_with_shared_policy(
            bind_ip,
            shared_dns_policy.clone().unwrap(),
            upstream,
            name.to_string(),
        ).with_audit_path(audit_path)
         .with_daemon_sock(daemon_sock)
         .with_filter_aaaa(true);
        let handle = dns_server.spawn().await
            .context("DNS server failed to start — pod cannot resolve hostnames")?;
        Some(handle)
    } else {
        None
    };

    // CLI --mount-cwd / --no-mount-cwd override pod.yaml mount_cwd setting
    if let Some(ref mut cfg) = pod_config {
        if cli_mount_cwd {
            cfg.filesystem.mount_cwd = true;
            // If no cwd_path was captured at init time, use current CWD
            if cfg.filesystem.cwd_path.is_none() {
                cfg.filesystem.cwd_path = std::env::current_dir().ok();
            }
        } else if cli_no_mount_cwd {
            cfg.filesystem.mount_cwd = false;
        }
    }

    // Override user with cloned host user when clone_host is true
    if let Some(ref mut cfg) = pod_config {
        if cfg.host_user.clone_host {
            if let Ok(host_user) = envpod_core::user_clone::get_host_user() {
                cfg.user = host_user.username;
            }
        }
    }

    // Resolve effective user: CLI --user > CLI --root > pod.yaml user > default "agent"
    // --user root is equivalent to --root
    let config_user = pod_config.as_ref().map(|c| c.user.as_str()).unwrap_or("agent");
    let user_is_root = user.map(|u| u == "root").unwrap_or(false);
    let is_root = root || user_is_root || config_user == "root";
    let mut effective_user: Option<&str> = if let Some(u) = user {
        if u == "root" { None } else { Some(u) }
    } else if is_root {
        None // root = no setuid
    } else {
        Some(config_user) // pod.yaml user or default "agent"
    };

    // When web_display is active, display services (Xvfb, x11vnc, websockify,
    // PulseAudio) need root. Pass the requested user via env var so the
    // supervisor script drops privileges only for the final user command.
    if web_display_type != envpod_core::config::WebDisplayType::None {
        if let Some(u) = effective_user {
            extra_env.push(format!("ENVPOD_RUN_USER={u}"));
            effective_user = None; // run supervisor as root
        }
    }
    let env_vars = &extra_env;

    // Auto-snapshot before run if configured (snapshots.auto_on_run: true)
    if let Some(ref cfg) = pod_config {
        if cfg.snapshots.auto_on_run {
            use envpod_core::snapshot::SnapshotStore;
            let snap_store = SnapshotStore::new(&state.pod_dir);
            let upper_dir = state.pod_dir.join("upper");
            match snap_store.create(&upper_dir, None, true) {
                Ok(snap) => eprintln!("  {}  {} ({})", color::dim("Snapshot"), snap.id, format_bytes(snap.size_bytes)),
                Err(e) => eprintln!("  {} auto-snapshot failed: {e:#}", color::yellow("⚠")),
            }
            let _ = snap_store.prune(cfg.snapshots.max_keep);
        }
    }

    // Pre-create queue socket BEFORE backend.start() so the child's mount namespace
    // can bind-mount it at /run/envpod/queue.sock during namespace setup.
    let queue_socket_listener = if pod_config.as_ref().map_or(false, |c| c.queue.socket) {
        match envpod_core::queue_socket::QueueSocketServer::bind(&state.pod_dir) {
            Ok(listener) => {
                Some(listener)
            }
            Err(e) => {
                eprintln!("  {} queue socket failed to bind: {e:#}", color::yellow("⚠"));
                None
            }
        }
    } else {
        None
    };

    // Print banners before starting the process (so they appear before the shell prompt)
    print_welcome_banner(&state.pod_dir);
    print_run_banner(name, &pod_config, &state);
    eprintln!("  {}  {}", color::dim("Command "), command.join(" "));
    // Always show user in banner
    let user_display = if let Some(u) = user {
        u.to_string()
    } else if is_root {
        "root (elevated)".to_string()
    } else {
        config_user.to_string()
    };
    eprintln!("  {}  {}", color::dim("User    "), user_display);
    if let Some(ref net) = state.network {
        eprintln!("  {}  {}", color::dim("Pod IP  "), net.pod_ip);
    }
    if web_display_type != envpod_core::config::WebDisplayType::None {
        let display_label = match web_display_type {
            envpod_core::config::WebDisplayType::Novnc => "noVNC",
            envpod_core::config::WebDisplayType::Webrtc => "WebRTC",
            _ => "none",
        };
        let pod_ip = state.network.as_ref().map(|n| n.pod_ip.as_str()).unwrap_or("localhost");
        eprintln!("  {}  {} → http://{}:{}/vnc.html", color::dim("Display "), display_label, pod_ip, web_display_port);
        if let Some(ref cfg) = pod_config {
            if cfg.web_display.audio {
                eprintln!("  {}  Opus/WebM → port {} (click speaker icon in noVNC) {}", color::dim("Audio   "), cfg.web_display.audio_port, color::dim("[beta]"));
            }
            if cfg.web_display.file_upload {
                eprintln!("  {}  Click upload icon in noVNC → /tmp/uploads/", color::dim("Upload  "));
            }
        }
    }
    if is_root {
        eprintln!();
        eprintln!("  {} Running as root — known gaps: iptables modification, raw sockets.", color::yellow("⚠"));
        eprintln!("    Default non-root provides full pod boundary protection.");
    }
    eprintln!();
    // Flush stderr so banner appears before the child's prompt
    use std::io::Write;
    std::io::stderr().flush().ok();

    // In background mode, the daemon sets ENVPOD_QUIET_LOG so spawn_isolated
    // writes output directly to the log file (no tee, no doubled output).
    let quiet_log_path = std::env::var("ENVPOD_QUIET_LOG").ok().map(std::path::PathBuf::from);
    let proc_handle = backend.start(&handle, command, effective_user, env_vars, quiet_log_path.as_deref())?;

    // Guardian cgroup: migrate display/audio/upload PIDs from app/ to guardian/
    // so they survive freeze/thaw (envpod lock/unlock).
    if web_display_type != envpod_core::config::WebDisplayType::None {
        if let Some(ref cg) = state.cgroup_path {
            let mut min_expected = match web_display_type {
                envpod_core::config::WebDisplayType::Novnc => 3,   // Xvfb + x11vnc + websockify
                envpod_core::config::WebDisplayType::Webrtc => 2,  // Xvfb + gst-launch
                _ => 1,
            };
            if let Some(ref cfg) = pod_config {
                if cfg.web_display.audio {
                    min_expected += 3; // pulseaudio + socat (audio-proxy) + audio websockify
                }
                if cfg.web_display.file_upload {
                    min_expected += 1; // upload server
                }
            }
            let mut total_migrated = 0;
            for _ in 0..30 {
                match envpod_core::backend::native::migrate_display_pids(cg) {
                    Ok(n) => {
                        total_migrated += n;
                        if total_migrated >= min_expected {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "guardian migration attempt failed");
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if total_migrated > 0 {
                eprintln!("  {}  {} display process{} protected (guardian cgroup)",
                    color::dim("Guardian"),
                    total_migrated,
                    if total_migrated == 1 { "" } else { "es" },
                );
            }
        }
    }

    // Port forwarding: merge pod.yaml ports/public_ports with CLI -p/-P flags.
    // `ports` / `-p` → localhost-only (127.0.0.1: prefix applied automatically).
    // `public_ports` / `-P` → all network interfaces (no prefix, PREROUTING added).
    let port_forward_active = if let Some(ref net) = state.network {
        let normalize_local = |p: &String| -> String {
            if p.starts_with("127.0.0.1:") { p.clone() } else { format!("127.0.0.1:{p}") }
        };
        let mut all_ports: Vec<String> = pod_config.as_ref()
            .map(|c| c.network.ports.iter().map(normalize_local).collect::<Vec<_>>())
            .unwrap_or_default();
        let public: Vec<String> = pod_config.as_ref()
            .map(|c| c.network.public_ports.clone())
            .unwrap_or_default();
        all_ports.extend(public);
        all_ports.extend(cli_ports.iter().map(normalize_local));
        all_ports.extend_from_slice(cli_public_ports);

        // Auto-add web display port forward (localhost only)
        if web_display_type != envpod_core::config::WebDisplayType::None {
            all_ports.push(format!("127.0.0.1:{}:6080", web_display_port));
            // Auto-add audio WebSocket port forward if audio enabled
            if let Some(ref cfg) = pod_config {
                if cfg.web_display.audio {
                    all_ports.push(format!("127.0.0.1:{}:{}", cfg.web_display.audio_port, cfg.web_display.audio_port));
                }
                // Auto-add upload server port forward if file upload enabled
                if cfg.web_display.file_upload {
                    all_ports.push(format!("127.0.0.1:{}:{}", cfg.web_display.upload_port, cfg.web_display.upload_port));
                }
            }
        }

        if !all_ports.is_empty() {
            envpod_core::backend::native::cleanup_port_forwards(&state.pod_dir);
            match envpod_core::backend::native::setup_port_forwards(
                &state.pod_dir, &net.host_veth, &net.pod_ip, &all_ports,
            ) {
                Ok(()) => {
                    // Only print user-defined port forwards (not auto-added display/audio/upload)
                    let auto_port_count = if web_display_type != envpod_core::config::WebDisplayType::None {
                        let mut n = 1; // display port
                        if let Some(ref cfg) = pod_config { if cfg.web_display.audio { n += 1; } if cfg.web_display.file_upload { n += 1; } }
                        n
                    } else { 0 };
                    let user_ports = all_ports.len().saturating_sub(auto_port_count);
                    for spec in &all_ports[..user_ports] {
                        let scope = if spec.starts_with("127.0.0.1:") { "local " } else { "public" };
                        eprintln!("  {}  [{}] {} → pod", color::dim("Port   "), scope, spec);
                    }
                    true
                }
                Err(e) => {
                    eprintln!("warning: port forwarding failed: {e:#}");
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // Internal (pod-to-pod) ports: FORWARD rules scoped to pod subnet only
    let internal_active = if let Some(ref net) = state.network {
        let mut all_internal: Vec<String> = pod_config.as_ref()
            .map(|c| c.network.internal_ports.clone())
            .unwrap_or_default();
        all_internal.extend_from_slice(cli_internal_ports);
        if !all_internal.is_empty() {
            envpod_core::backend::native::cleanup_internal_ports(&state.pod_dir);
            let subnet_base = net.subnet_base.as_str();
            match envpod_core::backend::native::setup_internal_ports(
                &state.pod_dir, &net.pod_ip, subnet_base, &all_internal,
            ) {
                Ok(()) => {
                    for spec in &all_internal {
                        eprintln!("  {}  [pods  ] {}:{} → pod", color::dim("Port   "), net.pod_ip, spec);
                    }
                    true
                }
                Err(e) => {
                    eprintln!("warning: internal port setup failed: {e:#}");
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // Pod discovery: register with the central envpod-dns daemon.
    // All pods register (so the daemon knows their allow_pods list for bilateral enforcement).
    // Only allow_discovery: true pods are resolvable by peers.
    let discovery_active = if let Some(ref net) = state.network {
        let allow_discovery = pod_config.as_ref().map(|c| c.network.allow_discovery).unwrap_or(false);
        let allow_pods = pod_config.as_ref().map(|c| c.network.allow_pods.clone()).unwrap_or_default();
        let daemon = envpod_core::dns_daemon::DaemonClient::new(
            envpod_core::dns_daemon::DaemonClient::default_path()
        );
        match daemon.register(name, &net.pod_ip, allow_discovery, &allow_pods).await {
            Ok(()) => {
                if allow_discovery {
                    eprintln!("  {}  [pods  ] {}.pods.local → {}", color::dim("Disc   "), name, net.pod_ip);
                }
                true
            }
            Err(e) => {
                if allow_discovery || !allow_pods.is_empty() {
                    eprintln!("  warning: envpod dns-daemon not running — pod discovery disabled ({e:#})");
                }
                false
            }
        }
    } else {
        false
    };

    // Persist init_pid and Running status so `envpod status` reflects reality
    {
        let mut updated_state = NativeState::from_handle(&handle)?;
        updated_state.init_pid = Some(proc_handle.pid);
        updated_state.status = NativeStatus::Running;
        let mut updated_handle = handle.clone();
        updated_handle.backend_state = updated_state.to_json();
        store.save(&updated_handle)?;
    }

    // Write supervisor PID so `envpod fg` can find us
    let supervisor_pid_path = state.pod_dir.join("supervisor.pid");
    std::fs::write(&supervisor_pid_path, std::process::id().to_string())?;

    // Budget enforcement: if max_duration is set, spawn a timer that kills the process
    let budget_handle = if let Some(ref cfg) = pod_config {
        if let Some(ref dur_str) = cfg.budget.max_duration {
            if let Some(secs) = config::parse_duration_string(dur_str) {
                let pid_for_timer = proc_handle.pid;
                let pod_dir = state.pod_dir.clone();
                let pod_name = name.to_string();
                let dur_str_owned = dur_str.clone();
                let duration = std::time::Duration::from_secs(secs);
                Some(tokio::spawn(async move {
                    tokio::time::sleep(duration).await;
                    // Time's up — kill the process
                    eprintln!("Budget exceeded: max_duration={dur_str_owned} — killing process");
                    let pid = nix::unistd::Pid::from_raw(pid_for_timer as i32);
                    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL).ok();
                    // Audit the budget exceeded event
                    let log = AuditLog::new(&pod_dir);
                    let entry = AuditEntry {
                        timestamp: chrono::Utc::now(),
                        pod_name,
                        action: AuditAction::BudgetExceeded,
                        detail: format!("max_duration={dur_str_owned} ({secs}s)"),
                        success: true,
                    };
                    log.append(&entry).ok();
                }))
            } else {
                eprintln!("warning: could not parse max_duration '{dur_str}'");
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Start remote control server (always — allows external tools to control the pod)
    let control_handle = if let Some(ref cgroup) = state.cgroup_path {
        let server = match shared_dns_policy {
            Some(ref policy) => ControlServer::with_dns_policy(
                state.pod_dir.clone(),
                name.to_string(),
                cgroup.clone(),
                policy.clone(),
            ),
            None => ControlServer::new(
                state.pod_dir.clone(),
                name.to_string(),
                cgroup.clone(),
            ),
        };
        match server.spawn().await {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!(error = %e, "control server failed to start");
                eprintln!("warning: control server failed to start: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Start monitor agent (only if monitoring-policy.yaml exists in pod dir)
    let monitor_handle = {
        let policy_path = state.pod_dir.join("monitoring-policy.yaml");
        if policy_path.exists() {
            if let Some(ref cgroup) = state.cgroup_path {
                match MonitorPolicy::from_file(&policy_path) {
                    Ok(policy) => {
                        let agent = MonitorAgent::new(
                            policy,
                            state.pod_dir.clone(),
                            name.to_string(),
                            cgroup.clone(),
                        );
                        Some(agent.spawn())
                    }
                    Err(e) => {
                        eprintln!("warning: failed to load monitoring policy: {e:#}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    // Spawn queue socket server (agent-side API at /run/envpod/queue.sock)
    let queue_socket_handle = if let Some(listener) = queue_socket_listener {
        let server = envpod_core::queue_socket::QueueSocketServer::new(
            state.pod_dir.clone(),
            name.to_string(),
        );
        Some(server.spawn_with_listener(listener))
    } else {
        None
    };

    // Start queue executor (auto-executes delayed actions past their deadline)
    let queue_executor_handle = {
        let executor = QueueExecutor::new(state.pod_dir.clone(), name.to_string());
        executor.spawn()
    };

    // Wait for the child process to exit, Ctrl+C to kill, or Ctrl+Z to detach.
    // The pod runs in its own session (setsid) so terminal signals don't reach it
    // directly — envpod must forward Ctrl+C (SIGINT) to the child.
    let pid_raw = proc_handle.pid as i32;
    let wait_task = tokio::task::spawn_blocking(move || {
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid_raw), None)
    });

    let mut sigtstp = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(20))
        .context("failed to register SIGTSTP handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("failed to register SIGINT handler")?;

    let detached = tokio::select! {
        result = wait_task => {
            // Process exited normally
            let wait_result = result.context("wait task panicked")?;
            match wait_result {
                Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
                    if code != 0 {
                        println!("Process exited with code {code}");
                    }
                }
                Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
                    println!("Process killed by signal {sig}");
                }
                Ok(status) => {
                    println!("Process exited: {status:?}");
                }
                Err(nix::Error::ECHILD) => {}
                Err(e) => {
                    anyhow::bail!("wait failed: {e}");
                }
            }
            false
        }
        _ = sigint.recv() => {
            // Ctrl+C: forward SIGTERM to the child (pod runs in its own session
            // via setsid, so terminal SIGINT doesn't reach it directly).
            let pid = nix::unistd::Pid::from_raw(proc_handle.pid as i32);
            nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).ok();
            // Brief wait, then SIGKILL if still alive
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL).ok();
            false
        }
        _ = sigtstp.recv() => {
            // Ctrl+Z: detach from terminal. The current process stays alive
            // as a background daemon (DNS server, port forwards, etc. persist).
            eprintln!();
            eprintln!("Detaching from pod '{}' (Ctrl+Z)", name);
            eprintln!("Pod continues running in background. Use {} to reattach.",
                color::green(&format!("envpod fg {name}")));

            // Redirect stdio to run.log / /dev/null and detach from terminal
            let run_log_path = state.pod_dir.join("run.log");
            let log_fd = std::fs::OpenOptions::new()
                .create(true).append(true).open(&run_log_path).ok();
            let null_fd = std::fs::File::open("/dev/null").ok();
            if let Some(f) = null_fd {
                let fd = std::os::unix::io::AsRawFd::as_raw_fd(&f);
                nix::unistd::dup2(fd, 0).ok(); // stdin = /dev/null
            }
            if let Some(f) = log_fd {
                let fd = std::os::unix::io::AsRawFd::as_raw_fd(&f);
                nix::unistd::dup2(fd, 1).ok(); // stdout = run.log
                nix::unistd::dup2(fd, 2).ok(); // stderr = run.log
            }
            nix::unistd::setsid().ok(); // new session, detach from terminal

            true
        }
    };

    // Cancel budget timer
    if let Some(bh) = budget_handle {
        if !detached { bh.abort(); }
    }

    if detached {
        // Continue running as background daemon — wait for child to exit
        let wait_result = tokio::task::spawn_blocking(move || {
            nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(proc_handle.pid as i32), None)
        }).await;
        // Child exited — fall through to normal cleanup
        if let Ok(Ok(nix::sys::wait::WaitStatus::Exited(_, code))) = wait_result {
            if code != 0 {
                eprintln!("Process exited with code {code}");
            }
        }
    }

    // Clear init_pid and set Stopped status now that process has exited.
    // Skip if the pod was destroyed while we were running (e.g. `envpod destroy`
    // killed our child and removed the pod dir + handle).
    if state.pod_dir.exists() {
        let mut updated_state = NativeState::from_handle(&handle)?;
        updated_state.init_pid = None;
        updated_state.status = NativeStatus::Stopped;
        let mut updated_handle = handle.clone();
        updated_handle.backend_state = updated_state.to_json();
        store.save(&updated_handle)?;
    }

    // Clean up supervisor PID file
    let _ = std::fs::remove_file(state.pod_dir.join("supervisor.pid"));

    // Clean up port forwards and discovery registration
    if port_forward_active {
        envpod_core::backend::native::cleanup_port_forwards(&state.pod_dir);
    }
    if internal_active {
        envpod_core::backend::native::cleanup_internal_ports(&state.pod_dir);
    }
    if discovery_active {
        let daemon = envpod_core::dns_daemon::DaemonClient::new(
            envpod_core::dns_daemon::DaemonClient::default_path()
        );
        daemon.unregister(name).await.ok();
    }

    // Shut down queue socket server
    if let Some(qsh) = queue_socket_handle {
        qsh.shutdown();
        qsh.join().await;
    }

    // Shut down queue executor
    queue_executor_handle.shutdown();
    queue_executor_handle.join().await;

    // Shut down monitor agent
    if let Some(mh) = monitor_handle {
        mh.shutdown();
        mh.join().await;
    }

    // Shut down control server
    if let Some(ch) = control_handle {
        ch.shutdown();
        ch.join().await;
    }

    // Shut down DNS server
    if let Some(dns_handle) = dns_handle {
        dns_handle.shutdown();
        dns_handle.join().await;
    }

    Ok(())
}

/// Build a DnsPolicy from persisted NetworkState.
fn build_dns_policy(net: &envpod_core::backend::native::state::NetworkState) -> DnsPolicy {
    let mode = match net.dns_mode.as_str() {
        "blacklist" => DnsPolicyMode::Blacklist,
        "monitor" => DnsPolicyMode::Monitor,
        _ => DnsPolicyMode::Whitelist,
    };

    DnsPolicy {
        mode,
        allowed_domains: net.dns_allow.clone(),
        denied_domains: net.dns_deny.clone(),
        remap: net.dns_remap.clone(),
    }
}

/// Print a one-time welcome banner on the first run of a pod.
/// Subsequent runs skip this — tracked by a `.welcome-shown` marker file.
fn print_welcome_banner(pod_dir: &std::path::Path) {
    let marker = pod_dir.join(".welcome-shown");
    if marker.exists() {
        return;
    }

    let version = env!("CARGO_PKG_VERSION");

    let title = format!("envpod CE v{version} — Zero-trust governance for AI agents");
    // Box width = title display width + 2 padding (not byte length — em dash is multi-byte)
    let inner_width = title.chars().count() + 2;
    let top = format!("┌{}┐", "─".repeat(inner_width));
    let bot = format!("└{}┘", "─".repeat(inner_width));
    let mid = format!(
        "{} {} {}",
        color::cyan("│"),
        color::bold(&title),
        color::cyan("│"),
    );

    eprintln!();
    eprintln!("  {}", color::cyan(&top));
    eprintln!("  {mid}");
    eprintln!("  {}", color::cyan(&bot));
    eprintln!();
    eprintln!("  This pod is {}. Every action is:", color::bold("governed"));
    eprintln!("    {} {}    — writes go to a COW overlay, never the host", color::green("·"), color::bold("Isolated"));
    eprintln!("    {} {}   — all actions logged to the audit trail", color::green("·"), color::bold("Auditable"));
    eprintln!("    {} {}  — review changes with diff, commit or rollback", color::green("·"), color::bold("Reversible"));
    eprintln!("    {} {} — network filtered, resources capped", color::green("·"), color::bold("Restricted"));
    eprintln!();
    eprintln!("  Commands: {} | {} | {} | {}",
        color::cyan("envpod diff"),
        color::cyan("commit"),
        color::cyan("rollback"),
        color::cyan("audit"),
    );
    eprintln!();
    eprintln!("  {}", color::dim("© 2026 Xtellix Inc. — BSL 1.1 License"));
    eprintln!("  {}", color::dim("https://envpod.com"));
    eprintln!();

    // Create marker file — warn on failure, don't bail
    if let Err(e) = std::fs::File::create(&marker) {
        eprintln!("warning: could not write welcome marker: {e}");
    }
}

/// Print a clean banner summarizing the pod's configuration when it starts.
fn print_run_banner(
    name: &str,
    pod_config: &Option<PodConfig>,
    state: &NativeState,
) {
    let header = format!(
        "{} {} {}",
        color::bold("envpod"),
        color::dim("·"),
        color::cyan(name),
    );
    eprintln!();
    eprintln!("  {header}");

    if let Some(cfg) = pod_config {
        // Type
        let pod_type = format!("{:?}", cfg.pod_type).to_lowercase();
        eprintln!("  {}  {pod_type}", color::dim("Type    "));

        // Network
        let net_mode = format!("{:?}", cfg.network.mode).to_lowercase();
        if state.network.is_some() {
            eprintln!(
                "  {}  {net_mode} {} dns {}",
                color::dim("Network "),
                color::dim("·"),
                format!("{:?}", cfg.network.dns.mode).to_lowercase(),
            );
        } else {
            eprintln!("  {}  host (no isolation)", color::dim("Network "));
        }

        // Filesystem
        {
            use envpod_core::config::SystemAccess;
            let access = match cfg.filesystem.system_access {
                SystemAccess::Safe => "safe",
                SystemAccess::Advanced => "advanced",
                SystemAccess::Dangerous => "dangerous",
            };
            let setup_count = cfg.setup.len();
            let setup_info = if setup_count > 0 {
                format!(" {} {setup_count} setup steps", color::dim("·"))
            } else {
                String::new()
            };
            eprintln!(
                "  {}  system {access}{setup_info}",
                color::dim("FS      "),
            );
        }

        // CPU + Memory
        let cores_str = match cfg.processor.cores {
            Some(1.0) => "1 core".to_string(),
            Some(c) => format!("{c} cores"),
            None => "default".to_string(),
        };
        let mem = cfg.processor.memory.as_deref().unwrap_or("default");
        eprintln!(
            "  {}  {cores_str} {} {mem}",
            color::dim("CPU     "),
            color::dim("·"),
        );

        // Disk + Tmp
        let disk = cfg.processor.disk_size.as_deref().unwrap_or("unlimited");
        let tmp = cfg.processor.tmp_size.as_deref().unwrap_or("100MB");
        eprintln!(
            "  {}  {disk} {} /tmp {tmp}",
            color::dim("Disk    "),
            color::dim("·"),
        );

        // GPU
        let gpu_status = if cfg.devices.gpu {
            color::green("allowed")
        } else {
            "denied".to_string()
        };
        eprintln!("  {}  {gpu_status}", color::dim("GPU     "));

        // Extra devices
        if !cfg.devices.extra.is_empty() {
            let extras = cfg.devices.extra.join(", ");
            eprintln!("  {}  {extras}", color::dim("Devices "));
        }

        // Budget
        if let Some(ref dur) = cfg.budget.max_duration {
            eprintln!("  {}  {dur}", color::dim("Budget  "));
        }
    }

    eprintln!("  {}  Ctrl+Z detach, envpod fg {name} reattach", color::dim("Detach  "));
    eprintln!();
}

fn print_pod_info(
    _name: &str,
    config: &PodConfig,
    state: Option<&NativeState>,
) {
    // Backend
    eprintln!("  {}  {}", color::dim("Backend"), &config.backend);

    // Network
    let net_mode = format!("{:?}", config.network.mode).to_lowercase();
    if state.is_some_and(|s| s.network.is_some()) {
        let dns_mode = format!("{:?}", config.network.dns.mode).to_lowercase();
        let domain_count = config.network.dns.allow.len() + config.network.dns.deny.len();
        let domain_info = if domain_count > 0 {
            format!(" ({domain_count} domains)")
        } else {
            String::new()
        };
        eprintln!(
            "  {}  {net_mode} {} dns {dns_mode}{domain_info}",
            color::dim("Network"),
            color::dim("·"),
        );
    } else {
        eprintln!("  {}  host (no isolation)", color::dim("Network"));
    }

    // Filesystem
    {
        use envpod_core::config::SystemAccess;
        let access = match config.filesystem.system_access {
            SystemAccess::Safe => "safe",
            SystemAccess::Advanced => "advanced",
            SystemAccess::Dangerous => "dangerous",
        };
        eprintln!("  {}  system {access}", color::dim("FS     "));
    }

    // CPU + Memory
    let cores_str = match config.processor.cores {
        Some(1.0) => "1 core".to_string(),
        Some(c) => format!("{c} cores"),
        None => "default".to_string(),
    };
    let mem = config.processor.memory.as_deref().unwrap_or("default");
    eprintln!(
        "  {}  {cores_str} {} {mem}",
        color::dim("CPU    "),
        color::dim("·"),
    );

    // Disk + Tmp
    let disk = config.processor.disk_size.as_deref().unwrap_or("unlimited");
    let tmp = config.processor.tmp_size.as_deref().unwrap_or("100MB");
    eprintln!(
        "  {}  {disk} {} /tmp {tmp}",
        color::dim("Disk   "),
        color::dim("·"),
    );

    // Web display
    if config.web_display.display_type != envpod_core::config::WebDisplayType::None {
        let label = match config.web_display.display_type {
            envpod_core::config::WebDisplayType::Novnc => "noVNC",
            envpod_core::config::WebDisplayType::Webrtc => "WebRTC",
            _ => "enabled",
        };
        let pod_ip = state.and_then(|s| s.network.as_ref()).map(|n| n.pod_ip.as_str()).unwrap_or("localhost");
        eprintln!("  {}  {} → http://{}:{}/vnc.html", color::dim("Display"), label, pod_ip, config.web_display.port);
    }

    // Budget
    if let Some(ref dur) = config.budget.max_duration {
        eprintln!("  {}  {dur}", color::dim("Budget "));
    }
}

// ---------------------------------------------------------------------------
// fg (foreground / attach)
// ---------------------------------------------------------------------------

async fn cmd_fg(store: &PodStore, _base_dir: &std::path::Path, name: &str) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;

    let run_log = state.pod_dir.join("run.log");
    let supervisor_pid_path = state.pod_dir.join("supervisor.pid");

    // Check if there's a background supervisor process
    let supervisor_pid: Option<i32> = std::fs::read_to_string(&supervisor_pid_path)
        .ok()
        .and_then(|s| s.trim().parse().ok());

    // Determine the PID to monitor: init_pid (child in namespace) or supervisor PID
    let pid = if let Some(p) = state.init_pid {
        p
    } else if let Some(sp) = supervisor_pid {
        // init_pid not set yet — pod is still starting up, monitor the supervisor
        sp as u32
    } else {
        anyhow::bail!("pod '{name}' is not running");
    };

    // Verify process is alive
    let nix_pid = nix::unistd::Pid::from_raw(pid as i32);
    if nix::sys::signal::kill(nix_pid, None).is_err() {
        anyhow::bail!("pod '{name}' process (PID {pid}) is not running");
    }

    eprintln!("Attaching to pod '{}' (PID {})...", color::green(name), pid);
    eprintln!("Press Ctrl+Z to detach (pod continues running in background)");
    eprintln!();

    // Tail the run.log file (if it exists) while waiting for the process
    let log_task = if run_log.exists() {
        // Print existing log content first
        if let Ok(content) = std::fs::read_to_string(&run_log) {
            if !content.is_empty() {
                print!("{content}");
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
        }

        // Then tail -f
        let log_path = run_log.clone();
        Some(tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let file = tokio::fs::File::open(&log_path).await.ok();
            if let Some(file) = file {
                let metadata = file.metadata().await.ok();
                let start_pos = metadata.map(|m| m.len()).unwrap_or(0);

                // Open again for tailing from current position
                let mut file = tokio::fs::File::open(&log_path).await.unwrap();
                file.seek(std::io::SeekFrom::Start(start_pos)).await.ok();

                let mut buf = [0u8; 4096];
                loop {
                    match file.read(&mut buf).await {
                        Ok(0) => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        Ok(n) => {
                            use std::io::Write;
                            std::io::stdout().write_all(&buf[..n]).ok();
                            std::io::stdout().flush().ok();
                        }
                        Err(_) => break,
                    }
                }
            }
        }))
    } else {
        None
    };

    // Wait for process to exit or Ctrl+Z (detach)
    let mut sigtstp = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(20))
        .context("failed to register SIGTSTP handler")?;

    let wait_result = {
        let poll_pid = pid;
        let wait_task = tokio::task::spawn_blocking(move || {
            // We can't waitpid on a process we didn't fork — poll /proc instead
            loop {
                let proc_path = format!("/proc/{poll_pid}");
                if !std::path::Path::new(&proc_path).exists() {
                    return true; // Process exited
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });

        tokio::select! {
            result = wait_task => result.unwrap_or(true),
            _ = sigtstp.recv() => false, // Ctrl+Z = detach
        }
    };

    // Stop log tailing
    if let Some(lt) = log_task {
        lt.abort();
    }

    if wait_result {
        // Process exited
        eprintln!();
        eprintln!("Process exited");
    } else {
        // Detached via Ctrl+Z
        eprintln!();
        eprintln!("Detached from pod '{}' (still running in background, PID {})", name, pid);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

fn cmd_diff(store: &PodStore, base_dir: &std::path::Path, name: &str, json: bool, all: bool) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    let all_diffs = backend.diff(&handle)?;

    // Apply tracking filter unless --all is set
    let (diffs, filtered) = if all {
        (all_diffs, false)
    } else {
        let state = NativeState::from_handle(&handle)?;
        let tracking = state
            .load_config()?
            .map(|c| c.filesystem.tracking)
            .unwrap_or_default();
        let total = all_diffs.len();
        let result = envpod_core::backend::native::filter_diff(all_diffs, &tracking);
        let was_filtered = result.len() < total;
        (result, was_filtered)
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&diffs)?);
        return Ok(());
    }

    if diffs.is_empty() {
        println!("No changes in pod '{name}'");
        if filtered {
            println!("{}", color::dim("(filtered by tracking config — use --all to see all changes)"));
        }
        return Ok(());
    }

    for d in &diffs {
        // Annotate system paths when showing all changes
        let rel = d.path.strip_prefix("/").unwrap_or(&d.path);
        let system_tag = if all && envpod_core::backend::native::is_protected(rel) {
            " [system]"
        } else {
            ""
        };
        let line = if d.size > 0 {
            format!("  {} ({} bytes){system_tag}", d.path.display(), d.size)
        } else {
            format!("  {}{system_tag}", d.path.display())
        };
        match d.kind {
            DiffKind::Added => println!("{}", color::green(&format!("+{line}"))),
            DiffKind::Modified => println!("{}", color::yellow(&format!("~{line}"))),
            DiffKind::Deleted => println!("{}", color::red(&format!("-{line}"))),
        }
    }
    println!("\n{} change(s) in pod '{name}'", diffs.len());
    if filtered {
        println!("{}", color::dim("(filtered by tracking config — use --all to see all changes)"));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// commit
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_commit(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    paths: &[String],
    exclude: &[String],
    output: Option<&str>,
    all: bool,
    include_system: bool,
    rollback_rest: bool,
) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;

    let diffs = backend.diff(&handle)?;
    if diffs.is_empty() {
        println!("Nothing to commit in pod '{name}'");
        return Ok(());
    }

    // Load pod config for system_access profile
    let state = NativeState::from_handle(&handle)?;
    let pod_config = state.load_config()?;
    let system_access = pod_config
        .as_ref()
        .map(|c| c.filesystem.system_access)
        .unwrap_or_default();

    // Build the set of paths to commit
    let commit_paths: Option<Vec<PathBuf>> = if !paths.is_empty() {
        // Validate that each specified path exists in the diff
        let diff_paths: std::collections::HashSet<PathBuf> =
            diffs.iter().map(|d| d.path.clone()).collect();
        for p in paths {
            let pb = PathBuf::from(p);
            if !diff_paths.contains(&pb) {
                anyhow::bail!("path not in diff: {p}");
            }
        }
        Some(paths.iter().map(PathBuf::from).collect())
    } else if !exclude.is_empty() {
        // Commit everything except excluded paths
        let excluded: std::collections::HashSet<PathBuf> =
            exclude.iter().map(PathBuf::from).collect();
        let remaining: Vec<PathBuf> = diffs
            .iter()
            .map(|d| d.path.clone())
            .filter(|p| !excluded.contains(p))
            .collect();
        if remaining.is_empty() {
            println!("Nothing to commit after exclusions in pod '{name}'");
            return Ok(());
        }
        Some(remaining)
    } else if !all {
        // No explicit paths and --all not set: apply tracking filter
        let tracking = pod_config
            .as_ref()
            .map(|c| c.filesystem.tracking.clone())
            .unwrap_or_default();
        let filtered = envpod_core::backend::native::filter_diff(diffs.clone(), &tracking);
        if filtered.is_empty() {
            println!("Nothing to commit after tracking filter in pod '{name}'");
            println!("{}", color::dim("(filtered by tracking config — use --all to commit all changes)"));
            return Ok(());
        }
        if filtered.len() < diffs.len() {
            // Some paths were filtered — use selective commit
            Some(filtered.iter().map(|d| d.path.clone()).collect())
        } else {
            None // all diffs passed filter, commit everything
        }
    } else {
        None // --all: commit all
    };

    // Apply system path protection based on system_access profile
    let commit_paths = apply_system_protection(
        commit_paths,
        &diffs,
        system_access,
        include_system,
        name,
    )?;

    // Print what's being committed
    let commit_count = commit_paths
        .as_ref()
        .map(|p| p.len())
        .unwrap_or(diffs.len());

    if commit_count == 0 {
        println!("Nothing to commit in pod '{name}'");
        return Ok(());
    }

    let target = output.unwrap_or("host filesystem");
    println!(
        "Committing {} change(s) from pod '{name}' to {target}...",
        commit_count
    );

    if let Some(ref cp) = commit_paths {
        for p in cp {
            if let Some(d) = diffs.iter().find(|d| &d.path == p) {
                let line = if d.size > 0 {
                    format!("  {} ({} bytes)", d.path.display(), d.size)
                } else {
                    format!("  {}", d.path.display())
                };
                match d.kind {
                    DiffKind::Added => println!("{}", color::green(&format!("+{line}"))),
                    DiffKind::Modified => println!("{}", color::yellow(&format!("~{line}"))),
                    DiffKind::Deleted => println!("{}", color::red(&format!("-{line}"))),
                }
            }
        }
    }

    // Queue gate: if require_commit_approval is set, submit to queue and stop.
    let requires_approval = pod_config
        .as_ref()
        .map_or(false, |c| c.queue.require_commit_approval);
    if requires_approval {
        let paths_value: serde_json::Value = match &commit_paths {
            Some(cp) => {
                let strs: Vec<_> = cp.iter().map(|p| p.to_string_lossy().to_string()).collect();
                serde_json::json!(strs)
            }
            None => serde_json::Value::Null,
        };
        let payload = serde_json::json!({
            "type": "commit",
            "paths": paths_value,
            "include_system": include_system,
            "output": output,
        });
        let queue = ActionQueue::new(&state.pod_dir);
        let description = format!("commit {commit_count} change(s) to {target}");
        let action = queue.submit_with_payload(ActionTier::Staged, &description, payload)?;
        ActionQueue::emit_audit(&state.pod_dir, name, AuditAction::QueueSubmit, &action);
        println!(
            "Commit queued for approval (id: {})",
            &action.id.to_string()[..8]
        );
        println!(
            "Approve and execute: {}",
            color::bold(&format!("envpod approve {name} {}", &action.id.to_string()[..8]))
        );
        return Ok(());
    }

    backend.commit(&handle, commit_paths.as_deref(), output.map(Path::new))?;

    // Rollback uncommitted changes if --rollback-rest was specified
    if rollback_rest && commit_paths.is_some() {
        let remaining = backend.diff(&handle)?;
        if remaining.is_empty() {
            println!(
                "Committed {} file(s). Nothing to roll back.",
                commit_count
            );
        } else {
            let rollback_count = remaining.len();
            backend.rollback(&handle)?;
            println!(
                "Committed {} file(s). Rolled back {}.",
                commit_count, rollback_count
            );
        }
    } else {
        println!("Done");
    }

    Ok(())
}

/// Apply system path protection based on the pod's system_access profile.
///
/// Returns the (possibly modified) commit paths after protection rules.
fn apply_system_protection(
    commit_paths: Option<Vec<PathBuf>>,
    diffs: &[envpod_core::types::FileDiff],
    system_access: envpod_core::config::SystemAccess,
    include_system: bool,
    pod_name: &str,
) -> Result<Option<Vec<PathBuf>>> {
    use envpod_core::config::SystemAccess;
    use envpod_core::backend::native::partition_protected;

    // Safe mode: system dirs are read-only bind mounts, no changes possible
    if system_access == SystemAccess::Safe {
        return Ok(commit_paths);
    }

    // For advanced/dangerous: check if any commit paths are protected
    let paths_to_check: Vec<envpod_core::types::FileDiff> = match &commit_paths {
        Some(cp) => diffs.iter().filter(|d| cp.contains(&d.path)).cloned().collect(),
        None => diffs.to_vec(),
    };

    let (_safe_diffs, protected_diffs) = partition_protected(paths_to_check);

    if protected_diffs.is_empty() {
        return Ok(commit_paths);
    }

    let protected_count = protected_diffs.len();

    match system_access {
        SystemAccess::Advanced => {
            if include_system {
                // --include-system: allow everything
                Ok(commit_paths)
            } else {
                // Block protected paths, commit only safe ones
                eprintln!(
                    "{} {} system path change(s) in pod '{}' (blocked by advanced profile):",
                    color::yellow("warning:"),
                    protected_count,
                    pod_name,
                );
                for d in &protected_diffs {
                    eprintln!("  {}", color::yellow(&format!("[system] {}", d.path.display())));
                }
                eprintln!(
                    "{}",
                    color::dim("Use --include-system to commit system directory changes")
                );

                // Remove protected paths from the commit set
                let protected_set: std::collections::HashSet<PathBuf> =
                    protected_diffs.iter().map(|d| d.path.clone()).collect();
                match commit_paths {
                    Some(cp) => {
                        let filtered: Vec<PathBuf> = cp.into_iter()
                            .filter(|p| !protected_set.contains(p))
                            .collect();
                        Ok(Some(filtered))
                    }
                    None => {
                        // Was going to commit all — now commit only safe paths
                        let filtered: Vec<PathBuf> = diffs.iter()
                            .map(|d| d.path.clone())
                            .filter(|p| !protected_set.contains(p))
                            .collect();
                        Ok(Some(filtered))
                    }
                }
            }
        }
        SystemAccess::Dangerous => {
            if !include_system {
                // Warn but allow
                eprintln!(
                    "{} committing {} system path change(s) in pod '{}' (dangerous profile):",
                    color::yellow("warning:"),
                    protected_count,
                    pod_name,
                );
                for d in &protected_diffs {
                    eprintln!("  {}", color::yellow(&format!("[system] {}", d.path.display())));
                }
                eprintln!(
                    "{}",
                    color::dim("Use --include-system to suppress this warning")
                );
            }
            // Dangerous: always allow
            Ok(commit_paths)
        }
        SystemAccess::Safe => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// rollback
// ---------------------------------------------------------------------------

fn cmd_rollback(store: &PodStore, base_dir: &std::path::Path, name: &str) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;

    let diffs = backend.diff(&handle)?;
    if diffs.is_empty() {
        println!("Nothing to roll back in pod '{name}'");
        return Ok(());
    }

    // Load pod config to check queue setting
    let state = NativeState::from_handle(&handle)?;
    let pod_config = state.load_config()?;
    let requires_approval = pod_config
        .as_ref()
        .map_or(false, |c| c.queue.require_rollback_approval);

    if requires_approval {
        let payload = serde_json::json!({"type": "rollback"});
        let queue = ActionQueue::new(&state.pod_dir);
        let description = format!("rollback {} change(s)", diffs.len());
        let action = queue.submit_with_payload(ActionTier::Staged, &description, payload)?;
        ActionQueue::emit_audit(&state.pod_dir, name, AuditAction::QueueSubmit, &action);
        println!(
            "Rollback queued for approval (id: {})",
            &action.id.to_string()[..8]
        );
        println!(
            "Approve and execute: {}",
            color::bold(&format!("envpod approve {name} {}", &action.id.to_string()[..8]))
        );
        return Ok(());
    }

    println!("Discarding {} change(s) in pod '{name}'...", diffs.len());
    backend.rollback(&handle)?;
    println!("Done");

    Ok(())
}

// ---------------------------------------------------------------------------
// security audit
// ---------------------------------------------------------------------------

struct SecurityFinding {
    id: &'static str,
    severity: &'static str,
    title: &'static str,
    explanation: String,
    fix: &'static str,
}

fn security_findings(config: &PodConfig) -> Vec<SecurityFinding> {
    let mut findings = Vec::new();

    // N-03: DNS bypass via blacklist/monitor mode
    match config.network.dns.mode {
        envpod_core::types::DnsMode::Blacklist => {
            findings.push(SecurityFinding {
                id: "N-03",
                severity: "HIGH",
                title: "Direct DNS bypass possible",
                explanation: "dns.mode is Blacklist — direct IP queries bypass domain filtering.".into(),
                fix: "use dns.mode: Whitelist to block all non-allowed traffic.",
            });
        }
        envpod_core::types::DnsMode::Monitor => {
            findings.push(SecurityFinding {
                id: "N-03",
                severity: "HIGH",
                title: "Direct DNS bypass possible",
                explanation: "dns.mode is Monitor — all queries are logged but not blocked.".into(),
                fix: "use dns.mode: Whitelist to block all non-allowed traffic.",
            });
        }
        _ => {}
    }

    // N-05: Root user can modify iptables
    if config.user == "root" {
        findings.push(SecurityFinding {
            id: "N-05",
            severity: "CRITICAL",
            title: "Pod can modify iptables",
            explanation: "user is root — CAP_NET_ADMIN allows iptables -F inside the pod.".into(),
            fix: "remove user: root (default non-root 'agent' user blocks this).",
        });
    }

    // N-06: Root user has raw sockets
    if config.user == "root" {
        findings.push(SecurityFinding {
            id: "N-06",
            severity: "HIGH",
            title: "Raw sockets available",
            explanation: "user is root — raw ICMP/TCP sockets accessible inside the pod.".into(),
            fix: "remove user: root (default non-root 'agent' user blocks this).",
        });
    }

    // S-03: Browser seccomp profile allows extra syscalls
    if config.security.seccomp_profile == "browser" {
        findings.push(SecurityFinding {
            id: "S-03",
            severity: "HIGH",
            title: "Relaxed seccomp profile",
            explanation: "seccomp_profile is browser — allows clone3, unshare, and other syscalls blocked by default.".into(),
            fix: "remove security.seccomp_profile or set to default if browser features are not needed.",
        });
    }

    // P-03: Browser seccomp allows nested namespaces
    if config.security.seccomp_profile == "browser" {
        findings.push(SecurityFinding {
            id: "P-03",
            severity: "MEDIUM",
            title: "Nested namespaces possible",
            explanation: "seccomp_profile is browser — unshare/clone3 allow nested namespace creation.".into(),
            fix: "remove security.seccomp_profile or set to default if browser features are not needed.",
        });
    }

    // S-04: No seccomp filter (all syscalls allowed)
    if config.security.seccomp_profile == "none" {
        findings.push(SecurityFinding {
            id: "S-04",
            severity: "CRITICAL",
            title: "No seccomp filter — all syscalls allowed",
            explanation: "seccomp_profile is none — no syscall filtering applied. The agent can invoke any syscall including mount, reboot, and kexec_load. Use only for debugging.".into(),
            fix: "set seccomp_profile to default (or browser if needed).",
        });
    }

    // I-04: Display forwarding (protocol-aware)
    if config.devices.display {
        use envpod_core::config::DisplayProtocol;
        match config.devices.display_protocol {
            DisplayProtocol::Wayland => {
                findings.push(SecurityFinding {
                    id: "I-04",
                    severity: "LOW",
                    title: "Wayland display access — client isolation enforced by compositor",
                    explanation: "devices.display is true with Wayland — compositor prevents cross-client keylogging and screenshot.".into(),
                    fix: "set devices.display: false if GUI access is not needed.",
                });
            }
            DisplayProtocol::X11 => {
                findings.push(SecurityFinding {
                    id: "I-04",
                    severity: "CRITICAL",
                    title: "X11 display access — keylogging possible",
                    explanation: "devices.display is true with X11 — clients can capture keystrokes, take screenshots, and inject input into other windows.".into(),
                    fix: "set display_protocol: wayland for secure display forwarding.",
                });
            }
            DisplayProtocol::Auto => {
                findings.push(SecurityFinding {
                    id: "I-04",
                    severity: "CRITICAL",
                    title: "X11 display access — keylogging possible",
                    explanation: "devices.display is true with auto-detection — may use X11, which allows keylogging, screenshots, and input injection.".into(),
                    fix: "set display_protocol: wayland for secure display forwarding.",
                });
            }
        }
    }

    // I-05: Audio device access (protocol-aware)
    if config.devices.audio {
        use envpod_core::config::AudioProtocol;
        match config.devices.audio_protocol {
            AudioProtocol::Pipewire => {
                findings.push(SecurityFinding {
                    id: "I-05",
                    severity: "MEDIUM",
                    title: "Audio access available (PipeWire — finer-grained permissions)",
                    explanation: "devices.audio is true with PipeWire — audio access with finer-grained permission model.".into(),
                    fix: "set devices.audio: false if audio is not needed.",
                });
            }
            AudioProtocol::Pulseaudio => {
                findings.push(SecurityFinding {
                    id: "I-05",
                    severity: "HIGH",
                    title: "Microphone access available",
                    explanation: "devices.audio is true with PulseAudio — unrestricted microphone recording from host.".into(),
                    fix: "set audio_protocol: pipewire for finer-grained audio permissions.",
                });
            }
            AudioProtocol::Auto => {
                findings.push(SecurityFinding {
                    id: "I-05",
                    severity: "HIGH",
                    title: "Microphone access available",
                    explanation: "devices.audio is true with auto-detection — may use PulseAudio, which allows unrestricted microphone recording.".into(),
                    fix: "set audio_protocol: pipewire for finer-grained audio permissions.",
                });
            }
        }
    }

    // I-06: GPU information leakage
    if config.devices.gpu {
        findings.push(SecurityFinding {
            id: "I-06",
            severity: "LOW",
            title: "GPU information leakage",
            explanation: "devices.gpu is true — nvidia-smi exposes host GPU model to the agent.".into(),
            fix: "set devices.gpu: false if GPU access is not needed.",
        });
    }

    // D-01: allow_discovery with Unsafe network mode
    // A discoverable pod in Unsafe mode has no network isolation — any process on the
    // host's network stack can connect to it once its IP is resolved.
    if config.network.allow_discovery
        && config.network.mode == envpod_core::types::NetworkMode::Unsafe
    {
        findings.push(SecurityFinding {
            id: "D-01",
            severity: "HIGH",
            title: "Discoverable pod with no network isolation",
            explanation: "network.allow_discovery is true but network.mode is Unsafe — the pod \
                          registers as <name>.pods.local and has no iptables isolation, so any \
                          process on the host network that resolves the name can connect to any port.".into(),
            fix: "set network.mode: Isolated or Monitored to enforce network boundaries.",
        });
    }

    // D-02: allow_pods wildcard grants broad fleet visibility
    // A pod with allow_pods: ["*"] can resolve every discoverable pod in the fleet,
    // giving it wider infrastructure knowledge than it may need.
    if config.network.allow_pods.iter().any(|p| p == "*") {
        findings.push(SecurityFinding {
            id: "D-02",
            severity: "MEDIUM",
            title: "Wildcard pod discovery — agent can enumerate fleet",
            explanation: "network.allow_pods contains \"*\" — this pod can resolve the name of \
                          every discoverable pod in the fleet, giving it full fleet topology knowledge.".into(),
            fix: "list specific pod names in allow_pods instead of using \"*\".",
        });
    }

    // N-04: public_ports exposes pod service on all host network interfaces.
    // network.ports (localhost-only) and 127.0.0.1:-prefixed specs are exempt.
    if !config.network.public_ports.is_empty() {
        let specs = config.network.public_ports.join(", ");
        findings.push(SecurityFinding {
            id: "N-04",
            severity: "LOW",
            title: "Port forwarding exposes pod service to host network",
            explanation: format!(
                "network.public_ports is set ({specs}) — PREROUTING DNAT applies to all \
                 host network interfaces. Other machines on the host's network can reach \
                 the pod service on the forwarded port(s)."
            ),
            fix: "use network.ports (or -p) instead if only localhost access is needed.",
        });
    }

    // W-01: noVNC enabled — VNC unencrypted (mitigated by localhost-only)
    if config.web_display.display_type == envpod_core::config::WebDisplayType::Novnc {
        findings.push(SecurityFinding {
            id: "W-01",
            severity: "MEDIUM",
            title: "noVNC enabled — VNC protocol is unencrypted",
            explanation: "web_display.type is novnc — VNC traffic between x11vnc and websockify \
                          is unencrypted. Mitigated by localhost-only port forwarding by default."
                .into(),
            fix: "use web_display.type: webrtc for DTLS-encrypted transport, or keep novnc with \
                  localhost-only access (ports, not public_ports).",
        });
    }

    // W-02: web_display port in public_ports — remote VNC without encryption
    if config.web_display.display_type != envpod_core::config::WebDisplayType::None {
        let display_port = config.web_display.port.to_string();
        let exposed = config.network.public_ports.iter().any(|spec| {
            spec.starts_with(&format!("{display_port}:")) || spec == &display_port
        });
        if exposed {
            findings.push(SecurityFinding {
                id: "W-02",
                severity: "HIGH",
                title: "Web display port exposed on all interfaces",
                explanation: format!(
                    "web_display port {display_port} is in network.public_ports — the display \
                     is accessible from other machines on the host network."
                ),
                fix: "use network.ports instead of public_ports for localhost-only display access.",
            });
        }
    }

    // W-03: WebRTC enabled — uses DTLS, lower risk
    if config.web_display.display_type == envpod_core::config::WebDisplayType::Webrtc {
        findings.push(SecurityFinding {
            id: "W-03",
            severity: "LOW",
            title: "WebRTC display enabled",
            explanation: "web_display.type is webrtc — uses DTLS encryption for transport. \
                          Lower risk than VNC but still exposes a desktop stream."
                .into(),
            fix: "set web_display.type: none if GUI access is not needed.",
        });
    }

    // C-01: No memory limit
    if config.processor.memory.is_none() {
        findings.push(SecurityFinding {
            id: "C-01",
            severity: "MEDIUM",
            title: "No memory limit set",
            explanation: "processor.memory is not set — pod can consume all host memory.".into(),
            fix: "set processor.memory (e.g. \"2GB\") to cap memory usage.",
        });
    }

    // C-02: No CPU limit
    if config.processor.cores.is_none() {
        findings.push(SecurityFinding {
            id: "C-02",
            severity: "MEDIUM",
            title: "No CPU limit set",
            explanation: "processor.cores is not set — pod can consume all host CPU.".into(),
            fix: "set processor.cores (e.g. 2.0) to cap CPU usage.",
        });
    }

    // C-03: No PID limit
    if config.processor.max_pids.is_none() {
        findings.push(SecurityFinding {
            id: "C-03",
            severity: "MEDIUM",
            title: "No PID limit set",
            explanation: "processor.max_pids is not set — pod can fork-bomb the host.".into(),
            fix: "set processor.max_pids (e.g. 512) to limit process count.",
        });
    }

    findings
}

fn cmd_security_audit(
    store: &PodStore,
    name: Option<&str>,
    config_path: Option<&std::path::Path>,
    json: bool,
) -> Result<()> {
    let (config, display_name) = match (config_path, name) {
        (Some(path), _) => {
            let c = PodConfig::from_file(path)
                .with_context(|| format!("load config: {}", path.display()))?;
            let label = c.name.clone();
            let label = if label.is_empty() {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".into())
            } else {
                label
            };
            (c, label)
        }
        (None, Some(n)) => {
            let handle = store.load(n)?;
            let state = NativeState::from_handle(&handle)?;
            let c = state.load_config()?.unwrap_or_default();
            (c, n.to_string())
        }
        (None, None) => {
            anyhow::bail!("provide a pod name or --config <path> for security audit");
        }
    };

    let findings = security_findings(&config);

    if json {
        let json_findings: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id,
                    "severity": f.severity,
                    "title": f.title,
                    "explanation": f.explanation,
                    "fix": f.fix,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_findings)?);
        return Ok(());
    }

    // Header
    println!();
    println!(
        "  {} {}",
        color::bold("envpod security audit"),
        color::dim(&format!("· {display_name}")),
    );
    println!();

    // Pod boundary score
    let is_root = config.user == "root";
    let total_checks: u32 = 17;
    let deducted: u32 = if is_root { 2 } else { 0 };
    let score = total_checks - deducted;
    if is_root {
        println!(
            "  {}  {}/{} — root reduces protection",
            color::dim("Pod boundary"),
            color::yellow(&score.to_string()),
            total_checks,
        );
    } else {
        println!(
            "  {}  {}/{} with default user",
            color::dim("Pod boundary"),
            color::green(&score.to_string()),
            total_checks,
        );
    }
    println!();

    if findings.is_empty() {
        println!(
            "  {} All security checks passed",
            color::green("✓"),
        );
        println!();
        return Ok(());
    }

    // Findings
    let note_word = if findings.len() == 1 { "note" } else { "notes" };
    println!(
        "  {} {} security {}:",
        color::yellow("⚠"),
        findings.len(),
        note_word,
    );
    println!();

    for f in &findings {
        let severity_colored = match f.severity {
            "CRITICAL" => color::red(&format!("[{}]", f.severity)),
            "HIGH" => color::red(&format!("[{}]", f.severity)),
            "MEDIUM" => color::yellow(&format!("[{}]", f.severity)),
            "LOW" => color::dim(&format!("[{}]", f.severity)),
            _ => format!("[{}]", f.severity),
        };
        println!(
            "  {}  {:10} {}",
            color::bold(f.id),
            severity_colored,
            f.title,
        );
        println!(
            "  {}  {}",
            " ".repeat(f.id.len()),
            color::dim(&f.explanation),
        );
        println!(
            "  {}  Fix: {}",
            " ".repeat(f.id.len()),
            f.fix,
        );
        println!();
    }

    // Passed checks summary
    let passed_checks = collect_passed_checks(&config);
    if !passed_checks.is_empty() {
        let shown: Vec<&str> = passed_checks.iter().take(5).copied().collect();
        let extra = if passed_checks.len() > 5 {
            ", ...".to_string()
        } else {
            String::new()
        };
        println!(
            "  {} {} checks passed ({}{})",
            color::green("✓"),
            passed_checks.len(),
            shown.join(", "),
            extra,
        );
        println!();
    }

    Ok(())
}

/// Return names of security checks that passed (no finding generated).
fn collect_passed_checks(config: &PodConfig) -> Vec<&'static str> {
    let mut passed = Vec::new();

    if config.user != "root" {
        passed.push("user");
    }
    if matches!(config.network.dns.mode, envpod_core::types::DnsMode::Whitelist) {
        passed.push("dns");
    }
    if config.security.seccomp_profile != "browser" {
        passed.push("seccomp");
    }
    if config.processor.memory.is_some() {
        passed.push("memory");
    }
    if config.processor.cores.is_some() {
        passed.push("cpu");
    }
    if config.processor.max_pids.is_some() {
        passed.push("pids");
    }
    if !config.devices.display {
        passed.push("display");
    } else if config.devices.display_protocol == envpod_core::config::DisplayProtocol::Wayland {
        passed.push("display (wayland)");
    }
    if !config.devices.audio {
        passed.push("audio");
    } else if config.devices.audio_protocol == envpod_core::config::AudioProtocol::Pipewire {
        passed.push("audio (pipewire)");
    }
    if !config.devices.gpu {
        passed.push("gpu");
    }

    passed
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

fn cmd_audit(store: &PodStore, _base_dir: &std::path::Path, name: &str, json: bool) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;

    let log = envpod_core::audit::AuditLog::new(&state.pod_dir);
    let mut entries = log.read_all()?;

    // Merge file upload audit entries from in-pod upload server.
    // The upload server writes to /tmp/envpod-uploads.jsonl inside the overlay.
    let upload_log = state.pod_dir.join("upper/tmp/envpod-uploads.jsonl");
    if upload_log.exists() {
        if let Ok(upload_entries) = envpod_core::audit::AuditLog::read_file(&upload_log) {
            entries.extend(upload_entries);
            entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No audit entries for pod '{name}'");
        return Ok(());
    }

    println!(
        "{:<24} {:<12} {:<4} DETAIL",
        "TIMESTAMP", "ACTION", "OK"
    );
    println!("{}", "-".repeat(72));
    for e in &entries {
        println!(
            "{:<24} {:<12} {:<4} {}",
            e.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            e.action,
            if e.success { "yes" } else { "no" },
            e.detail,
        );
    }
    println!("\n{} entry(ies) for pod '{name}'", entries.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// lock
// ---------------------------------------------------------------------------

fn cmd_lock(store: &PodStore, base_dir: &std::path::Path, name: Option<&str>, all: bool) -> Result<()> {
    if all {
        let pods = store.list()?;
        if pods.is_empty() {
            println!("No pods to lock");
            return Ok(());
        }
        for handle in &pods {
            let backend = create_backend(&handle.backend, base_dir)?;
            backend.freeze(handle)?;

            // Register undo
            if let Ok(state) = NativeState::from_handle(handle) {
                let registry = UndoRegistry::new(&state.pod_dir);
                registry.register(
                    &format!("lock pod '{}'", handle.name),
                    UndoMechanism::Thaw,
                )?;
            }

            println!("Locked pod '{}'", handle.name);
        }
    } else if let Some(name) = name {
        let handle = store.load(name)?;
        let backend = create_backend(&handle.backend, base_dir)?;
        backend.freeze(&handle)?;

        // Register undo
        let state = NativeState::from_handle(&handle)?;
        let registry = UndoRegistry::new(&state.pod_dir);
        registry.register(
            &format!("lock pod '{name}'"),
            UndoMechanism::Thaw,
        )?;

        println!("Locked pod '{name}'");
    } else {
        anyhow::bail!("specify a pod name or --all");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// unlock (resume a frozen pod)
// ---------------------------------------------------------------------------

fn cmd_unlock(store: &PodStore, base_dir: &std::path::Path, name: &str) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    backend.resume(&handle)?;
    println!("Resumed pod '{name}'");
    Ok(())
}

// ---------------------------------------------------------------------------
// kill
// ---------------------------------------------------------------------------

fn cmd_kill(store: &PodStore, base_dir: &std::path::Path, name: &str) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;

    // Emit Kill audit entry (distinct from Stop — indicates forced termination)
    let state = NativeState::from_handle(&handle)?;
    let log = AuditLog::new(&state.pod_dir);
    log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.to_string(),
        action: AuditAction::Kill,
        detail: "forced kill + rollback".into(),
        success: true,
    })?;

    backend.stop(&handle)?;
    println!("Stopped pod '{name}'");

    backend.rollback(&handle)?;
    println!("Rolled back changes");

    Ok(())
}

// ---------------------------------------------------------------------------
// destroy
// ---------------------------------------------------------------------------

async fn cmd_destroy(store: &PodStore, base_dir: &std::path::Path, name: &str, remove_base: bool, full: bool) -> Result<()> {
    let handle = store.load(name)?;

    // Resolve base name + clean up port forwards before destroying
    let native_state = NativeState::from_handle(&handle).ok();
    let base_name = native_state.as_ref().and_then(|s| resolve_base_name(&s.pod_dir));
    if let Some(ref s) = native_state {
        envpod_core::backend::native::cleanup_port_forwards(&s.pod_dir);
        envpod_core::backend::native::cleanup_internal_ports(&s.pod_dir);
    }
    // Always try to unregister — cleans up stale entries from crashed pods
    {
        let daemon = envpod_core::dns_daemon::DaemonClient::new(
            envpod_core::dns_daemon::DaemonClient::default_path()
        );
        daemon.unregister(name).await.ok();
    }

    // Kill the supervisor process (background `envpod run -b`) before destroying,
    // so it can't re-create the handle file during cleanup.
    if let Some(ref s) = native_state {
        let supervisor_pid_path = s.pod_dir.join("supervisor.pid");
        if let Ok(pid_str) = std::fs::read_to_string(&supervisor_pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                let nix_pid = nix::unistd::Pid::from_raw(pid);
                // SIGKILL the supervisor — it must not run cleanup after us
                nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGKILL).ok();
                // Brief wait for it to die
                for _ in 0..10 {
                    if nix::sys::signal::kill(nix_pid, None).is_err() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }

    let backend = create_backend(&handle.backend, base_dir)?;

    // Best-effort stop before destroying (kills processes inside the cgroup)
    backend.stop(&handle).ok();

    let destroy_result = if full {
        // Full cleanup: also remove iptables rules for this pod immediately
        let native = NativeBackend::new(base_dir)?;
        native.destroy_full(&handle)
    } else {
        backend.destroy(&handle)
    };

    match &destroy_result {
        Ok(()) => {
            // Pod directory removed successfully — safe to remove handle
            store.remove(name)?;
            println!("Destroyed pod '{name}'");
        }
        Err(e) => {
            // Pod directory still exists (e.g. busy mount) — keep handle so
            // `envpod ls` shows it and a second `destroy` can finish the job.
            eprintln!("warning: cleanup incomplete for '{name}': {e}");
            eprintln!("  Try again or run `sudo envpod gc` to clean up leftover resources");
        }
    }

    // Optionally destroy the base pod too
    if remove_base {
        let bases_dir = base_dir.join("bases");
        // Use the resolved base name, or fall back to the pod name
        let bname = base_name.as_deref().unwrap_or(name);

        // Check if any remaining pods still use this base
        let users = find_base_users(store, &bases_dir, bname);
        if !users.is_empty() {
            let list = users.join(", ");
            eprintln!(
                "warning: base pod '{bname}' still used by: {list} — not removed"
            );
        } else {
            destroy_base(&bases_dir, bname)?;
            println!("Destroyed base pod '{bname}'");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// prune
// ---------------------------------------------------------------------------

async fn cmd_prune(store: &PodStore, base_dir: &std::path::Path, prune_bases: bool) -> Result<()> {
    let pods = store.list()?;
    let mut removed = 0;

    for handle in &pods {
        let backend = match create_backend(&handle.backend, base_dir) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let info = match backend.info(handle) {
            Ok(i) => i,
            Err(_) => continue,
        };

        // Only prune stopped or created (never-started) pods
        match info.status {
            envpod_core::types::PodStatus::Running | envpod_core::types::PodStatus::Frozen => continue,
            _ => {}
        }

        let name = &handle.name;
        cmd_destroy(store, base_dir, name, false, false).await?;
        removed += 1;
    }

    if removed == 0 {
        println!("No stopped pods to prune");
    } else {
        println!("\nPruned {} pod{}", removed, if removed == 1 { "" } else { "s" });
    }

    // Optionally prune unused bases too
    if prune_bases {
        let bases_dir = base_dir.join("bases");
        if bases_dir.exists() {
            let entries: Vec<String> = std::fs::read_dir(&bases_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().join("rootfs").exists())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();

            let mut base_removed = 0;
            for name in &entries {
                let users = find_base_users(store, &bases_dir, name);
                if users.is_empty() {
                    destroy_base(&bases_dir, name)?;
                    println!("Pruned base pod '{name}'");
                    base_removed += 1;
                }
            }

            if base_removed == 0 && removed == 0 {
                // already printed "No stopped pods" above
            } else if base_removed == 0 {
                println!("No unused base pods to prune");
            } else {
                println!("Pruned {} base pod{}", base_removed, if base_removed == 1 { "" } else { "s" });
            }
        }
    }

    Ok(())
}

/// Find pods whose rootfs symlink points into the given base pod.
fn find_base_users(store: &PodStore, bases_dir: &Path, base_name: &str) -> Vec<String> {
    let base_rootfs = bases_dir.join(base_name).join("rootfs");
    let base_canonical = std::fs::canonicalize(&base_rootfs).unwrap_or(base_rootfs);

    let pods = match store.list() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut users = Vec::new();
    for pod in &pods {
        if let Ok(state) = NativeState::from_handle(pod) {
            let rootfs = state.pod_dir.join("rootfs");
            if let Ok(target) = std::fs::read_link(&rootfs) {
                let target_canonical = std::fs::canonicalize(&target).unwrap_or(target);
                if target_canonical == base_canonical {
                    users.push(pod.name.clone());
                }
            }
        }
    }
    users
}

// ---------------------------------------------------------------------------
// queue
// ---------------------------------------------------------------------------

fn cmd_queue(store: &PodStore, name: &str, json: bool, action: Option<QueueAction>) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let queue = ActionQueue::new(&state.pod_dir);

    match action {
        Some(QueueAction::Add { tier, description, delay }) => {
            let tier = parse_tier(&tier)?;
            let queued = queue.submit_with_delay(tier, &description, delay)?;

            let audit_action = if queued.status == envpod_core::queue::ActionStatus::Blocked {
                AuditAction::QueueBlock
            } else {
                AuditAction::QueueSubmit
            };
            ActionQueue::emit_audit(&state.pod_dir, name, audit_action, &queued);

            println!(
                "Queued action {} (tier: {}, status: {})",
                &queued.id.to_string()[..8],
                queued.tier,
                queued.status,
            );
        }
        None => {
            let actions = queue.list(None)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&actions)?);
                return Ok(());
            }

            if actions.is_empty() {
                println!("No queued actions for pod '{name}'");
                return Ok(());
            }

            println!(
                "{:<12} {:<12} {:<12} DESCRIPTION",
                "ID", "TIER", "STATUS"
            );
            println!("{}", "-".repeat(64));
            for a in &actions {
                println!(
                    "{:<12} {:<12} {:<12} {}",
                    &a.id.to_string()[..8],
                    a.tier,
                    a.status,
                    a.description,
                );
            }
            println!("\n{} action(s) for pod '{name}'", actions.len());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// approve
// ---------------------------------------------------------------------------

async fn cmd_approve(store: &PodStore, base_dir: &std::path::Path, name: &str, id_prefix: &str) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let queue = ActionQueue::new(&state.pod_dir);

    let action_id = resolve_action_id(&queue, id_prefix)?;
    let approved = queue.approve(action_id)?;
    ActionQueue::emit_audit(&state.pod_dir, name, AuditAction::QueueApprove, &approved);

    println!(
        "Approved action {} — {}",
        &approved.id.to_string()[..8],
        approved.description,
    );

    // Execute payload if present
    if let Some(ref payload) = approved.payload {
        if let Some(action_type) = payload.get("type").and_then(|v| v.as_str()) {
            match action_type {
                "commit" => {
                    let backend = create_backend(&handle.backend, base_dir)?;
                    let paths: Option<Vec<PathBuf>> = payload
                        .get("paths")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.as_str())
                                .map(PathBuf::from)
                                .collect()
                        });
                    let output: Option<&str> = payload.get("output").and_then(|v| v.as_str());
                    backend.commit(&handle, paths.as_deref(), output.map(Path::new))?;
                    let count = paths.as_ref().map_or(0, |p| p.len());
                    if count > 0 {
                        println!("Committed {count} change(s).");
                    } else {
                        println!("Committed all changes.");
                    }
                }
                "rollback" => {
                    let backend = create_backend(&handle.backend, base_dir)?;
                    backend.rollback(&handle)?;
                    println!("Rolled back.");
                }
                "action_call" => {
                    // Dispatch to built-in action executor
                    let executor = envpod_core::action_types::ActionExecutor::new(&state.pod_dir);
                    let action_name = payload.get("action_type")
                        .and_then(|v| serde_json::from_value::<envpod_core::action_types::ActionType>(v.clone()).ok());
                    let params: std::collections::HashMap<String, serde_json::Value> = payload
                        .get("params")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let config: envpod_core::action_types::ExecutorConfig = payload
                        .get("config")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    match action_name {
                        Some(at) => {
                            let result = executor.execute(&at, &params, &config).await;
                            if result.success {
                                println!("Executed: {}", result.output.as_deref().unwrap_or("done"));
                            } else {
                                println!(
                                    "{} Execution failed: {}",
                                    color::red("✗"),
                                    result.error.as_deref().unwrap_or("unknown error")
                                );
                                if let Some(code) = result.status_code {
                                    println!("  HTTP status: {code}");
                                }
                            }
                            // Mark executed in queue
                            let mut state_reload = queue.load()?;
                            if let Some(entry) = state_reload.actions.iter_mut().find(|a| a.id == approved.id) {
                                entry.status = envpod_core::queue::ActionStatus::Executed;
                                entry.updated_at = chrono::Utc::now();
                            }
                            queue.save(&state_reload)?;
                        }
                        None => {
                            println!("(no action_type in payload — action marked approved, no execution)");
                        }
                    }
                }
                other => {
                    println!("(unknown payload type '{other}' — no action taken)");
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cancel
// ---------------------------------------------------------------------------

fn cmd_cancel(store: &PodStore, name: &str, id_prefix: &str) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let queue = ActionQueue::new(&state.pod_dir);

    let action_id = resolve_action_id(&queue, id_prefix)?;
    let cancelled = queue.cancel(action_id)?;
    ActionQueue::emit_audit(&state.pod_dir, name, AuditAction::QueueCancel, &cancelled);

    println!(
        "Cancelled action {} — {}",
        &cancelled.id.to_string()[..8],
        cancelled.description,
    );
    Ok(())
}

/// Parse a tier string into an ActionTier.
fn parse_tier(s: &str) -> Result<ActionTier> {
    match s.to_lowercase().as_str() {
        "immediate" | "immediate_protected" => Ok(ActionTier::ImmediateProtected),
        "delayed" => Ok(ActionTier::Delayed),
        "staged" => Ok(ActionTier::Staged),
        "blocked" => Ok(ActionTier::Blocked),
        _ => anyhow::bail!(
            "unknown tier '{s}' — expected: immediate, delayed, staged, blocked"
        ),
    }
}

/// Resolve an action ID from a full UUID or a unique prefix (at least 8 chars).
fn resolve_action_id(queue: &ActionQueue, prefix: &str) -> Result<uuid::Uuid> {
    // Try full UUID first
    if let Ok(id) = uuid::Uuid::parse_str(prefix) {
        return Ok(id);
    }

    // Prefix match
    let actions = queue.list(None)?;
    let matches: Vec<_> = actions
        .iter()
        .filter(|a| a.id.to_string().starts_with(prefix))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("no action matching prefix '{prefix}'"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!(
            "ambiguous prefix '{prefix}' — matches {n} actions, use more characters"
        ),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn cmd_status(store: &PodStore, base_dir: &std::path::Path, name: &str, json: bool) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    let info = backend.info(&handle)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("Pod:      {}", info.handle.name);
    println!("ID:       {}", info.handle.id);
    println!("Backend:  {}", info.handle.backend);
    println!("Created:  {}", info.handle.created_at.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Status:   {:?}", info.status);

    if let Some(ref proc) = info.process {
        println!();
        println!("Process:");
        println!("  PID:     {}", proc.pid);
        if !proc.command.is_empty() {
            println!("  Command: {}", proc.command.join(" "));
        }
        println!("  Started: {}", proc.started_at.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    let ru = &info.resource_usage;
    if ru.memory_bytes > 0 || ru.pid_count > 0 || ru.cpu_percent > 0.0 {
        println!();
        println!("Resources:");
        println!("  Memory:  {:.1} MB", ru.memory_bytes as f64 / 1_048_576.0);
        println!("  PIDs:    {}", ru.pid_count);
        println!("  CPU:     {:.1}%", ru.cpu_percent);
    }

    // Network info from backend state
    if let Ok(state) = NativeState::from_handle(&handle) {
        if let Some(ref net) = state.network {
            println!();
            println!("Network:");
            println!("  Namespace: {}", net.netns_name);
            println!("  Pod IP:    {}", net.pod_ip);
            println!("  DNS mode:  {}", net.dns_mode);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

fn cmd_logs(store: &PodStore, name: &str, follow: bool, lines: usize) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let log_path = state.log_path();

    if !log_path.exists() {
        println!("No logs for pod '{name}'");
        return Ok(());
    }

    let content = std::fs::read_to_string(&log_path)
        .with_context(|| format!("read log: {}", log_path.display()))?;

    let all_lines: Vec<&str> = content.lines().collect();

    if lines == 0 || lines >= all_lines.len() {
        for line in &all_lines {
            println!("{line}");
        }
    } else {
        for line in &all_lines[all_lines.len() - lines..] {
            println!("{line}");
        }
    }

    if follow {
        use std::io::Read;
        let mut file = std::fs::File::open(&log_path)?;
        // Seek to end
        file.seek(std::io::SeekFrom::End(0))?;
        let mut buf = [0u8; 4096];
        loop {
            match file.read(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(200)),
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    print!("{text}");
                }
                Err(e) => {
                    eprintln!("log read error: {e}");
                    break;
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

fn cmd_actions(store: &PodStore, name: &str, action: ActionsSubcmd) -> Result<()> {
    use envpod_core::actions::{ActionCatalog, ActionDef, ParamDef};

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let catalog = ActionCatalog::new(&state.pod_dir);

    match action {
        ActionsSubcmd::Ls => {
            let actions = catalog.load()?;
            if actions.is_empty() {
                println!("No actions defined for pod '{name}'.");
                println!("Add one: envpod actions {name} add <name> --description \"...\"");
                return Ok(());
            }
            println!("Actions for pod '{name}':");
            for a in &actions {
                let params_str = if a.params.is_empty() {
                    "(no params)".to_string()
                } else {
                    a.params
                        .iter()
                        .map(|p| {
                            if p.required {
                                format!("{}*", p.name)
                            } else {
                                p.name.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!(
                    "  {}  [{}]  {}  ({})",
                    color::bold(&a.name),
                    a.tier,
                    a.description,
                    color::dim(&params_str),
                );
            }
            println!();
            println!("{}", color::dim("* = required parameter"));
        }

        ActionsSubcmd::Add { name: action_name, description, tier, params } => {
            let tier_parsed = parse_tier(&tier)?;
            let param_defs: Vec<ParamDef> = params
                .iter()
                .map(|p| {
                    if let Some(base) = p.strip_suffix(":required") {
                        ParamDef { name: base.to_string(), description: None, required: true }
                    } else {
                        ParamDef { name: p.clone(), description: None, required: false }
                    }
                })
                .collect();
            let def = ActionDef {
                name: action_name.clone(),
                description,
                tier: tier_parsed,
                params: param_defs,
                action_type: None,
                config: std::collections::HashMap::new(),
            };
            catalog.upsert(def)?;
            println!("Action '{}' added to pod '{}'.", action_name, name);
            println!(
                "{}",
                color::dim("Agents will see it immediately on next list_actions call.")
            );
        }

        ActionsSubcmd::Remove { name: action_name } => {
            let removed = catalog.remove(&action_name)?;
            if removed {
                println!("Removed action '{}' from pod '{}'.", action_name, name);
            } else {
                anyhow::bail!("action '{}' not found in pod '{}'", action_name, name);
            }
        }

        ActionsSubcmd::SetTier { name: action_name, tier } => {
            let new_tier = parse_tier(&tier)?;
            let mut def = catalog
                .get(&action_name)?
                .with_context(|| format!("action '{}' not found in pod '{}'", action_name, name))?;
            let old_tier = def.tier;
            def.tier = new_tier;
            catalog.upsert(def)?;
            println!(
                "Action '{}': tier changed {} → {}.",
                action_name, old_tier, new_tier,
            );
            println!(
                "{}",
                color::dim("Takes effect immediately — no pod restart needed.")
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// vault
// ---------------------------------------------------------------------------

fn cmd_vault(store: &PodStore, name: &str, action: VaultAction) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let vault = envpod_core::vault::Vault::new(&state.pod_dir)?;

    match action {
        VaultAction::Set { key } => {
            use std::io::Read;
            eprintln!("Enter value for '{key}' (then press Ctrl+D):");
            let mut value = String::new();
            std::io::stdin().read_to_string(&mut value)
                .context("read value from stdin")?;
            let value = value.trim_end_matches('\n');
            vault.set(&key, value)?;
            let _ = vault.refresh_env_file(&state.pod_dir);  // keep live file in sync

            let log = AuditLog::new(&state.pod_dir);
            log.append(&AuditEntry {
                timestamp: chrono::Utc::now(),
                pod_name: name.into(),
                action: AuditAction::VaultSet,
                detail: format!("key={key}"),
                success: true,
            })?;

            println!("Set vault key '{key}' in pod '{name}'");
        }
        VaultAction::Get { key } => {
            match vault.get(&key)? {
                Some(value) => {
                    let log = AuditLog::new(&state.pod_dir);
                    log.append(&AuditEntry {
                        timestamp: chrono::Utc::now(),
                        pod_name: name.into(),
                        action: AuditAction::VaultGet,
                        detail: format!("key={key}"),
                        success: true,
                    })?;
                    println!("{value}");
                }
                None => {
                    anyhow::bail!("key '{key}' not found in vault");
                }
            }
        }
        VaultAction::List => {
            let keys = vault.list()?;
            if keys.is_empty() {
                println!("No secrets in vault for pod '{name}'");
            } else {
                for key in &keys {
                    println!("{key}");
                }
                println!("\n{} secret(s) in pod '{name}'", keys.len());
            }
        }
        VaultAction::Rm { key } => {
            vault.remove(&key)?;
            let _ = vault.refresh_env_file(&state.pod_dir);  // keep live file in sync

            let log = AuditLog::new(&state.pod_dir);
            log.append(&AuditEntry {
                timestamp: chrono::Utc::now(),
                pod_name: name.into(),
                action: AuditAction::VaultRemove,
                detail: format!("key={key}"),
                success: true,
            })?;

            println!("Removed vault key '{key}' from pod '{name}'");
        }
        VaultAction::Import { path, overwrite } => {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("read .env file: {}", path.display()))?;
            let parsed = envpod_core::vault::parse_env_file(&content)?;
            let existing = vault.list()?;
            let mut added = 0usize;
            let mut skipped = 0usize;
            for (key, value) in &parsed {
                if !overwrite && existing.contains(key) {
                    skipped += 1;
                    continue;
                }
                vault.set(key, value)?;
                added += 1;
            }
            let _ = vault.refresh_env_file(&state.pod_dir);
            let log = AuditLog::new(&state.pod_dir);
            let _ = log.append(&AuditEntry {
                timestamp: chrono::Utc::now(),
                pod_name: name.into(),
                action: AuditAction::VaultSet,
                detail: format!("import {} key(s) from {}", added, path.display()),
                success: true,
            });
            println!("{} Imported {added} secret(s) into pod '{name}'", color::green("✓"));
            if skipped > 0 {
                println!("  Skipped {skipped} existing key(s) (use --overwrite to replace)");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// remote
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// resize
// ---------------------------------------------------------------------------

/// Shared config mutation for resize (used by both pod resize and base resize).
/// Validates inputs, mutates the PodConfig on disk, returns a summary string.
fn apply_config_resize(
    config_path: &Path,
    cpus: Option<f64>,
    memory: Option<&str>,
    tmp_size: Option<&str>,
    disk_size: Option<&str>,
    max_pids: Option<u32>,
    cpu_affinity: Option<&str>,
    gpu: Option<bool>,
    display: Option<bool>,
    audio: Option<bool>,
    desktop: Option<&str>,
    web_display: Option<&str>,
) -> Result<String> {
    use envpod_core::config::{parse_memory_string, DesktopEnv, WebDisplayType};

    // Validate at least one option
    if cpus.is_none() && memory.is_none() && tmp_size.is_none() && disk_size.is_none()
        && max_pids.is_none() && cpu_affinity.is_none() && gpu.is_none()
        && display.is_none() && audio.is_none() && desktop.is_none() && web_display.is_none()
    {
        anyhow::bail!("nothing to resize — specify at least one option");
    }

    // Validate values
    if let Some(m) = memory {
        if parse_memory_string(m).is_none() {
            anyhow::bail!("invalid memory value: '{m}' (expected e.g. 512MB, 2GB)");
        }
    }
    if let Some(t) = tmp_size {
        if parse_memory_string(t).is_none() {
            anyhow::bail!("invalid tmp-size value: '{t}' (expected e.g. 512MB, 2GB)");
        }
    }
    if let Some(d) = disk_size {
        if parse_memory_string(d).is_none() {
            anyhow::bail!("invalid disk-size value: '{d}' (expected e.g. 1GB, 10GB)");
        }
    }
    if let Some(d) = desktop {
        match d {
            "none" | "xfce" | "openbox" | "sway" => {}
            _ => anyhow::bail!("invalid desktop: '{d}' (expected: none, xfce, openbox, sway)"),
        }
    }
    if let Some(w) = web_display {
        match w {
            "none" | "novnc" => {}
            _ => anyhow::bail!("invalid web-display: '{w}' (expected: none, novnc)"),
        }
    }

    let mut config = PodConfig::from_file(config_path)?;
    let mut changes = Vec::new();

    if let Some(c) = cpus {
        config.processor.cores = Some(c);
        changes.push(format!("cpus: {c}"));
    }
    if let Some(m) = memory {
        config.processor.memory = Some(m.to_string());
        changes.push(format!("memory: {m}"));
    }
    if let Some(t) = tmp_size {
        config.processor.tmp_size = Some(t.to_string());
        changes.push(format!("tmp_size: {t}"));
    }
    if let Some(d) = disk_size {
        config.processor.disk_size = Some(d.to_string());
        changes.push(format!("disk_size: {d}"));
    }
    if let Some(p) = max_pids {
        config.processor.max_pids = Some(p);
        changes.push(format!("max_pids: {p}"));
    }
    if let Some(a) = cpu_affinity {
        config.processor.cpu_affinity = Some(a.to_string());
        changes.push(format!("cpu_affinity: {a}"));
    }
    if let Some(g) = gpu {
        config.devices.gpu = g;
        changes.push(format!("gpu: {g}"));
    }
    if let Some(d) = display {
        config.devices.display = d;
        changes.push(format!("display: {d}"));
    }
    if let Some(a) = audio {
        config.devices.audio = a;
        changes.push(format!("audio: {a}"));
    }
    if let Some(d) = desktop {
        config.devices.desktop_env = match d {
            "xfce" => DesktopEnv::Xfce,
            "openbox" => DesktopEnv::Openbox,
            "sway" => DesktopEnv::Sway,
            _ => DesktopEnv::None,
        };
        changes.push(format!("desktop: {d}"));
    }
    if let Some(w) = web_display {
        config.web_display.display_type = match w {
            "novnc" => WebDisplayType::Novnc,
            _ => WebDisplayType::None,
        };
        changes.push(format!("web_display: {w}"));
    }

    let yaml = serde_yaml::to_string(&config).context("serialize pod.yaml")?;
    std::fs::write(config_path, yaml)
        .with_context(|| format!("write {}", config_path.display()))?;

    Ok(changes.join(", "))
}

fn cmd_resize(
    store: &PodStore,
    base_dir: &Path,
    name: &str,
    cpus: Option<f64>,
    memory: Option<&str>,
    tmp_size: Option<&str>,
    disk_size: Option<&str>,
    max_pids: Option<u32>,
    cpu_affinity: Option<&str>,
    gpu: Option<bool>,
    display: Option<bool>,
    audio: Option<bool>,
    desktop: Option<&str>,
    web_display: Option<&str>,
) -> Result<()> {
    use envpod_core::config::parse_memory_string;
    use envpod_core::types::ResourceLimits;

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let is_running = state.status == NativeStatus::Running;

    // Device/display changes require a stopped pod (bind mounts set at start)
    let device_changes = gpu.is_some() || display.is_some() || audio.is_some() || desktop.is_some() || web_display.is_some();
    if device_changes && is_running {
        anyhow::bail!("--gpu, --display, --audio, --desktop, and --web-display can only be changed on a stopped pod");
    }

    // --- 1. Apply live cgroup changes if pod is running ---
    let mut live_changes = Vec::new();

    if is_running && (cpus.is_some() || memory.is_some() || max_pids.is_some() || cpu_affinity.is_some()) {
        let limits = ResourceLimits {
            cpu_cores: cpus,
            memory_bytes: memory.and_then(parse_memory_string),
            disk_bytes: disk_size.and_then(parse_memory_string),
            max_pids,
            cpuset_cpus: cpu_affinity.map(|s| s.to_string()),
        };
        let backend = NativeBackend::new(base_dir)?;
        backend.set_limits(&handle, &limits)?;

        if let Some(c) = cpus { live_changes.push(format!("cpus → {c}")); }
        if let Some(m) = memory { live_changes.push(format!("memory → {m}")); }
        if max_pids.is_some() { live_changes.push(format!("max_pids → {}", max_pids.unwrap())); }
        if let Some(a) = cpu_affinity { live_changes.push(format!("cpu_affinity → {a}")); }
    }

    // Live tmpfs remount if running
    if is_running {
        if let Some(t) = tmp_size {
            if let Some(bytes) = parse_memory_string(t) {
                let merged = state.pod_dir.join("merged");
                let tmp_path = merged.join("tmp");
                if tmp_path.exists() {
                    std::process::Command::new("mount")
                        .args(["-o", &format!("remount,size={bytes}"), tmp_path.to_str().unwrap()])
                        .status()
                        .context("remount /tmp")?;
                    live_changes.push(format!("tmp_size → {t}"));
                }
            }
        }
    }

    // --- 2. Update pod.yaml (persists across restarts) ---
    let config_path = state.pod_dir.join("pod.yaml");
    let detail = apply_config_resize(
        &config_path, cpus, memory, tmp_size, disk_size,
        max_pids, cpu_affinity, gpu, display, audio, desktop, web_display,
    )?;

    // --- 3. Audit ---
    let log = AuditLog::new(&state.pod_dir);
    log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.to_string(),
        action: AuditAction::Resize,
        detail: detail.clone(),
        success: true,
    })?;

    // --- 4. Print summary ---
    if !live_changes.is_empty() {
        eprintln!("  {} applied live: {}", color::green("✓"), live_changes.join(", "));
    }
    eprintln!("  {} config updated: {}", color::green("✓"), detail);
    if !is_running && (cpus.is_some() || memory.is_some() || max_pids.is_some() || cpu_affinity.is_some()) {
        eprintln!("  {} changes take effect on next start", color::dim("i"));
    }
    if device_changes {
        eprintln!("  {} device/display changes take effect on next start", color::dim("i"));
    }

    Ok(())
}

async fn cmd_remote(store: &PodStore, name: &str, cmd: &str, payload: Option<&str>) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let socket_path = state.pod_dir.join("control.sock");

    if !socket_path.exists() {
        anyhow::bail!(
            "control socket not found — is pod '{name}' running? (expected: {})",
            socket_path.display()
        );
    }

    let stream = tokio::net::UnixStream::connect(&socket_path)
        .await
        .with_context(|| format!("connect to control socket: {}", socket_path.display()))?;

    let (reader, mut writer) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(reader).lines();

    // Build command line
    let line = match payload {
        Some(p) => format!("{cmd} {p}\n"),
        None => format!("{cmd}\n"),
    };

    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;

    if let Some(response_line) = lines.next_line().await? {
        // Try to pretty-print as JSON
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response_line) {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            println!("{response_line}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// monitor
// ---------------------------------------------------------------------------

fn cmd_monitor(store: &PodStore, name: &str, action: MonitorAction) -> Result<()> {
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;

    match action {
        MonitorAction::SetPolicy { path } => {
            // Validate the policy file
            let policy = MonitorPolicy::from_file(&path)?;
            println!(
                "Validated policy: {} rule(s), check_interval={}s",
                policy.rules.len(),
                policy.check_interval_secs
            );

            // Copy into pod dir
            let dest = state.pod_dir.join("monitoring-policy.yaml");
            std::fs::copy(&path, &dest).with_context(|| {
                format!(
                    "copy {} → {}",
                    path.display(),
                    dest.display()
                )
            })?;
            println!(
                "Installed monitoring policy for pod '{name}' → {}",
                dest.display()
            );
        }
        MonitorAction::Alerts { json } => {
            let log = AuditLog::new(&state.pod_dir);
            let entries = log.read_all()?;

            let alerts: Vec<&AuditEntry> = entries
                .iter()
                .filter(|e| {
                    matches!(
                        e.action,
                        AuditAction::MonitorAlert
                            | AuditAction::MonitorFreeze
                            | AuditAction::MonitorRestrict
                    )
                })
                .collect();

            if json {
                println!("{}", serde_json::to_string_pretty(&alerts)?);
                return Ok(());
            }

            if alerts.is_empty() {
                println!("No monitor alerts for pod '{name}'");
                return Ok(());
            }

            println!(
                "{:<24} {:<18} DETAIL",
                "TIMESTAMP", "ACTION"
            );
            println!("{}", "-".repeat(72));
            for e in &alerts {
                println!(
                    "{:<24} {:<18} {}",
                    e.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                    e.action,
                    e.detail,
                );
            }
            println!("\n{} alert(s) for pod '{name}'", alerts.len());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// mount
// ---------------------------------------------------------------------------

fn cmd_mount(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    host_path: &std::path::Path,
    target: Option<&std::path::Path>,
    readonly: bool,
) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    let state = NativeState::from_handle(&handle)?;

    let permission = if readonly {
        MountPermission::ReadOnly
    } else {
        MountPermission::ReadWrite
    };

    let mount_config = MountConfig {
        host_path: host_path.to_path_buf(),
        pod_path: target.map(|p| p.to_path_buf()),
        permission,
    };

    backend.mount(&handle, &mount_config)?;

    // Audit
    let pod_path = target.unwrap_or(host_path);
    let log = AuditLog::new(&state.pod_dir);
    log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.into(),
        action: AuditAction::Mount,
        detail: format!(
            "{} -> {} ({})",
            host_path.display(),
            pod_path.display(),
            if readonly { "ro" } else { "rw" },
        ),
        success: true,
    })?;

    // Register undo entry
    let registry = UndoRegistry::new(&state.pod_dir);
    registry.register(
        &format!("mount {} -> {}", host_path.display(), pod_path.display()),
        UndoMechanism::Unmount {
            path: pod_path.to_path_buf(),
        },
    )?;

    println!(
        "Mounted {} -> {} in pod '{name}' ({})",
        host_path.display(),
        pod_path.display(),
        if readonly { "read-only" } else { "read-write" },
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// unmount
// ---------------------------------------------------------------------------

fn cmd_unmount(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    path: &std::path::Path,
) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    let state = NativeState::from_handle(&handle)?;

    backend.unmount(&handle, path)?;

    // Audit
    let log = AuditLog::new(&state.pod_dir);
    log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: name.into(),
        action: AuditAction::Unmount,
        detail: format!("{}", path.display()),
        success: true,
    })?;

    println!("Unmounted {} from pod '{name}'", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// undo
// ---------------------------------------------------------------------------

fn cmd_undo(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    id_prefix: Option<&str>,
    all: bool,
) -> Result<()> {
    let handle = store.load(name)?;
    let backend = create_backend(&handle.backend, base_dir)?;
    let state = NativeState::from_handle(&handle)?;
    let registry = UndoRegistry::new(&state.pod_dir);

    if all {
        // Undo all pending
        let pending = registry.list_pending()?;
        if pending.is_empty() {
            println!("No pending undo actions for pod '{name}'");
            return Ok(());
        }
        let mut count = 0usize;
        for entry in &pending {
            execute_undo(&registry, &state, &*backend, &handle, name, entry.id)?;
            count += 1;
        }
        println!("Undid {count} action(s) in pod '{name}'");
    } else if let Some(prefix) = id_prefix {
        // Undo specific action
        let id = resolve_undo_id(&registry, prefix)?;
        execute_undo(&registry, &state, &*backend, &handle, name, id)?;
    } else {
        // List mode
        let pending = registry.list_pending()?;
        if pending.is_empty() {
            println!("No pending undo actions for pod '{name}'");
            return Ok(());
        }

        println!(
            "{:<12} {:<24} DESCRIPTION",
            "ID", "TIMESTAMP"
        );
        println!("{}", "-".repeat(64));
        for entry in &pending {
            println!(
                "{:<12} {:<24} {}",
                &entry.id.to_string()[..8],
                entry.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
                entry.description,
            );
        }
        println!("\n{} undo-able action(s) for pod '{name}'", pending.len());
    }

    Ok(())
}

/// Execute a single undo action: apply the mechanism, mark undone, audit.
fn execute_undo(
    registry: &UndoRegistry,
    state: &NativeState,
    backend: &dyn envpod_core::backend::IsolationBackend,
    handle: &envpod_core::types::PodHandle,
    pod_name: &str,
    id: uuid::Uuid,
) -> Result<()> {
    let entry = registry.mark_undone(id)?;

    match &entry.mechanism {
        UndoMechanism::Unmount { path } => {
            backend.unmount(handle, path)?;
        }
        UndoMechanism::Rollback => {
            backend.rollback(handle)?;
        }
        UndoMechanism::Thaw => {
            backend.resume(handle)?;
        }
        UndoMechanism::RestoreLimits { limits } => {
            backend.set_limits(handle, limits)?;
        }
    }

    // Audit
    let log = AuditLog::new(&state.pod_dir);
    log.append(&AuditEntry {
        timestamp: chrono::Utc::now(),
        pod_name: pod_name.into(),
        action: AuditAction::Undo,
        detail: format!("{} ({})", entry.description, &entry.id.to_string()[..8]),
        success: true,
    })?;

    println!(
        "Undid: {} ({})",
        entry.description,
        &entry.id.to_string()[..8],
    );
    Ok(())
}

/// Resolve an undo entry ID from a full UUID or unique prefix.
fn resolve_undo_id(registry: &UndoRegistry, prefix: &str) -> Result<uuid::Uuid> {
    // Try full UUID first
    if let Ok(id) = uuid::Uuid::parse_str(prefix) {
        return Ok(id);
    }

    // Prefix match against pending entries
    let pending = registry.list_pending()?;
    let matches: Vec<_> = pending
        .iter()
        .filter(|e| e.id.to_string().starts_with(prefix))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("no pending undo action matching prefix '{prefix}'"),
        1 => Ok(matches[0].id),
        n => anyhow::bail!(
            "ambiguous prefix '{prefix}' — matches {n} actions, use more characters"
        ),
    }
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

fn cmd_ls(store: &PodStore, base_dir: &std::path::Path, json: bool) -> Result<()> {
    let pods = store.list()?;

    // Resolve base name, status, pod IP, and noVNC URL for each pod
    struct PodInfo {
        base: Option<String>,
        status: String,
        ip: String,
        novnc_url: Option<String>,
    }
    let pod_info: Vec<PodInfo> = pods
        .iter()
        .map(|h| {
            let state_opt = NativeState::from_handle(h).ok();
            let base = state_opt.as_ref()
                .and_then(|state| resolve_base_name(&state.pod_dir));
            let (status, ip) = if let Ok(backend) = create_backend(&h.backend, base_dir) {
                if let Ok(info) = backend.info(h) {
                    let s = format!("{:?}", info.status).to_lowercase();
                    let ip = state_opt.as_ref()
                        .and_then(|st| st.network.as_ref().map(|n| n.pod_ip.clone()))
                        .unwrap_or_else(|| "-".into());
                    (s, ip)
                } else {
                    ("unknown".into(), "-".into())
                }
            } else {
                ("unknown".into(), "-".into())
            };
            // Check for web_display config to build noVNC URL
            let novnc_url = state_opt.as_ref()
                .and_then(|st| {
                    std::fs::read_to_string(st.config_path()).ok()
                })
                .and_then(|yaml| {
                    serde_yaml::from_str::<envpod_core::config::PodConfig>(&yaml).ok()
                })
                .and_then(|cfg| {
                    if cfg.web_display.display_type != envpod_core::config::WebDisplayType::None {
                        Some(format!("http://{}:{}/vnc.html", ip, cfg.web_display.port))
                    } else {
                        None
                    }
                });
            PodInfo { base, status, ip, novnc_url }
        })
        .collect();

    if json {
        let json_pods: Vec<serde_json::Value> = pods
            .iter()
            .zip(pod_info.iter())
            .map(|(h, info)| {
                let mut obj = serde_json::json!({
                    "name": h.name,
                    "backend": h.backend,
                    "id": h.id,
                    "created_at": h.created_at,
                    "base": info.base.as_deref().unwrap_or(""),
                    "status": info.status,
                    "ip": info.ip,
                });
                if let Some(ref url) = info.novnc_url {
                    obj["novnc"] = serde_json::Value::String(url.clone());
                }
                obj
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_pods)?);
        return Ok(());
    }

    if pods.is_empty() {
        println!("No pods");
        return Ok(());
    }

    // Check if any pod has noVNC — if so, show the URL column
    let has_novnc = pod_info.iter().any(|i| i.novnc_url.is_some());

    if has_novnc {
        println!(
            "{:<20} {:<10} {:<16} {:<16} {}",
            "NAME", "STATUS", "IP", "BASE", "DISPLAY"
        );
        println!("{}", "-".repeat(100));
        for (handle, info) in pods.iter().zip(pod_info.iter()) {
            let base_display = info.base.as_deref().unwrap_or("-");
            let novnc = info.novnc_url.as_deref().unwrap_or("");
            println!(
                "{:<20} {:<10} {:<16} {:<16} {}",
                handle.name,
                info.status,
                info.ip,
                base_display,
                novnc,
            );
        }
    } else {
        println!(
            "{:<20} {:<10} {:<16} {:<16}",
            "NAME", "STATUS", "IP", "BASE"
        );
        println!("{}", "-".repeat(62));
        for (handle, info) in pods.iter().zip(pod_info.iter()) {
            let base_display = info.base.as_deref().unwrap_or("-");
            println!(
                "{:<20} {:<10} {:<16} {:<16}",
                handle.name,
                info.status,
                info.ip,
                base_display,
            );
        }
    }
    println!("\n{} pod(s)", pods.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// dns — live DNS policy update
// ---------------------------------------------------------------------------

async fn cmd_dns(
    store: &PodStore,
    name: &str,
    allow: &[String],
    deny: &[String],
    remove_allow: &[String],
    remove_deny: &[String],
) -> Result<()> {
    if allow.is_empty() && deny.is_empty() && remove_allow.is_empty() && remove_deny.is_empty() {
        anyhow::bail!(
            "specify at least one of --allow, --deny, --remove-allow, --remove-deny"
        );
    }

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;

    // Load or create network state
    let mut net = envpod_core::backend::native::state::NetworkState::load(&state.pod_dir)?
        .or_else(|| state.network.clone())
        .with_context(|| format!("pod '{name}' has no network state"))?;

    // Update the DNS lists
    net.update_dns_lists(allow, deny, remove_allow, remove_deny);
    net.save(&state.pod_dir)?;

    // Report what changed
    if !allow.is_empty() {
        println!("Added to allow list: {}", allow.join(", "));
    }
    if !deny.is_empty() {
        println!("Added to deny list: {}", deny.join(", "));
    }
    if !remove_allow.is_empty() {
        println!("Removed from allow list: {}", remove_allow.join(", "));
    }
    if !remove_deny.is_empty() {
        println!("Removed from deny list: {}", remove_deny.join(", "));
    }

    // Send dns-reload to control socket if pod is running
    let socket_path = state.pod_dir.join("control.sock");
    if socket_path.exists() {
        let stream = tokio::net::UnixStream::connect(&socket_path)
            .await
            .with_context(|| "connect to control socket for dns-reload")?;
        let (reader, mut writer) = stream.into_split();
        let mut lines = tokio::io::BufReader::new(reader).lines();

        writer.write_all(b"dns-reload\n").await?;
        writer.flush().await?;

        if let Some(response_line) = lines.next_line().await? {
            let resp: envpod_core::remote::ControlResponse = serde_json::from_str(&response_line)
                .unwrap_or(envpod_core::remote::ControlResponse {
                    ok: false,
                    message: response_line.clone(),
                    data: None,
                });
            if resp.ok {
                println!("DNS policy reloaded on running pod '{name}'");
            } else {
                eprintln!("warning: dns-reload failed: {}", resp.message);
            }
        }
    } else {
        println!("State saved. DNS policy will apply on next pod start.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// ports
// ---------------------------------------------------------------------------

fn cmd_ports(
    store: &PodStore,
    name: &str,
    add_publish: &[String],
    add_publish_all: &[String],
    add_internal: &[String],
    remove: &[String],
    remove_internal: &[String],
) -> Result<()> {
    use envpod_core::backend::native::{
        add_port_forward, remove_port_forward,
        add_internal_port, remove_internal_port,
        read_active_ports,
    };
    use envpod_core::backend::native::state::NativeState;

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let net = state.network.as_ref()
        .with_context(|| format!("pod '{name}' has no network state (not yet run?)"))?;

    let is_mutation = !add_publish.is_empty()
        || !add_publish_all.is_empty()
        || !add_internal.is_empty()
        || !remove.is_empty()
        || !remove_internal.is_empty();

    if !is_mutation {
        // Status display
        let (forwards, internals) = read_active_ports(&state.pod_dir);
        if forwards.is_empty() && internals.is_empty() {
            println!("No active port forwards for pod '{name}'.");
            return Ok(());
        }
        if !forwards.is_empty() {
            println!("Port forwards (-p / -P):");
            for r in &forwards {
                let host_port = r["host_port"].as_u64().unwrap_or(0);
                let cont_port = r["container_port"].as_u64().unwrap_or(0);
                let proto     = r["proto"].as_str().unwrap_or("tcp");
                let host_only = r["host_only"].as_bool().unwrap_or(false);
                let scope = if host_only { "localhost" } else { "public" };
                println!("  {proto}/{host_port} → {cont_port}  ({scope})");
            }
        }
        if !internals.is_empty() {
            println!("Internal ports (-i):");
            for r in &internals {
                let port  = r["container_port"].as_u64().unwrap_or(0);
                let proto = r["proto"].as_str().unwrap_or("tcp");
                let subnet = r["pod_subnet"].as_str().unwrap_or("?");
                println!("  {proto}/{port}  (pod-to-pod, src {subnet})");
            }
        }
        return Ok(());
    }

    // --- Mutations ---

    // localhost-only forwards (spec prefix will be "127.0.0.1:" for host_only detection)
    for spec in add_publish {
        // Normalise: prepend "127.0.0.1:" so parse_port_spec sets host_only=true
        let normalised = format!("127.0.0.1:{spec}");
        add_port_forward(&state.pod_dir, &net.host_veth, &net.pod_ip, &normalised)
            .with_context(|| format!("add localhost port forward '{spec}'"))?;
        println!("Added localhost port forward: {spec} (host:{} → pod:{})",
            spec.split(':').next().unwrap_or(spec), spec.split(':').nth(1).unwrap_or(spec));
    }

    // public (all-interfaces) forwards
    for spec in add_publish_all {
        add_port_forward(&state.pod_dir, &net.host_veth, &net.pod_ip, spec)
            .with_context(|| format!("add public port forward '{spec}'"))?;
        println!("Added public port forward: {spec}");
    }

    // internal (pod-to-pod) ports
    for spec in add_internal {
        add_internal_port(&state.pod_dir, &net.pod_ip, &net.subnet_base, spec)
            .with_context(|| format!("add internal port '{spec}'"))?;
        println!("Added internal port: {spec}");
    }

    // remove port forwards by host port
    for spec in remove {
        remove_port_forward(&state.pod_dir, spec)
            .with_context(|| format!("remove port forward '{spec}'"))?;
        println!("Removed port forward: {spec}");
    }

    // remove internal ports by container port
    for spec in remove_internal {
        remove_internal_port(&state.pod_dir, spec)
            .with_context(|| format!("remove internal port '{spec}'"))?;
        println!("Removed internal port: {spec}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// discover
// ---------------------------------------------------------------------------

async fn cmd_discover(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    on: bool,
    off: bool,
    add_pods: &[String],
    remove_pods: &[String],
) -> Result<()> {
    use envpod_core::dns_daemon::DaemonClient;

    let is_mutation = on || off || !add_pods.is_empty() || !remove_pods.is_empty();
    let daemon = DaemonClient::new(DaemonClient::default_path());

    if !is_mutation {
        // Status query
        match daemon.query_discovery(name).await {
            Ok(resp) if resp.error.is_none() => {
                let disc = resp.allow_discovery.unwrap_or(false);
                let pods = resp.allow_pods.unwrap_or_default();
                let ip = resp.ip.unwrap_or_else(|| "?".to_string());
                println!(
                    "Pod:              {name}\nIP:               {ip}\nAllow discovery:  {}\nAllow pods:       {}",
                    if disc { color::green("yes") } else { color::dim("no") },
                    if pods.is_empty() { color::dim("(none)") } else { pods.join(", ") },
                );
            }
            Ok(resp) => {
                // daemon running but pod not registered (not started yet, or never registered)
                eprintln!("warning: {}", resp.error.unwrap_or_default());
                eprintln!("(show pod.yaml values instead)");
                show_config_discovery(store, base_dir, name)?;
            }
            Err(_) => {
                // daemon not running — fall back to pod.yaml
                eprintln!("envpod-dns not running. Showing pod.yaml values:");
                show_config_discovery(store, base_dir, name)?;
            }
        }
        return Ok(());
    }

    // --- Mutation ---

    // 1. Determine new allow_discovery flag (None = unchanged)
    let allow_discovery = if on { Some(true) } else if off { Some(false) } else { None };

    // 2. Send update to daemon (takes effect immediately if pod is running)
    let daemon_ok = match daemon.update_discovery(name, allow_discovery, add_pods, remove_pods).await {
        Ok(resp) if resp.error.is_none() => {
            let disc = resp.allow_discovery.unwrap_or(false);
            let pods = resp.allow_pods.clone().unwrap_or_default();
            if on  { println!("Discovery {}  {} is now discoverable as {}.pods.local", color::green("enabled:"), name, name); }
            if off { println!("Discovery {}  {} is no longer discoverable", color::yellow("disabled:"), name); }
            for p in add_pods { println!("Added to allow_pods:    {p}"); }
            for p in remove_pods { println!("Removed from allow_pods: {p}"); }
            println!("Live state → allow_discovery={disc}, allow_pods=[{}]", pods.join(", "));
            true
        }
        Ok(resp) => {
            eprintln!("warning: daemon error: {}", resp.error.unwrap_or_default());
            eprintln!("(pod may not be running — updating pod.yaml only)");
            false
        }
        Err(e) => {
            eprintln!("warning: envpod-dns not running ({e:#}) — updating pod.yaml only");
            false
        }
    };

    // 3. Persist the change to pod.yaml so it survives pod restarts
    patch_config_discovery(store, base_dir, name, allow_discovery, add_pods, remove_pods)?;
    if !daemon_ok {
        println!("pod.yaml updated. Changes will apply when the pod next starts.");
    }

    Ok(())
}

/// Print discovery settings from pod.yaml (fallback when daemon is not running).
fn show_config_discovery(store: &PodStore, base_dir: &std::path::Path, name: &str) -> Result<()> {
    use envpod_core::backend::native::state::NativeState;
    use envpod_core::config::PodConfig;
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let config = PodConfig::from_file(&state.pod_dir.join("pod.yaml")).unwrap_or_default();
    println!(
        "Pod:              {name}\nAllow discovery:  {}\nAllow pods:       {}",
        if config.network.allow_discovery { color::green("yes") } else { color::dim("no") },
        if config.network.allow_pods.is_empty() {
            color::dim("(none)")
        } else {
            config.network.allow_pods.join(", ")
        },
    );
    let _ = base_dir; // unused but keeps signature symmetric with other cmds
    Ok(())
}

/// Apply discovery mutations to pod.yaml on disk.
fn patch_config_discovery(
    store: &PodStore,
    base_dir: &std::path::Path,
    name: &str,
    allow_discovery: Option<bool>,
    add_pods: &[String],
    remove_pods: &[String],
) -> Result<()> {
    use envpod_core::backend::native::state::NativeState;
    use envpod_core::config::PodConfig;
    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let config_path = state.pod_dir.join("pod.yaml");
    let mut config = PodConfig::from_file(&config_path).unwrap_or_default();

    if let Some(v) = allow_discovery {
        config.network.allow_discovery = v;
    }
    if remove_pods.iter().any(|p| p == "*") {
        config.network.allow_pods.clear();
    } else {
        config.network.allow_pods.retain(|p| !remove_pods.contains(p));
    }
    for p in add_pods {
        if !config.network.allow_pods.contains(p) {
            config.network.allow_pods.push(p.clone());
        }
    }

    let yaml = serde_yaml::to_string(&config).context("serialize pod.yaml")?;
    std::fs::write(&config_path, yaml)
        .with_context(|| format!("write {}", config_path.display()))?;

    let _ = base_dir;
    Ok(())
}

// ---------------------------------------------------------------------------
// dns-daemon
// ---------------------------------------------------------------------------

async fn cmd_dns_daemon(base_dir: &std::path::Path, socket: Option<PathBuf>) -> Result<()> {
    use envpod_core::dns_daemon::{DnsDaemon, DAEMON_SOCK};

    let sock_path = socket.unwrap_or_else(|| PathBuf::from(DAEMON_SOCK));
    eprintln!("envpod-dns: starting on {}", sock_path.display());

    let daemon = DnsDaemon::new(sock_path);

    // Recover any pods registered before this daemon instance started
    daemon.load_persisted().await;
    // Also scan the pod store for running pods that couldn't register
    // (started before the daemon was up) — no pod restart required
    daemon.load_from_store(base_dir).await;

    let handle = daemon.spawn().await?;

    // Wait for Ctrl-C then shut down
    tokio::signal::ctrl_c().await.ok();
    eprintln!("envpod-dns: shutting down");
    handle.shutdown();
    handle.join().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// snapshot
// ---------------------------------------------------------------------------

fn cmd_snapshot(store: &PodStore, base_dir: &std::path::Path, name: &str, action: SnapshotAction) -> Result<()> {
    use envpod_core::snapshot::SnapshotStore;

    let handle = store.load(name)?;
    let state = NativeState::from_handle(&handle)?;
    let snap_store = SnapshotStore::new(&state.pod_dir);
    let upper_dir = state.pod_dir.join("upper");

    match action {
        SnapshotAction::Create { name: label } => {
            let snap = snap_store.create(&upper_dir, label.as_deref(), false)?;
            let size_display = format_bytes(snap.size_bytes);
            println!(
                "{} snapshot created: {} ({} file{}, {})",
                color::green("✓"),
                color::bold(&snap.id),
                snap.file_count,
                if snap.file_count == 1 { "" } else { "s" },
                size_display,
            );
            if let Some(ref n) = snap.name {
                println!("  Label: {n}");
            }
            Ok(())
        }
        SnapshotAction::Ls => {
            let snapshots = snap_store.list()?;
            if snapshots.is_empty() {
                println!("No snapshots for pod '{name}'");
                return Ok(());
            }
            println!("{:<10}  {:<20}  {:<22}  {:>7}  {:>8}  {}", "ID", "LABEL", "TIMESTAMP", "FILES", "SIZE", "AUTO");
            println!("{}", "-".repeat(82));
            for s in &snapshots {
                let label = s.name.as_deref().unwrap_or("-");
                let ts = s.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string();
                let size = format_bytes(s.size_bytes);
                let auto_flag = if s.auto { color::dim("auto") } else { "     ".to_string() };
                println!("{:<10}  {:<20}  {:<22}  {:>7}  {:>8}  {}", s.id, label, ts, s.file_count, size, auto_flag);
            }
            Ok(())
        }
        SnapshotAction::Restore { id, yes } => {
            let snap = snap_store.get(&id)?;
            if !yes {
                eprintln!(
                    "{} This will replace the current overlay with snapshot {} ({})",
                    color::yellow("⚠"),
                    color::bold(&snap.id),
                    snap.display_name(),
                );
                eprintln!("  Current unsaved changes will be permanently lost.");
                eprint!("  Type 'yes' to continue: ");
                use std::io::BufRead;
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                if line.trim() != "yes" {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            snap_store.restore(&upper_dir, &id)?;
            println!("{} Restored snapshot {} ({})", color::green("✓"), color::bold(&snap.id), snap.display_name());
            Ok(())
        }
        SnapshotAction::Destroy { id } => {
            let meta = snap_store.destroy(&id)?;
            println!("{} Deleted snapshot {} ({})", color::green("✓"), color::bold(&meta.id), meta.display_name());
            Ok(())
        }
        SnapshotAction::Prune => {
            let pod_config = state.load_config()?;
            let max_keep = pod_config.as_ref().map(|c| c.snapshots.max_keep).unwrap_or(10);
            let removed = snap_store.prune(max_keep)?;
            if removed == 0 {
                println!("Nothing to prune (max_keep={max_keep})");
            } else {
                println!("{} Pruned {removed} auto-snapshot{} (max_keep={max_keep})", color::green("✓"), if removed == 1 { "" } else { "s" });
            }
            Ok(())
        }
        SnapshotAction::Promote { id, base_name } => {
            cmd_snapshot_promote(store, base_dir, name, &id, &base_name)
        }
    }
}

fn cmd_snapshot_promote(
    store: &PodStore,
    base_dir: &std::path::Path,
    pod_name: &str,
    snap_id: &str,
    base_name: &str,
) -> Result<()> {
    use envpod_core::snapshot::SnapshotStore;
    use envpod_core::backend::native::has_base;

    let handle = store.load(pod_name)?;
    let state = NativeState::from_handle(&handle)?;
    let snap_store = SnapshotStore::new(&state.pod_dir);

    // Resolve snapshot
    let snap = snap_store.get(snap_id)?;
    let snap_dir = state.pod_dir.join("snapshots").join(&snap.id);
    if !snap_dir.exists() {
        anyhow::bail!("snapshot data for '{}' is missing from disk", snap.id);
    }

    let bases_dir = base_dir.join("bases");
    if has_base(&bases_dir, base_name) {
        anyhow::bail!("base '{}' already exists — choose a different name or run: envpod base destroy {base_name}", base_name);
    }

    let base_pod_dir = bases_dir.join(base_name);
    std::fs::create_dir_all(&base_pod_dir)
        .with_context(|| format!("create base dir: {}", base_pod_dir.display()))?;

    // 1. Symlink rootfs — resolve through any existing symlink so cloning a clone
    //    still points to the canonical rootfs (no chain).
    let pod_rootfs = state.pod_dir.join("rootfs");
    let canonical_rootfs = std::fs::canonicalize(&pod_rootfs)
        .unwrap_or(pod_rootfs.clone());
    let base_rootfs = base_pod_dir.join("rootfs");
    std::os::unix::fs::symlink(&canonical_rootfs, &base_rootfs)
        .context("symlink base rootfs")?;

    // 2. Copy snapshot upper/ → base_upper/ (this IS the base state)
    let base_upper = base_pod_dir.join("base_upper");
    let status = std::process::Command::new("cp")
        .args(["--reflink=auto", "-a", "--",
               &snap_dir.to_string_lossy(),
               &base_upper.to_string_lossy()])
        .status()
        .context("cp snapshot → base_upper")?;
    if !status.success() {
        anyhow::bail!("cp snapshot → base_upper failed (exit {status})");
    }

    // 3. Copy pod.yaml so clones inherit the pod config
    let pod_yaml = state.pod_dir.join("pod.yaml");
    if pod_yaml.exists() {
        std::fs::copy(&pod_yaml, base_pod_dir.join("pod.yaml"))
            .context("copy pod.yaml to base")?;
    }

    let size_display = format_bytes(snap.size_bytes);
    println!(
        "{} Promoted snapshot {} → base '{}'",
        color::green("✓"),
        color::bold(&snap.id),
        color::bold(base_name),
    );
    println!(
        "  Label: {}  |  {} file{}  |  {}",
        snap.display_name(),
        snap.file_count,
        if snap.file_count == 1 { "" } else { "s" },
        size_display,
    );
    println!("  Clone: envpod clone {} <new-pod-name>", color::dim(base_name));
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------
// completions
// ---------------------------------------------------------------------------

/// Subcommands whose first positional argument is a pod name.
const POD_SUBCOMMANDS: &[&str] = &[
    "init", "setup", "run", "start", "stop", "restart", "fg", "diff", "commit", "rollback", "audit",
    "lock", "unlock", "kill", "destroy", "queue", "approve", "cancel", "status", "logs",
    "vault", "mount", "unmount", "undo", "remote", "monitor", "dns", "clone", "actions",
    "ports", "discover", "snapshot",
];

fn print_completions(shell: Shell, base_dir: &std::path::Path) {
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    generate(shell, &mut cmd, "envpod", &mut buf);
    let base_script = String::from_utf8(buf).expect("clap_complete produced invalid UTF-8");

    match shell {
        Shell::Bash => print_bash_completions(&base_script, base_dir),
        Shell::Zsh => print_zsh_completions(&base_script, base_dir),
        Shell::Fish => print_fish_completions(&base_script, base_dir),
        _ => {
            // For other shells, just print the base clap_complete script
            print!("{base_script}");
        }
    }
}

fn print_bash_completions(base_script: &str, base_dir: &std::path::Path) {
    // Print the base clap_complete script, renaming _envpod to _envpod_clap
    let renamed = base_script.replace("_envpod", "_envpod_clap");
    print!("{renamed}");

    let subcmds_pattern = POD_SUBCOMMANDS.join("|");
    let base_dir_str = base_dir.display();

    // Print wrapper function that adds dynamic pod-name completion
    print!(
        r#"
_envpod() {{
    _envpod_clap
    local cur prev subcmd
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    # Find the subcommand (first non-flag word after "envpod")
    subcmd=""
    for ((i=1; i < COMP_CWORD; i++)); do
        case "${{COMP_WORDS[i]}}" in
            -*) ;;
            *) subcmd="${{COMP_WORDS[i]}}"; break ;;
        esac
    done
    # If cursor is at the first positional arg of a pod subcommand, add pod names
    case "$subcmd" in
        {subcmds_pattern})
            # Only complete pod name at position right after the subcommand
            local pos=0
            for ((j=i+1; j < COMP_CWORD; j++)); do
                case "${{COMP_WORDS[j]}}" in
                    -*) ;;
                    *) ((pos++)) ;;
                esac
            done
            if [[ $pos -eq 0 ]]; then
                # Suppress file fallback — only show pod names here
                compopt +o default +o bashdefault 2>/dev/null
                local state_dir="${{ENVPOD_DIR:-{base_dir_str}}}/state"
                if [[ -d "$state_dir" ]]; then
                    local pods
                    pods=$(cd "$state_dir" 2>/dev/null && for f in *.json; do [[ -f "$f" ]] && echo "${{f%.json}}"; done)
                    COMPREPLY+=($(compgen -W "$pods" -- "$cur"))
                fi
            fi
            ;;
        base)
            # Find the base sub-subcommand (create, ls, destroy)
            local base_sub=""
            for ((j=i+1; j < COMP_CWORD; j++)); do
                case "${{COMP_WORDS[j]}}" in
                    -*) ;;
                    *) base_sub="${{COMP_WORDS[j]}}"; break ;;
                esac
            done
            if [[ "$base_sub" == "destroy" ]]; then
                compopt +o default +o bashdefault 2>/dev/null
                local bases_dir="${{ENVPOD_DIR:-{base_dir_str}}}/bases"
                if [[ -d "$bases_dir" ]]; then
                    local bases
                    bases=$(cd "$bases_dir" 2>/dev/null && for d in */; do [[ -d "$d" ]] && echo "${{d%/}}"; done)
                    COMPREPLY+=($(compgen -W "$bases" -- "$cur"))
                fi
            fi
            ;;
    esac
}}
complete -o nosort -o bashdefault -o default -F _envpod envpod
"#
    );
}

fn print_zsh_completions(base_script: &str, base_dir: &std::path::Path) {
    // Print the base clap_complete script, renaming _envpod to _envpod_clap
    let renamed = base_script.replace("_envpod", "_envpod_clap");
    print!("{renamed}");

    let subcmds: Vec<String> = POD_SUBCOMMANDS.iter().map(|s| format!("\"{s}\"")).collect();
    let subcmds_list = subcmds.join(" ");
    let base_dir_str = base_dir.display();

    print!(
        r#"
_envpod() {{
    _envpod_clap "$@"
    local subcmd=""
    local pod_subcommands=({subcmds_list})
    # Find the subcommand
    for ((i=1; i < ${{#words[@]}} - 1; i++)); do
        case "${{words[i]}}" in
            -*) ;;
            *) subcmd="${{words[i]}}"; break ;;
        esac
    done
    if (( ${{+pod_subcommands[(r)$subcmd]}} )); then
        # Count positional args after subcommand
        local pos=0
        for ((j=i+1; j < CURRENT; j++)); do
            case "${{words[j]}}" in
                -*) ;;
                *) ((pos++)) ;;
            esac
        done
        if [[ $pos -eq 0 ]]; then
            local state_dir="${{ENVPOD_DIR:-{base_dir_str}}}/state"
            if [[ -d "$state_dir" ]]; then
                local pods=()
                for f in "$state_dir"/*.json(N); do
                    pods+=("${{${{f:t}}%.json}}")
                done
                compadd -a pods
            fi
        fi
    elif [[ "$subcmd" == "base" ]]; then
        # Find base sub-subcommand
        local base_sub=""
        for ((j=i+1; j < CURRENT; j++)); do
            case "${{words[j]}}" in
                -*) ;;
                *) base_sub="${{words[j]}}"; break ;;
            esac
        done
        if [[ "$base_sub" == "destroy" ]]; then
            local bases_dir="${{ENVPOD_DIR:-{base_dir_str}}}/bases"
            if [[ -d "$bases_dir" ]]; then
                local bases=()
                for d in "$bases_dir"/*(N/); do
                    bases+=("${{d:t}}")
                done
                compadd -a bases
            fi
        fi
    fi
}}
compdef _envpod envpod
"#
    );
}

fn print_fish_completions(base_script: &str, base_dir: &std::path::Path) {
    // Print the base clap_complete script as-is
    print!("{base_script}");

    let base_dir_str = base_dir.display();

    // Add dynamic pod-name completions for each pod subcommand
    for subcmd in POD_SUBCOMMANDS {
        print!(
            r#"
complete -c envpod -n "__fish_seen_subcommand_from {subcmd}" -f -a "(
    set -l state_dir (set -q ENVPOD_DIR; and echo \$ENVPOD_DIR/state; or echo '{base_dir_str}/state')
    if test -d \$state_dir
        for f in \$state_dir/*.json
            if test -f \$f
                basename \$f .json
            end
        end
    end
)"
"#
        );
    }

    // Add dynamic base pod name completion for `envpod base destroy`
    print!(
        r#"
complete -c envpod -n "__fish_seen_subcommand_from base; and __fish_seen_subcommand_from destroy" -f -a "(
    set -l bases_dir (set -q ENVPOD_DIR; and echo \$ENVPOD_DIR/bases; or echo '{base_dir_str}/bases')
    if test -d \$bases_dir
        for d in \$bases_dir/*/
            if test -d \$d
                basename \$d
            end
        end
    end
)"
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        // clap's built-in validation — catches mismatched types, missing fields, etc.
        Cli::command().debug_assert();
    }

    #[test]
    fn completions_bash_contains_wrapper() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Bash, &mut cmd, "envpod", &mut buf);
        let base_script = String::from_utf8(buf).unwrap();

        // Verify base script is non-empty and contains expected content
        assert!(!base_script.is_empty());
        assert!(base_script.contains("envpod"));
    }

    #[test]
    fn completions_generates_for_all_shells() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            generate(shell, &mut cmd, "envpod", &mut buf);
            let script = String::from_utf8(buf).unwrap();
            assert!(
                !script.is_empty(),
                "completion script for {shell:?} should not be empty"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // security_findings tests
    // ---------------------------------------------------------------------------

    fn findings_ids(config: &PodConfig) -> Vec<&'static str> {
        security_findings(config).into_iter().map(|f| f.id).collect()
    }

    fn findings_by_id<'a>(findings: &'a [SecurityFinding], id: &str) -> Vec<&'a SecurityFinding> {
        findings.iter().filter(|f| f.id == id).collect()
    }

    // N-04: port forwarding
    #[test]
    fn security_no_ports_no_n04() {
        let config = PodConfig::default();
        assert!(!findings_ids(&config).contains(&"N-04"));
    }

    #[test]
    fn security_ports_localhost_only_no_n04() {
        // network.ports = localhost-only (-p), never triggers N-04
        let mut config = PodConfig::default();
        config.network.ports = vec!["8080:3000".to_string()];
        assert!(
            !findings_ids(&config).contains(&"N-04"),
            "N-04 should NOT fire for network.ports (localhost-only by default)"
        );
    }

    #[test]
    fn security_public_ports_trigger_n04() {
        // network.public_ports = all interfaces (-P), triggers N-04
        let mut config = PodConfig::default();
        config.network.public_ports = vec!["8080:3000".to_string()];
        let findings = security_findings(&config);
        let n04 = findings_by_id(&findings, "N-04");
        assert!(!n04.is_empty(), "N-04 should fire for network.public_ports");
        assert_eq!(n04[0].severity, "LOW");
    }

    #[test]
    fn security_n04_explanation_lists_public_specs() {
        let mut config = PodConfig::default();
        config.network.public_ports = vec!["9090:9090".to_string()];
        let findings = security_findings(&config);
        let n04 = findings_by_id(&findings, "N-04");
        assert!(!n04.is_empty());
        assert!(
            n04[0].explanation.contains("9090:9090"),
            "N-04 explanation should list the public_ports specs"
        );
    }

    #[test]
    fn security_ports_and_public_ports_only_n04_for_public() {
        // ports = localhost only (no N-04), public_ports = network (N-04 fires)
        let mut config = PodConfig::default();
        config.network.ports = vec!["8080:3000".to_string()];
        config.network.public_ports = vec!["9090:9090".to_string()];
        let findings = security_findings(&config);
        let n04 = findings_by_id(&findings, "N-04");
        assert!(!n04.is_empty(), "N-04 fires for public_ports");
        assert!(n04[0].explanation.contains("9090:9090"));
        assert!(!n04[0].explanation.contains("8080:3000"), "localhost port should not appear");
    }

    // N-03: DNS mode
    #[test]
    fn security_n03_fires_for_monitor_mode() {
        let mut config = PodConfig::default();
        config.network.dns.mode = envpod_core::types::DnsMode::Monitor;
        let ids = findings_ids(&config);
        assert!(ids.contains(&"N-03"), "N-03 fires for Monitor mode");
    }

    #[test]
    fn security_n03_fires_for_blacklist_mode() {
        let mut config = PodConfig::default();
        config.network.dns.mode = envpod_core::types::DnsMode::Blacklist;
        let ids = findings_ids(&config);
        assert!(ids.contains(&"N-03"));
    }

    #[test]
    fn security_n03_clear_for_whitelist_mode() {
        let mut config = PodConfig::default();
        config.network.dns.mode = envpod_core::types::DnsMode::Whitelist;
        assert!(!findings_ids(&config).contains(&"N-03"));
    }

    // N-05 / N-06: root user
    #[test]
    fn security_root_user_triggers_n05_n06() {
        let mut config = PodConfig::default();
        config.user = "root".to_string();
        let ids = findings_ids(&config);
        assert!(ids.contains(&"N-05"), "N-05 fires for root user");
        assert!(ids.contains(&"N-06"), "N-06 fires for root user");
    }

    #[test]
    fn security_nonroot_user_no_n05_n06() {
        let config = PodConfig::default(); // default user is "agent"
        let ids = findings_ids(&config);
        assert!(!ids.contains(&"N-05"));
        assert!(!ids.contains(&"N-06"));
    }

    // C-01/C-02/C-03: resource limits
    #[test]
    fn security_no_limits_trigger_c01_c02_c03() {
        let mut config = PodConfig::default();
        config.processor.memory = None;
        config.processor.cores = None;
        config.processor.max_pids = None;
        let ids = findings_ids(&config);
        assert!(ids.contains(&"C-01"));
        assert!(ids.contains(&"C-02"));
        assert!(ids.contains(&"C-03"));
    }

    #[test]
    fn security_limits_set_no_c_findings() {
        let mut config = PodConfig::default();
        config.processor.memory = Some("2GB".to_string());
        config.processor.cores = Some(2.0);
        config.processor.max_pids = Some(512);
        let ids = findings_ids(&config);
        assert!(!ids.contains(&"C-01"));
        assert!(!ids.contains(&"C-02"));
        assert!(!ids.contains(&"C-03"));
    }

    // S-03 / P-03: browser seccomp profile
    #[test]
    fn security_browser_seccomp_triggers_s03_p03() {
        let mut config = PodConfig::default();
        config.security.seccomp_profile = "browser".to_string();
        let ids = findings_ids(&config);
        assert!(ids.contains(&"S-03"), "S-03 fires for browser seccomp");
        assert!(ids.contains(&"P-03"), "P-03 fires for browser seccomp");
    }

    #[test]
    fn security_default_seccomp_no_s03_p03() {
        let config = PodConfig::default(); // default seccomp
        let ids = findings_ids(&config);
        assert!(!ids.contains(&"S-03"));
        assert!(!ids.contains(&"P-03"));
    }

    // S-04: no seccomp filter
    #[test]
    fn security_none_seccomp_triggers_s04() {
        let mut config = PodConfig::default();
        config.security.seccomp_profile = "none".to_string();
        let findings = security_findings(&config);
        let s04 = findings_by_id(&findings, "S-04");
        assert!(!s04.is_empty(), "S-04 fires for seccomp_profile: none");
        assert_eq!(s04[0].severity, "CRITICAL");
    }

    #[test]
    fn security_none_seccomp_no_s03_p03() {
        // "none" should NOT trigger S-03/P-03 (those are browser-specific)
        let mut config = PodConfig::default();
        config.security.seccomp_profile = "none".to_string();
        let ids = findings_ids(&config);
        assert!(!ids.contains(&"S-03"), "S-03 should not fire for none profile");
        assert!(!ids.contains(&"P-03"), "P-03 should not fire for none profile");
    }

    #[test]
    fn security_default_seccomp_no_s04() {
        let config = PodConfig::default();
        assert!(!findings_ids(&config).contains(&"S-04"));
    }

    // D-01: allow_discovery + Unsafe network mode
    #[test]
    fn security_d01_discovery_with_unsafe_network() {
        let mut config = PodConfig::default();
        config.network.allow_discovery = true;
        config.network.mode = envpod_core::types::NetworkMode::Unsafe;
        let ids = findings_ids(&config);
        assert!(ids.contains(&"D-01"), "D-01 fires when allow_discovery=true and mode=Unsafe");
    }

    #[test]
    fn security_d01_no_discovery_unsafe_no_finding() {
        let mut config = PodConfig::default();
        config.network.allow_discovery = false;
        config.network.mode = envpod_core::types::NetworkMode::Unsafe;
        assert!(!findings_ids(&config).contains(&"D-01"), "D-01 silent when allow_discovery=false");
    }

    #[test]
    fn security_d01_discovery_isolated_no_finding() {
        let mut config = PodConfig::default();
        config.network.allow_discovery = true;
        config.network.mode = envpod_core::types::NetworkMode::Isolated;
        assert!(!findings_ids(&config).contains(&"D-01"), "D-01 silent when mode=Isolated");
    }

    // D-02: wildcard allow_pods
    #[test]
    fn security_d02_wildcard_allow_pods() {
        let mut config = PodConfig::default();
        config.network.allow_pods = vec!["*".to_string()];
        let ids = findings_ids(&config);
        assert!(ids.contains(&"D-02"), "D-02 fires when allow_pods contains \"*\"");
    }

    #[test]
    fn security_d02_specific_allow_pods_no_finding() {
        let mut config = PodConfig::default();
        config.network.allow_pods = vec!["api-pod".to_string(), "worker-pod".to_string()];
        assert!(!findings_ids(&config).contains(&"D-02"), "D-02 silent for specific pod names");
    }

    #[test]
    fn security_d02_empty_allow_pods_no_finding() {
        let config = PodConfig::default(); // allow_pods defaults to []
        assert!(!findings_ids(&config).contains(&"D-02"));
    }

    // D-01 + D-02 together
    #[test]
    fn security_d01_d02_both_fire_when_combined() {
        let mut config = PodConfig::default();
        config.network.allow_discovery = true;
        config.network.mode = envpod_core::types::NetworkMode::Unsafe;
        config.network.allow_pods = vec!["*".to_string()];
        let ids = findings_ids(&config);
        assert!(ids.contains(&"D-01"));
        assert!(ids.contains(&"D-02"));
    }

    #[test]
    fn pod_subcommands_list_is_nonempty() {
        assert!(!POD_SUBCOMMANDS.is_empty());
        // Every entry in POD_SUBCOMMANDS should be a valid subcommand
        let cmd = Cli::command();
        let subcommand_names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        for &pod_sub in POD_SUBCOMMANDS {
            assert!(
                subcommand_names.contains(&pod_sub),
                "'{pod_sub}' is in POD_SUBCOMMANDS but not a real CLI subcommand"
            );
        }
    }
}
