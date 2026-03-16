# Capabilities

> **EnvPod v0.1.1** — Zero-trust governance environments for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

What envpod can do today (v0.1.1). For how-to guides, see [Quickstart](QUICKSTART.md) and [Tutorials](TUTORIALS.md).

## At a Glance

| Category | Capability | Status |
|----------|-----------|--------|
| **Isolation** | PID, mount, network, UTS, user namespaces | Shipped |
| | cgroups v2 (CPU, memory, PID limits) | Shipped |
| | seccomp-BPF syscall filtering (default + browser profiles) | Shipped |
| | OverlayFS copy-on-write filesystem | Shipped |
| | Per-pod network namespace with veth pairs | Shipped |
| | Per-pod embedded DNS resolver | Shipped |
| **Governance** | Diff / commit / rollback (file-level review) | Shipped |
| | Selective commit (per-path, `--output`, `--exclude`) | Shipped |
| | Mount working directory (`mount_cwd` / `-w`) | Shipped |
| | Credential vault (encrypted, env var injection) | Shipped |
| | Vault proxy injection (transparent HTTPS, zero-knowledge) | Shipped |
| | Web dashboard (fleet overview, pod detail, actions) | Shipped |
| | Action staging queue (approve / cancel) | Shipped |
| | Undo registry (reverse any reversible action) | Shipped |
| | Append-only audit trail (JSONL) | Shipped |
| | Static security analysis (`--security`) | Shipped |
| | Live DNS mutation (add/remove domains without restart) | Shipped |
| | Remote control (freeze / resume / kill / restrict) | Shipped |
| | Monitoring agent (policy-driven auto-freeze/restrict) | Shipped |
| **Performance** | Pod init: ~1.3s | Shipped |
| | Pod clone: ~8ms (170x faster than init) | Shipped |
| | Warm run: ~23ms | Shipped |
| | 50-pod fleet: 407ms creation, 9.5s full lifecycle | Shipped |
| **Devices** | NVIDIA GPU passthrough (zero-copy bind-mount) | Shipped |
| | Display forwarding (Wayland / X11 / auto-detect) | Shipped |
| | Audio forwarding (PipeWire / PulseAudio / auto-detect) | Shipped |
| | Web display via noVNC (full desktop in browser, audio, clipboard, file upload) | Shipped |
| | Desktop environment auto-install (`desktop_env`: xfce / openbox / sway) | Shipped |
| | Custom device passthrough (`/dev/fuse`, `/dev/kvm`, etc.) | Shipped |
| **Live Mutation** | Live resource resize (CPU, memory, tmpfs, PIDs) on running pods | Shipped |
| | Stopped mutation (GPU, display, audio, desktop) on stopped pods | Shipped |
| | Base pod resize (`envpod base resize`) | Shipped |
| | Live port forwarding mutations (`envpod ports`) | Shipped |
| | Live discovery mutations (`envpod discover`) | Shipped |
| **Discovery** | Pod-to-pod resolution (`<name>.pods.local`) via central daemon | Shipped |
| **Backends** | Native Linux (namespaces + cgroups + OverlayFS) | Shipped |
| | x86_64 static binary (`musl`) | Shipped |
| | aarch64 static binary (Raspberry Pi / Jetson Orin) | Shipped |
| | Docker (container isolation) | Planned |
| | VM (Firecracker / QEMU microVMs) | Planned |

## Isolation Boundaries

Every pod has a foundation, four walls, and a governance ceiling:

![Pod Architecture — governance ceiling, four walls, OverlayFS foundation](images/fig-02-capabilities-architecture.svg)

### What the agent cannot do

- **Escape the pod** — 17/17 jailbreak tests pass (non-root user)
- **Escalate to root** — `NO_NEW_PRIVS` flag + seccomp-BPF blocks `sudo`, `su`, and all setuid-based escalation. The only way to run as root is `envpod run --root` from the host.
- **Fingerprint host CPU** — `/proc/cpuinfo` model name is always sanitized to "CPU" regardless of actual hardware
- **See host processes** — PID namespace, /proc masked
- **Write to host filesystem** — all writes go to COW overlay
- **Reach unauthorized network** — DNS resolver + iptables per pod
- **Fork-bomb the host** — PID limit via cgroups
- **Exhaust host memory** — memory limit via cgroups
- **Use dangerous syscalls** — seccomp-BPF filtering

### What the human can do

