<!-- type-delay 0.03 -->
# envpod User Guide

> **EnvPod v0.1.0** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Comprehensive reference for envpod. This guide covers every command, configuration option, and workflow.

## Table of Contents

1. [Overview](#overview)
2. [Core Concepts](#core-concepts)
3. [Installation](#installation)
4. [Quick Start](#quick-start)
5. [CLI Reference](#cli-reference)
6. [Pod Configuration (pod.yaml)](#pod-configuration-podyaml)
7. [Filesystem Isolation](#filesystem-isolation)
8. [Network Isolation](#network-isolation)
9. [Process Isolation](#process-isolation)
10. [Device Masking](#device-masking)
11. [Web Display (noVNC)](#web-display-novnc)
12. [Security Hardening](#security-hardening)
13. [Credential Vault](#credential-vault)
14. [Action Queue & Undo](#action-queue--undo)
15. [Monitoring & Alerts](#monitoring--alerts)
16. [Remote Control](#remote-control)
17. [Audit Trail](#audit-trail)
18. [Snapshots](#snapshots)
19. [Live Mutation](#live-mutation)
20. [Host App Auto-Mount](#host-app-auto-mount)
21. [Example Configs](#example-configs)
22. [FAQ](FAQ.md)
23. [Troubleshooting](#troubleshooting)

---

## Overview

Envpod wraps AI agents in governed isolation environments called **pods**. Each pod provides:

- **Filesystem isolation** — copy-on-write overlay; agent writes never touch host files
- **Network isolation** — per-pod DNS resolver with domain-level allow/deny
- **Process isolation** — PID namespace, cgroup resource limits, seccomp syscall filtering
- **Device masking** — minimal `/dev` with opt-in GPU, display, and audio passthrough
- **Governance** — audit logging, credential vault, action queue, monitoring, remote lockdown

<!-- output -->
```
Docker isolates. Envpod governs.
```

Every action is auditable. Every capability is revocable. Every change is reversible.

---

## Core Concepts

### The Pod

A pod is a self-contained execution environment with four isolation walls and a governance ceiling.

<!-- output -->
```
┌──────────────────────────────────────────────────────────┐
│                  GOVERNANCE CEILING                       │
│  Audit · Vault · Action Queue · Monitoring · Lockdown    │
├──────────────┬─────────────┬─────────────┬──────────────┤
│   MEMORY     │    FILE     │   NETWORK   │  PROCESSOR   │
│  /proc mask  │ OverlayFS  │ DNS filter  │ cgroups v2   │
│  coredump    │ COW diff/  │ namespace   │ CPU affinity │
│  prevention  │ commit     │ iptables    │ PID ns       │
└──────────────┴─────────────┴─────────────┴──────────────┘
```

<!-- pause 2 -->

### Lifecycle

<!-- output -->
```
init  →  run  →  diff  →  commit  or  rollback  →  destroy
                   ↑                                  ↑
                   └── review changes ────────────────┘
```

1. **init** — Create the pod (overlay dirs, cgroup, network namespace)
2. **run** — Execute commands inside the pod
3. **diff** — See what the agent changed
4. **commit** — Accept changes to host filesystem
5. **rollback** — Discard all changes
6. **destroy** — Remove pod entirely

### Pod Types

| Type | Description |
|------|-------------|
| `standard` | Balanced isolation and functionality (default) |
| `hardened` | Stricter defaults, higher isolation |
| `ephemeral` | Auto-cleanup after termination |
| `supervised` | Requires continuous monitoring |
| `airgapped` | No external network access |

---

## Installation

See [INSTALL.md](INSTALL.md) for detailed instructions including prerequisites, building from source, and firewall configuration.

**Quick version:**

<!-- no-exec -->
```bash
# Build
cargo build --release

# Install
sudo cp target/release/envpod /usr/local/bin/envpod

# Verify
sudo envpod ls
```

**Requirements:** Linux kernel 5.11+, cgroups v2, OverlayFS, iptables, iproute2, root access.

---

## Quick Start

See [QUICKSTART.md](QUICKSTART.md) for a hands-on tutorial.

**30-second version:**

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Create a pod
sudo envpod init my-agent -c examples/basic-cli.yaml

# Run a shell inside it
sudo envpod run my-agent -- /bin/bash

# See what changed, then accept or reject
sudo envpod diff my-agent
sudo envpod commit my-agent              # commit all
# or: sudo envpod commit my-agent /opt/output.txt  # commit one file
# or: sudo envpod rollback my-agent                # discard all

# Clean up
sudo envpod destroy my-agent
```

---

## CLI Reference

All commands require root (`sudo`). Override the state directory with `--dir <path>` or `ENVPOD_DIR` env var (default: `/var/lib/envpod`).

### Pod Lifecycle

#### `envpod init <name> [-c <config>] [--preset <name>] [--backend <backend>] [-v]`

Create a new pod. Three ways to configure:

| Option | Description |
|--------|-------------|
| `<name>` | Pod name (required) |
| `-p`, `--preset <preset>` | Use a built-in preset (18 available — see `envpod presets`) |
| `-c`, `--config <path>` | Path to pod.yaml config file |
| `--backend <backend>` | Isolation backend: `native` (default) |
| `-v`, `--verbose` | Show verbose output during setup |

If neither `--preset` nor `-c` is given and stdin is a terminal, an **interactive wizard** launches — showing all presets by category with resource customization (CPU, memory, GPU).

If a config contains `setup` commands or a `setup_script`, they run automatically after creation.

<!-- no-exec -->
```bash
# Use a preset (auto-installs dependencies)
sudo envpod init my-agent --preset claude-code

# Interactive wizard (shows categorized presets, lets you customize)
sudo envpod init my-agent

# Use a config file directly
sudo envpod init my-agent -c examples/coding-agent.yaml
```

#### `envpod presets`

List all built-in presets by category (Coding Agents, Frameworks, Browser Agents, Environments).

#### `envpod run <name> [options] -- <command> [args...]`

Run a command inside a pod. Press **Ctrl+Z** to detach (pod continues in background). Use `envpod fg` to reattach.

| Option | Description |
|--------|-------------|
| `--root` | Run as root inside the pod (default is non-root `agent` user). Prints a security warning. |
| `--user <name\|uid>` | Run as a specific user inside the pod (`--user root` is equivalent to `--root`) |
| `-b`, `--background` | Run in background. Use `envpod fg <name>` to reattach. |
| `--env <KEY=VALUE>` | Set environment variables (repeatable) |
| `-d`, `--enable-display` | Forward display (Wayland preferred, X11 fallback). Override with `display_protocol` in pod.yaml. |
| `-a`, `--enable-audio` | Forward audio (PipeWire preferred, PulseAudio fallback). Override with `audio_protocol` in pod.yaml. |

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Single command (runs as 'agent' user by default)
sudo envpod run my-agent -- echo "hello"

# Interactive shell
sudo envpod run my-agent -- /bin/bash

# Run a script
sudo envpod run my-agent -- python3 train.py --epochs 10

# Run as root (with security warning)
sudo envpod run my-agent --root -- /bin/bash

# Run in background, reattach later
sudo envpod run my-agent -b -- python3 long_task.py
sudo envpod fg my-agent

# Run as a specific user with display and audio
sudo envpod run my-agent -d -a --user browseruser -- google-chrome https://example.com

# Pass environment variables
sudo envpod run my-agent --env MY_VAR=hello --env DEBUG=1 -- /bin/bash
```

The command runs inside full isolation: PID namespace, mount namespace with OverlayFS, network namespace with DNS filtering, cgroup resource limits, and seccomp syscall filtering.

**Default user:** Pods run as a non-root `agent` user (UID 60000) by default, providing full pod boundary protection (17/17 jailbreak tests pass). The user is created automatically during `envpod init`. Override with `--root` (prints a warning), `--user <name>`, or `user:` in pod.yaml. CLI flags take precedence over pod.yaml.

#### `envpod fg <name>`

Reattach to a background or detached pod. Shows existing log output and tails new output in real-time. Press **Ctrl+Z** to detach again (pod continues running).

**User precedence:** `--user` > `--root` > pod.yaml `user:` field > default `agent`

#### `envpod setup <name>`

Re-run setup commands and setup script from the pod's config. Useful if setup was interrupted or failed partway through (e.g., a `pip install` failed due to a network timeout). The pod remains usable even if setup is incomplete — you can re-run `envpod setup` at any time to retry.

<!-- no-exec -->
```bash
sudo envpod setup my-agent
```

#### `envpod destroy <name> [name2 ...] [--base] [--full]`

Remove one or more pods — overlay, cgroup, network namespace, state. Accepts multiple names for fast batch deletion.

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod destroy my-agent
sudo envpod destroy clone-1 clone-2 clone-3    # batch destroy
sudo envpod destroy my-agent --base             # also remove the base pod
sudo envpod destroy my-agent --full             # also clean up iptables immediately
```

| Option | Description |
|--------|-------------|
| `--base` | Also remove the base pod (only with a single name) |
| `--full` | Full cleanup: also remove iptables rules immediately (slower, no gc needed) |

By default, `destroy` defers iptables cleanup for speed — dead rules are harmless and cleaned up by `envpod gc`. Use `--full` when you want a completely clean teardown without running gc afterward.

#### `envpod gc`

Clean up all orphaned resources left by destroyed pods or unclean shutdowns. When pods are destroyed, some resources are cleaned up immediately (veth pair, network namespace) while others are deferred for performance (iptables rules). Additionally, crashes or manual filesystem operations can leave orphaned resources.

`envpod gc` cleans up:
- **Stale iptables rules** — rules referencing deleted veth interfaces
- **Orphaned network namespaces** — `envpod-*` namespaces with no matching pod
- **Orphaned cgroups** — directories under `/sys/fs/cgroup/envpod/` with no matching pod
- **Orphaned pod directories** — directories in the pods directory with no state file
- **Stale state files** — state files pointing to non-existent pod directories
- **Stale index files** — netns index entries for non-existent pods

<!-- no-exec -->
```bash
sudo envpod gc
# Removed 15 stale iptables rules
# Removed 2 orphaned network namespaces
# Removed 3 orphaned cgroups
```

<!-- pause 2 -->

Run periodically on hosts that create and destroy many pods, or after a system crash.

### Clone & Base Pod Management

#### `envpod clone <source> <name> [--current]`

Clone a pod or base pod. Cloning is ~10x faster than `envpod init` (~130ms vs ~1.3s) because it symlinks the rootfs instead of rebuilding it.

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Clone from base snapshot (after init+setup, before agent changes)
sudo envpod clone my-agent clone-1

# Clone from current state (includes agent modifications)
sudo envpod clone my-agent clone-2 --current

# Clone from a standalone base pod
sudo envpod clone python-base worker-1
```

| Option | Description |
|--------|-------------|
| `<source>` | Source pod or base pod name (required) |
| `<name>` | Name for the new clone (required) |
| `--current` | Clone from current state instead of base snapshot (pods only) |

Each clone gets its own overlay, cgroup, and network namespace. The rootfs is shared via symlink — near-zero disk overhead (~1 KB per clone).

#### `envpod base create <name> [-c <config>] [-v]`

Create a standalone base pod (reusable snapshot for cloning).

<!-- no-exec -->
```bash
sudo envpod base create python-base -c examples/python-env.yaml
sudo envpod base create browser-base -c examples/browser.yaml -v
```

| Option | Description |
|--------|-------------|
| `<name>` | Base pod name (required) |
| `-c`, `--config <path>` | Path to pod.yaml config file |
| `-v`, `--verbose` | Show verbose output during setup |

#### `envpod base ls [--json]`

List all base pods.

<!-- no-exec -->
```bash
sudo envpod base ls
sudo envpod base ls --json
```

#### `envpod base destroy <name> [--force]`

Remove a base pod.

<!-- no-exec -->
```bash
sudo envpod base destroy python-base
sudo envpod base destroy python-base --force   # even if pods still reference it
```

| Option | Description |
|--------|-------------|
| `--force` | Force removal even if pods still reference this base |

### Filesystem Operations

#### `envpod diff <name> [--json]`

Show filesystem changes in the pod's overlay.

<!-- no-exec -->
```bash
sudo envpod diff my-agent
```

<!-- output -->
```
  Added    /opt/output.txt           (42 bytes)
  Modified /etc/hosts                (156 bytes)
  Deleted  /opt/old-file.txt
```

<!-- pause 2 -->

<!-- no-exec -->
```bash
sudo envpod diff my-agent --json    # machine-readable
```

#### `envpod commit <name> [paths...] [--exclude <paths...>] [--output <dir>] [--include-system]`

Apply overlay changes to the host filesystem. Supports several modes:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Commit all changes (default)
sudo envpod commit my-agent

# Commit only specific files (selective)
sudo envpod commit my-agent /opt/output.csv /opt/results/

# Commit everything except specific files
sudo envpod commit my-agent --exclude /etc/config.ini

# Export to a directory instead of the host filesystem
sudo envpod commit my-agent --output /tmp/agent-output

# Include system directory changes (/usr, /bin, /sbin, /lib, /lib64)
sudo envpod commit my-agent --include-system
```

| Option | Description |
|--------|-------------|
| `paths...` | Specific paths to commit (default: all) |
| `--exclude <paths...>` | Paths to exclude from commit |
| `-o`, `--output <dir>` | Export committed files to this directory instead of the host filesystem |
| `--all` | Commit all changes including protected paths |
| `--include-system` | Include system directory changes (`/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`) |

Paths must match entries shown by `envpod diff`. Selective commit removes committed files from the overlay and leaves the rest — you can run additional commits for remaining files.

#### `envpod rollback <name>`

Discard all overlay changes. Clears the upper and work directories.

<!-- no-exec -->
```bash
sudo envpod rollback my-agent
```

### Monitoring & Status

#### `envpod status <name> [--json]`

Show pod status, process info, and resource usage.

<!-- no-exec -->
```bash
sudo envpod status my-agent
```

<!-- output -->
```
Pod:      my-agent
ID:       a1b2c3d4-...
Backend:  native
Status:   running
PID:      12345
CPU:      45.2%
Memory:   256 MB / 4096 MB
Network:  10.200.1.2 (envpod-a1b2c3d4)
```

<!-- pause 2 -->

#### `envpod ls [--json]`

List all pods.

<!-- no-exec -->
```bash
sudo envpod ls
```

<!-- output -->
```
NAME          BACKEND  CREATED
my-agent      native   2026-02-27T10:00:00Z
browser-pod   native   2026-02-27T09:30:00Z
```

#### `envpod logs <name> [-f] [-n <lines>]`

Show pod stdout/stderr output.

| Option | Description |
|--------|-------------|
| `-f`, `--follow` | Follow log output (like `tail -f`) |
| `-n`, `--lines <N>` | Show last N lines (default: 50, 0 = all) |

<!-- no-exec -->
```bash
sudo envpod logs my-agent -f         # stream live output
sudo envpod logs my-agent -n 100     # last 100 lines
sudo envpod logs my-agent -n 0       # all output
```

#### `envpod audit <name> [--json] [--security] [-c <config>]`

Show the pod's audit trail, or run a static security analysis.

**Audit trail** (requires a pod name):

<!-- no-exec -->
```bash
sudo envpod audit my-agent
sudo envpod audit my-agent --json     # machine-readable
```

<!-- output -->
```
TIME                 ACTION           DETAILS
2026-02-27T10:00:00  create           backend=native
2026-02-27T10:00:01  start            pid=12345, cmd=/bin/bash
2026-02-27T10:05:00  diff             3 change(s)
2026-02-27T10:05:01  commit
```

<!-- pause 2 -->

**Security audit** (works on created pods or raw YAML files):

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Audit a created pod
sudo envpod audit my-agent --security

# Audit a config file (no pod needed)
sudo envpod audit --security -c examples/browser.yaml

# Machine-readable security audit
sudo envpod audit --security --json -c examples/coding-agent.yaml
```

<!-- pause 2 -->

| Option | Description |
|--------|-------------|
| `--json` | Output as JSON |
| `--security` | Run static security analysis instead of showing audit trail |
| `-c`, `--config <path>` | Path to pod.yaml (for `--security` without a created pod) |

See [Security Report](SECURITY.md) for a full audit of all example configs and a reference of all finding IDs.

### Live Mutation

#### `envpod mount <name> <host_path> [--target <path>] [--readonly]`

Bind-mount a host path into a running pod.

| Option | Description |
|--------|-------------|
| `<host_path>` | Host filesystem path (required) |
| `--target <path>` | Path inside pod (default: same as host path) |
| `--readonly` | Mount as read-only (default: read-write) |

<!-- no-exec -->
```bash
sudo envpod mount my-agent ~/data --readonly
sudo envpod mount my-agent ~/code --target /workspace
```

#### `envpod unmount <name> <path>`

Remove a bind-mount from a pod.

<!-- no-exec -->
```bash
sudo envpod unmount my-agent /workspace
```

#### `envpod dns <name> --allow/--deny/--remove-allow/--remove-deny <domain>`

Update DNS policy on a running pod without restart.

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Grant access to a new domain
sudo envpod dns my-agent --allow api.example.com

# Block a domain
sudo envpod dns my-agent --deny malicious.io

# Revoke previously granted access
sudo envpod dns my-agent --remove-allow api.example.com

# Multiple changes at once
sudo envpod dns my-agent --allow a.com --allow b.com --deny c.com
```

### Lockdown & Undo

#### `envpod lock [<name>] [--all]`

Freeze a pod (pause all processes, preserve state). Use `--all` for building-wide lockdown.

For desktop pods with noVNC, the display stack (Xvfb, x11vnc, websockify) runs
in a guardian cgroup that survives freeze — the browser connection stays alive
while the app is frozen. Resume with `envpod unlock` to continue exactly where
you left off. Single-process apps (GIMP, xterm, etc.) survive freeze/thaw
cleanly. Multi-process apps (Chrome, VS Code/Electron) may crash on resume
due to internal watchdog timeouts — relaunch them from the desktop terminal.

<!-- no-exec -->
```bash
sudo envpod lock my-agent       # freeze one pod
sudo envpod lock --all           # freeze all pods
sudo envpod unlock my-agent     # resume a frozen pod
```

#### `envpod kill <name>`

Terminate all pod processes and rollback changes.

<!-- no-exec -->
```bash
sudo envpod kill my-agent
```

#### `envpod undo <name> [<id>] [--all]`

Undo reversible actions.

<!-- no-exec -->
```bash
sudo envpod undo my-agent              # list undo-able actions
sudo envpod undo my-agent a1b2c3d4     # undo specific action
sudo envpod undo my-agent --all        # undo everything
```

Supported undo mechanisms:
- **OverlayRollback** — discard filesystem changes
- **Unmount** — reverse a mount operation
- **Thaw** — resume a frozen pod
- **RestoreLimits** — restore previous resource limits

### Action Queue

#### `envpod queue <name> [--json]`

List queued actions awaiting human approval or delayed execution.

<!-- no-exec -->
```bash
sudo envpod queue my-agent
```

#### `envpod queue <name> add --tier <tier> --description <text> [--delay <secs>]`

Submit an action to the queue.

| Tier | Behavior |
|------|----------|
| `immediate` | Execute now, COW overlay protects |
| `delayed` | Hold N seconds, auto-execute unless cancelled |
| `staged` | Hold until human explicitly approves |
| `blocked` | Denied by default |

<!-- no-exec -->
```bash
sudo envpod queue my-agent add --tier staged --description "deploy to prod"
sudo envpod queue my-agent add --tier delayed --description "send email" --delay 120
```

#### `envpod approve <name> <id>`

Approve a queued action.

<!-- no-exec -->
```bash
sudo envpod approve my-agent a1b2c3d4
```

#### `envpod cancel <name> <id>`

Cancel a queued action.

<!-- no-exec -->
```bash
sudo envpod cancel my-agent a1b2c3d4
```

### Credential Vault

#### `envpod vault <name> set <key>`

Store a secret. Reads the value from stdin (never in shell history).

<!-- no-exec -->
```bash
sudo envpod vault my-agent set ANTHROPIC_API_KEY
# Type or paste the secret, then press Enter
```

#### `envpod vault <name> get <key>`

Retrieve a secret value.

<!-- no-exec -->
```bash
sudo envpod vault my-agent get ANTHROPIC_API_KEY
```

#### `envpod vault <name> list`

List all secret keys (not values).

<!-- no-exec -->
```bash
sudo envpod vault my-agent list
```

#### `envpod vault <name> rm <key>`

Remove a secret.

<!-- no-exec -->
```bash
sudo envpod vault my-agent rm ANTHROPIC_API_KEY
```

Vault secrets are injected as environment variables when the pod runs. The agent process sees them as normal env vars but they never appear in the pod's config, overlay, or audit log values.

### Monitoring

#### `envpod monitor <name> set-policy <path>`

Install a monitoring policy that watches for anomalous behavior.

<!-- no-exec -->
```bash
sudo envpod monitor my-agent set-policy examples/monitoring-policy.yaml
```

#### `envpod monitor <name> alerts [--json]`

Show monitoring alerts.

<!-- no-exec -->
```bash
sudo envpod monitor my-agent alerts
```

### Remote Control

#### `envpod remote <name> <command> [--payload <json>]`

Send a control command to a running pod.

| Command | Description |
|---------|-------------|
| `freeze` | Pause all processes |
| `resume` | Unpause processes |
| `kill` | Terminate all processes |
| `restrict` | Dynamically reduce capabilities |
| `status` | Query current status |
| `alerts` | Get monitoring alerts |

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod remote my-agent freeze
sudo envpod remote my-agent resume
sudo envpod remote my-agent restrict --payload '{"cpu_cores": 0.5}'
sudo envpod remote my-agent status
```

### Shell Completions

#### `envpod completions <shell>`

Generate tab completions for your shell.

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Bash
sudo envpod completions bash > /etc/bash_completion.d/envpod

# Zsh
sudo envpod completions zsh > /usr/local/share/zsh/site-functions/_envpod

# Fish
sudo envpod completions fish > ~/.config/fish/completions/envpod.fish
```

---

## Pod Configuration (pod.yaml)

Pods are configured via YAML files. All sections are optional — secure defaults apply.

### Complete Schema

<!-- output -->
<!-- type-delay 0.02 -->
```yaml
# ─── Identity ─────────────────────────────────────────────
name: my-agent                           # Pod name (required)
type: standard                           # standard | hardened | ephemeral | supervised | airgapped
backend: native                          # Isolation backend (default: native)

# ─── Filesystem ───────────────────────────────────────────
filesystem:
  workspace: ~/projects/my-app           # Working directory (optional)
  mounts:
    - path: ~/projects/my-app            # Host path to mount
      permissions: ReadWrite             # ReadOnly (default) | ReadWrite
  tracking:
    watch:                               # Only show these paths in diff/commit
      - /home                            # (empty = show everything)
      - /opt
      - /root
      - /srv
      - /workspace
    ignore:                              # Always hide these from diff/commit
      - /var/lib/apt
      - /var/lib/dpkg
      - /var/cache
      - /var/log
      - /tmp
      - /run

# ─── Network ──────────────────────────────────────────────
network:
  mode: Isolated                         # Isolated (default) | Monitored | Unsafe
  dns:
    mode: Whitelist                      # Whitelist (default) | Blacklist | Monitor
    allow:                               # Domains that resolve (Whitelist mode)
      - api.anthropic.com
      - "*.github.com"                   # Wildcard subdomain matching
    deny:                                # Domains that don't resolve (Blacklist mode)
      - "*.internal"
    remap:                               # DNS aliasing
      old-api.example.com: new-api.example.com
  rate_limit: "100/min"                  # Request rate cap (planned, not yet enforced)
  bandwidth_cap: "500MB"                 # Total bandwidth cap (planned, not yet enforced)
  subnet: "10.200"                       # Subnet base for pod IPs (default: "10.200")
                                         # Pods get {subnet}.{idx}.0/30 — same subnet = future inter-pod routing

# ─── Processor ────────────────────────────────────────────
processor:
  cores: 2.0                             # CPU core limit (e.g., 2.0, 0.5)
  memory: "4GB"                          # RAM limit (KB, MB, GB)
  cpu_affinity: "0-3"                    # Pin to specific CPUs (cpuset.cpus)

# ─── Devices ──────────────────────────────────────────────
devices:
  gpu: false              # GPU passthrough (NVIDIA + DRI)
  display: false          # Display forwarding (auto-detects Wayland or X11)
  audio: false            # Audio forwarding (auto-detects PipeWire or PulseAudio)
  display_protocol: auto  # auto | wayland | x11
  audio_protocol: auto    # auto | pipewire | pulseaudio
  desktop_env: none       # Auto-install desktop env: none | xfce | openbox | sway
  extra:                                 # Additional device passthrough
    - "/dev/fuse"
    - "/dev/kvm"

# ─── Security ─────────────────────────────────────────────
security:
  seccomp_profile: default               # default | browser
  shm_size: "64MB"                       # /dev/shm tmpfs size (default: 64MB)

# ─── Budget ───────────────────────────────────────────────
budget:
  max_duration: "4h"                     # Max session time (s, m, h)
  max_requests: 5000                     # Max network requests (planned, not yet enforced)
  max_bandwidth: "2GB"                   # Max bandwidth (planned, not yet enforced)

# ─── Tools ────────────────────────────────────────────────
tools:
  allowed_commands:                      # Command whitelist (empty = allow all)
    - /bin/bash
    - /usr/bin/git

# ─── Audit ────────────────────────────────────────────────
audit:
  action_log: true                       # Log all actions (default: true)
  system_trace: false                    # Trace syscalls (planned, not yet enforced)

# ─── User ─────────────────────────────────────────────────
user: agent                              # Default user (agent=non-root UID 60000, or "root")

# ─── Setup ────────────────────────────────────────────────
setup:                                   # Shell commands run during init (always runs as root)
  - "apt-get update && apt-get install -y python3"
  - "pip install -r requirements.txt"
setup_script: ~/my-project/setup.sh      # Host script injected + executed after setup commands
```

<!-- pause 2 -->

### Secure Defaults

When a section is omitted, the most restrictive option applies:

| Setting | Default |
|---------|---------|
| User | `agent` (non-root, UID 60000) |
| Network mode | `Isolated` (no network) |
| DNS mode | `Whitelist` (nothing resolves) |
| DNS allow list | Empty (no domains) |
| Mount permissions | `ReadOnly` |
| GPU access | `false` (no GPU devices) |
| Display forwarding | `false` (no display socket) |
| Display protocol | `auto` (Wayland preferred, X11 fallback) |
| Audio forwarding | `false` (no audio socket) |
| Audio protocol | `auto` (PipeWire preferred, PulseAudio fallback) |
| Seccomp profile | `default` (~130 safe syscalls) |
| Audit action log | `true` (always logging) |
| Tools allowed | Empty (all commands allowed) |
| Subnet base | `10.200` (pods get `10.200.{idx}.0/30`) |
| Setup script | `None` (no host script) |

---

## Filesystem Isolation

Every pod uses Linux OverlayFS for copy-on-write filesystem isolation.

### How It Works

<!-- output -->
```
┌─────────────┐
│   Agent      │ ← sees "merged" view
│   Process    │
└──────┬───────┘
       │
┌──────▼───────┐     ┌──────────────┐
│   Merged     │ = │  Upper (RW)  │ + Lower (RO)
│   (overlay)  │     │  (pod writes)│   (rootfs)
└──────────────┘     └──────────────┘
```

- **Lower layer** — minimal rootfs skeleton (read-only)
- **Upper layer** — all agent writes land here
- **Merged view** — what the agent sees (lower + upper combined)

The host filesystem is never modified directly.

### Rootfs Isolation

Pods do not see the host's full filesystem. The rootfs contains only:

- `/usr` — system binaries and libraries (bind-mounted read-only)
- `/bin`, `/sbin`, `/lib`, `/lib64` — essential system dirs (bind-mounted or symlinked)
- `/etc` — copied from host at init time (DNS, passwd, etc.)
- `/proc` — fresh procfs (only pod's own processes visible)
- `/sys` — sysfs (read-only)
- `/dev` — minimal device tree (see [Device Masking](#device-masking))
- `/tmp` — fresh tmpfs (not shared with host)

Directories like `/home`, `/var`, `/opt` start empty — agents cannot see host user data.

### Review Workflow

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# 1. Run agent
sudo envpod run my-agent -- python3 analyze.py

# 2. See what changed (filtered by tracking config — workspace files only)
sudo envpod diff my-agent
#   Added    /opt/results/output.csv    (1.2 MB)
#   Added    /opt/results/plots/        (directory)
#   (filtered by tracking config — use --all to see all changes)

# 2b. See ALL changes including system files (apt, dpkg, cache, etc.)
sudo envpod diff my-agent --all

# 3. Accept or reject (all at once, or file-by-file)
sudo envpod commit my-agent                              # accept tracked changes
sudo envpod commit my-agent --all                        # accept ALL changes
sudo envpod commit my-agent /opt/results/output.csv      # accept one file
sudo envpod commit my-agent --exclude /etc/config.ini    # accept all but one
# or
sudo envpod rollback my-agent                            # discard everything
```

### Tracking Config

By default, `envpod diff` and `envpod commit` filter out system noise (apt/dpkg state,
cache files, log files) and only show changes under workspace paths. This is controlled
by the `filesystem.tracking` section in pod.yaml:

- **watch** — paths to include in diff/commit (prefix-matched). Empty = show everything.
- **ignore** — paths to always exclude (even under watched paths).

Use `--all` on either command to bypass filtering and see/commit everything.

### Bind Mounts

Give the agent access to specific host directories:

<!-- output -->
```yaml
# In pod.yaml
filesystem:
  mounts:
    - path: ~/projects/my-app
      permissions: ReadWrite      # agent can modify files
    - path: ~/datasets
      permissions: ReadOnly       # agent can read, not write
```

Or at runtime:

<!-- no-exec -->
```bash
sudo envpod mount my-agent ~/data --readonly
sudo envpod unmount my-agent ~/data
```

### Working Directory Mount (`mount_cwd`)

The simplest way to give an agent access to your project is `mount_cwd`. When enabled, `envpod init` captures your current working directory and `envpod run` bind-mounts it read-only into the pod at the same path. Writes go to the COW overlay.

<!-- output -->
```yaml
# In pod.yaml
filesystem:
  mount_cwd: true    # captures $PWD at init time
```

Or use the `-w` flag at run time (no config needed):

<!-- no-exec -->
```bash
# Mount CWD on-the-fly — uses current directory if no cwd_path was captured
sudo envpod run my-agent -w -- claude

# Skip CWD mount even if pod.yaml says mount_cwd: true
sudo envpod run my-agent --no-mount-cwd -- sh
```

After the agent runs, review and apply changes with the standard workflow:

<!-- no-exec -->
```bash
sudo envpod diff my-agent        # see project file changes
sudo envpod commit my-agent      # apply back to the real directory
```

---

## Network Isolation

Each pod gets its own Linux network namespace with a virtual ethernet pair (veth), dedicated IP subnet, and an embedded per-pod DNS resolver.

### Subnet Configuration

By default, each pod gets an IP in the `10.200.{idx}.0/30` range (host gets `.1`, pod gets `.2`). You can override the subnet base in pod.yaml:

<!-- output -->
```yaml
network:
  subnet: "10.201"    # Pods get 10.201.{idx}.0/30 instead of 10.200.{idx}.0/30
```

Pods with the same subnet base share an IP range, enabling pod-to-pod communication via `internal_ports` and `allow_discovery`. Up to 254 pods can share a single subnet base.

### Network Modes

| Mode | Description |
|------|-------------|
| `Isolated` | Full isolation. DNS filtered + iptables rules enforce it. Default. |
| `Monitored` | Network access with monitoring and DNS policy. All queries logged. |
| `Unsafe` | Host network (no namespace). Requires explicit acknowledgment. |

### DNS Modes

| Mode | Description |
|------|-------------|
| `Whitelist` | Only explicitly allowed domains resolve. Everything else returns NXDOMAIN. Default. |
| `Blacklist` | All domains resolve except explicitly denied ones. |
| `Monitor` | All domains resolve. All queries are logged for audit. |

### DNS Configuration

<!-- output -->
```yaml
network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow:
      - api.anthropic.com        # exact match
      - "*.github.com"           # wildcard subdomain
      - pypi.org
    deny:
      - "*.internal"             # used in Blacklist mode
    remap:
      old-api.example.com: new-api.example.com  # redirect to different domain
      internal.dev: 10.0.0.5                     # redirect to specific IP
```

### How DNS Filtering Works

1. Pod's `/etc/resolv.conf` points to envpod's embedded DNS server
2. Every DNS query is intercepted and checked against the allow/deny list
3. Allowed queries are forwarded to the host's upstream DNS
4. Blocked queries return NXDOMAIN
5. Remapped queries are resolved to the target — if the target is an IP address, a synthetic DNS response is returned; if the target is a domain, a query for the target domain is forwarded instead
6. In `Isolated` mode, iptables rules inside the pod block DNS to any other server (prevents bypass)
7. Every DNS query is logged to the pod's audit trail as a `dns_query` action (visible via `envpod audit`)

### Live DNS Mutation

Update DNS policy on a running pod without restart:

<!-- no-exec -->
```bash
sudo envpod dns my-agent --allow newdomain.com
sudo envpod dns my-agent --deny suspicious.io
sudo envpod dns my-agent --remove-allow old-domain.com
```

### Port Forwarding

Expose pod services to the host or other pods. Three scopes — choose based on who needs access:

| Key | CLI | Format | Scope |
|-----|-----|--------|-------|
| `ports` | `-p` | `host:container[/proto]` | Localhost only (`127.0.0.1`) |
| `public_ports` | `-P` | `host:container[/proto]` | All network interfaces |
| `internal_ports` | `-i` | `container[/proto]` | Other pods only (pod subnet) |

<!-- output -->
```yaml
network:
  ports:
    - "8080:3000"    # curl localhost:8080 → pod:3000
  public_ports:
    - "9090:9090"    # curl host-ip:9090 from any machine
  internal_ports:
    - "3000"         # other pods → this pod:3000 (no host port)
```

CLI flags for per-run overrides (without editing pod.yaml):

<!-- no-exec -->
```bash
sudo envpod run my-pod -p 8080:3000 -- node server.js     # localhost only
sudo envpod run my-pod -P 9090:9090 -- node server.js     # all interfaces
sudo envpod run my-pod -i 3000 -- node server.js          # other pods only
```

All three flags can be combined. Port rules are set up on start and cleaned up automatically on exit or `envpod destroy`.

### Inter-Pod Networking

Pods can communicate with each other directly. Each pod has a routable IP in the `10.200.0.0/16` range — visible in `envpod ls`. Two features work together for full service-to-service communication:

**Step 1 — Open connections** (`internal_ports` on the service pod):

<!-- output -->
```yaml
# api-pod/pod.yaml
network:
  internal_ports: ["3000"]   # accept TCP from other pods on port 3000
```

This adds a FORWARD rule allowing `10.200.0.0/16 → this pod:3000`. No DNAT, no host port.

**Step 2 — Resolve by name** (bilateral: `allow_discovery` on service + `allow_pods` on client):

<!-- output -->
```yaml
# api-pod/pod.yaml — service
network:
  allow_discovery: true      # register as api-pod.pods.local
  internal_ports: ["3000"]

# client-pod/pod.yaml — client
network:
  allow_pods: ["api-pod"]    # permitted to resolve api-pod.pods.local
  mode: Isolated
  dns:
    mode: Whitelist
    allow: []
```

Discovery uses the central `envpod-dns` daemon. Start it once (runs as a system service or in a terminal):

<!-- no-exec -->
```bash
sudo envpod dns-daemon
```

Each pod's DNS server forwards `*.pods.local` queries to the daemon. The daemon enforces both sides: `api-pod.allow_discovery == true` AND the querying pod appears in its `allow_pods` list. Both conditions must hold; otherwise → NXDOMAIN.

Agent in client pod:
<!-- no-exec -->
```bash
curl http://api-pod.pods.local:3000/status   # resolves and connects
```

**Registry lifecycle:**
- Pod starts with `allow_discovery: true` → registers with daemon immediately
- Pod exits cleanly → daemon unregisters → NXDOMAIN for peers
- Pod crashes → stale entry GC'd on daemon startup (PID check)
- `envpod destroy` → daemon unregistered unconditionally
- Daemon not running → all `*.pods.local` → NXDOMAIN; pods unaffected otherwise
- Daemon started after pods → auto-registers already-running pods (no pod restart needed)

**Live mutation — no restart required:**

Use `envpod discover` to change discovery settings on a running pod:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod discover api-pod              # show current state
sudo envpod discover api-pod --on         # enable discoverability
sudo envpod discover api-pod --off        # hide from peer DNS
sudo envpod discover api-pod --add-pod client-pod
sudo envpod discover api-pod --remove-pod client-pod
sudo envpod discover api-pod --remove-pod '*'   # clear all allow_pods
```

Changes take effect immediately in the daemon and are written to `pod.yaml` for persistence.

**Without `allow_discovery`:** the pod is invisible to peer DNS. Use `envpod ls` to find its IP and tell the agent directly — `allow_discovery` is optional.

---

## Process Isolation

### PID Namespace

Each pod runs in its own PID namespace. The agent process is PID 1 inside the pod and cannot see host processes. A fresh `/proc` is mounted showing only the pod's own processes.

### cgroups v2

Resource limits are enforced via cgroups v2:

<!-- output -->
```yaml
processor:
  cores: 2.0            # CPU bandwidth limit (cpu.max)
  memory: "4GB"          # Hard memory limit (memory.max)
  cpu_affinity: "0-3"    # Pin to specific CPUs (cpuset.cpus)
```

### /proc Masking

When cgroup limits are set, `/proc/cpuinfo` and `/proc/meminfo` are masked to reflect the pod's limits, not the host's hardware. Tools like `nproc`, `free`, `htop`, and `lscpu` all report the pod's allocated resources.

The pod's `/proc/1/` has `root`, `cwd`, and `environ` individually masked (bind-mounted to `/dev/null`) to prevent host path traversal, while preserving `/proc/1/fd` for `/dev/fd` symlink resolution.

### seccomp-BPF

Syscall filtering restricts which kernel operations the agent can perform:

| Profile | Description |
|---------|-------------|
| `default` | ~130 safe syscalls. Blocks `ptrace`, `mount`, `reboot`, `kexec`, etc. |
| `browser` | Default + 7 extra syscalls needed by Chromium's zygote process. |

<!-- output -->
```yaml
security:
  seccomp_profile: browser    # for browser-based agents
```

### UTS Namespace

Each pod gets its own hostname (set to the pod name). Running `hostname` inside the pod returns the pod name, not the host's hostname.

### Coredump Prevention

Pods disable core dumps to prevent memory disclosure:

- `PR_SET_DUMPABLE` set to 0
- `RLIMIT_CORE` set to 0
- `PR_SET_NO_NEW_PRIVS` enabled

---

## Device Masking

By default, pods see a minimal `/dev` with only essential pseudo-devices. GPU and other hardware devices are hidden unless explicitly allowed.

### Default Devices (Always Available)

Every pod gets these essential devices:

| Device | Purpose |
|--------|---------|
| `/dev/null` | Discard output |
| `/dev/zero` | Zero bytes source |
| `/dev/full` | Always-full device |
| `/dev/random` | Cryptographic random |
| `/dev/urandom` | Non-blocking random |
| `/dev/tty` | Controlling terminal |
| `/dev/pts/*` | PTY support (devpts) |
| `/dev/shm` | Shared memory (pod-private tmpfs) |

Standard symlinks are also created: `/dev/stdin`, `/dev/stdout`, `/dev/stderr`, `/dev/fd`.

### GPU Passthrough

GPU devices (NVIDIA + DRI) are only exposed when explicitly opted in:

<!-- output -->
```yaml
devices:
  gpu: true
```

When `gpu: true`, these devices are bind-mounted if they exist on the host:

**NVIDIA:** `nvidia0`–`nvidia3`, `nvidiactl`, `nvidia-modeset`, `nvidia-uvm`, `nvidia-uvm-tools`

**DRI:** `dri/card0`, `dri/card1`, `dri/renderD128`, `dri/renderD129`

When GPU is **not** allowed (the default), GPU-related info paths are masked with empty read-only tmpfs:

- `/proc/driver/nvidia`
- `/sys/module/nvidia`
- `/sys/class/drm`
- `/sys/bus/pci/drivers/nvidia`

This prevents agents from fingerprinting host GPU hardware even through `/proc` and `/sys`.

Non-root users automatically get supplementary groups for GPU device access via `setgroups()`.

### Display Passthrough

Forward the host display to the pod for GUI applications:

<!-- output -->
```yaml
devices:
  display: true
  display_protocol: auto    # auto (default) | wayland | x11
```

Envpod auto-detects the display protocol. Wayland is checked first (via `$WAYLAND_DISPLAY` or `/run/user/{uid}/wayland-0`), then X11 fallback. Override with `display_protocol: wayland` or `display_protocol: x11` in pod.yaml.

- **Wayland** (preferred): mounts the Wayland compositor socket (from `$WAYLAND_DISPLAY` or `/run/user/<uid>/wayland-0`) to `/tmp/wayland-0` inside the pod. Wayland isolates clients by design — no cross-client keylogging, screenshots, or input injection (security audit I-04: LOW).
- **X11** (fallback): mounts `/tmp/.X11-unix` and runs `xhost +local:` to authorize connections. X11 has no client isolation — any connected app can keylog, screenshot, or inject input into other windows (security audit I-04: CRITICAL).

Non-root users automatically get supplementary groups for display socket access via `setgroups()`.

Combine with the `--enable-display` (`-d`) CLI flag to set the correct environment variables:

- **Wayland**: sets `WAYLAND_DISPLAY=/tmp/wayland-0`, `XDG_RUNTIME_DIR=/tmp`, `XCURSOR_THEME=Adwaita`, `XCURSOR_SIZE=24`
- **X11**: sets `DISPLAY=:N`, `XCURSOR_THEME=Adwaita`, `XCURSOR_SIZE=24`, runs `xhost`

<!-- no-exec -->
```bash
sudo envpod run my-agent -d -- google-chrome --ozone-platform=wayland https://google.com
```

> **Note:** Firefox on Ubuntu 24.04 is distributed as a snap, and snaps do not work inside namespace-based pods. Use Google Chrome (deb) or Chromium instead.

### Audio Passthrough

Forward host audio to the pod for playback and recording:

<!-- output -->
```yaml
devices:
  audio: true
  audio_protocol: auto    # auto (default) | pipewire | pulseaudio
```

Envpod auto-detects the audio protocol. PipeWire is checked first (`/run/user/{uid}/pipewire-0`), then PulseAudio fallback. Override with `audio_protocol: pipewire` or `audio_protocol: pulseaudio` in pod.yaml.

- **PipeWire** (preferred): mounts `/run/user/<uid>/pipewire-0` to `/tmp/pipewire-0`. PipeWire has finer-grained per-stream permissions (security audit I-05: MEDIUM).
- **PulseAudio** (fallback): mounts PulseAudio socket file to `/tmp/pulse-native` (bypasses 0700 directory permission). Copies auth cookie with world-readable permissions. PulseAudio gives unrestricted microphone access (security audit I-05: HIGH).

ALSA device nodes (`/dev/snd/*`) are always mounted when `audio: true`.

Non-root users automatically get supplementary groups for audio socket access via `setgroups()`.

Combine with the `--enable-audio` (`-a`) CLI flag to set the correct environment variables:

- **PipeWire**: sets `PIPEWIRE_RUNTIME_DIR=/tmp`, `DBUS_SESSION_BUS_ADDRESS=disabled:`
- **PulseAudio**: sets `PULSE_SERVER=unix:/tmp/pulse-native`, copies auth cookie to `/tmp/pulse-cookie`, sets `PULSE_COOKIE=/tmp/pulse-cookie`, `DBUS_SESSION_BUS_ADDRESS=disabled:`

<!-- no-exec -->
```bash
sudo envpod run my-agent -d -a --user browseruser -- google-chrome https://youtube.com
```

### Extra Devices

Pass through arbitrary device nodes:

<!-- output -->
```yaml
devices:
  extra:
    - "/dev/fuse"       # FUSE filesystem support
    - "/dev/kvm"        # KVM virtualization
```

Extra devices are optional — if they don't exist on the host, they are silently skipped.

### Desktop Environment

Auto-install a desktop environment into the pod during `envpod init`:

<!-- output -->
```yaml
devices:
  desktop_env: xfce    # none (default) | xfce | openbox | sway
```

| Value | Packages | Size |
|-------|----------|------|
| `xfce` | xfce4, xfce4-terminal, dbus-x11 | ~200 MB |
| `openbox` | openbox, tint2, xterm | ~50 MB |
| `sway` | sway, foot terminal | ~150 MB (Wayland-native) |

Pairs with `web_display` (noVNC/WebRTC) for browser-based access, or `devices.display: true` for host display passthrough. The `desktop` preset uses `desktop_env: xfce` with noVNC — see `examples/desktop.yaml`.

---

## Web Display (noVNC)

Run a full graphical desktop inside a pod, accessible from any browser. No host display, Wayland, or X11 needed. Works on headless servers and SSH sessions.

<!-- output -->
```yaml
web_display:
  type: novnc              # none (default), novnc (CE), webrtc (Premium)
  port: 6080               # host port for browser access
  resolution: "1920x1080"  # virtual display resolution
  audio: true              # PulseAudio + Opus/WebM audio streaming
  audio_port: 6081         # audio WebSocket port
  file_upload: true        # upload button in noVNC panel
  upload_port: 5080        # upload server port
```

Features:
- **Auto-branding** — envpod logo, favicon, and page title (shows pod name)
- **Auto-connect** — skips the VNC connect dialog (opt out with `?autoconnect=false`)
- **Audio streaming** — PulseAudio null sink -> Opus/WebM via WebSocket (click speaker icon in panel) [beta]
- **File upload** — click upload icon in side panel to send files to `/tmp/uploads/` inside the pod (upload-only, no download — files come out via `envpod diff`/`commit`)
- **Guardian cgroup** — display services (Xvfb, x11vnc, websockify) survive pod freeze/thaw

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Desktop pod example
sudo envpod init my-desktop --preset desktop
sudo envpod run my-desktop -b -- startxfce4
# Open http://localhost:6080 in your browser
```

Pairs with `devices.desktop_env` (xfce, openbox, sway) for a complete desktop experience.

### Default Ports & Services

All ports are forwarded to `127.0.0.1` (localhost only) automatically when `web_display.type: novnc` is set. No manual port configuration needed.

| Port | Service | Direction | Configurable | Notes |
|------|---------|-----------|-------------|-------|
| 6080 | noVNC (websockify) | browser → pod | `web_display.port` | Main display — open this in your browser |
| 6081 | Audio WebSocket | browser → pod | `web_display.audio_port` | Opus/WebM audio stream (when `audio: true`) |
| 5080 | File upload server | browser → pod | `web_display.upload_port` | Python HTTP server (when `file_upload: true`) |
| 5900 | VNC (x11vnc) | internal | — | Not exposed to host — websockify bridges it |
| 5711 | Audio proxy (GStreamer) | internal | — | Not exposed — audio websockify bridges it |

**Internal pod IP:** Each pod gets a unique IP in the `10.200.x.0/30` range. The pod-side IP is `10.200.x.2`, host-side veth is `10.200.x.1`. Port forwarding uses iptables DNAT from `127.0.0.1:<port>` to the pod IP.

**Upload location:** Files uploaded via the noVNC upload button are saved to `/tmp/uploads/` inside the pod. This is upload-only — files come out through the governance layer (`envpod diff` / `envpod commit`).

**Auto-restart:** All supervisor processes (Xvfb, x11vnc, websockify, audio proxy, audio websockify, upload server) run in auto-restart loops. If any process crashes, it restarts within 1 second.

**Guardian cgroup:** Display, audio, and upload processes are migrated to a `guardian/` subcgroup so they survive `envpod lock` / `envpod unlock` (cgroup freeze/thaw). The user's application runs in the `app/` subcgroup.

---

### Why This Matters

Without device masking, a pod with access to the full host `/dev` could:

- Access GPU hardware for unauthorized computation
- Fingerprint host hardware through NVIDIA/DRI device nodes
- Access USB devices, storage controllers, or other peripherals
- Potentially exploit device driver vulnerabilities

With device masking, the default pod sees only the 6 essential pseudo-devices needed to function. Everything else requires explicit opt-in.

---

## Security Hardening

### Security Configuration

<!-- output -->
```yaml
security:
  seccomp_profile: default    # default | browser
  shm_size: "256MB"           # /dev/shm size (default: 64MB)
```

### Browser Agents

Browser-based agents (Chromium, Playwright, Puppeteer) need special configuration:

<!-- output -->
```yaml
security:
  seccomp_profile: browser    # allows Chromium zygote syscalls
  shm_size: "256MB"           # Chromium needs large /dev/shm

devices:
  gpu: true                    # WebGL rendering
  display: true                # Display socket for GUI mode (auto-detects Wayland/X11)
  audio: true                  # Audio socket + ALSA for media playback (auto-detects PipeWire/PulseAudio)
  # display_protocol: wayland  # Uncomment to enforce Wayland (more secure)
  # audio_protocol: pipewire   # Uncomment to enforce PipeWire (finer permissions)
```

Run with display and audio forwarding:

<!-- no-exec -->
```bash
sudo envpod run browser-agent -d -a --user browseruser -- google-chrome https://youtube.com
```

> For maximum security, use `browser-wayland.yaml` which enforces Wayland + PipeWire (I-04 LOW, I-05 MEDIUM) instead of auto-detection (I-04 CRITICAL for X11, I-05 HIGH for PulseAudio).

### Tool Whitelist

Restrict which commands the agent can execute:

<!-- output -->
```yaml
tools:
  allowed_commands:
    - /bin/bash
    - /bin/sh
    - /usr/bin/git
    - /usr/bin/python3
```

When `allowed_commands` is empty (default), all commands are allowed. When populated, any command not on the list is blocked and logged as `tool_blocked` in the audit trail.

### Defense in Depth

Envpod layers multiple isolation mechanisms:

| Layer | Mechanism | Protects Against |
|-------|-----------|-----------------|
| Mount namespace | OverlayFS | Host filesystem modification |
| Rootfs isolation | Minimal skeleton | Data exfiltration from host dirs |
| PID namespace | Fresh procfs | Process enumeration |
| Network namespace | veth + DNS filter | Unauthorized network access |
| cgroups v2 | Resource limits | Resource exhaustion |
| seccomp-BPF | Syscall filter | Kernel exploitation |
| Device masking | Minimal /dev | Hardware access, GPU fingerprinting |
| /proc masking | Cgroup-aware values | Hardware fingerprinting |
| Non-root default | `agent` user (UID 60000) | iptables modification, raw sockets |
| Coredump prevention | prctl + rlimit | Memory disclosure |
| UTS namespace | Pod hostname | Host identity disclosure |
| Vault encryption | ChaCha20-Poly1305 AEAD | Secret exfiltration from pod dir |

<!-- pause 2 -->

### Jailbreak Test Script

Envpod ships a comprehensive test script that probes all isolation boundaries from inside a pod. Use it to verify your pod is secure before trusting it with real workloads.

<!-- no-exec -->
```bash
# Run all 48 tests
sudo envpod run my-agent -- bash /usr/local/share/envpod/examples/jailbreak-test.sh

# Or if running from the source repo
sudo envpod run my-agent -- bash /path/to/examples/jailbreak-test.sh
```

The script tests **8 categories** with **48 individual probes**:

| Category | Tests | What It Probes |
|----------|-------|----------------|
| Filesystem (F-01 to F-10) | 10 | Overlay escape, mount/unmount, /sys write, mknod, device access |
| PID Namespace (P-01 to P-04) | 4 | PID 1 identity, host process visibility, ptrace, cross-ns signals |
| Network (N-01 to N-08) | 8 | Netns isolation, DNS resolver, external DNS bypass, IPv6 bypass, iptables, raw sockets |
| Seccomp (S-01 to S-08) | 8 | mount, unshare, ptrace, init_module, mknod, keyctl, bpf, reboot syscalls |
| Process Hardening (H-01 to H-04) | 4 | NO_NEW_PRIVS, DUMPABLE, RLIMIT_CORE, SUID escalation |
| Cgroups (C-01 to C-03) | 3 | Memory limit, CPU limit, PID limit |
| Info Leakage (I-01 to I-06) | 6 | /proc/cpuinfo, /proc/meminfo, /proc/stat, hostname, kernel version, GPU |
| Advanced (A-01 to A-05) | 5 | Symlink traversal, /proc/self/exe, fd passing, TOCTOU, /dev/mem |

**Options:**

<!-- no-exec -->
```bash
# Machine-readable JSON output
jailbreak-test.sh --json

# Test a single category
jailbreak-test.sh --category seccomp
```

**Exit codes:** 0 = all walls held, 1 = CRITICAL or HIGH breach detected, 2 = MEDIUM or LOW gaps only.

**Severity levels:** CRITICAL (actual escape possible), HIGH (weakened isolation), MEDIUM (information leak), LOW (hardening gap), INFO (known/documented behavior).

**Known gaps (v0.1):** The script will surface these expected limitations:

| Test | Gap | Severity | Applies When |
|------|-----|----------|--------------|
| N-05 | Pod root can `iptables -F` (no user namespace) | CRITICAL | `--root` only |
| N-06 | Raw sockets available (root, no user namespace) | HIGH | `--root` only |
| I-03 | /proc/stat leaks host CPU counters | MEDIUM | Always |

<!-- pause 2 -->

N-05 and N-06 only affect pods running as root (`--root` flag or `user: root` in pod.yaml). The default non-root `agent` user is immune — 17/17 pod boundary tests pass. IPv6 DNS bypass (N-04) is blocked since v0.1 (IPv6 is disabled in the pod network namespace); full ip6tables rules are planned for v0.2.

The DNS policy engine is the primary enforcement layer; iptables is defense-in-depth.

---

## Credential Vault

The vault stores secrets outside the pod. Secrets are injected as environment variables at runtime — they never appear in pod config files, overlay layers, or audit log values.

### Store a Secret

<!-- no-exec -->
```bash
sudo envpod vault my-agent set ANTHROPIC_API_KEY
# Reads from stdin — never appears in shell history
```

### Use in Pod

Vault secrets are automatically available as environment variables:

<!-- no-exec -->
```bash
sudo envpod run my-agent -- env | grep ANTHROPIC
# ANTHROPIC_API_KEY=sk-ant-...
```

### Manage Secrets

<!-- no-exec -->
```bash
sudo envpod vault my-agent list      # list key names
sudo envpod vault my-agent get KEY   # retrieve value
sudo envpod vault my-agent rm KEY    # delete
```

### Security Properties

- **Encrypted at rest** — secrets are encrypted with ChaCha20-Poly1305 (AEAD) using a per-pod 256-bit key (`vault.key`). Each file stores `[12-byte nonce][ciphertext + 16-byte Poly1305 tag]`. A backup that copies `vault/` without `vault.key` gets nothing useful.
- Vault directory is mode `0700`, each secret file is mode `0600`, key file is mode `0600`
- Secret values never appear in audit logs
- Secrets are not part of the overlay — they cannot be committed to the host
- Key names must be alphanumeric with underscores
- Secrets are injected as environment variables at pod runtime — the agent never sees raw credentials in config files
- **Automatic migration** — plaintext vaults from older versions are auto-encrypted on first access

### Vault Proxy Injection (v0.2)

For high-security deployments, vault proxy injection eliminates credential exposure entirely. Instead of injecting secrets as environment variables (where a compromised agent can read and exfiltrate them), the proxy intercepts API requests at the transport layer and injects real credentials — the agent never has access to real API keys.

**Setup:**

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Store the real API key
sudo envpod vault my-agent set ANTHROPIC_API_KEY

# Bind it to the API domain
sudo envpod vault my-agent bind ANTHROPIC_API_KEY api.anthropic.com "Authorization: Bearer {value}"

# Run the agent — proxy handles auth transparently
sudo envpod run my-agent -- claude
```

Or configure in `pod.yaml`:

<!-- output -->
```yaml
vault:
  proxy: true
  bindings:
    - key: ANTHROPIC_API_KEY
      domain: api.anthropic.com
      header: "Authorization: Bearer {value}"
```

**How it works:**

1. At `envpod init`: a per-pod ephemeral CA is generated and installed into the pod's TLS trust store
2. At `envpod run`: DNS remap routes bound domains (e.g., `api.anthropic.com`) to the host-side veth IP where the proxy listens on port 443
3. The agent makes normal HTTPS requests with a dummy API key
4. The proxy terminates TLS (using a leaf cert signed by the pod's CA), strips the dummy auth header, injects the real secret from the vault, and forwards to the real API server
5. Audit trail logs domain + key name used (never the secret value)

**Security properties:**

- The proxy runs **outside** the pod's network namespace — the agent cannot tamper with it
- Credentials never enter the agent's address space (not in env vars, config, or memory)
- Per-pod ephemeral CA means compromising one pod's trust store doesn't affect others
- Security audit (`envpod audit --security`) checks for misconfigurations: V-01 (bindings without proxy), V-02 (proxy with unsafe network), V-03 (bound domain not in DNS allow list)

<!-- pause 2 -->

**Managing bindings:**

<!-- no-exec -->
```bash
sudo envpod vault my-agent bindings              # list all bindings
sudo envpod vault my-agent unbind ANTHROPIC_API_KEY  # remove a binding
```

See [Pod Configuration Reference — Vault](POD-CONFIG.md#vault) for full details.

---

## Action Queue & Undo

### Action Tiers

| Tier | Behavior | Example |
|------|----------|---------|
| `immediate` | Execute now, overlay protects | File modifications |
| `delayed` | Hold N seconds, auto-execute unless cancelled | Email, Slack messages |
| `staged` | Hold until human approves | Payments, prod deployments |
| `blocked` | Denied by default | Destructive operations |

### Workflow

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Submit an action
sudo envpod queue my-agent add --tier staged --description "deploy to production"

# View pending actions
sudo envpod queue my-agent
# [a1b2c3d4]  staged   deploy to production    NEEDS APPROVAL

# Approve or cancel
sudo envpod approve my-agent a1b2c3d4
sudo envpod cancel my-agent a1b2c3d4
```

### Undo

<!-- no-exec -->
```bash
# List undo-able actions
sudo envpod undo my-agent

# Undo a specific action
sudo envpod undo my-agent a1b2c3d4

# Undo everything
sudo envpod undo my-agent --all
```

---

## Monitoring & Alerts

Install a monitoring policy to detect anomalous behavior:

<!-- output -->
<!-- type-delay 0.02 -->
```yaml
# monitoring-policy.yaml
check_interval_secs: 5

rules:
  - name: memory_high
    condition:
      type: resource_threshold
      resource: memory
      max_percent: 90.0
    response:
      type: restrict
      memory_bytes: 268435456      # 256MB

  - name: action_flood
    condition:
      type: max_actions_per_minute
      limit: 200
    response:
      type: freeze

  - name: exfiltration_pattern
    condition:
      type: forbidden_sequence
      actions: [vault_get, dns_query]
      window_secs: 10
    response:
      type: freeze

  - name: budget_exceeded
    condition:
      type: forbidden_action
      action: budget_exceeded
    response:
      type: freeze
```

<!-- pause 2 -->

<!-- no-exec -->
```bash
sudo envpod monitor my-agent set-policy monitoring-policy.yaml
sudo envpod monitor my-agent alerts
```

---

## Remote Control

Send control commands to a running pod:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod remote my-agent freeze       # pause all processes
sudo envpod remote my-agent resume       # unpause
sudo envpod remote my-agent kill         # terminate
sudo envpod remote my-agent status       # query status
sudo envpod remote my-agent alerts       # get monitoring alerts

# Dynamically restrict resources
sudo envpod remote my-agent restrict --payload '{"cpu_cores": 0.5, "memory_bytes": 268435456}'
```

---

## Audit Trail

Every action inside a pod is logged to `{pod_dir}/audit.jsonl`. The audit log is append-only and records:

| Field | Description |
|-------|-------------|
| `timestamp` | ISO 8601 timestamp |
| `pod_name` | Pod that generated the event |
| `action` | Action type (see below) |
| `detail` | Human-readable details |
| `success` | Whether the action succeeded |

### Audit Actions

| Action | Description |
|--------|-------------|
| `create` | Pod created |
| `start` | Process started |
| `stop` | Process stopped |
| `kill` | Pod force-killed (`envpod kill`) |
| `freeze` | Pod frozen |
| `resume` | Pod resumed |
| `destroy` | Pod destroyed |
| `diff` | Filesystem diff requested |
| `commit` | Changes committed to host |
| `rollback` | Changes discarded |
| `dns_query` | DNS query resolved (domain, type, decision) |
| `tool_blocked` | Command rejected by tool whitelist |
| `set_limits` | Resource limits changed |
| `queue_submit` | Action submitted to queue |
| `queue_approve` | Queued action approved |
| `queue_cancel` | Queued action cancelled |
| `budget_exceeded` | Budget limit reached (e.g. max_duration) |
| `vault_set` | Secret stored |
| `vault_get` | Secret retrieved |
| `vault_remove` | Secret removed |
| `monitor_alert` | Monitoring rule triggered |
| `monitor_freeze` | Pod frozen by monitor agent |
| `undo` | Action undone |
| `restore` | Pod state restored after host reboot |

---

## Snapshots

Snapshots capture the pod's overlay (upper/) at a point in time. Use them to checkpoint work, experiment safely, and promote proven states into reusable base pods.

### Configuration

<!-- output -->
```yaml
snapshots:
  auto_on_run: true    # auto-snapshot at the start of each `envpod run` (default: false)
  max_keep: 10         # max auto-snapshots to retain (default: 10)
```

### CLI Commands

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Create a named snapshot
sudo envpod snapshot my-agent create -n "before-refactor"

# List all snapshots
sudo envpod snapshot my-agent ls

# Restore a snapshot (replaces current upper/ with snapshot)
sudo envpod snapshot my-agent restore <id>

# Delete a snapshot
sudo envpod snapshot my-agent destroy <id>

# Prune old auto-snapshots (keeps named snapshots)
sudo envpod snapshot my-agent prune

# Promote a snapshot to a standalone base pod (instantly clonable)
sudo envpod snapshot my-agent promote <id> my-base
```

### Snapshot Details

| Feature | Description |
|---------|-------------|
| **Named snapshots** | Created manually with `-n label`. Never pruned automatically. |
| **Auto-snapshots** | Created at start of `envpod run` when `auto_on_run: true`. Labeled `auto`. |
| **Prune policy** | `prune` only removes auto-snapshots beyond `max_keep`. Named/manual snapshots are always preserved. |
| **Promote** | `promote <id> <base-name>` copies the snapshot upper/ as a base pod — instantly clonable via `envpod clone`. |
| **Restore** | Replaces the current overlay upper/ with the snapshot contents. Pod must be stopped. |
| **Storage** | Each snapshot stored at `{pod_dir}/snapshots/{id}/` with an `index.json` manifest. |

### Dashboard

The web dashboard (Snapshots tab) provides a visual timeline of all snapshots with one-click Restore, Promote, and Delete actions.

---

## Live Mutation

All isolation walls are dynamically mutable during execution without restart or state loss:

| Mutation | Command |
|----------|---------|
| Mount/unmount paths | `envpod mount` / `envpod unmount` |
| Add/remove DNS rules | `envpod dns --allow` / `--deny` |
| Freeze/resume | `envpod lock` / `envpod remote resume` |
| Restrict resources | `envpod remote restrict --payload {...}` |
| Update monitoring | `envpod monitor set-policy` |
| Store/revoke secrets | `envpod vault set` / `envpod vault rm` |

This enables:
- **Incident response without state loss** — restrict a pod while keeping it running
- **Progressive trust** — grant capabilities as the agent earns trust
- **Ephemeral access** — mount a directory for one operation, then unmount

---

## Host App Auto-Mount

Mount host applications into a pod without reinstalling them. Envpod resolves each binary via `which` + `ldd`, then bind-mounts the binary, its shared libraries, and known data directories read-only.

### Configuration

<!-- output -->
```yaml
filesystem:
  apps:
    - google-chrome
    - python3
    - node
    - git
```

Each entry is a binary name. Envpod automatically resolves:
- The binary path (via `which`)
- All shared library dependencies (via `ldd`)
- Known data directories (e.g. Chrome profile dirs, Python site-packages)

All mounts are **read-only** — the app runs inside the pod but cannot modify host files.

### Usage

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Init with host apps
sudo envpod init my-pod -c examples/host-apps.yaml

# Or add apps to any existing pod.yaml
sudo envpod init my-pod
# Then add filesystem.apps to pod.yaml and re-init
```

This avoids the overhead of reinstalling large applications (Chrome, VS Code, etc.) inside every pod.

---

## Presets & Example Configs

### Built-in Presets

18 presets are compiled into the binary — no config files needed. Use `envpod presets` to list all, `--preset <name>` to use one directly, or run `envpod init <name>` with no flags for an interactive wizard.

<!-- no-exec -->
```bash
sudo envpod init my-agent --preset claude-code    # direct
sudo envpod init my-agent                          # interactive wizard
```

**Coding Agents**

| Preset | Description | Network | Setup |
|--------|-------------|---------|-------|
| `claude-code` | Anthropic Claude Code CLI | Monitored, Claude + GitHub | `curl` installer |
| `codex` | OpenAI Codex CLI | Monitored, OpenAI + GitHub | nvm + `npm install -g @openai/codex` |
| `gemini-cli` | Google Gemini CLI | Monitored, Google + GitHub | nvm + `npm install -g @google/gemini-cli` |
| `opencode` | OpenCode terminal agent | Monitored, multi-LLM | `curl` installer |
| `aider` | Aider AI pair programmer | Monitored, multi-LLM | `pip install aider-chat` |
| `swe-agent` | SWE-agent autonomous coder | Monitored, LLM + GitHub | `pip install sweagent` |

**Frameworks**

| Preset | Description | Network | Setup |
|--------|-------------|---------|-------|
| `langgraph` | LangGraph workflows | Monitored, LLM + PyPI | `pip install langgraph langchain-openai` |
| `google-adk` | Google Agent Development Kit | Monitored, Google + PyPI | `pip install google-adk` |
| `openclaw` | OpenClaw messaging assistant | Monitored, LLM + messaging | nvm + `npm install -g openclaw` |

**Browser Agents**

| Preset | Description | Network | Setup |
|--------|-------------|---------|-------|
| `browser-use` | Browser-use web automation | Monitored, Blacklist | `pip install browser-use playwright` + Chromium |
| `playwright` | Playwright browser automation | Monitored, Blacklist | `pip install playwright` + Chromium |
| `browser` | Headless Chrome sandbox | Monitored, Blacklist | Chrome (check host, else install) |

**Environments**

| Preset | Description | Network | Setup |
|--------|-------------|---------|-------|
| `devbox` | General dev sandbox | Monitored, Blacklist | None |
| `python-env` | Python environment | Monitored, PyPI whitelist | numpy, pandas, matplotlib, scipy, scikit-learn |
| `nodejs` | Node.js environment | Monitored, npm + Node.js | nvm + Node.js 22 |
| `web-display` | noVNC desktop | Monitored | Supervisor-managed |
| `desktop` | XFCE desktop via noVNC | Monitored, Blacklist | XFCE4 + Chrome (~550MB, 2-4 min) |
| `vscode` | VS Code in the browser | Monitored, Blacklist | code-server |

### Additional Example Configs

42 example configs total in `examples/` — the 18 presets above plus configs for specialized use cases:

| Config | Description |
|--------|-------------|
| `basic-cli.yaml` | Minimal CLI sandbox, no network |
| `basic-internet.yaml` | CLI with DNS monitor mode |
| `coding-agent.yaml` | General-purpose coding agent |
| `browser-wayland.yaml` | Chrome with secure Wayland + PipeWire |
| `ml-training.yaml` | GPU ML training (torch, numpy, pandas) |
| `hardened-sandbox.yaml` | Maximum isolation, no network |
| `fuse-agent.yaml` | FUSE filesystem support |
| `demo-pod.yaml` | Minimal quick demo |
| `monitoring-policy.yaml` | Example monitoring rules |
| `discovery-service.yaml` | Pod discovery (target) |
| `discovery-client.yaml` | Pod discovery (client) |
| `jetson-orin.yaml` | NVIDIA Jetson Orin (ARM64) |
| `raspberry-pi.yaml` | Raspberry Pi 4/5 (ARM64) |
| `web-display-novnc.yaml` | noVNC web display |
| `desktop-openbox.yaml` | Openbox ultra-minimal desktop |
| `desktop-sway.yaml` | Sway Wayland-native desktop |
| `desktop-user.yaml` | Desktop with host user environment |
| `desktop-web.yaml` | Desktop with Chrome + VS Code |
| `workstation.yaml` | Standard workstation |
| `workstation-full.yaml` | Full workstation (desktop, GPU, audio) |
| `workstation-gpu.yaml` | GPU-focused workstation |
| `gimp.yaml` | GIMP image editor in desktop pod |
| `host-apps.yaml` | Auto-mount host apps (no reinstall) |
| `clone-user.yaml` | Clone host user environment |

### Security Testing

| Script | Description |
|--------|-------------|
| `jailbreak-test.sh` | 48-test isolation probe script (see [Jailbreak Test Script](#jailbreak-test-script)) |

### Using Presets & Examples

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Use a preset (recommended — auto-installs everything)
sudo envpod init my-agent --preset claude-code

# Or use a config file for full control
sudo envpod init my-agent -c examples/coding-agent.yaml

# Store API keys
sudo envpod vault my-agent set ANTHROPIC_API_KEY

# Run the agent
sudo envpod run my-agent -- /bin/bash
```

---

## Troubleshooting

### "Permission denied" Errors

Envpod requires root for namespaces, cgroups, overlayfs, and network setup:

<!-- no-exec -->
```bash
sudo envpod <command>
```

### Pod Cannot Reach the Internet

1. Check DNS mode — `Whitelist` with an empty allow list blocks everything:
<!-- no-exec -->
   ```bash
   sudo envpod dns my-agent --allow api.example.com
   ```

2. Verify IP forwarding is enabled:
<!-- no-exec -->
   ```bash
   cat /proc/sys/net/ipv4/ip_forward    # should be 1
   sudo sysctl -w net.ipv4.ip_forward=1
   ```

3. Check iptables rules:
<!-- no-exec -->
   ```bash
   sudo iptables -L -n -v
   sudo iptables -t nat -L -n -v
   ```

### Host Internet Breaks After Creating a Pod

See [INSTALL.md](INSTALL.md#troubleshooting-host-internet-breaks-after-creating-a-pod) for detailed diagnosis and fixes. Common causes:

- UFW + iptables-nft conflict
- conntrack module side effects
- Docker iptables interaction

### Agent Cannot Find Commands

If the agent gets "command not found" errors, the command may not exist in the minimal rootfs. Verify the command exists on the host:

<!-- no-exec -->
```bash
which python3    # should return a path under /usr or /bin
```

Commands in `/usr/bin`, `/bin`, `/sbin`, `/lib` are available inside the pod (bind-mounted read-only from host).

### GPU Not Visible Inside Pod

Ensure your pod.yaml has GPU enabled:

<!-- output -->
```yaml
devices:
  gpu: true
```

And verify GPU devices exist on the host:

<!-- no-exec -->
```bash
ls /dev/nvidia*      # NVIDIA devices
ls /dev/dri/         # DRI devices
```

### "warning: network isolation failed" or "warning: cgroup creation failed"

These warnings appear during `envpod init` when the kernel or system doesn't support the required isolation feature. The pod is still created and usable, but with degraded isolation:

- **cgroup creation failed** — the pod runs without CPU/memory resource limits
- **cgroup limit failed** — limits were not applied (pod runs unconstrained)
- **network isolation failed** — the pod uses host networking instead of an isolated network namespace
- **NAT setup failed** — the pod has a network namespace but cannot reach the internet

Check that you have the required kernel features (cgroups v2, network namespaces) and that `iptables` and `ip` commands are installed. See [INSTALL.md](INSTALL.md) for prerequisites.

### Diff Shows Unexpected Files

Files in the overlay's upper layer appear in `envpod diff`. Infrastructure files (`.wh.*` whiteout files used by OverlayFS) are automatically excluded. If you see unexpected files, the agent likely created them. Use `envpod rollback` to discard.

### State Directory

All pod data is stored under `/var/lib/envpod/` by default:

<!-- output -->
```
/var/lib/envpod/
├── state/                  # Pod handle JSON files
├── pods/{uuid}/            # Per-pod data
│   ├── rootfs/             # Minimal rootfs skeleton
│   ├── upper/              # OverlayFS upper layer (agent writes)
│   ├── work/               # OverlayFS work directory
│   ├── merged/             # OverlayFS merged mount point
│   ├── audit.jsonl         # Audit log
│   ├── vault.key           # Per-pod encryption key (32 bytes, 0600)
│   ├── vault/              # Encrypted secrets (0700 dir, 0600 files)
│   └── output.log          # stdout/stderr capture
└── netns_index/            # Network namespace index allocation
```

Override with `--dir` or `ENVPOD_DIR`:

<!-- no-exec -->
```bash
sudo envpod --dir /opt/envpod ls
export ENVPOD_DIR=/opt/envpod
sudo -E envpod ls
```

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
