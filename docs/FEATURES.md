# envpod CE — Features

> "Docker isolates. Envpod governs."

envpod gives every AI agent a foundation, four hard walls, and a governance ceiling.
The agent runs inside a **pod** — isolated, auditable, and fully reversible.
You stay in control of every change it makes.

---

## Foundation — Copy-on-Write Filesystem

**OverlayFS copy-on-write** — every file the agent writes goes into
a private overlay. The host filesystem is untouched until you explicitly approve
the changes. The COW foundation is what makes everything else reversible — the
entire pod sits on top of it.

```bash
envpod diff   my-agent        # see exactly what the agent changed
envpod commit my-agent        # apply approved changes to host
envpod rollback my-agent      # discard everything — host unchanged
```

**Impact:** an agent can run for hours writing thousands of files. Nothing
reaches your real filesystem until you review and approve it. One command
to undo everything if it went wrong.

---

## Credential Vault

Secrets are stored encrypted at rest (ChaCha20-Poly1305) in a per-pod vault.
They are injected as environment variables at run time — they never appear in
pod.yaml, command lines, or logs.

```bash
echo "sk-..." | envpod vault my-agent set ANTHROPIC_API_KEY
envpod vault my-agent list      # shows key names, never values
envpod vault my-agent import .env
```

**Impact:** API keys, database passwords, and tokens stay out of config files
and out of version control. The agent gets its credentials at runtime only.

---

## Action Queue & Human Approval

Agents submit actions through a Unix socket. You decide what executes
immediately, what waits for human approval, and what is blocked entirely.

Four tiers:
- **Immediate** — executes without interruption (safe, reversible actions)
- **Delayed** — executes after a grace period (cancel window)
- **Staged** — waits for explicit human approval before running
- **Blocked** — permanently denied

```bash
envpod approve my-agent <action-id>
envpod cancel  my-agent <action-id>
envpod queue   my-agent ls
```

Every executed action is tracked with an undo mechanism — so even approved
actions can be reversed.

**Impact:** the agent cannot send a request, push to git, or delete files
without going through your approval gate. You define the rules per pod.

---

## Action Catalog — 20 Built-in Types

The host defines a menu of exactly what the agent is allowed to do.
Agents discover available actions at runtime (MCP-style tool discovery)
and call them by name. envpod executes them — after validation, tier
checks, and any required approval. Credentials are fetched from the vault
at execution time; the agent never sees them.

**HTTP (6):** `http_get`, `http_post`, `http_put`, `http_patch`, `http_delete`, `webhook`

**Filesystem (7):** `file_create`, `file_write`, `file_delete`, `file_copy`, `file_move`, `dir_create`, `dir_delete`

**Git (6):** `git_commit`, `git_push`, `git_pull`, `git_checkout`, `git_branch`, `git_tag`

**Custom (1):** host-defined schema, host-side executor — bring your own tool

```bash
envpod actions my-agent ls              # agent discovers available tools
envpod actions my-agent add            # host adds a new action
envpod actions my-agent set-tier send_file staged
```

Hot-reload — update the catalog without restarting the pod.

**Impact:** the agent cannot call tools you have not explicitly listed.
No surprise API calls, no arbitrary shell commands. The tool menu is
defined and controlled by you.

---

## DNS Filtering

Every pod runs its own embedded DNS resolver. You control what the agent
can reach on the network.

```yaml
dns:
  mode: allowlist
  allow:
    - api.anthropic.com
    - pypi.org
    - github.com
```

Modes: `allowlist` (allow-list only), `denylist` (block specific domains),
`monitor` (log all queries), `remap` (redirect domains to different IPs).

Anti-DNS-tunneling protection is built in — prevents data exfiltration via
crafted DNS queries.

**Impact:** the agent cannot phone home, exfiltrate data, or reach unexpected
services. You define the network surface exactly.

---

## Network Isolation

Each pod gets its own network namespace with a dedicated veth pair.
Pods are fully isolated from each other and from the host network by default.

**Port forwarding — three scopes:**
- `ports` — localhost only (your machine, not the LAN)
- `public_ports` — all network interfaces
- `internal_ports` — pod-to-pod only (no host involvement, no DNAT)

**Pod discovery** — pods find each other by name (`agent-b.pods.local`)
when explicitly allowed. Bilateral: both pods must opt in.

---

## Process Isolation

- **PID namespace** — pod processes cannot see host processes
- **cgroups v2** — hard CPU, memory, and IO limits enforced by the kernel
- **seccomp-BPF** — syscall filtering blocks dangerous system calls
- **UTS namespace** — pod has its own hostname
- **User namespace** — agent runs as an unprivileged user inside the pod

```yaml
resources:
  memory_mb: 2048
  cpu_shares: 512
  pids_max: 256
```

