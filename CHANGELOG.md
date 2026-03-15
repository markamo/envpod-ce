# Changelog

All notable changes to envpod are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-03-13

### New Commands

- `envpod start <names...> [--all]` — start stopped pods in background (uses `start_command` from pod.yaml, or `sleep infinity`)
- `envpod stop <names...> [--all]` — stop running pods gracefully
- `envpod restart <names...> [--all]` — stop + start in one step
- `envpod resize <name>` — live resource mutation on running pods (CPU, memory, tmpfs, pids), config-only mutation on stopped pods (GPU, display, audio, desktop)
- `envpod base resize <name>` — resize base pod configs so future clones inherit settings
- `envpod prune` — remove all stopped pods
- `envpod about` / `envpod --about` — version, license, project info, and system details

### New Configuration

- `start_command` in pod.yaml — default command for `envpod start` (e.g. `start_command: ["sleep", "infinity"]`)
- `processor.tmp_size` — configurable `/tmp` tmpfs size (e.g. `tmp_size: 500MB`, default 100MB)
- `processor.disk_size` — upper layer disk image for large pods (e.g. `disk_size: 10GB`)

### Dashboard

- `envpod dashboard --daemon/-d` — run dashboard in background
- `envpod dashboard --stop` — stop background dashboard

### Web Display (noVNC)

- Clipboard sync between host and noVNC pod (bidirectional copy/paste)
- NumLock state sync on display startup
- noVNC info button showing pod details
- Default `desktop_env`: openbox for agents, xfce for desktops
- Auto-added port forwards no longer print noise on exit

### Security

- S-04 security finding for `seccomp_profile: none` — `envpod audit --security` now flags missing seccomp
- Comprehensive security model document (`docs/SECURITY-MODEL.md`)
- Host hardening recommendations and isolation backend hierarchy

### Documentation

- 14 SVG architecture diagrams replacing all ASCII art across docs
- Security model deep-dive: filesystem, network, process, vault, display isolation
- Tutorial 15: commit workflow with selective paths and `--output` export
- Troubleshooting guide with common issues and fixes
- API key authentication docs added to all agent example configs
- Demo GIFs for quickstart, diff/commit, vault, web display, dashboard

### Fixed

- **TUI apps broken in pods** — devpts not mounted, seccomp blocking `restart_syscall`, SIGINT not forwarded to child process. Vim, htop, claude, and other terminal apps now work correctly.
- **XFCE desktop startup failures** — D-Bus auth cookie path, ICE authority, seccomp syscalls for desktop session
- **Display wrapper killing user shell on exit** — wrapper now detaches cleanly
- **noVNC clipboard error on non-HTTPS** — clipboard API requires secure context; error bar added with dismiss button
- **VS Code desktop shortcut** — patches original `.desktop` instead of overwriting (preserves icon path)
- **Chrome set as default browser** in desktop pod examples
- **setup_script injection in advanced mode** — `inject_setup_script` now writes to `sys_upper/` when `system_access` is advanced/dangerous, fixing exit 127 for configs with `setup_script`
- **browser.yaml /etc/alternatives** — removed ReadOnly mount that blocked `update-alternatives` during openbox post-install
- **LibreOffice install in pods** — postinst `install(1)` fails with EPERM in user namespaces; patched to use `touch`, all 7 components (Writer, Calc, Impress, Draw, Math, Base) now fully functional
- **Idempotent git clone in setup** — `test -d || git clone` pattern prevents failure on re-run
- **PEP 668 in Ubuntu 24.04** — setup commands remove `EXTERNALLY-MANAGED` file before pip installs
- **tmp_size for large installs** — aider, google-adk, ml-training, python-env, swe-agent configs bumped to 1-8GB to prevent ENOSPC during pip downloads

### Example Configs

- 41 example configs, all 37 testable configs pass (3 skipped: ARM64-only, monitoring policy)
- Test suite: `tests/test-all-examples.sh` with selective execution, pass/fail/skip summary

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
- `--create-base [name]` flag for `envpod init` and `envpod setup`
- `devices.desktop_env` — auto-install desktop environment (xfce, openbox, sway)
- `filesystem.mount_cwd` — mount working directory into pod with COW isolation
- `screen` auto-installed in web display pods for resumable sessions
- Multiple simultaneous `envpod run` sessions in web display pods
- Universal installer with `--version` flag and `uninstall.sh`

### Platform

- Static musl binary (13 MB, no runtime dependencies)
- x86_64 and ARM64 (aarch64) support
- Raspberry Pi 4/5 and Jetson Orin tested

### Example Configs

- 20 example pod configs: coding-agent, claude-code, codex, opencode, aider, swe-agent, browser, browser-wayland, browser-use, openclaw, ml-training, nodejs, python-env, devbox, hardened-sandbox, fuse-agent, demo-pod, monitoring-policy, raspberry-pi, jetson-orin

[0.1.1]: https://github.com/markamo/envpod-ce/releases/tag/v0.1.1
[0.1.0]: https://github.com/markamo/envpod-ce/releases/tag/v0.1.0
