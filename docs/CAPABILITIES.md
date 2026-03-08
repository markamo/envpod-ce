# Capabilities

> **EnvPod v0.1.0** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

What envpod can do today (v0.1.0). For how-to guides, see [Quickstart](QUICKSTART.md) and [Tutorials](TUTORIALS.md).

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
| | Vault proxy injection (transparent HTTPS, zero-knowledge) | Shipped v0.2 |
| | Web dashboard (fleet overview, pod detail, actions) | Shipped v0.2 |
| | Action staging queue (approve / cancel) | Shipped |
| | Undo registry (reverse any reversible action) | Shipped |
| | Append-only audit trail (JSONL) | Shipped |
| | Static security analysis (`--security`) | Shipped |
| | Live DNS mutation (add/remove domains without restart) | Shipped |
| | Remote control (freeze / resume / kill / restrict) | Shipped |
| | Monitoring agent (policy-driven auto-freeze/restrict) | Shipped |
| **Performance** | Pod init: ~1.3s | Shipped |
| | Pod clone: ~130ms (10x faster than init) | Shipped |
| | Warm run: ~23ms | Shipped |
| | 50-pod fleet: 407ms creation, 9.5s full lifecycle | Shipped |
| **Devices** | NVIDIA GPU passthrough (zero-copy bind-mount) | Shipped |
| | Display forwarding (Wayland / X11 / auto-detect) | Shipped |
| | Audio forwarding (PipeWire / PulseAudio / auto-detect) | Shipped |
| | Desktop environment auto-install (`desktop_env`: xfce / openbox / sway) | Shipped v0.2 |
| | Custom device passthrough (`/dev/fuse`, `/dev/kvm`, etc.) | Shipped |
| **Discovery** | Pod-to-pod resolution (`<name>.pods.local`) via central daemon | Shipped v0.2 |
| | Live discovery mutations (`envpod discover`) | Shipped v0.2 |
| | Live port forwarding mutations (`envpod ports`) | Shipped v0.2 |
| **Backends** | Native Linux (namespaces + cgroups + OverlayFS) | Shipped |
| | x86_64 static binary (`musl`) | Shipped |
| | aarch64 static binary (Raspberry Pi / Jetson Orin) | Shipped v0.2 |
| | Docker (container isolation) | Planned v0.2 |
| | VM (Firecracker / QEMU microVMs) | Planned v0.3 |

## Isolation Boundaries

Every pod runs inside four walls:

```
┌──────────────────────────────────────────────────┐
│                  GOVERNANCE CEILING               │
│  Vault · Queue · Undo · Monitor · Audit · DNS    │
├───────────┬───────────┬──────────┬───────────────┤
│ MEMORY    │FILESYSTEM │ NETWORK  │ PROCESSOR     │
│ PID ns    │ OverlayFS │ Net ns   │ cgroups v2    │
│ /proc     │ COW diff/ │ veth     │ CPU cores     │
│ masking   │ commit/   │ DNS      │ CPU affinity  │
│ seccomp   │ rollback  │ iptables │ Memory limit  │
│           │           │          │ PID limit     │
└───────────┴───────────┴──────────┴───────────────┘
```

### What the agent cannot do

- **Escape the pod** — 17/17 jailbreak tests pass (non-root user)
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

## Supported AI Agents

Pre-built configs in `examples/`:

| Agent | Config | Network | Setup |
|-------|--------|---------|-------|
| Claude Code (Anthropic) | `claude-code.yaml` | Anthropic API, GitHub, npm/pip/cargo | Native installer |
| Codex (OpenAI) | `codex.yaml` | OpenAI API, GitHub, npm | nvm + npm |
| Aider | `aider.yaml` | Multi-provider APIs, GitHub, PyPI | pip |
| SWE-agent (Princeton) | `swe-agent.yaml` | Multi-provider APIs, GitHub, PyPI | git clone + pip |
| OpenClaw | `openclaw.yaml` | Multi-provider APIs, messaging, npm | nvm + npm |
| OpenCode | `opencode.yaml` | Multi-provider APIs, GitHub | Go binary |
| browser-use | `browser-use.yaml` | Open web (blacklist mode) | pip + Playwright |

Any tool that runs on Linux works inside a pod — envpod doesn't require agent-specific integration.

## Filesystem Operations

| Operation | Description |
|-----------|-------------|
| `envpod diff` | Show all files the agent created, modified, or deleted |
| `envpod commit` | Apply changes to host filesystem |
| `envpod commit /path` | Commit specific paths only |
| `envpod commit --exclude /path` | Commit everything except specific paths |
| `envpod commit --output /dir` | Export changes to a custom directory |
| `envpod commit --include-system` | Include system directory changes (advanced mode) |
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
| 1 pod | 130ms (clone) | 23ms | 30ms | 183ms |
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

## What's Not Yet Built

These features are designed but not shipped:

| Feature | Target | Description |
|---------|--------|-------------|
| Docker backend | v0.2 | Run pods inside Docker containers instead of native namespaces |
| VM backend | v0.3 | Run pods inside Firecracker/QEMU microVMs |
| Base pod export/import | Premium | `envpod base export/import` — package and transfer base pods |
| Custom rootfs sources | v0.2 | Use debootstrap, Alpine tarballs, or OCI images as rootfs |
| Advanced diff viewer | Premium | Inline git-style per-hunk diff in dashboard, per-hunk staging |
| FEBO policy engine | v0.3 | Full policy language for governance rules |
| Cloud relay | v0.3 | Remote control plane for managing pods across machines |
| Pod encryption | v0.2 | Encrypt pod data at rest |

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