- **Review changes** before they touch the host (`envpod diff`)
- **Accept or reject** file changes selectively (`envpod commit`, `envpod rollback`)
- **Freeze the agent** instantly (`envpod lock`)
- **Undo actions** after the fact (`envpod undo`)
- **Audit everything** that happened (`envpod audit`)
- **Mutate policy** on a live pod (`envpod dns`, `envpod remote`, `envpod discover`)
- **Resize resources** live or stopped (`envpod resize`)

## Supported AI Agents

Pre-built configs in `examples/`:

| Agent | Config | Network | Setup |
|-------|--------|---------|-------|
| Claude Code (Anthropic) | `claude-code.yaml` | Anthropic API, GitHub, npm/pip/cargo | Native installer |
| Codex (OpenAI) | `codex.yaml` | OpenAI API, GitHub, npm | nvm + npm |
| Gemini CLI (Google) | `gemini-cli.yaml` | Google API, GitHub, npm | nvm + npm |
| Google ADK | `google-adk.yaml` | Google API, PyPI | pip |
| Aider | `aider.yaml` | Multi-provider APIs, GitHub, PyPI | pip |
| SWE-agent (Princeton) | `swe-agent.yaml` | Multi-provider APIs, GitHub, PyPI | git clone + pip |
| LangGraph | `langgraph.yaml` | Multi-provider APIs, PyPI | pip |
| FUSE Agent | `fuse-agent.yaml` | Multi-provider APIs, npm | nvm + npm |
| OpenClaw | `openclaw.yaml` | Multi-provider APIs, messaging, npm | nvm + npm |
| OpenCode | `opencode.yaml` | Multi-provider APIs, GitHub | Go binary |
| browser-use | `browser-use.yaml` | Open web (blacklist mode) | pip + Playwright |
| Playwright | `playwright.yaml` | Open web (blacklist mode) | pip + Playwright |
| Full Workstation | `workstation-full.yaml` | Blacklist mode | Chrome, Firefox, VS Code, GIMP, LibreOffice in XFCE via noVNC |

41 example configs ship with envpod. Any tool that runs on Linux works inside a pod — no agent-specific integration required.

## Filesystem Operations

| Operation | Description |
|-----------|-------------|
| `envpod diff` | Show all files the agent created, modified, or deleted |
| `envpod commit` | Apply changes to host filesystem |
| `envpod commit /path` | Commit specific paths only |
| `envpod commit --exclude /path` | Commit everything except specific paths |
| `envpod commit --output /dir` | Export changes to a custom directory |
| `envpod commit --include-system` | Include system directory changes (advanced mode) |
| `envpod commit <name> <paths> --rollback-rest` | Commit specified paths and discard everything else |
| `envpod rollback` | Discard all changes (reset overlay) |
| `envpod run -w` | Mount working directory into pod (COW isolated) |
| `envpod diff --all` | Show changes including ignored paths |

## DNS Filtering Modes

| Mode | Behavior | Use case |
|------|----------|----------|
| **Whitelist** | Only listed domains resolve | API agents — lock to specific endpoints |
| **Blacklist** | Everything resolves except listed domains | Browser agents — block internal/corp domains |
| **Monitor** | Everything resolves, all queries logged | Dev environments — full access with audit trail |

All modes log every DNS query to the audit trail. Live mutation (`envpod dns --allow/--deny`) works without pod restart.

## Pod Discovery

Pods can resolve each other by name (`<name>.pods.local`) using the central `envpod-dns` daemon. Discovery is bilateral — both sides must opt in:

| Setting | Side | Effect |
|---------|------|--------|
| `network.allow_discovery: true` | Service pod | Registers as `<name>.pods.local` |
| `network.allow_pods: ["other"]` | Client pod | Permitted to resolve listed pods |

| Operation | Description |
|-----------|-------------|
| `envpod dns-daemon` | Start the central discovery daemon (required once per host) |
| `envpod discover <pod>` | Show current discovery state for a running pod |
| `envpod discover <pod> --on` | Enable discoverability (takes effect immediately) |
| `envpod discover <pod> --off` | Disable discoverability (takes effect immediately) |
| `envpod discover <pod> --add-pod name` | Add pod to allow_pods list |
| `envpod discover <pod> --remove-pod name` | Remove pod from allow_pods list |
| `envpod discover <pod> --remove-pod '*'` | Clear entire allow_pods list |

All `envpod discover` mutations take effect immediately in the running daemon and are persisted to `pod.yaml`. No pod restart required. If the daemon is not running, mutations are written to `pod.yaml` only.