**Impact:** a runaway agent cannot starve the host of memory or CPU.
A compromised agent cannot call dangerous syscalls or see host processes.

---

## Audit Log

Every action in the pod lifecycle is recorded in an append-only JSONL file.
Create, run, diff, commit, rollback, vault access, queue events, DNS queries
— all timestamped and structured.

```bash
envpod audit my-agent              # timeline view
envpod audit my-agent --json       # machine-readable
envpod audit my-agent --security   # static security analysis of pod config
```

The security audit checks your pod.yaml for misconfigurations — unsafe network
mode, missing resource limits, root execution, and more — before you ever run.

**Impact:** full traceability of everything the agent did and every decision
you made. If something goes wrong, you have a complete record to investigate.

---

## Snapshots

Save and restore the agent's overlay state at any point.

```bash
envpod snapshot my-agent create before-refactor
envpod snapshot my-agent ls
envpod snapshot my-agent restore before-refactor
envpod snapshot my-agent promote <id> my-base   # promote snapshot to base pod
```

Auto-snapshot before every run — a checkpoint always exists from before
the last execution. Configurable retention (`keep_last: 5`).

Promote any snapshot to a base pod with `promote` — the snapshot's overlay
becomes an instantly-clonable base, ready for `envpod clone`.

**Impact:** experiment freely. The agent tried something destructive?
Restore to before the run with one command. Found a good state? Promote it
to a base and clone a fleet from that exact checkpoint.

---

## Base Pods & Fast Cloning

Run `envpod init` once with all your setup commands (install dependencies,
configure tools). That becomes a base. Clone from it instantly.

```bash
envpod base create python-base    # ~1.3s — runs setup once
envpod clone python-base agent-1  # ~130ms — no setup re-run
envpod clone python-base agent-2
envpod clone python-base agent-3
```

10× faster than re-running setup. Clone a fleet of identical agents in seconds.

**Impact:** spin up 50 identical coding agents in under a minute. Each gets
its own isolated overlay — changes in one never affect the others.

---

## Web Dashboard

`envpod dashboard` starts a local web interface on `localhost:9090`.

- **Fleet overview** — all pods, status, resource usage, pending changes
- **Pod detail** — live cgroup stats (CPU / memory / PIDs)
- **Audit tab** — filterable event timeline
- **Diff tab** — filesystem changes with commit and rollback buttons

No database, no external dependencies — reads existing pod state directly.

**Impact:** review and approve agent changes from a browser instead of
the terminal. Useful when managing multiple agents at the same time.

---

## Remote Control

Send control commands to a running pod without stopping it.

```bash
envpod lock   my-agent            # freeze — pause all processes instantly
envpod remote my-agent resume     # unfreeze
envpod remote my-agent kill       # terminate immediately
envpod remote my-agent restrict network=off
```

Live mutation — update DNS rules or port forwarding on a running pod
without restarting it.

**Impact:** something looks wrong mid-run? Freeze the agent in place
in milliseconds. Inspect, decide, then resume or kill.

---

## Web Display

Access a pod's graphical desktop from any browser — no X11 or Wayland needed.

```yaml
web_display:
  type: novnc
  port: 6080
  resolution: "1920x1080"
  audio: true              # PulseAudio + Opus/WebM streaming via WebSocket
  audio_port: 6081         # audio WebSocket port (default: 6081)
  file_upload: true        # upload button in noVNC panel (default: true)
  upload_port: 5080        # Python HTTP upload server port (default: 5080)
```

```bash
sudo envpod start my-agent
# Open http://localhost:6080 in your browser — auto-connects, no click needed
```

A virtual display (Xvfb) + VNC server (x11vnc) + WebSocket bridge (websockify)
run inside the pod. Port forwarded to localhost automatically. Works on headless
servers, SSH sessions, and remote machines.

**Audio streaming** — when `audio: true`, PulseAudio runs inside the pod and
streams audio via Opus/WebM over a WebSocket on the audio port. A speaker button
appears in the noVNC sidebar to toggle audio on/off. Works with Chrome, Firefox,
and any application that outputs to PulseAudio.

**File upload** — when `file_upload: true` (the default), an upload button
appears in the noVNC sidebar. Click it to upload files from your host into the
pod at `/tmp/uploads/`. Uses a lightweight Python HTTP server on the upload port.

**Auto-branding** — envpod automatically replaces the noVNC logo with the envpod
logo, sets a custom favicon, updates the page title to "envpod -- <pod-name>",
and injects auto-connect so the VNC session starts immediately without clicking
a connect button.

**Impact:** run browser agents, GUI applications, or desktop environments
inside governed pods — accessible from any browser with audio and file transfer.
No host display required.

