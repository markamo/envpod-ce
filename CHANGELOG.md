# Changelog

All notable changes to envpod are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added

- `--create-base [name]` flag for `envpod init` and `envpod setup` — base pods are no longer auto-created. Use `--create-base` to opt in. Accepts an optional name (defaults to pod name). Auto-increments on collision (e.g. `my-agent-2`) instead of overwriting.
- `devices.desktop_env` — auto-install a desktop environment during `envpod init`. Options: `none` (default), `xfce` (xfce4 + xfce4-terminal + dbus-x11, ~200 MB), `openbox` (openbox + tint2 + xterm, ~50 MB), `sway` (sway + foot, ~150 MB, Wayland-native). Pairs with `web_display` or `devices.display` for browser-based or host display access.
- `filesystem.mount_cwd` — mount the working directory into the pod with COW isolation. `envpod init` captures `$PWD`; `envpod run` bind-mounts it read-only. Agent sees real files, writes go to overlay. CLI: `-w`/`--mount-cwd` to force, `--no-mount-cwd` to skip.
- `screen` auto-installed in all web display and desktop pods for resumable sessions
- Multiple simultaneous `envpod run` sessions in the same web display pod (each gets an independent terminal, display services are shared)
- `--version` flag for install scripts to pin a specific release version
- `uninstall.sh` bundled in release tarballs
- Universal installer (distro detection, container support) in release tarballs

### Fixed

- APT GPG signature failures in OverlayFS pods (`rm -rf /var/lib/apt/lists/*` before update)
- Display services blocking user terminal (split into background daemon + lightweight wrapper)
- D-Bus "Could not connect to bus" error in web display pods (auto-start `dbus-daemon --session`)

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
