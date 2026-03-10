# Web Display — Browser-Based Pod Desktop

> Access a pod's graphical desktop from any browser. No X11 or Wayland needed.

---

## Overview

Web display runs a virtual display stack **inside** the pod and exposes it
via a browser-accessible WebSocket. Open `http://localhost:6080/vnc.html`
to see and interact with the pod desktop.

**Stack:** Xvfb (virtual display) → x11vnc (VNC server) → websockify (WebSocket bridge) → browser

This is different from [display passthrough](TUTORIALS.md#tutorial-1-browser-pod-with-display--audio),
which forwards the host's Wayland/X11 display socket into the pod. Web display
works on headless servers, SSH sessions, and remote machines — no host display
required.

---

## Quick Start

### 1. Create the pod

```yaml
# web-display.yaml
name: my-desktop
type: standard
user: root

web_display:
  type: novnc
  port: 6080
  resolution: "1280x720"

filesystem:
  system_access: advanced
  mounts:
    - path: /opt/google
      permissions: ReadOnly

network:
  mode: Monitored
  dns:
    mode: Blacklist
    deny:
      - "*.internal"
      - "*.local"

processor:
  cores: 4.0
  memory: "4GB"
  max_pids: 1024

security:
  seccomp_profile: browser
  shm_size: "256MB"
```

### 2. Init and start

```bash
sudo envpod init my-desktop -c web-display.yaml
sudo envpod start my-desktop
```

### 3. Open in browser

```
http://localhost:6080/vnc.html
```

Click **Connect** in the noVNC interface. Display services auto-start
with `envpod start` — the desktop is ready immediately.

### 4. Run commands inside the pod

```bash
sudo envpod run my-desktop -- google-chrome --no-sandbox --start-maximized
sudo envpod run my-desktop -- bash
```

### 5. Stop when done

```bash
sudo envpod stop my-desktop
# Start again later — overlay is preserved
sudo envpod start my-desktop
```

> **Tip:** `envpod start` is the recommended way to launch desktop pods.
> It starts the pod in the background with all services (display, audio,
> upload) running automatically. Use `envpod run` to execute commands
> inside the already-running pod.

---

## How It Works

```
Browser ──WebSocket──→ websockify:6080 ──VNC──→ x11vnc:5900 ──X11──→ Xvfb:99
                           │                                           │
                     pod network ns                              virtual display
                           │                                     (1280x720x24)
              host:6080 ─DNAT─→ pod:6080
```

### At `envpod init`

1. Third-party apt sources are cleaned and `/var/lib/apt/lists/*` is cleared (prevents GPG failures from stale host apt cache leaking through OverlayFS)
2. `apt-get install xvfb x11vnc novnc websockify screen dbus-x11` runs inside the pod
3. Two scripts are written:
   - `/usr/local/bin/envpod-display-services` — background daemon that manages Xvfb, x11vnc, websockify, audio, and upload services with auto-restart loops. Writes a PID file and starts a D-Bus session bus.
   - `/usr/local/bin/envpod-display-start` — lightweight wrapper that starts the daemon if not already running, exports `DISPLAY=:99` and `DBUS_SESSION_BUS_ADDRESS`, then `exec`s your command.
4. Your `setup:` commands run after

### At `envpod start` or `envpod run`

1. The wrapper script starts the display services daemon if it is not already running
2. `DISPLAY=:99` and `DBUS_SESSION_BUS_ADDRESS` are exported
3. Port forward `localhost:{port}` → `pod_ip:6080` is set up via iptables
4. Your command launches on the virtual display (with `run`), or the pod waits for connections (with `start`)
5. Display services run independently of your command — they persist across multiple `envpod run` sessions and survive `envpod stop`/`envpod start` cycles

---

## Configuration

```yaml
web_display:
  type: novnc              # none (default) | novnc
  port: 6080               # host port for browser access
  resolution: "1280x720"   # virtual display resolution
```

### type

| Value | Description |
|-------|-------------|
| `none` | No web display (default) |
| `novnc` | Xvfb + x11vnc + websockify — browser desktop via VNC-over-WebSocket |

### port

Host port for browser access. Default `6080`. A localhost-only port forward
is automatically created.

### resolution

Virtual display resolution. Common values:

| Resolution | Use Case |
|-----------|----------|
| `1024x768` | Default, small footprint |
| `1280x720` | Good balance (720p) |
| `1920x1080` | Full HD (higher CPU) |

---

## Clipboard

**Pod to host (copy):** Works automatically. Text copied inside the pod (e.g., Ctrl+C in a text editor) appears in the host clipboard.

**Host to pod (paste):** Use the **sidebar clipboard panel** in the noVNC interface. Click the clipboard icon in the left sidebar, paste your text into the panel (Ctrl+V), and it is sent to the VNC clipboard. Direct Ctrl+V on the desktop canvas does not work -- browsers block clipboard access to canvas elements for security reasons.

**Pasting into terminals:** After sending text via the sidebar panel, use Ctrl+Shift+V in xfce4-terminal (or middle-click in xterm). Terminals do not respond to Ctrl+V -- that is interpreted as a literal control character.

**Note:** Pasting into the sidebar panel **replaces** the pod clipboard contents (it does not append to previous text).

---

## Key Repeat

Holding a key to repeat characters works out of the box. The display services configure `xset r rate 250 30` (250ms delay, 30 repeats/sec) and x11vnc runs with the `-repeat` flag.

---

## Use Cases

### Browser Agent (Chrome from host)

Mount Chrome from the host — no need to install it inside the pod:

```yaml
filesystem:
  system_access: advanced
  mounts:
    - path: /opt/google
      permissions: ReadOnly

security:
  seccomp_profile: browser
  shm_size: "256MB"
```

```bash
sudo envpod run my-agent -- google-chrome --no-sandbox --start-maximized
```

> The `--no-sandbox` warning is expected — envpod's namespace isolation
> replaces Chrome's internal sandbox.

### GUI Desktop

Three desktop tiers are available via `devices.desktop_env`:

| Tier | `desktop_env` | Install size | Best for |
|------|---------------|-------------|----------|
| CLI only | `none` (default) | 0 MB | Headless agents, scripts |
| Lightweight | `openbox` | ~50 MB | Agent pods, browser automation |
| Full desktop | `xfce` | ~200 MB | Interactive desktop sessions |

The `desktop_env` setting auto-installs the window manager during `envpod init`
and **auto-starts it** when display services launch — no manual commands needed.

#### Openbox (lightweight — recommended for agents)

```yaml
# pod.yaml
name: my-agent
web_display:
  type: novnc
  port: 6080
  resolution: "1280x720"
devices:
  desktop_env: openbox
filesystem:
  system_access: advanced
processor:
  cores: 2.0
  memory: "2GB"
security:
  seccomp_profile: browser
  shm_size: "256MB"
```

```bash
sudo envpod init my-agent -c pod.yaml
sudo envpod start my-agent             # desktop auto-starts in noVNC
sudo envpod run my-agent -- bash       # open a shell (optional)
```

Open `http://localhost:6080/vnc.html` — openbox is already running with
right-click context menu. Lightweight and fast.

#### XFCE (full desktop — recommended for interactive use)

```yaml
# pod.yaml
name: my-desktop
web_display:
  type: novnc
  port: 6080
  resolution: "1920x1080"
devices:
  desktop_env: xfce
filesystem:
  system_access: advanced
processor:
  cores: 4.0
  memory: "4GB"
security:
  seccomp_profile: browser
  shm_size: "256MB"
```

```bash
sudo envpod init my-desktop -c pod.yaml
sudo envpod start my-desktop           # XFCE auto-starts in noVNC
sudo envpod run my-desktop -- bash     # open a shell (optional)
```

Open `http://localhost:6080/vnc.html` — full desktop with taskbar, file manager,
terminal emulator, and settings. Use 1920x1080 resolution for best experience.

#### Desktop without `desktop_env` (manual start)

If you prefer to start a WM manually (e.g., installed via `setup:` commands):

```bash
sudo envpod run my-desktop -b -- openbox-session   # start desktop in background
sudo envpod run my-desktop -- bash                  # shell in same pod
```

#### Tips

- **Auto-start**: The configured desktop starts automatically as part of the
  display services daemon — just `envpod run <pod> -- bash` and connect via noVNC
- **Multiple sessions**: All sessions share the same virtual display — commands
  run from bash appear on the noVNC desktop
- **Browser inside desktop**: Launch Chrome from the openbox right-click menu
  or from bash: `google-chrome --no-sandbox --start-maximized &`
- **Screen resolution**: Match `web_display.resolution` to your monitor for best
  results. Change at runtime: `xrandr --output SCREEN --mode 1920x1080`

### Quick Visual Test

Just verify the display pipeline works:

```yaml
setup:
  - "DEBIAN_FRONTEND=noninteractive apt-get install -y x11-apps"
```

```bash
sudo envpod run my-pod -- xeyes
```

### Headless Agent with Visual Debugging

Run an agent normally, peek at the display when needed:

```bash
sudo envpod run my-agent -- python3 agent.py &
open http://localhost:6080/vnc.html   # watch what the agent sees
```

---

## Requirements

| Requirement | Why |
|-------------|-----|
| `system_access: advanced` | noVNC packages install to `/usr/bin`, `/usr/lib` — needs writable COW overlays |
| `seccomp_profile: browser` | Required for Chrome (7 extra syscalls for Chromium zygote) |
| `shm_size: "256MB"` | Chrome uses `/dev/shm` for renderer IPC |
| Google Chrome on host | If bind-mounting `/opt/google` (alternative: install inside pod) |

---

## Web Display vs Display Passthrough

| | Web Display (noVNC) | Display Passthrough |
|---|---|---|
| **Host display needed?** | No | Yes (Wayland or X11) |
| **Works over SSH?** | Yes | No (unless X forwarding) |
| **Works on headless servers?** | Yes | No |
| **Latency** | Medium (~100ms) | Native |
| **Audio** | No (CE) | Yes (PipeWire/PulseAudio) |
| **Input** | VNC (keyboard + mouse) | Native |
| **Security** | Localhost-only by default | I-04 finding (X11 keylogging risk) |
| **Config** | `web_display.type: novnc` | `devices.display: true` + `-d` flag |

**Use web display** when you don't have a host display, are on a remote server,
or want browser-based access. **Use passthrough** for native performance on a
local machine with a running desktop.

---

## Security

### Audit Findings

| ID | Severity | Condition | Description |
|----|----------|-----------|-------------|
| W-01 | MEDIUM | `type: novnc` | VNC traffic unencrypted. Mitigated by localhost-only port forwarding. |
| W-02 | HIGH | Display port in `public_ports` | Display accessible from other machines. |

### Best Practices

- Keep the display port on **localhost only** (default behavior)
- Do not add the display port to `public_ports` unless you understand the risk
- The VNC stream is unencrypted — only safe on localhost or a trusted tunnel

---

## Troubleshooting

### Black screen

If `desktop_env` is set, the desktop auto-starts — you should see it immediately.
If using `envpod start`, services launch automatically.

If not using `desktop_env`, run a GUI application or start a desktop manually:
```bash
sudo envpod run my-pod -- xeyes                      # quick test
sudo envpod run my-pod -- google-chrome --no-sandbox  # browser
sudo envpod run my-pod -b -- openbox-session          # openbox desktop (manual)
sudo envpod run my-pod -b -- startxfce4               # XFCE desktop (manual)
```

### apt-get fails during setup

Host apt sources (CUDA, Chrome, VirtualBox repos) leak into the pod overlay
and cause GPG errors. envpod auto-removes third-party sources and clears
`/var/lib/apt/lists/*` before `apt-get update` to prevent stale package
lists from causing GPG signature failures. If your custom `setup:` commands
add repos, ensure they have valid GPG keys.

### Xvfb crashes on NVIDIA hosts

The display services daemon prevents this by setting `__EGL_VENDOR_LIBRARY_FILENAMES=""`
to block NVIDIA EGL library loading. Xvfb runs with mesa software rendering.

### Terminal says "Could not connect to bus"

D-Bus session bus is auto-started by the display services daemon. If you
see this error when running commands manually (outside `envpod run`), set
the D-Bus address explicitly:

```bash
export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/envpod-dbus
```

### x11vnc dies immediately

Usually `shmget: Operation not permitted` — SysV shared memory is blocked by
seccomp. The supervisor uses `-noshm` automatically. If running x11vnc
manually, add the `-noshm` flag.

### Port 6080 not accessible

Check iptables rules: `sudo iptables -t nat -L OUTPUT -n | grep 6080`

If connecting from another machine, use `public_ports` instead of `ports`
(note: W-02 security finding).

---

## Multiple Sessions & Resumable Terminals

Display services (Xvfb, x11vnc, websockify, audio, upload) run as a
background daemon inside the pod. Use `envpod start` to launch the pod
with all services, then run multiple commands simultaneously:

```bash
sudo envpod start my-pod                          # start pod, services auto-launch
sudo envpod run my-pod -- bash                    # get a shell
sudo envpod run my-pod -- python3 agent.py        # run an agent (another session)
```

Or use the traditional approach with `envpod run -b`:

```bash
sudo envpod run my-pod -b -- startxfce4          # start desktop in background
sudo envpod run my-pod -- bash                    # get a shell (separate session)
```

### Resumable sessions with screen

`screen` is auto-installed in all web display and desktop pods. Use it
for sessions that survive SSH disconnections:

```bash
# Start a named screen session inside the pod
sudo envpod run my-pod -- screen -S work

# Detach from screen: Ctrl-A then D
# Reconnect later:
sudo envpod run my-pod -- screen -r work

# List active screen sessions
sudo envpod run my-pod -- screen -ls
```

### Tips

- Use `screen -S <name>` to name sessions for easy identification
- `Ctrl-A d` detaches without killing the session
- `screen -r <name>` reattaches to a named session
- `screen -ls` lists all active sessions in the pod
- Display services are shared — all sessions see the same desktop

---

*Copyright 2026 Xtellix Inc. Licensed under the Business Source License 1.1.*