See [WEB-DISPLAY.md](WEB-DISPLAY.md) for the full guide.

---

## Device Passthrough

Selectively forward host devices into the pod.

```yaml
devices:
  gpu: true           # NVIDIA / AMD GPU (CUDA, ROCm)
  display: true       # Wayland or X11 (auto-detected)
  audio: true         # PipeWire or PulseAudio (auto-detected)
  desktop_env: xfce   # Auto-install desktop: none | xfce | openbox | sway
```

Use `desktop_env` to auto-install a desktop environment during `envpod init` —
xfce (~200 MB), openbox (~50 MB), or sway (~150 MB, Wayland-native). Pairs
with `web_display` (noVNC/WebRTC) for browser-based access.

**Impact:** run GUI applications and GPU workloads inside governed pods.
Agents that manipulate images, video, or use ML inference get hardware access
without escaping the governance layer.

---

## ARM64

The same static binary runs on x86\_64 and ARM64 with no runtime dependencies.

- **Raspberry Pi 4 / 5** — RPi OS 64-bit or Ubuntu 24.04 (enable cgroups v2 in cmdline.txt)
- **NVIDIA Jetson Orin** — JetPack 6 (cgroups v2 default, GPU passthrough via `/dev/nvhost-*`)

Single `musl`-linked binary — copies anywhere, runs anywhere.

---

## Host App Auto-Mount

Use host-installed applications inside a pod without reinstalling them.
List binaries in `filesystem.apps` and envpod resolves each one via `which`
and `ldd`, then bind-mounts the binary, all shared libraries, and known
data directories read-only into the pod.

```yaml
filesystem:
  system_access: safe
  apps:
    - google-chrome
    - python3
    - node
```

Supported applications include Chrome/Chromium, Python, Node.js, VS Code,
and any binary on the host `$PATH`. Each app is resolved at init time —
symlinks are followed, library directories are deduplicated, and known
data paths (e.g. `/opt/google`, `/usr/lib/python3.*`) are included
automatically.

**Impact:** skip lengthy `apt-get install` steps in setup. The host already
has Chrome? The pod gets it instantly via read-only bind mounts — no
download, no disk duplication, no version drift.

---

## Clone Host User

Mirror your host user environment into the pod. Same username, shell,
dotfiles, and workspace directories — but isolated under UID 60000 with
sensitive paths excluded by default.

```yaml
host_user:
  clone_host: true
  dirs:                       # workspace dirs to bind-mount (read-only)
    - Documents
    - Projects
    - src
  # include_dotfiles:         # extra dotfiles beyond the defaults
  #   - .cargo
  # exclude:                  # override default exclusions
  #   - .ssh
```

When `clone_host: true`, envpod:
1. Reads your host username and shell from `/etc/passwd`
2. Creates a matching user in the pod (UID 60000, same home path)
3. Copies dotfiles (`.bashrc`, `.gitconfig`, `.vimrc`, `.tmux.conf`, etc.)
4. Bind-mounts workspace directories read-only into the pod

Sensitive paths are excluded by default: `.ssh`, `.gnupg`, `.aws`,
`.docker`, `.kube`, `.netrc`, `.npmrc`, and others. Override with the
`exclude` list.

**Impact:** the agent works in an environment that looks like yours —
same shell, same git config, same editor settings — without exposing
private keys, cloud credentials, or browser profiles.

---

## Mount Working Directory

The simplest way to give an agent access to your project.

```yaml
filesystem:
  mount_cwd: true    # captures $PWD at init time
```

When enabled, `envpod init` captures your current working directory and
`envpod run` bind-mounts it read-only into the pod at the same path.
Writes go to the COW overlay — review with `envpod diff`, apply with
`envpod commit`.

Or use the `-w` flag at run time (no config needed):

```bash
sudo envpod run my-agent -w -- claude         # mount CWD on-the-fly
sudo envpod run my-agent --no-mount-cwd -- sh # skip even if pod.yaml enables it
```

**Impact:** point an agent at your project directory in one flag. It sees
your files read-only, writes go to the overlay, and you approve changes
before they touch disk.

---

## Pod Lifecycle

```bash
envpod init      my-agent -c agent.yaml   # create pod
envpod setup     my-agent                 # run setup commands
envpod run       my-agent -- claude       # run agent
envpod diff      my-agent                 # review changes
envpod commit    my-agent                 # apply to host
envpod rollback  my-agent                 # discard changes
envpod snapshot  my-agent create v1       # checkpoint
envpod clone     my-agent my-agent-2      # duplicate
envpod audit     my-agent                 # review history
envpod destroy   my-agent                 # remove pod
envpod gc                                 # clean up stale resources
```

---

*Copyright 2026 Xtellix Inc. All rights reserved. Licensed under the Business Source License 1.1.*
