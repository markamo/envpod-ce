# Changelog

All notable changes to envpod are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added

- `filesystem.mount_cwd` — mount the working directory into the pod with COW isolation. `envpod init` captures `$PWD`; `envpod run` bind-mounts it read-only. Agent sees real files, writes go to overlay. CLI: `-w`/`--mount-cwd` to force, `--no-mount-cwd` to skip.

## [0.1.0] - 2026-03-03

First public release.

### Core

- Pod lifecycle: `init`, `setup`, `run`, `destroy`, `clone`, `ls`, `status`, `logs`
- OverlayFS copy-on-write filesystem with `diff`, `commit` (full/partial/exclude), `rollback`
- Commit to custom output directory (`--output <dir>`)
- Base pods: `base create`, `base ls`, `base destroy` for reusable snapshots
- Clone from base or current state (`clone --current`), ~10x faster than init
- Pod snapshots: `snapshot create`, `ls`, `restore`, `destroy`, `prune`, `promote`
- Garbage collection: `gc` cleans orphaned iptables, netns, cgroups, pod dirs

### Network

- Network namespace isolation with veth pairs
- Embedded per-pod DNS resolver (whitelist, blacklist, monitor modes)
- Live DNS mutation (`dns` command) without pod restart
- Anti-DNS-tunneling detection
- Port forwarding: localhost (`-p`), public (`-P`), pod-to-pod (`-i`)
- Central DNS daemon for pod discovery (`dns-daemon`, `discover`)
- Bilateral discovery policy (`allow_discovery` + `allow_pods`)

### Security

- PID namespace isolation with /proc masking
- cgroups v2 enforcement (CPU, memory, PID limits)
- seccomp-BPF syscall filtering (default + browser profiles)
- User namespace support (root or non-root agent user)
- Static security analysis (`audit --security`, `--json`)
- 13 security findings: namespace, network, vault, seccomp, device, resource

### Governance

- Credential vault with ChaCha20-Poly1305 encryption (`vault set/get/remove`)
- Action staging queue with tier classification (immediate, delayed, staged, blocked)
- Undo registry for reversible actions (`undo`)
- Monitoring agent with configurable policy rules (`monitor`)
- Remote control: freeze, resume, kill, restrict (`remote`, `lock`)
- Append-only JSONL audit trail (`audit`)

### Operations

- Web dashboard with fleet overview, pod detail, audit viewer (`dashboard`)
- Display forwarding (Wayland/X11 auto-detect)
- Audio forwarding (PipeWire/PulseAudio auto-detect)
- GPU passthrough (NVIDIA, zero overhead)
- System access profiles: safe, advanced, dangerous
- Host path bind mounts (`mount`, `unmount`)
- Shell completions (bash, zsh, fish)

### Platform

- Static musl binary (12 MB, no runtime dependencies)
- x86_64 and ARM64 (aarch64) support
- Raspberry Pi 4/5 and Jetson Orin tested

### Example Configs

- 20 example pod configs: coding-agent, claude-code, codex, opencode, aider, swe-agent, browser, browser-wayland, browser-use, openclaw, ml-training, nodejs, python-env, devbox, hardened-sandbox, fuse-agent, demo-pod, monitoring-policy, raspberry-pi, jetson-orin

[0.1.0]: https://github.com/markamo/envpod-ce/releases/tag/v0.1.0