## Live and Stopped Mutation

| Resource | Running Pod | Stopped Pod | Base Pod |
|----------|------------|-------------|----------|
| CPU cores | `--cpus` (cgroup write) | `--cpus` (config) | `--cpus` (config) |
| Memory | `--memory` (cgroup write) | `--memory` (config) | `--memory` (config) |
| tmpfs /tmp | `--tmp-size` (remount) | `--tmp-size` (config) | `--tmp-size` (config) |
| Max PIDs | `--max-pids` (cgroup write) | `--max-pids` (config) | `--max-pids` (config) |
| GPU | — | `--gpu` (config) | `--gpu` (config) |
| Display | — | `--display` (config) | `--display` (config) |
| Audio | — | `--audio` (config) | `--audio` (config) |
| Desktop env | — | `--desktop` (config) | `--desktop` (config) |
| Web display | — | `--web-display` (config) | `--web-display` (config) |
| DNS policy | `envpod dns` (live) | config only | — |
| Port forwarding | `envpod ports` (live) | config only | — |
| Discovery | `envpod discover` (live) | config only | — |

## Web Display (noVNC)

Full desktop accessible via browser — no VNC client required.

| Feature | Details |
|---------|---------|
| Display server | Xvfb + x11vnc + websockify → noVNC |
| Desktop environments | XFCE, Openbox, Sway |
| Audio | PulseAudio → Opus/WebM streaming (speaker icon in noVNC) |
| Clipboard | Bidirectional sync between host and pod |
| File upload | Drag-and-drop via noVNC → `/tmp/uploads/` |
| Resolution | Configurable (default 1920x1080) |
| GPU | Direct passthrough — hardware acceleration in browser apps |

## Pod Types

| Type | Description |
|------|-------------|
| `standard` | Default. Balanced isolation and usability. |
| `hardened` | Maximum isolation. Tighter seccomp, no writable system dirs. |
| `ephemeral` | Auto-destroy after session ends. |
| `supervised` | Requires human approval for all staged actions. |
| `airgapped` | No network, no DNS, no external communication. |

## System Access Levels

| Level | System dirs | Agent can install packages | Commit behavior |
|-------|-------------|---------------------------|-----------------|
| `safe` (default) | Read-only | No | System changes impossible |
| `advanced` | COW overlay | Yes (goes to overlay) | Blocks system changes unless `--include-system` |
| `dangerous` | COW overlay | Yes (goes to overlay) | Warns but allows system changes |

## Security Profiles

| Profile | Syscalls blocked | Use case |
|---------|-----------------|----------|
| `default` | mount, reboot, kexec, etc. | Most agents |
| `browser` | Default minus 7 Chrome syscalls | Chrome, Playwright, browser-use |

Run `envpod audit --security -c config.yaml` to see the security posture of any configuration before creating a pod. See [Security Report](SECURITY.md) for findings across all example configs.

## Scale Performance

| Count | Create | Run | Destroy | Full lifecycle |
|-------|--------|-----|---------|---------------|
| 1 pod | 8ms (clone) | 23ms | 30ms | 61ms |
| 10 pods | 80ms | 1.5s | 320ms | 1.9s |
| 50 pods | 407ms | 7.5s | 1.6s | 9.5s |

Envpod is **15x faster** than Docker at pod creation and **2x faster** at full lifecycle (create + run + destroy). See [Benchmarks](BENCHMARKS.md) for detailed numbers.

## Embedded Systems

envpod runs on ARM64 Linux with a static `aarch64-unknown-linux-musl` binary. No runtime dependencies.

| Platform | Notes |
|----------|-------|
| **Raspberry Pi 4** (4GB / 8GB) | Raspberry Pi OS 64-bit (Bookworm) or Ubuntu 24.04. Enable cgroups v2 in `cmdline.txt`. |
| **Raspberry Pi 5** (4GB / 8GB) | cgroups v2 enabled by default. Supports Hailo AI HAT+ (`/dev/hailo0`). |
| **NVIDIA Jetson Orin NX / AGX** | JetPack 6 (Ubuntu 22.04). GPU passthrough: CUDA + DLA via `/dev/nvhost-*`. |
| Generic ARM64 Linux | Any distro with kernel 5.11+, cgroups v2, OverlayFS, iptables. |

See [docs/EMBEDDED.md](EMBEDDED.md) for setup instructions, resource limits, and GPU configuration.

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
