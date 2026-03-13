# Pod Configuration Reference

> **EnvPod v0.1.1** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Complete guide to `pod.yaml` — the configuration file that defines every aspect of an envpod pod. Each section controls the foundation (filesystem COW), one of the four walls (processor, network, memory, devices), or the governance ceiling (security, audit, budget, tools).

<p align="center">
  <img src="../assets/demos/13-pod-config-guide.gif" alt="envpod pod config guide" width="720">
</p>

**Prerequisites:** envpod installed. See [Installation](INSTALL.md) and [Quickstart](QUICKSTART.md) if you're new.

---

## Table of Contents

- [Minimal Config](#minimal-config)
- [Top-Level Fields](#top-level-fields)
- [Filesystem](#filesystem)
- [Network](#network)
- [Processor](#processor)
- [Devices](#devices)
- [Web Display](#web-display)
- [Tailscale (Premium)](#tailscale-premium)
- [Security](#security)
- [Budget](#budget)
- [Audit](#audit)
- [Tools](#tools)
- [Vault](#vault)
- [Snapshots](#snapshots)
- [Queue](#queue)
- [Host User](#host-user)
- [Setup Commands](#setup-commands)
- [Full Examples](#full-examples)
- [Defaults Reference](#defaults-reference)

---

## Minimal Config

The smallest valid pod config:

```yaml
name: my-pod
```

Everything else has secure defaults. This creates a pod with:
- No network access (Isolated mode, empty DNS whitelist)
- Read-only system directories (safe mode)
- No GPU, display, or audio
- Audit logging enabled
- Runs as non-root `agent` user (UID 60000)

---

## Top-Level Fields

```yaml
name: my-pod              # Required. Pod name (used in all envpod commands)
type: standard            # Pod type (see below)
backend: native           # Isolation backend (only "native" in v0.1)
user: agent               # Default user inside pod (see below)
start_command: ["claude-pod"]  # Default command for `envpod start` (optional)
```

### `name`

The pod name. Used in all CLI commands (`envpod run my-pod`, `envpod diff my-pod`, etc.). Must be unique across all pods.

### `type`

Controls the security posture template. Options:

| Type | Description |
|------|-------------|
| `standard` | Default. Balanced isolation and usability. |
| `hardened` | Maximum isolation. Tighter seccomp, no writable system dirs. |
| `ephemeral` | Auto-destroy after session ends. |
| `supervised` | Requires human approval for all staged actions. |
| `airgapped` | No network access, no DNS, no external communication. |

### `backend`

The isolation backend. Currently only `native` (Linux namespaces + cgroups v2 + OverlayFS). Docker and VM backends are planned for v0.2 and v0.3.

### `user`

Default user for commands run inside the pod.

| Value | UID | Notes |
|-------|-----|-------|
| `agent` (default) | 60000 | Non-root. Full pod boundary protection (17/17 jailbreak tests pass). |
| `root` | 0 | Root inside pod. 2 additional boundary gaps (N-05 iptables, N-06 raw sockets). |
| Any name | varies | Runs as that user (must exist inside the pod). |

Override per-command with `envpod run my-pod --user root -- ...` or `envpod run my-pod --root -- ...`.

### `start_command`

Default command for `envpod start`. When not set, falls back to `sleep infinity`.

```yaml
start_command: ["claude-pod"]           # Single command
start_command: ["aider", "--model", "sonnet"]  # Command with arguments
```

Resolution order for `envpod start`:
1. CLI override: `envpod start my-pod -- custom-command`
2. `start_command` from pod.yaml
3. Fallback: `sleep infinity`

This is especially useful with `envpod start --all` (e.g., after a host reboot) — each pod starts with its configured command without needing to specify it manually.

---

## Filesystem

Controls the filesystem wall — what the agent can see and write.

```yaml
filesystem:
  system_access: safe       # How system dirs are handled
  mount_cwd: true           # Mount working directory into pod with COW isolation
  apps:                     # Host apps to auto-mount (resolved via which + ldd)
    - python3
    - google-chrome
  mounts:                   # Extra host paths to mount
    - path: /opt/google
      permissions: ReadOnly
    - path: /data/project
      permissions: ReadWrite
  tracking:                 # What appears in diff/commit
    watch:
      - /home
      - /opt
      - /workspace
    ignore:
      - /var/cache
      - /tmp
```

### `system_access`

Controls how system directories (`/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`) are handled:

| Mode | Behavior | Use case |
|------|----------|----------|
| `safe` (default) | Read-only bind mounts. Agent cannot write to system dirs. | Most pods. Agents that only need to write to `/home`, `/opt`, `/workspace`. |
| `advanced` | COW overlay on system dirs. Agent can write, but `envpod commit` blocks system changes unless `--include-system` is passed. | Development pods that need `apt install`, `npm install -g`, etc. |
| `dangerous` | COW overlay on system dirs. `envpod commit` warns but allows system changes by default. | Full development environments where you trust the agent. |

With `advanced` or `dangerous`, each system directory gets its own OverlayFS. Writes go to `pod_dir/sys_upper/{dir}/`, never touching the host. Use `envpod diff` to see system changes and `envpod commit --include-system` to apply them.

### `apps`

Auto-mount host applications into the pod without reinstalling them. Each app name is resolved via `which` on the host, then `ldd` finds all shared library dependencies. The binary and its libraries are bind-mounted read-only into the pod.

```yaml
filesystem:
  apps:
    - python3          # Resolves /usr/bin/python3 + all .so deps
    - google-chrome    # Resolves Chrome binary + GPU/rendering libs
    - node             # Resolves Node.js binary + deps
```

This eliminates the need for `apt install` or `pip install` inside the pod for apps already on the host. The agent gets the exact same binary and libraries as the host system.

Known apps (Chrome, Python, Node, VS Code) also mount their standard data directories (e.g., `/opt/google`, `/usr/lib/python3`).

### `mount_cwd`

Mount the current working directory into the pod with COW isolation. When enabled, `envpod init` captures `$PWD` and stores it as `cwd_path` in pod.yaml. At `envpod run` time, that path is bind-mounted read-only into the pod at the same location — agent sees the real files, but writes go to the COW overlay.

```yaml
filesystem:
  mount_cwd: true       # captures CWD at init time
  # cwd_path: /home/user/project   # auto-set by envpod init (do not set manually)
```

| Field | Default | Description |
|-------|---------|-------------|
| `mount_cwd` | `false` | Enable CWD mounting |
| `cwd_path` | `null` | Absolute path captured at `envpod init` time. Auto-set — do not set manually. |

After mounting, use the standard review workflow:

```bash
sudo envpod diff my-agent        # see what the agent changed in your project
sudo envpod commit my-agent      # apply changes back to the real directory
sudo envpod rollback my-agent    # discard everything
```

**CLI overrides** for `envpod run`:

| Flag | Description |
|------|-------------|
| `-w`, `--mount-cwd` | Force mount CWD even if `mount_cwd: false` in config. Uses current CWD at run time if no `cwd_path` was captured at init. |
| `--no-mount-cwd` | Skip CWD mount even if `mount_cwd: true` in config. |

### `mounts`

Extra host filesystem paths to make available inside the pod. Each mount has:

| Field | Required | Description |
|-------|----------|-------------|
| `path` | Yes | Host path to mount into the pod (same path inside) |
| `permissions` | No | `ReadOnly` (default) or `ReadWrite` |

```yaml
filesystem:
  mounts:
    - path: /opt/google          # Chrome binary (browser pods)
      permissions: ReadOnly
    - path: /etc/alternatives    # System alternatives (browser pods)
      permissions: ReadOnly
    - path: /data/datasets       # Training data (ML pods)
      permissions: ReadOnly
```

Mounts bypass the COW overlay — `ReadWrite` mounts write directly to the host filesystem. Use with care.

### `tracking`

Controls which file changes appear in `envpod diff` and `envpod commit`:

| Field | Default | Description |
|-------|---------|-------------|
| `watch` | `/home`, `/opt`, `/root`, `/srv`, `/workspace` | Only changes under these paths appear in filtered diff. Empty = watch everything. |
| `ignore` | `/var/lib/apt`, `/var/lib/dpkg`, `/var/cache`, `/var/log`, `/tmp`, `/run` | Always excluded from diff/commit, even under watched paths. |

Use `envpod diff --all` or `envpod commit --all` to bypass filtering and see every change.

**Tip:** For development pods with language toolchain caches, add them to `ignore`:

```yaml
filesystem:
  tracking:
    watch:
      - /home
      - /opt
      - /workspace
    ignore:
      - /var/cache
      - /tmp
      - /run
      - /root/.nvm
      - /root/.npm
      - /root/.cargo
      - /root/.rustup
      - /root/.cache
      - /root/.local/lib
```

---

## Network

Controls the network wall — what the agent can reach over the network.

```yaml
network:
  mode: Isolated            # Network isolation mode
  subnet: "10.201"          # Optional custom subnet
  rate_limit: "100/s"       # Optional rate limiting
  bandwidth_cap: "10MB/s"   # Optional bandwidth cap
  dns:
    mode: Whitelist         # DNS resolution policy
    allow:                  # Domains that can resolve
      - api.anthropic.com
      - pypi.org
    deny:                   # Domains that cannot resolve
      - "*.internal"
    remap:                  # DNS remapping (domain → IP)
      internal.api: "10.0.0.5"
```

### `mode`

| Mode | Description |
|------|-------------|
| `Isolated` (default) | Own network namespace with veth pair. DNS resolver per pod. Iptables rules block DNS bypass. Most secure. |
| `Monitored` | Own network namespace with veth pair. Same isolation as `Isolated`, but allows outbound traffic to resolved IPs. DNS queries logged. |
| `Unsafe` | Shares host network namespace. No network isolation. Only use for debugging. |

**Key difference:** `Isolated` + empty `allow` list = no internet. `Monitored` + DNS whitelist = internet access only to whitelisted domains. Both modes create a separate network namespace.

### `dns`

The embedded per-pod DNS resolver. Each pod gets its own resolver, and `/etc/resolv.conf` inside the pod points to it.

#### `dns.mode`

| Mode | Description |
|------|-------------|
| `Whitelist` | Only domains in `allow` list resolve. Everything else returns NXDOMAIN. |
| `Blacklist` | All domains resolve except those in `deny` list. |
| `Monitor` | All domains resolve. Every query is logged to the audit trail. |

#### `dns.allow`

List of domains that can resolve (only used with `Whitelist` mode). Supports wildcards:

```yaml
dns:
  mode: Whitelist
  allow:
    - api.anthropic.com        # Exact domain
    - "*.anthropic.com"        # All subdomains of anthropic.com
    - pypi.org                 # Exact domain
    - "*.pypi.org"             # All subdomains
    - files.pythonhosted.org   # Exact domain
```

**Tip:** Quote wildcard entries (`"*.domain.com"`) to prevent YAML parsing issues.

#### `dns.deny`

List of domains to block (only used with `Blacklist` mode):

```yaml
dns:
  mode: Blacklist
  deny:
    - "*.internal"      # Block internal domains
    - "*.local"         # Block mDNS domains
    - "*.corp"          # Block corporate domains
    - malware.example   # Block specific domain
```

#### `dns.remap`

Map domain names to specific IP addresses:

```yaml
dns:
  remap:
    internal.api: "10.0.0.5"
    test.service: "127.0.0.1"
```

#### Live DNS mutation

DNS policy can be updated on a running pod without restarting:

```bash
sudo envpod dns my-pod --allow newdomain.com
sudo envpod dns my-pod --remove-allow newdomain.com
sudo envpod dns my-pod --deny suspicious.io
```

### `subnet`

Custom subnet base for pod IP assignment. Default: `10.200`. Each pod gets a unique IP within the subnet (e.g., `10.200.1.1`, `10.200.2.1`).

```yaml
network:
  subnet: "10.201"    # Pods with same subnet share an IP range
```

### `ports`, `public_ports`, `internal_ports`

Three keys control port forwarding scope — choose based on who needs access:

| Key | CLI | Format | Scope | Use when |
|-----|-----|--------|-------|----------|
| `ports` | `-p` | `host:container[/proto]` | **Localhost only** | Agent web UI, dev tools — only you need access |
| `public_ports` | `-P` | `host:container[/proto]` | **All network interfaces** | Intentional public service — LAN/internet can reach it |
| `internal_ports` | `-i` | `container[/proto]` | **Other pods only** | Multi-agent: Pod A calls Pod B's API directly by IP or by name |

```yaml
network:
  ports:           # localhost-only: curl localhost:8080 works; LAN cannot reach it
    - "8080:3000"
    - "3000"       # same port both sides
    - "3000/udp"   # UDP
  public_ports:    # all interfaces: curl host-ip:9090 works from LAN too
    - "9090:9090"
  internal_ports:  # pod-to-pod: other pods reach this pod at pod-ip:3000 directly
    - "3000"
    - "5353/udp"
```

**Internal ports — no host mapping.** The pod is accessed directly at its pod IP (e.g. `10.200.2.2:3000`) or by name when `allow_discovery: true` is set. No DNAT — just a FORWARD rule scoped to `10.200.0.0/16 → this pod`.

**CLI override** — add ports per-run without modifying pod.yaml:

```bash
sudo envpod run my-pod -p 8080:3000 -- node server.js     # localhost only
sudo envpod run my-pod -P 9090:9090 -- node server.js     # all interfaces
sudo envpod run my-pod -i 3000 -- node server.js          # other pods only
```

Port forwards are set up when the pod starts and cleaned up automatically on exit. All three flags can be combined.

**Security:** `envpod audit --security` raises **N-04 (LOW)** for `public_ports` only. `ports` and `internal_ports` never raise N-04.

### `allow_discovery` and `allow_pods`

Pod-to-pod DNS discovery lets pods resolve each other by name (`<name>.pods.local`) without sharing IP addresses in config files. Discovery uses **bilateral enforcement** — both the target pod and the querying pod must opt in.

**Target side** — make this pod resolvable:
```yaml
network:
  allow_discovery: true    # default: false
```

**Querying side** — allow this pod to discover named targets:
```yaml
network:
  allow_pods:
    - api-pod              # can resolve api-pod.pods.local
    - worker-pod           # and worker-pod.pods.local
  # or use ["*"] to allow resolution of all discoverable pods
```

**How it works:**
Discovery is handled by the central `envpod-dns` daemon (start once with `sudo envpod dns-daemon`). Each pod's DNS server forwards `*.pods.local` queries to the daemon; the daemon enforces both conditions and returns a synthetic A record or NXDOMAIN.

1. Pod A starts with `allow_discovery: true` → registers with envpod-dns daemon via Unix socket
2. Pod B queries `api-pod.pods.local` → B's DNS server forwards to daemon
3. Daemon checks: `api-pod.allow_discovery == true` AND `B` is in `api-pod.allow_pods` list → returns IP
4. Either check fails → NXDOMAIN
5. On pod exit or `envpod destroy` → daemon unregisters the pod immediately
6. Stale entries from crashed pods are GC'd on daemon startup (PID check)

**Fail-safe:** if `envpod-dns` is not running, all `*.pods.local` → NXDOMAIN. Pods continue running normally with no impact on other DNS.

**Typical multi-agent setup:**

```yaml
# api-pod/pod.yaml — the service
network:
  allow_discovery: true      # resolvable as api-pod.pods.local
  internal_ports: ["3000"]   # accept connections from other pods

# client-pod/pod.yaml — the client
network:
  allow_pods: ["api-pod"]    # permitted to discover api-pod
```

Start the daemon (once, as a system service or in a terminal):
```bash
sudo envpod dns-daemon
```

Agent in client pod:
```bash
curl http://api-pod.pods.local:3000/api/v1/status
```

**Discovery vs access:** `allow_discovery` controls name resolution only. Network access requires `internal_ports` FORWARD rules on the target pod. A pod can be discoverable but still reject connections on ports not listed in `internal_ports`.

**No discovery required for direct IP access.** Run `envpod ls` to see pod IPs. The user can provide the IP directly — `allow_discovery` is only needed for name-based lookup.

### Live discovery mutation — `envpod discover`

Change `allow_discovery` and `allow_pods` on a running pod without restarting it. Changes take effect immediately in the daemon and are also persisted to `pod.yaml` so they survive the next pod restart.

```bash
# Show current state (queries live daemon; falls back to pod.yaml if daemon is down)
sudo envpod discover api-pod

# Enable / disable discoverability
sudo envpod discover api-pod --on
sudo envpod discover api-pod --off

# Add or remove entries from allow_pods
sudo envpod discover api-pod --add-pod client-pod
sudo envpod discover api-pod --remove-pod client-pod
sudo envpod discover api-pod --remove-pod '*'      # clear entire allow_pods list

# Flags can be combined
sudo envpod discover api-pod --on --add-pod orchestrator --add-pod monitor-pod
```

**Status output (no flags):**
```
Pod:              api-pod
IP:               10.200.50.2
Allow discovery:  yes
Allow pods:       client-pod, orchestrator
```

If the daemon is not running, mutations are written to `pod.yaml` only and take effect on the next pod start:
```
warning: envpod-dns not running (...) — updating pod.yaml only
pod.yaml updated. Changes will apply when the pod next starts.
```

### `rate_limit` and `bandwidth_cap`

Future features. Parsed but not yet enforced.

---

## Processor

Controls the processor wall — CPU, memory, and process limits.

```yaml
processor:
  cores: 2.0            # CPU core limit (fractional OK)
  memory: "4GB"         # Memory limit
  cpu_affinity: "0-3"   # Pin to specific CPUs
  max_pids: 1024        # Maximum processes/threads
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cores` | float | unlimited | CPU core limit. `1.0` = one full core, `0.5` = half a core, `4.0` = four cores. Enforced via cgroup `cpu.max`. |
| `memory` | string | unlimited | Memory limit. Supports `KB`, `MB`, `GB` suffixes and fractional values (`1.5GB`). Enforced via cgroup `memory.max`. |
| `cpu_affinity` | string | none | Pin pod to specific CPU cores. Format: `"0-3"` (range), `"0,2,4"` (list), `"0-1,4-5"` (mixed). Maps to `cpuset.cpus`. |
| `max_pids` | integer | unlimited | Maximum number of processes and threads the pod can create. Prevents fork bombs. Enforced via cgroup `pids.max`. |

### Sizing guide

| Use case | Cores | Memory | PIDs |
|----------|-------|--------|------|
| Minimal sandbox | 1.0 | 256MB-512MB | 64-512 |
| Coding agent | 2.0 | 4GB | 1024 |
| Browser pod | 2.0 | 4GB | 1024 |
| ML training | 4.0+ | 16GB+ | — |
| Development box | 2.0 | 4GB | — |

**Note:** Leaving a field unset means no limit (the pod can use all available host resources for that dimension). For production use, always set explicit limits.

---

## Devices

Controls hardware device passthrough — GPU, display, audio, and custom devices.

```yaml
devices:
  gpu: true                      # NVIDIA + DRI devices
  display: true                  # Display socket (Wayland or X11)
  audio: true                    # Audio socket (PipeWire or PulseAudio)
  display_protocol: wayland      # Protocol override
  audio_protocol: pipewire       # Protocol override
  desktop_env: none              # Auto-install desktop environment: none | xfce | openbox | sway
  extra:                         # Custom device paths
    - "/dev/fuse"
    - "/dev/kvm"
```

### `gpu`

When `true`, mounts:
- `/dev/nvidia*` — NVIDIA GPU devices
- `/dev/dri/*` — Direct Rendering Infrastructure
- NVIDIA driver libraries (auto-detected)

Required for CUDA, PyTorch, TensorFlow GPU training, and hardware-accelerated browser rendering.

### `display`

When `true`, enables display forwarding. Use with `envpod run -d` (or `--enable-display`) to activate at runtime.

The display socket is mounted into the pod automatically. Auto-detection checks for Wayland first, then falls back to X11.

### `audio`

When `true`, enables audio forwarding. Use with `envpod run -a` (or `--enable-audio`) to activate at runtime.

The audio socket is mounted into the pod automatically. Auto-detection checks for PipeWire first, then falls back to PulseAudio.

### `display_protocol`

Override automatic display protocol detection:

| Value | Description | Security |
|-------|-------------|----------|
| `auto` (default) | Wayland if available, X11 fallback | Depends on host |
| `wayland` | Wayland only. Compositor isolates clients. | LOW risk (I-04) |
| `x11` | X11 only. Shared display — keylogging/screenshot possible. | CRITICAL risk (I-04) |

### `audio_protocol`

Override automatic audio protocol detection:

| Value | Description | Security |
|-------|-------------|----------|
| `auto` (default) | PipeWire if available, PulseAudio fallback | Depends on host |
| `pipewire` | PipeWire only. Per-stream permissions. | MEDIUM risk (I-05) |
| `pulseaudio` | PulseAudio only. Unrestricted microphone access. | HIGH risk (I-05) |

### `desktop_env`

Auto-install a desktop environment during `envpod init`. Packages are installed into the pod's overlay so the host is never modified.

| Value | Packages | Size | Best for |
|-------|----------|------|----------|
| `none` (default) | — | — | CLI-only pods (coding agents, scripts) |
| `openbox` | openbox, tint2, xterm | ~50 MB | Agent pods (browser automation, web agents) |
| `xfce` | xfce4, xfce4-terminal, dbus-x11 | ~200 MB | Desktop pods (human interaction, workstations) |
| `sway` | sway, foot terminal | ~150 MB | Wayland-native tiling compositor |

**Choosing a desktop:**
- **CLI agents** (Claude Code, Codex, SWE-agent): no desktop needed — leave as `none`
- **Browser agents** (browser-use, Playwright): use `openbox` — minimal WM, just enough to tile browser windows
- **Desktop/workstation pods** (human use, GUI apps): use `xfce` — full desktop with file manager, settings, and taskbar

Pairs with `web_display` (noVNC/WebRTC) for browser-based access, or `devices.display` for host display passthrough.

```yaml
# Agent pod — minimal WM for browser windows
devices:
  desktop_env: openbox

# Desktop pod — full DE for human use
devices:
  desktop_env: xfce

web_display:
  type: novnc
  port: 6080
```

### `extra`

Additional device paths to pass through. Each path is bind-mounted read-write into the pod:

```yaml
devices:
  extra:
    - "/dev/fuse"     # FUSE filesystem support (sshfs, s3fs, rclone)
    - "/dev/kvm"      # KVM virtualization (nested VMs)
```

### Display + audio cheat sheet

| Config | Run flags | Chrome flags | Security |
|--------|-----------|--------------|----------|
| Wayland + PipeWire | `-d -a` | `--ozone-platform=wayland --no-sandbox` | Best |
| X11 + PulseAudio | `-d -a` | `--no-sandbox` | Worst |
| Auto (detect both) | `-d -a` | `--no-sandbox` | Depends on host |
| Headless (no display) | (none) | `--headless --no-sandbox` | N/A |

---

## Web Display

Browser-based desktop access to the pod via noVNC or WebRTC.
All display services run inside the pod. The host only provides port forwarding.

See [WEB-DISPLAY.md](WEB-DISPLAY.md) for full documentation.

```yaml
web_display:
  type: novnc              # none (default) | novnc | webrtc
  port: 6080               # host port for browser access
  resolution: "1280x720"   # virtual display resolution
  codec: vp8               # WebRTC only: vp8 | h264
  audio: false             # WebRTC only: enable audio capture
```

### type

| Value | Tier | Description |
|-------|------|-------------|
| `none` | — | No web display (default) |
| `novnc` | CE | Xvfb + x11vnc + websockify — browser desktop via VNC-over-WebSocket |
| `webrtc` | Premium | GStreamer pipeline — low-latency video + audio via WebRTC |

### port

Host port for browser access. Default `6080`. A localhost-only port forward
is automatically created. Open `http://localhost:{port}/vnc.html` to connect.

### resolution

Virtual display resolution (e.g. `1024x768`, `1280x720`, `1920x1080`).
Default `1024x768`.

### codec (WebRTC only)

Video codec: `vp8` (default, wider browser support) or `h264` (lower CPU,
hardware encode possible).

### audio (WebRTC only)

Enable PulseAudio capture. Adds an Opus audio stream to the WebRTC session.

> **Requirements**: `system_access: advanced` (or `dangerous`) is required
> for noVNC packages. Use `seccomp_profile: browser` and `shm_size: "256MB"`
> when running Chrome.

---

## Tailscale (Premium)

Run a Tailscale node inside the pod for secure remote access via your tailnet.

```yaml
tailscale:
  enabled: true            # default: false
  hostname: my-agent       # tailnet hostname (default: pod name)
  auth_key: "tskey-..."    # auth key (or omit to use vault key TAILSCALE_AUTH_KEY)
  accept_dns: false        # use tailnet DNS (default: false — keeps envpod DNS)
  accept_routes: false     # accept advertised routes (default: false)
```

### enabled

Enable Tailscale inside the pod. Installs `tailscaled` during init and starts
it before the main command at run time.

### hostname

Tailscale hostname for this pod. Defaults to the pod name.

### auth_key

Tailscale auth key for `tailscale up --authkey`. If omitted, envpod checks
the vault for a `TAILSCALE_AUTH_KEY` secret.

> **Security**: Store auth keys in the vault rather than plain text in pod.yaml.
> The `T-01` audit finding flags plain text keys.

---

## Security

Controls seccomp syscall filtering and shared memory.

```yaml
security:
  seccomp_profile: default    # Syscall filter profile
  shm_size: "64MB"            # /dev/shm size
```

### `seccomp_profile`

| Profile | Description | Extra syscalls |
|---------|-------------|----------------|
| `default` | Standard seccomp-BPF filter. Blocks dangerous syscalls (mount, reboot, kexec, etc.). | — |
| `browser` | Default + 7 extra syscalls needed by Chromium's zygote process (clone3, unshare, etc.). | 7 |

Use `browser` for any pod that runs Chrome, Chromium, or Playwright.

### `shm_size`

Size of the pod-private `/dev/shm` tmpfs. Chrome and other browsers need at least 256MB to avoid crashes.

| Use case | Recommended |
|----------|-------------|
| No browser | `64MB` (default) |
| Browser / Chrome | `256MB` |
| Heavy browser workloads | `512MB` |

**Tip:** Run `envpod audit --security -c your-config.yaml` to check for security findings before creating a pod.

---

## Vault

Controls credential injection — how API keys and secrets are delivered to the agent.

```yaml
vault:
  proxy: true                    # Enable transparent HTTPS proxy
  bindings:
    - key: ANTHROPIC_API_KEY     # Vault key name
      domain: api.anthropic.com  # Target domain
      header: "Authorization: Bearer {value}"  # Header template
    - key: OPENAI_API_KEY
      domain: api.openai.com
      header: "Authorization: Bearer {value}"
```

### `vault.proxy`

When `true`, enables the vault proxy — a transparent HTTPS proxy that intercepts API requests and injects real credentials. The agent sends requests with a dummy key; the proxy strips the dummy header and injects the real secret from the vault at the transport layer.

| Value | Description |
|-------|-------------|
| `false` (default) | Secrets injected as environment variables (v0.1 behavior) |
| `true` | Transparent HTTPS proxy intercepts and injects credentials |

### `vault.bindings`

List of vault key-to-domain bindings. Each binding tells the proxy which vault secret to inject for requests to a specific domain.

| Field | Required | Description |
|-------|----------|-------------|
| `key` | Yes | The vault key name (must exist in the pod's vault) |
| `domain` | Yes | The API domain to intercept (e.g., `api.anthropic.com`) |
| `header` | Yes | Header template. `{value}` is replaced with the secret. |

### How it works

1. During `envpod init` (when `proxy: true`): an ephemeral CA cert/key pair is generated and installed into the pod's TLS trust store
2. During `envpod run`: DNS remap entries route bound domains to the host-side veth IP, and the proxy starts on port 443
3. The agent makes normal HTTPS requests — DNS resolves `api.anthropic.com` to the proxy
4. The proxy terminates TLS (using a leaf cert signed by the pod's CA), injects the real `Authorization` header, and forwards upstream
5. The agent never sees the real API key — not in env vars, config files, or memory

### CLI management

```bash
# Store a secret
sudo envpod vault my-pod set ANTHROPIC_API_KEY

# Bind it to a domain (auto-enables proxy, generates CA if needed)
sudo envpod vault my-pod bind ANTHROPIC_API_KEY api.anthropic.com "Authorization: Bearer {value}"

# List bindings
sudo envpod vault my-pod bindings

# Remove a binding
sudo envpod vault my-pod unbind ANTHROPIC_API_KEY
```

### Security considerations

- The proxy runs on the **host side** of the veth pair (outside the pod's network namespace) — the agent cannot tamper with it
- Credentials are loaded from the encrypted vault per-request and never cached in memory beyond the request lifetime
- Audit trail logs which domain + key name was used, but never the secret value
- Run `envpod audit --security` to check for:
  - **V-01**: Bindings present but `proxy: false` (misconfiguration — secrets won't be proxied)
  - **V-02**: Proxy active with Unsafe network mode (secrets could leak via direct connections)
  - **V-03**: Binding domain not in DNS allow list (connections will fail)

### When to use proxy vs env var injection

| Method | Security | Compatibility | Use case |
|--------|----------|---------------|----------|
| **Env var injection** (default) | Medium — agent can read `$KEY` | Works with everything | Quick setup, trusted agents |
| **Proxy injection** (`proxy: true`) | High — agent never sees real key | Requires HTTPS API endpoints | Production, untrusted agents, high-value keys |

---

## Budget

Controls resource and time budgets for the pod.

```yaml
budget:
  max_duration: "4h"         # Maximum session duration
  max_requests: 1000         # Maximum API requests
  max_bandwidth: "1GB"       # Maximum bandwidth usage
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_duration` | string | unlimited | Maximum time the pod can run. Supports: `30s`, `5m`, `2h`, `1h30m`, `2h15m30s`, or plain seconds (`3600`). |
| `max_requests` | integer | unlimited | Maximum number of API requests (future enforcement). |
| `max_bandwidth` | string | unlimited | Maximum total bandwidth usage (future enforcement). |

### Duration examples

```yaml
budget:
  max_duration: "5m"       # 5 minutes (quick sandbox)
  max_duration: "30m"      # 30 minutes (agent session)
  max_duration: "2h"       # 2 hours (coding session)
  max_duration: "1h30m"    # 1.5 hours (compound format)
  max_duration: "8h"       # 8 hours (ML training)
  max_duration: "3600"     # 3600 seconds (plain integer)
```

---

## Audit

Controls what gets logged for compliance and review.

```yaml
audit:
  action_log: true       # Log all actions (default: true)
  system_trace: false    # System-level tracing (default: false)
```

### Fields

| Field | Default | Description |
|-------|---------|-------------|
| `action_log` | `true` | Log all pod actions (start, stop, dns_query, commit, rollback, etc.) to `audit.jsonl`. View with `envpod audit <pod>`. |
| `system_trace` | `false` | Enable system-level tracing (strace-like). Higher overhead, more detail. |

The audit log records timestamps, action types, and details for every operation. Use `envpod audit <pod> --json` for machine-readable output.

---

## Tools

Controls which commands the agent can execute inside the pod.

```yaml
tools:
  allowed_commands:
    - /bin/bash
    - /usr/bin/git
    - /usr/bin/python3
```

### `allowed_commands`

List of allowed command paths or basenames. When non-empty, only these commands can be executed inside the pod. Empty list (default) means all commands are allowed.

```yaml
# Allow everything (default)
tools:
  allowed_commands: []

# Restrictive — only specific tools
tools:
  allowed_commands:
    - /bin/bash
    - /bin/sh
    - /usr/bin/git
    - /usr/bin/python3
    - /usr/bin/pip
```

---

## Snapshots

Controls automatic overlay checkpoints.

```yaml
snapshots:
  auto_on_run: true     # Create snapshot before each `envpod run`
  max_keep: 10          # Maximum snapshots to keep (auto-prune oldest)
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_on_run` | bool | `false` | Automatically create a snapshot before each `envpod run`. Enables "rollback to before last session". |
| `max_keep` | integer | `10` | Maximum total snapshots. When exceeded, oldest auto-created snapshots are pruned. Named (manual) snapshots are never auto-pruned. |

### CLI

```bash
sudo envpod snapshot my-pod create -n "before-refactor"   # Named checkpoint
sudo envpod snapshot my-pod ls                             # List all
sudo envpod snapshot my-pod restore <id>                   # Restore
sudo envpod snapshot my-pod destroy <id>                   # Delete one
sudo envpod snapshot my-pod prune                          # Remove old auto-snapshots
sudo envpod snapshot my-pod promote <id> my-base           # Promote to clonable base
```

---

## Queue

Controls the action staging queue — human-in-the-loop approval for destructive operations.

```yaml
queue:
  socket: true                      # Mount queue socket inside pod
  require_commit_approval: true     # Require `envpod approve` before commit
  require_rollback_approval: false  # Require approval before rollback
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `socket` | bool | `false` | Mount the queue Unix socket at `/run/envpod/queue.sock` inside the pod. Agents submit and poll actions via this socket. |
| `require_commit_approval` | bool | `false` | When true, `envpod commit` creates a staged queue entry instead of executing immediately. Use `envpod approve <pod> <id>` to execute. |
| `require_rollback_approval` | bool | `false` | Same as above, but for `envpod rollback`. |

---

## Host User

Clone the host system user into the pod — agent works in your real environment with COW isolation.

```yaml
host_user:
  clone_host: true          # Clone current user into pod
  dirs:                     # Extra workspace directories to mount
    - /home/mark/Projects
    - /home/mark/src
  exclude:                  # Additional paths to exclude (added to defaults)
    - .config/sensitive-app
  include_dotfiles:         # Override default excludes (force-include)
    - .gitconfig
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `clone_host` | bool | `false` | Clone the host user (name, UID, shell, dotfiles) into the pod rootfs. |
| `dirs` | list | Documents, Desktop, Downloads, Pictures, Videos, Music, Projects, src, workspace | Host directories to bind-mount read-only into the pod. Agent sees your real files but writes go to the COW overlay. |
| `exclude` | list | .ssh, .gnupg, .aws, .config/gcloud, .mozilla, .password-store, .kube, .docker, .netrc, .npmrc, .pypirc, .gem/credentials, .config/google-chrome | Dotfile paths to exclude from cloning (security-sensitive). |
| `include_dotfiles` | list | empty | Force-include specific dotfiles that would otherwise be excluded by `exclude`. |

### How it works

1. During `envpod init`: the host user's `/etc/passwd` entry is cloned into the pod rootfs, home directory is created, and non-excluded dotfiles are copied
2. During `envpod run`: workspace directories from `dirs` are bind-mounted read-only into the pod
3. The agent works in the user's real environment — same shell, same git config, same editor settings
4. All changes go to the COW overlay — `envpod diff` shows what the agent modified, `envpod commit` applies changes back

### Security

Sensitive directories (.ssh, .gnupg, .aws, etc.) are excluded by default. The agent cannot access SSH keys, GPG keys, cloud credentials, or browser profiles unless explicitly added to `include_dotfiles`.

---

## Setup Commands

Commands to run inside the pod during `envpod init` or `envpod setup`. These run as root regardless of the `user` setting, since they typically install packages.

```yaml
# Inline commands (run in order)
setup:
  - "apt-get update && apt-get install -y python3 python3-pip"
  - "pip install numpy pandas matplotlib"
  - "git config --global user.name 'Agent'"

# External script (runs after inline commands)
setup_script: "/path/to/host/setup.sh"
```

### `setup`

List of shell commands executed sequentially inside the pod during setup. Each command runs via `/bin/bash -c "..."`. If any command fails, setup stops and the error is reported.

### `setup_script`

Path to a script on the **host** filesystem. The script is injected into the pod and executed after inline `setup` commands. Useful for complex setup logic that doesn't fit in one-liners.

### How setup works

Pods are container-like environments — they start with a copy of the host filesystem but run in isolation. This means:

1. **Each setup command runs in a fresh shell.** Environment variables, `cd`, and shell state from one command do not carry over to the next. If you install something that modifies the environment (like nvm), you must re-source it in each subsequent command.
2. **Setup runs as root.** Regardless of the `user` setting, setup commands run as root inside the pod so they can install system packages.
3. **The host filesystem is the starting point.** Whatever is installed on the host (Python, git, curl, etc.) is available inside the pod. You only need to install what's missing.
4. **Writes go to the COW overlay.** Setup changes are captured in the overlay, not written to the host. After setup, they become part of the base snapshot.

### Simple patterns

```yaml
# Python packages (pip is pre-installed on most systems)
setup:
  - "pip install numpy pandas matplotlib scipy scikit-learn"

# Install from PyPI and run Playwright's browser installer
setup:
  - "pip install browser-use"
  - "playwright install --with-deps chromium"

# Clone a repo and install from source
setup:
  - "git clone https://github.com/SWE-agent/SWE-agent.git /opt/swe-agent"
  - "cd /opt/swe-agent && pip install --editable ."

# Git configuration
setup:
  - "git config --global user.name 'Coding Agent'"
  - "git config --global user.email 'agent@envpod.local'"
```

### Installing Node.js (nvm pattern)

Node.js tools (Codex, OpenClaw) need Node.js installed inside the pod. The standard approach is nvm, but it requires special handling so the **non-root agent user** can access the installed binaries.

**The problem:** By default, nvm installs to `$HOME/.nvm/` (`/root/.nvm/` when setup runs as root). The `/root` directory is mode `700` — the non-root `agent` user (UID 60000) cannot traverse it, so any binaries symlinked from there fail with permission errors.

**The solution:** Install nvm to `/opt/nvm` (world-readable) and symlink node binaries to `/usr/local/bin` (always in PATH for every user, no `.bashrc` modification needed).

```yaml
filesystem:
  system_access: advanced    # Required — to create symlinks in /usr/local/bin
  tracking:
    ignore:
      - /opt/nvm             # nvm install dir (large, not project code)
      - /root/.npm           # npm cache

setup:
  # Install nvm to /opt/nvm — world-accessible, not /root/.nvm
  - "export NVM_DIR=/opt/nvm && curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash"

  # Source NVM_DIR and install Node.js (must be in the same command)
  - "export NVM_DIR=/opt/nvm && . \"$NVM_DIR/nvm.sh\" && nvm install 22"

  # Symlink to /usr/local/bin — always in PATH for root and non-root users
  - |
    export NVM_DIR=/opt/nvm
    . "$NVM_DIR/nvm.sh"
    ln -sf "$(which node)" /usr/local/bin/node
    ln -sf "$(which npm)" /usr/local/bin/npm
    ln -sf "$(which npx)" /usr/local/bin/npx

  # Install your tool — npm is now in /usr/local/bin, no PATH prefix needed
  - "npm install -g @openai/codex"
```

**Why `system_access: advanced`?** Creating symlinks in `/usr/local/bin` requires a writable system directory. With `safe` mode, system dirs are read-only. The `advanced` mode gives each system directory its own COW overlay so writes succeed without touching the host.

**Why `/usr/local/bin`?** It's always in PATH for all users — root and non-root — with no `.bashrc` modification. The agent user can execute `node`, `npm`, and any globally installed tools immediately.

This same pattern works for any nvm-based tool:

```yaml
# Claude Code (uses native installer, no nvm needed)
setup:
  - "curl -fsSL https://claude.ai/install.sh | bash"

# Codex (needs nvm + Node.js 18+)
setup:
  - "export NVM_DIR=/opt/nvm && curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash"
  - "export NVM_DIR=/opt/nvm && . \"$NVM_DIR/nvm.sh\" && nvm install 22"
  - |
    export NVM_DIR=/opt/nvm
    . "$NVM_DIR/nvm.sh"
    ln -sf "$(which node)" /usr/local/bin/node
    ln -sf "$(which npm)" /usr/local/bin/npm
    ln -sf "$(which npx)" /usr/local/bin/npx
  - "npm install -g @openai/codex"

# OpenClaw (needs nvm + Node.js 22+)
setup:
  - "export NVM_DIR=/opt/nvm && curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash"
  - "export NVM_DIR=/opt/nvm && . \"$NVM_DIR/nvm.sh\" && nvm install 22"
  - |
    export NVM_DIR=/opt/nvm
    . "$NVM_DIR/nvm.sh"
    ln -sf "$(which node)" /usr/local/bin/node
    ln -sf "$(which npm)" /usr/local/bin/npm
    ln -sf "$(which npx)" /usr/local/bin/npx
  - "npm install -g openclaw"
```

### Installing system packages (apt pattern)

When the host doesn't have a required package, install it during setup:

```yaml
setup:
  # Combine update + install in one command to avoid stale package lists
  - "apt-get update && apt-get install -y python3 python3-pip git curl wget"
```

**Tip:** Always use `-y` with `apt-get install` — setup commands run non-interactively and can't answer prompts.

### Installing Playwright / browser dependencies

Browser automation tools like browser-use and Playwright need Chromium and its system dependencies:

```yaml
filesystem:
  system_access: advanced    # Playwright installs system deps

security:
  seccomp_profile: browser   # Chrome needs extra syscalls
  shm_size: "256MB"          # Chrome needs large /dev/shm

setup:
  - "pip install browser-use"
  - "playwright install --with-deps chromium"
  # --with-deps installs system libraries (libgbm, libnss3, etc.)
```

### Multi-line commands

For complex setup steps, use YAML multi-line syntax (`|`) to write readable multi-step scripts:

```yaml
setup:
  - |
    export NVM_DIR=/opt/nvm
    . "$NVM_DIR/nvm.sh"
    ln -sf "$(which node)" /usr/local/bin/node
    ln -sf "$(which npm)" /usr/local/bin/npm
    ln -sf "$(which npx)" /usr/local/bin/npx
```

The `|` preserves newlines — the entire block runs as a single bash script.

### Environment persistence checklist

Since each setup command (and each `envpod run` session) starts a fresh shell, tools that modify the environment need extra steps to persist:

| Tool | Problem | Solution |
|------|---------|----------|
| **nvm** | Installs to `/root/.nvm` (mode 700) | Install to `/opt/nvm`, symlink binaries to `/usr/local/bin` |
| **pyenv** | Shell function, installs to `/root/.pyenv` | Same pattern as nvm: install to `/opt/pyenv`, symlink binaries |
| **rustup** | Modifies `.cargo/env` | Source in each command: `. "$HOME/.cargo/env" && ...` |
| **conda** | Shell function | `eval "$(conda shell.bash hook)" && conda activate myenv` |
| **Custom env vars** | Lost between commands | `echo 'export MY_VAR=value' >> /root/.bashrc` |
| **Working directory** | `cd` doesn't persist | Use absolute paths or `cd /dir && command` in same line |

### DNS requirements for setup

Setup commands often need to download packages from the internet. Make sure your DNS whitelist includes the registries your setup commands need:

| Setup action | Required domains |
|--------------|-----------------|
| `pip install ...` | `pypi.org`, `*.pypi.org`, `files.pythonhosted.org` |
| `npm install ...` | `registry.npmjs.org`, `*.npmjs.org` |
| `apt-get install ...` | Your distro's package mirrors (varies) |
| nvm install | `github.com`, `*.github.com`, `*.githubusercontent.com`, `nodejs.org`, `*.nodejs.org` |
| `cargo install ...` | `crates.io`, `*.crates.io`, `static.crates.io` |
| `playwright install ...` | `playwright.azureedge.net`, `*.blob.core.windows.net` |
| `git clone github.com/...` | `github.com`, `*.github.com`, `*.githubusercontent.com` |

If setup hangs or fails with connection errors, the domain is probably not in your DNS whitelist. Check with `envpod audit <pod>` to see denied DNS queries.

### Base snapshots

Use `--create-base` with `envpod init` or `envpod setup` to create a base snapshot. This freezes the fully-configured state — rootfs, installed packages, symlinks, `.bashrc` modifications, everything. Use `envpod clone` to create new pods from this snapshot in ~130ms — 10x faster than `envpod init`.

`--create-base` accepts an optional name. If omitted, the pod name is used. If a base with that name already exists, envpod auto-increments (e.g. `my-agent-2`, `my-agent-3`) and prints the actual name used.

```bash
# Slow: full init + setup (~1.3s + download time)
sudo envpod init my-agent -c my-config.yaml --create-base

# Fast: clone from snapshot (~130ms, all setup already done)
sudo envpod clone my-agent my-agent-2
sudo envpod clone my-agent my-agent-3

# Custom base name
sudo envpod init my-agent -c my-config.yaml --create-base ubuntu-dev
sudo envpod clone ubuntu-dev worker-1
```

This is especially valuable for Node.js agents where nvm + npm install can take 30+ seconds. Set up once, clone instantly.

---

## Full Examples

### 1. Minimal Sandbox

Air-gapped pod for running untrusted code. No network, tight limits, short timeout.

```yaml
name: sandbox
type: hardened
backend: native

network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow: []

processor:
  cores: 1.0
  memory: "256MB"
  max_pids: 64

budget:
  max_duration: "5m"

audit:
  action_log: true
```

```bash
sudo envpod init sandbox -c sandbox.yaml
sudo envpod run sandbox -- /bin/sh
```

---

### 2. Coding Agent (Claude Code)

Claude Code with API access, GitHub, and package registries. Browser seccomp profile for optional web research.

```yaml
name: claude-code
type: standard
backend: native

network:
  mode: Monitored
  dns:
    mode: Whitelist
    allow:
      # Claude API
      - api.anthropic.com
      - "*.anthropic.com"
      # Claude Code installer
      - claude.ai
      - "*.claude.ai"
      # GitHub
      - github.com
      - "*.github.com"
      - "*.githubusercontent.com"
      # Package registries
      - registry.npmjs.org
      - "*.npmjs.org"
      - pypi.org
      - "*.pypi.org"
      - files.pythonhosted.org
      - crates.io
      - "*.crates.io"
      - static.crates.io
      # Telemetry
      - "*.sentry.io"

filesystem:
  tracking:
    watch:
      - /home
      - /opt
      - /root
      - /workspace
    ignore:
      - /var/cache
      - /var/lib/apt
      - /var/lib/dpkg
      - /tmp
      - /run

processor:
  cores: 2.0
  memory: "4GB"
  max_pids: 1024

security:
  seccomp_profile: browser
  shm_size: "256MB"

budget:
  max_duration: "30m"

audit:
  action_log: true

setup:
  - "curl -fsSL https://claude.ai/install.sh | bash"
```

```bash
sudo envpod init claude-code -c claude-code.yaml
sudo envpod vault claude-code set ANTHROPIC_API_KEY
sudo envpod run claude-code -- claude
```

---

### 3. Secure Browser (Wayland + PipeWire)

Browser pod with the most secure display and audio configuration. Eliminates X11 keylogging (I-04 CRITICAL → LOW) and restricts microphone access (I-05 HIGH → MEDIUM).

```yaml
name: browser-secure
type: standard
backend: native

network:
  mode: Monitored
  dns:
    mode: Blacklist
    deny:
      - "*.internal"
      - "*.local"
      - "*.corp"

filesystem:
  mounts:
    - path: /opt/google
      permissions: ReadOnly
    - path: /etc/alternatives
      permissions: ReadOnly
  tracking:
    watch:
      - /home
      - /opt
      - /workspace
    ignore:
      - /var/cache
      - /var/lib/apt
      - /var/lib/dpkg
      - /tmp
      - /run

processor:
  cores: 2.0
  memory: "4GB"
  max_pids: 1024

security:
  seccomp_profile: browser
  shm_size: "256MB"

devices:
  gpu: true
  display: true
  audio: true
  display_protocol: wayland
  audio_protocol: pipewire

budget:
  max_duration: "30m"

audit:
  action_log: true
```

```bash
sudo envpod init browser-secure -c browser-secure.yaml
sudo envpod run browser-secure -d -a -- google-chrome --no-sandbox --ozone-platform=wayland https://example.com
```

---

### 4. GPU ML Training

Large-memory pod with NVIDIA GPU passthrough for model training. Whitelisted to PyPI and HuggingFace only.

```yaml
name: ml-trainer
type: standard
backend: native

network:
  mode: Monitored
  dns:
    mode: Whitelist
    allow:
      - pypi.org
      - "*.pypi.org"
      - files.pythonhosted.org
      - huggingface.co
      - "*.huggingface.co"
      - download.pytorch.org
      - "*.anaconda.org"

processor:
  cores: 4.0
  memory: "16GB"

devices:
  gpu: true

budget:
  max_duration: "8h"

audit:
  action_log: true

setup:
  - "pip install torch torchvision numpy pandas matplotlib"
```

```bash
sudo envpod init ml-trainer -c ml-trainer.yaml
sudo envpod run ml-trainer -- python train.py
sudo envpod diff ml-trainer    # review model checkpoints
sudo envpod commit ml-trainer  # export results
```

---

### 5. Development Box

Full development environment with unrestricted internet, advanced system access (can install packages), and generous ignore list for toolchain caches.

```yaml
name: devbox
type: standard
backend: native

filesystem:
  system_access: advanced
  tracking:
    watch:
      - /home
      - /opt
      - /root
      - /workspace
    ignore:
      - /var/cache
      - /var/lib/apt
      - /var/lib/dpkg
      - /tmp
      - /run
      # Language toolchain caches
      - /root/.nvm
      - /root/.npm
      - /root/.node-gyp
      - /root/.bun
      - /root/.cargo
      - /root/.rustup
      - /root/.cache
      - /root/.local/lib
      - /root/.config

network:
  mode: Monitored
  dns:
    mode: Monitor

processor:
  cores: 2.0
  memory: "4GB"

budget:
  max_duration: "8h"

audit:
  action_log: true
```

```bash
sudo envpod init devbox -c devbox.yaml
sudo envpod run devbox -- bash
# Install packages freely — changes go to COW overlay
# Use `envpod commit --include-system` to keep system changes
```

---

### 6. Node.js Development

Full Node.js environment with nvm, npm, and persistent PATH. Uses `system_access: advanced` so nvm can write to system directories, and symlinks node binaries to `/opt/bin` for availability in all sessions.

```yaml
name: nodejs-dev
type: standard
backend: native

filesystem:
  system_access: advanced     # nvm needs to write to system dirs
  tracking:
    watch:
      - /home
      - /opt
      - /root
      - /workspace
    ignore:
      - /var/cache
      - /var/lib/apt
      - /var/lib/dpkg
      - /tmp
      - /run
      - /root/.nvm             # nvm internals (large, not project code)
      - /root/.npm              # npm cache
      - /root/.node-gyp         # native addon build cache

network:
  mode: Monitored
  dns:
    mode: Whitelist
    allow:
      # nvm install script (hosted on GitHub)
      - raw.githubusercontent.com
      - "*.githubusercontent.com"
      - nodejs.org
      - "*.nodejs.org"
      # nvm git clone
      - github.com
      - "*.github.com"
      # npm registry
      - registry.npmjs.org
      - "*.npmjs.org"
      - "*.npmjs.com"

processor:
  cores: 2.0
  memory: "2GB"

budget:
  max_duration: "8h"

audit:
  action_log: true

setup:
  # Install nvm
  - "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash"
  # Source nvm and install Node.js (each command is a fresh shell)
  - ". \"$HOME/.nvm/nvm.sh\" && nvm install 22"
  # Symlink binaries to /opt/bin so they work without sourcing nvm
  - |
    . "$HOME/.nvm/nvm.sh"
    mkdir -p /opt/bin
    ln -sf "$(which node)" /opt/bin/node
    ln -sf "$(which npm)" /opt/bin/npm
    ln -sf "$(which npx)" /opt/bin/npx
    echo 'export PATH="/opt/bin:$PATH"' >> /root/.bashrc
```

```bash
sudo envpod init nodejs-dev -c nodejs-dev.yaml
sudo envpod run nodejs-dev -- node -v        # works without sourcing nvm
sudo envpod run nodejs-dev -- npm init -y
```

**Why this works:** Each setup command runs in a fresh shell, so nvm isn't available after the install step. The symlink trick creates stable binaries at `/opt/bin/node` etc., and the `.bashrc` line adds `/opt/bin` to PATH for interactive sessions. See [Setup Commands](#setup-commands) for the full explanation and patterns for other tools (Codex, OpenClaw, pyenv, rustup).

---

### 7. Multi-Agent Fleet Template

Base config for spawning many identical agents. Create once, clone many.

```yaml
name: agent-base
type: standard
backend: native

network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow:
      - api.anthropic.com
      - api.openai.com
      - registry.npmjs.org
      - pypi.org
      - files.pythonhosted.org
      - github.com
      - "*.github.com"
      - "*.githubusercontent.com"

filesystem:
  tracking:
    watch:
      - /home
      - /opt
      - /workspace
    ignore:
      - /var/cache
      - /var/lib/apt
      - /var/lib/dpkg
      - /tmp
      - /run

processor:
  cores: 1.0
  memory: "2GB"
  max_pids: 512

budget:
  max_duration: "1h"

audit:
  action_log: true

setup:
  - "git config --global user.name 'Agent'"
  - "git config --global user.email 'agent@envpod.local'"
  - "pip install aider-chat"
```

```bash
# Create once (slow — builds rootfs + runs setup)
sudo envpod init agent-base -c agent-fleet.yaml

# Clone many (fast — ~8ms each)
for i in $(seq 1 50); do
    sudo envpod clone agent-base "agent-$i"
done

# Run agents in parallel
sudo envpod run agent-1 -- aider --model sonnet &
sudo envpod run agent-2 -- aider --model sonnet &
sudo envpod run agent-3 -- aider --model sonnet &
```

---

### 8. FUSE Filesystem Agent

Agent with FUSE support for mounting remote filesystems (S3, Google Cloud Storage, SSH).

```yaml
name: fuse-agent
type: standard
backend: native

network:
  mode: Monitored
  dns:
    mode: Whitelist
    allow:
      - "*.amazonaws.com"
      - "*.storage.googleapis.com"
      - pypi.org
      - "*.pypi.org"
      - files.pythonhosted.org

processor:
  cores: 2.0
  memory: "2GB"

devices:
  extra:
    - "/dev/fuse"

budget:
  max_duration: "2h"

audit:
  action_log: true

setup:
  - "pip install s3fs rclone"
```

---

### 9. Legacy Browser (X11 + PulseAudio)

For hosts without Wayland or PipeWire. Higher security risk — see notes.

```yaml
name: browser-legacy
type: standard
backend: native

network:
  mode: Monitored
  dns:
    mode: Blacklist
    deny:
      - "*.internal"
      - "*.local"

filesystem:
  mounts:
    - path: /opt/google
      permissions: ReadOnly
    - path: /etc/alternatives
      permissions: ReadOnly

processor:
  cores: 2.0
  memory: "4GB"
  max_pids: 1024

security:
  seccomp_profile: browser
  shm_size: "256MB"

devices:
  gpu: true
  display: true
  audio: true
  display_protocol: x11
  audio_protocol: pulseaudio

budget:
  max_duration: "30m"

audit:
  action_log: true
```

**Security warnings:**
- I-04 **CRITICAL**: X11 allows any client to capture keystrokes and screenshots from other X11 clients
- I-05 **HIGH**: PulseAudio gives unrestricted microphone access

Prefer [Wayland + PipeWire](#3-secure-browser-wayland--pipewire) whenever possible.

---

## Defaults Reference

Every field that you omit uses a secure default. Here's the complete default config:

```yaml
name: ""                    # Must be set
type: standard
backend: native
user: agent                 # Non-root (UID 60000)

filesystem:
  system_access: safe       # Read-only system dirs
  mount_cwd: false          # No CWD mount
  cwd_path: null            # Auto-set by envpod init when mount_cwd: true
  apps: []                  # No auto-mounted host apps
  mounts: []                # No extra mounts
  tracking:
    watch:                  # Default watched paths
      - /home
      - /opt
      - /root
      - /srv
      - /workspace
    ignore:                 # Default ignored paths
      - /var/lib/apt
      - /var/lib/dpkg
      - /var/cache
      - /var/log
      - /tmp
      - /run

network:
  mode: Isolated            # No outbound network
  subnet: null              # Auto-assigned (10.200.x.x)
  rate_limit: null          # No rate limit
  bandwidth_cap: null       # No bandwidth cap
  dns:
    mode: Monitor           # Log all queries
    allow: []               # Empty whitelist (nothing resolves in Whitelist mode)
    deny: []                # Empty blacklist
    remap: {}               # No remapping

processor:
  cores: null               # No CPU limit
  memory: null              # No memory limit
  cpu_affinity: null        # No CPU pinning
  max_pids: null            # No PID limit

devices:
  gpu: false                # No GPU
  display: false            # No display
  audio: false              # No audio
  display_protocol: auto    # Auto-detect
  audio_protocol: auto      # Auto-detect
  desktop_env: none         # No desktop environment
  extra: []                 # No extra devices

security:
  seccomp_profile: ""       # Default seccomp filter
  shm_size: null            # 64MB default

budget:
  max_duration: null        # No time limit
  max_requests: null        # No request limit
  max_bandwidth: null       # No bandwidth limit

audit:
  action_log: true          # Logging enabled
  system_trace: false       # System tracing disabled

vault:
  proxy: false              # No proxy injection
  bindings: []              # No bindings

tools:
  allowed_commands: []      # All commands allowed

snapshots:
  auto_on_run: false        # No auto-snapshots
  max_keep: 10              # Keep up to 10 snapshots

queue:
  socket: false             # Queue socket not mounted
  require_commit_approval: false
  require_rollback_approval: false

host_user:
  clone_host: false         # Don't clone host user
  dirs: []                  # Default workspace dirs (Documents, Desktop, etc.)
  exclude: []               # Default security excludes (.ssh, .gnupg, etc.)
  include_dotfiles: []      # No forced includes

setup: []                   # No setup commands
setup_script: null          # No setup script
```

---

## Tips

1. **Start minimal.** Begin with the [Minimal Config](#minimal-config) and add only what you need.
2. **Check security before deploying.** Run `envpod audit --security -c your-config.yaml` to see findings.
3. **Use cloning for fleets.** Create one pod with `envpod init`, then `envpod clone` for each instance (~8ms per clone vs ~1.3s per init).
4. **Quote wildcards in YAML.** Always use `"*.domain.com"` (not `*.domain.com`) to avoid YAML parsing errors.
5. **Set processor limits.** Omitting `cores` and `memory` means no limit — the pod can use all host resources.
6. **Prefer Wayland over X11.** X11 display sharing is a CRITICAL security risk. Use `display_protocol: wayland` when possible.
7. **Use `advanced` system_access for dev pods.** If your agent needs to install system packages, `safe` mode will block it. Use `advanced` and review changes with `envpod commit --include-system`.

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
