[![Release](https://img.shields.io/badge/release-v0.1.0-brightgreen)](https://github.com/markamo/envpod-ce/releases/tag/v0.1.0)
[![License](https://img.shields.io/badge/license-BSL--1.1-blue)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20x86__64%20%7C%20ARM64-lightgrey)](docs/EMBEDDED.md)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)](https://www.rust-lang.org/)

# envpod

> **EnvPod v0.1.0** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc.

**Docker isolates. Envpod governs.**

Every AI agent runs inside a **pod** — an isolated environment with four walls (memory, filesystem, network, processor) and a governance ceiling (policy engine, tool security, vault, monitoring, audit). All agent actions are isolated, monitored, and reversible.

> **Why not just Docker?** Docker isolates processes but provides zero governance. No file change review, no action queue, no credential vault, no undo. Envpod adds the governance layer on top of the same Linux primitives. See [Docker vs Envpod](docs/COMPARE-DOCKER.md) for a full comparison.

## Why Envpod

AI agents are powerful but dangerous without boundaries. They can read sensitive files, exfiltrate data over the network, consume unbounded resources, and make irreversible changes — all while running with your credentials.

Existing sandboxes (Docker, E2B, Firecrackers) provide isolation but zero governance. Envpod adds the governance layer: every file change goes through copy-on-write review, every DNS query is logged, every action can be undone.

## Features

**Filesystem Isolation** — OverlayFS copy-on-write. Agent writes go to an overlay, never the host. Review changes with `diff`, accept with `commit`, discard with `rollback`.

**Network Isolation** — Each pod gets its own network namespace with veth pairs. Embedded DNS resolver per pod with whitelist, blacklist, or monitor modes. Every DNS query is logged.

**Process Isolation** — PID namespace, cgroups v2 (CPU, memory, PID limits), seccomp-BPF syscall filtering. Processes in the pod cannot see or signal host processes.

**Credential Vault** — Secrets stored encrypted (ChaCha20-Poly1305), injected as environment variables at runtime. **Vault proxy injection** (v0.2) goes further: a transparent HTTPS proxy intercepts API requests, strips dummy auth headers, and injects real credentials at the transport layer — the agent never has access to real API keys in env vars, memory, or config files.

**Web Dashboard** — Browser-based fleet management UI (`envpod dashboard`). Real-time pod overview with resource monitoring, audit trail viewer, filesystem diff inspector, and one-click commit/rollback/freeze/resume actions.

**Action Queue** — Actions classified by reversibility: immediate (COW-protected), delayed (auto-execute after timeout), staged (human approval required), blocked (denied).

**Audit Trail** — Append-only JSONL logs for every action. Static security analysis of pod configurations via `envpod audit --security`.

**Monitoring Agent** — Background policy engine polls resource usage and can autonomously freeze or restrict a pod based on configurable rules.

**Remote Control** — Freeze, resume, kill, or restrict a running pod in real time via `envpod remote`.

**Live DNS Mutation** — Update a pod's DNS allow/deny lists without restarting via `envpod dns`.

**Undo Registry** — Every executed action records its undo mechanism. `envpod undo` reverses any reversible action.

**Display + Audio Forwarding** — GPU passthrough, Wayland/X11 display forwarding, PipeWire/PulseAudio audio forwarding for GUI agents.

**Host App Auto-Mount** — List apps in `pod.yaml` and envpod resolves binaries, shared libraries, and data directories via `which` + `ldd`, then bind-mounts them read-only. No reinstalling Chrome, Python, or Node inside every pod — instant, zero disk overhead.

**Clone Host User** — `host_user: { clone_host: true }` clones your username, shell, dotfiles, and workspace directories into the pod with COW isolation. The agent works in your real environment — your files, your tools, your config — but every change is staged for review. Sensitive paths (`.ssh`, `.gnupg`, `.aws`) excluded by default.

**Mount Working Directory** — `filesystem: { mount_cwd: true }` or `envpod run -w` mounts the current working directory into the pod with COW isolation. The agent sees your project files; writes go to the overlay. `diff`/`commit`/`rollback` as usual.

## Quick Start

```bash
# Install (Linux x86_64 — single binary, no dependencies)
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz \
  | tar xz && sudo ./envpod-linux-x86_64/install.sh

# Create a pod using a built-in preset (18 available)
sudo envpod init my-agent --preset claude-code

# Or use the interactive wizard (shows all presets by category)
sudo envpod init my-agent

# Or use a custom config file
sudo envpod init my-agent -c examples/coding-agent.yaml

# Run a command inside the pod (fully isolated)
sudo envpod run my-agent -- /bin/bash

# See what the agent changed
sudo envpod diff my-agent

# Accept or reject changes
sudo envpod commit my-agent              # apply all changes to host
sudo envpod commit my-agent /opt/a       # commit specific paths only
sudo envpod rollback my-agent            # discard everything

# View audit trail
sudo envpod audit my-agent

# Security analysis
sudo envpod audit my-agent --security
```

See [Installation](docs/INSTALL.md), [Quickstart](docs/QUICKSTART.md), [Pod Config](docs/POD-CONFIG.md), [Tutorials](docs/TUTORIALS.md), [Action Catalog](docs/ACTION-CATALOG.md), [CLI Black Book](docs/CLI-BLACKBOOK.md), [Capabilities](docs/CAPABILITIES.md), [Features](docs/FEATURES.md), [Compare vs Docker](docs/COMPARE-DOCKER.md), [Benchmarks](docs/BENCHMARKS.md), [Security](docs/SECURITY.md), [Licensing](docs/LICENSING.md), [Roadmap](docs/ROADMAP.md), [FAQ](docs/FAQ.md), [Changelog](CHANGELOG.md), and [Contributing](CONTRIBUTING.md) for more.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                  GOVERNANCE CEILING                  │
│  Credential Vault  ·  Action Queue  ·  Undo         │
│  Monitoring Agent  ·  Remote Control ·  Audit Log   │
├──────────┬──────────┬──────────┬────────────────────┤
│ MEMORY   │FILESYSTEM│ NETWORK  │ PROCESSOR          │
│ WALL     │ WALL     │ WALL     │ WALL               │
│          │          │          │                     │
│ PID ns   │OverlayFS │ Net ns   │ cgroups v2         │
│ /proc    │ COW      │ veth     │ CPU cores/affinity │
│ masking  │ diff/    │ DNS      │ Memory limit       │
│ seccomp  │ commit/  │ filtering│ PID limit          │
│          │ rollback │ iptables │                     │
├──────────┴──────────┴──────────┴────────────────────┤
│              ISOLATION BACKEND (pluggable)           │
│                  native (Linux)                      │
└─────────────────────────────────────────────────────┘
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `envpod init <name> [-c config.yaml] [--preset name]` | Create a new pod (interactive wizard if no flags) |
| `envpod presets` | List all built-in presets by category |
| `envpod setup <name>` | Re-run setup commands |
| `envpod run <name> [--root] [-b] [-d] [-a] [-w] [-p h:p] [-P h:p] [-i port] -- <cmd>` | Run a command inside a pod (Ctrl+Z to detach) |
| `envpod fg <name>` | Reattach to a background/detached pod |
| `envpod diff <name>` | Show filesystem changes (COW overlay) |
| `envpod commit <name> [paths...] [--exclude ...]` | Apply changes to host |
| `envpod rollback <name>` | Discard all overlay changes |
| `envpod audit <name> [--security] [--json]` | View audit log or run security analysis |
| `envpod status <name>` | Show pod status and resource usage |
| `envpod logs <name>` | View pod stdout/stderr |
| `envpod lock <name>` | Freeze pod state |
| `envpod kill <name>` | Stop processes and rollback |
| `envpod destroy <names...> [--base] [--full]` | Remove pod(s). `--full` also cleans iptables immediately |
| `envpod clone <source> <name> [--current]` | Clone a pod or base pod (fast — skips rootfs rebuild) |
| `envpod base create/ls/destroy` | Manage base pods (reusable snapshots for cloning) |
| `envpod ls [--json]` | List all pods |
| `envpod queue <name>` | View action staging queue |
| `envpod approve <name> <id>` | Approve a queued action |
| `envpod cancel <name> <id>` | Cancel a queued action |
| `envpod undo <name>` | Undo last reversible action |
| `envpod vault <name> set/get/remove` | Manage pod credentials |
| `envpod vault <name> bind/unbind/bindings` | Manage vault proxy bindings (v0.2) |
| `envpod dashboard [--port 9090]` | Start web dashboard (v0.2) |
| `envpod mount <name> <path>` | Bind-mount a host path into a pod |
| `envpod unmount <name> <path>` | Unmount a path from a pod |
| `envpod dns <name>` | Update DNS policy on a running pod |
| `envpod remote <name> <cmd>` | Send remote control command |
| `envpod monitor <name>` | Manage monitoring policy |
| `envpod gc` | Clean up orphaned resources (iptables, netns, cgroups, pod dirs) |
| `envpod completions <shell>` | Generate shell completions (bash, zsh, fish) |

## Configuration

Pods are configured via `pod.yaml`:

```yaml
name: my-agent
type: standard           # standard, hardened, ephemeral, supervised, air_gapped
backend: native
user: agent              # "agent" (non-root, UID 60000) or "root"

filesystem:
  system_access: safe    # safe (read-only), advanced (COW), dangerous (COW + commit)
  mount_cwd: false       # mount working directory into pod (COW isolated)
  mounts:
    - path: /opt/google
      permissions: ReadOnly
  tracking:
    watch: [/home, /opt, /root, /workspace]
    ignore: [/var/cache, /var/lib/apt, /tmp, /run]

network:
  mode: Isolated         # Isolated, Monitored, Host
  subnet: "10.200"       # pod IP subnet (default: 10.200)
  dns:
    mode: Whitelist      # Whitelist, Blacklist, Monitor
    allow:
      - api.anthropic.com
      - github.com
    deny: []
    remap:               # redirect domains
      internal.api: "10.0.1.5"
  ports:                 # localhost-only port forwards: "host:container[/proto]"
    - "8080:3000"
  public_ports:          # all-interfaces port forwards (LAN-visible)
    - "9090:9090"
  internal_ports:        # pod-to-pod only: "container[/proto]" (no host port)
    - "3000"
  allow_discovery: false # when true: register as <name>.pods.local (requires envpod dns-daemon)
  allow_pods: []         # pod names this pod may resolve via *.pods.local

processor:
  cores: 2.0
  memory: "4GB"
  max_pids: 1024
  cpu_affinity: "0-3"    # pin to specific CPUs

security:
  seccomp_profile: default  # "default" or "browser"
  shm_size: "256MB"         # /dev/shm size

devices:
  gpu: true
  display: true             # auto-mount display socket
  audio: true               # auto-mount audio socket
  display_protocol: auto    # auto, wayland, x11
  audio_protocol: auto      # auto, pipewire, pulseaudio
  extra: ["/dev/fuse"]      # additional devices

vault:                       # v0.2 — vault proxy injection
  proxy: true                # enable transparent HTTPS proxy
  bindings:
    - key: ANTHROPIC_API_KEY
      domain: api.anthropic.com
      header: "Authorization: Bearer {value}"

budget:
  max_duration: "4h"

audit:
  action_log: true

setup:
  - "pip install numpy pandas"

setup_script: ~/setup.sh    # host script injected into pod
```

## Presets

18 built-in presets — no YAML needed. Use `envpod presets` to list all, or `envpod init <name>` for an interactive wizard.

```bash
sudo envpod init my-agent --preset claude-code    # direct
sudo envpod init my-agent                          # interactive wizard
```

**Coding Agents**

| Preset | Description | Setup |
|--------|-------------|-------|
| `claude-code` | Anthropic Claude Code CLI | `curl` installer |
| `codex` | OpenAI Codex CLI | nvm + `npm install -g @openai/codex` |
| `gemini-cli` | Google Gemini CLI | nvm + `npm install -g @google/gemini-cli` |
| `opencode` | OpenCode terminal agent | `curl` installer |
| `aider` | Aider AI pair programmer | `pip install aider-chat` |
| `swe-agent` | SWE-agent autonomous coder | `pip install sweagent` |

**Frameworks**

| Preset | Description | Setup |
|--------|-------------|-------|
| `langgraph` | LangGraph workflows | `pip install langgraph langchain-openai` |
| `google-adk` | Google Agent Development Kit | `pip install google-adk` |
| `openclaw` | OpenClaw messaging assistant | nvm + `npm install -g openclaw` |

**Browser Agents**

| Preset | Description | Setup |
|--------|-------------|-------|
| `browser-use` | Browser-use web automation | `pip install browser-use playwright` + Chromium |
| `playwright` | Playwright browser automation | `pip install playwright` + Chromium |
| `browser` | Headless Chrome sandbox | Chrome (check host, else install) |

**Environments**

| Preset | Description | Setup |
|--------|-------------|-------|
| `devbox` | General dev sandbox | None |
| `python-env` | Python environment | numpy, pandas, matplotlib, scipy, scikit-learn |
| `nodejs` | Node.js environment | nvm + Node.js 22 |
| `web-display` | noVNC desktop | Supervisor-managed |
| `desktop` | XFCE desktop via noVNC | XFCE4 + Chrome (~550MB, 2-4 min) |
| `vscode` | VS Code in the browser | code-server |

The interactive wizard also lets you customize CPU cores, memory, and GPU after selecting a preset.

## Additional Examples

34 example configs total in `examples/` — the 18 presets above plus:

| Example | Description |
|---------|-------------|
| [`basic-cli.yaml`](examples/basic-cli.yaml) | Minimal sandbox, no network |
| [`basic-internet.yaml`](examples/basic-internet.yaml) | CLI with monitored internet |
| [`coding-agent.yaml`](examples/coding-agent.yaml) | General-purpose coding agent |
| [`browser-wayland.yaml`](examples/browser-wayland.yaml) | Chrome with secure Wayland + PipeWire |
| [`ml-training.yaml`](examples/ml-training.yaml) | GPU-accelerated ML training |
| [`hardened-sandbox.yaml`](examples/hardened-sandbox.yaml) | Maximum isolation, no network |
| [`fuse-agent.yaml`](examples/fuse-agent.yaml) | FUSE filesystem support |
| [`demo-pod.yaml`](examples/demo-pod.yaml) | Minimal quick demo |
| [`monitoring-policy.yaml`](examples/monitoring-policy.yaml) | Example monitoring rules |
| [`discovery-service.yaml`](examples/discovery-service.yaml) | Pod discovery (target) |
| [`discovery-client.yaml`](examples/discovery-client.yaml) | Pod discovery (client) |
| [`jetson-orin.yaml`](examples/jetson-orin.yaml) | NVIDIA Jetson Orin (ARM64) |
| [`raspberry-pi.yaml`](examples/raspberry-pi.yaml) | Raspberry Pi 4/5 (ARM64) |
| [`web-display-novnc.yaml`](examples/web-display-novnc.yaml) | noVNC web display |
| [`host-apps.yaml`](examples/host-apps.yaml) | Auto-mount host apps (Chrome, Python, Node) |
| [`clone-user.yaml`](examples/clone-user.yaml) | Clone host user environment |

## Performance

### envpod vs Docker vs Podman (10 iterations, `tests/benchmark-podman.sh`)

Docker 29.2.1, Podman 4.9.3, envpod on Ubuntu 24.04, NVIDIA TITAN RTX x2:

| Test | Docker | Podman | Envpod | vs Docker | vs Podman |
|------|--------|--------|--------|-----------|-----------|
| fresh: run /bin/true | 552ms | 560ms | **401ms** | **151ms faster** | **159ms faster** |
| warm: run /bin/true | 95ms | 270ms | **32ms** | **63ms faster** | **238ms faster** |
| fresh: file I/O (write+read 1MB) | 604ms | 573ms | **413ms** | **191ms faster** | **160ms faster** |
| fresh: GPU nvidia-smi | 755ms | 745ms | **447ms** | **308ms faster** | **298ms faster** |
| warm: GPU nvidia-smi | 137ms | 244ms | **76ms** | **61ms faster** | **168ms faster** |

- **fresh** = create from base + run + destroy (`docker run --rm` / `podman run --rm` / `envpod clone+run+destroy`)
- **warm** = run in existing instance (`docker exec` / `podman exec` / `envpod run`)

Envpod is faster at every operation while adding governance features neither Docker nor Podman have.

### Core Benchmarks (50 iterations, `tests/benchmark.sh`)

| Command | Median | Min | Max | P95 |
|---------|--------|-----|-----|-----|
| `envpod init` | 1.363s | 1.329s | 1.413s | 1.386s |
| `envpod clone` | ~130ms | — | — | — |
| `envpod run -- /bin/true` | 23ms | 20ms | 1.348s* | 45ms |
| `envpod run --root -- /bin/true` | 21ms | 20ms | 44ms | 41ms |
| `envpod diff` | 7ms | 7ms | 8ms | 7ms |
| `envpod rollback` | 8ms | 7ms | 9ms | 9ms |
| Full lifecycle (init+run+diff+destroy) | 3.348s | 3.286s | 3.405s | 3.400s |

*First run after init is ~1.3s (cold start: DNS resolver, cgroup init). Subsequent runs are 20-45ms.

Clone is ~10x faster than init — rootfs is symlinked (not copied), only cgroup + network namespace are recreated.

### GPU Passthrough (NVIDIA TITAN RTX x2, 10 iterations, `tests/benchmark-gpu.sh`)

| Command | Host | Pod | Overhead |
|---------|------|-----|----------|
| `nvidia-smi` query | 52ms | 80ms | +28ms (namespace entry) |
| `nvidia-smi --list-gpus` | — | 73ms | — |
| `envpod init` (gpu: true vs false) | — | 1.358s vs 1.350s | ~0ms |
| `envpod run /bin/true` (gpu: true vs false) | — | 20ms vs 25ms | ~0ms |

GPU passthrough is a zero-copy bind-mount of `/dev/nvidia*` and `/dev/dri/*` — no virtualization layer, no measurable overhead.

### Disk Footprint (`tests/benchmark-size.sh`)

Docker 29.2.1, Podman 4.9.3, envpod on Ubuntu 24.04, NVIDIA TITAN RTX x2:

**Base image / base pod (ubuntu 24.04):**

| Runtime | Size |
|---------|------|
| Docker image | 119 MB |
| Podman image | 77 MB |
| Envpod base pod | **105 MB** |

Envpod base is 12% smaller than Docker. Docker/Podman copy the full distro userland into the image; envpod copies only `/etc` + apt state — `/usr`, `/bin`, `/lib` are bind-mounted from the host.

**GPU image / base pod (CUDA 12.0 ubuntu 22.04):**

| Runtime | Size |
|---------|------|
| Docker image | 338 MB |
| Podman image | 229 MB |
| Envpod base pod (gpu: true) | **105 MB** |

Envpod GPU base is **69% smaller** than Docker's CUDA image — CUDA libraries are bind-mounted from the host, not copied.

**Per-instance overhead:**

| Runtime | Size |
|---------|------|
| Docker container layer | 4 KB |
| Podman container layer | 11 KB |
| Envpod pod (unique) | **1 KB** |
| Envpod clone (unique) | **1 KB** |

Clones share the base rootfs via symlink — near-zero per-clone overhead.

### Scale Test (50 instances, `tests/benchmark-scale.sh`)

Docker 29.2.1, Podman 4.9.3, envpod on Ubuntu 24.04:

| Phase | Docker | Podman | Envpod | vs Docker | vs Podman |
|-------|--------|--------|--------|-----------|-----------|
| Create 50 instances | 6.3s | 6.9s | **407ms** | **15x faster** | **17x faster** |
| Run /bin/true in all 50 | 11.4s | 26.8s | **7.5s** | **1.5x faster** | **3.6x faster** |
| Destroy all 50 | 1.2s | 2.4s | **1.6s** | — | — |
| **Full lifecycle** | **19.0s** | **36.2s** | **9.5s** | **2x faster** | **3.8x faster** |

Envpod creation is **15x faster** than Docker because `envpod clone` copies only a symlink + empty dirs (~1 KB per clone), while `docker create` allocates a full container layer. Envpod uses a **two-phase destroy**: `envpod destroy` is fast (deletes veth + netns, skips iptables), then `envpod gc` cleans up stale iptables rules in one batch. Dead rules are harmless — they reference non-existent interfaces. See [Benchmarks](docs/BENCHMARKS.md) for details.

Run the benchmarks yourself:
```bash
sudo ./tests/benchmark-podman.sh 10  # Docker + Podman + envpod comparison
sudo ./tests/benchmark-docker.sh 10  # Docker vs envpod only
sudo ./tests/benchmark.sh 50         # core benchmarks
sudo ./tests/benchmark-clone.sh 10   # clone vs init
sudo ./tests/benchmark-gpu.sh 10     # GPU passthrough (requires NVIDIA GPU)
sudo ./tests/benchmark-size.sh       # disk footprint comparison
sudo ./tests/benchmark-scale.sh 50   # scale test (create + run + destroy N)
```

## How Envpod Compares

| | Docker Sandbox | E2B | Envpod |
|---|---|---|---|
| **Isolation** | Container (namespaces + cgroups) | Cloud microVM | Container (namespaces + cgroups + seccomp-BPF) |
| **Reversibility** | None — changes permanent | None | COW overlay + diff/commit/rollback + undo registry |
| **Governance** | None | None | Vault, action queue, monitoring, remote control, audit |
| **DNS Control** | None | None | Per-pod whitelist/blacklist/monitor with query logging |
| **Display/Audio** | Manual volume mounts | N/A | Auto-detect Wayland/X11, PipeWire/PulseAudio |
| **Security Audit** | None | None | Static analysis of pod config (`--security`) |

## Development

Requires Rust toolchain and Linux (kernel namespaces, cgroups v2, overlayfs).

```bash
cargo build                    # Build all crates
cargo build --release          # Release build (static musl binary)
cargo test                     # Run unit tests
cargo clippy                   # Lint
cargo fmt                      # Format

# Integration tests (require root)
sudo cargo test -- --ignored
sudo ./tests/e2e.sh
```

### Workspace Crates

- **`cli/`** — CLI binary (`envpod` command), 26 subcommands + web dashboard
- **`core/`** — Core library (isolation, governance, policy logic, vault proxy)
- **`dns/`** — Embedded per-pod DNS resolver with filtering

## License

[![License](https://img.shields.io/badge/license-BSL--1.1-blue)](LICENSE)

Copyright 2026 Xtellix Inc. Licensed under [BSL 1.1](LICENSE) — free to use, modify, and self-host. Converts to AGPL-3.0 on 2030-03-07. Commercial use that competes with envpod requires a separate license.

See [docs/LICENSING.md](docs/LICENSING.md) for full details. For commercial licensing, contact mark@envpod.dev.
