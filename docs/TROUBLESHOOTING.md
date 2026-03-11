# Troubleshooting Guide

Consolidated solutions for every known issue. Organized by category — jump to the relevant section.

**Contents:**
[Installation](#installation) | [APT & Packages](#apt--packages) | [Filesystem & Overlay](#filesystem--overlay) | [Network & DNS](#network--dns) | [cgroups & Resources](#cgroups--resources) | [Web Display & noVNC](#web-display--novnc) | [Devices & GPU](#devices--gpu) | [Pod Lifecycle](#pod-lifecycle) | [Port Forwarding](#port-forwarding) | [Base Pods & Cloning](#base-pods--cloning) | [Docker Testing](#docker-testing) | [Nested envpod](#nested-envpod-envpod-in-envpod)

---

## Installation

### `curl: (77) error setting certificate file`

Missing CA certificates.

```bash
# Debian/Ubuntu
sudo apt install ca-certificates

# Fedora/RHEL
sudo dnf install ca-certificates
```

### `tar: gzip: Cannot exec: No such file or directory`

Missing gzip.

```bash
# RHEL-based
sudo dnf install gzip

# openSUSE
sudo zypper install gzip
```

### `problem with installed package curl-minimal` (Rocky/Alma/Amazon Linux)

These distros ship `curl-minimal` which conflicts with full `curl`.

```bash
sudo dnf install -y --allowerasing curl
```

### `Kernel X.X is too old`

envpod requires Linux kernel 5.11+ with overlayfs and cgroups v2 support.

```bash
uname -r    # check current kernel
```

Upgrade your kernel or use a newer distro release. Supported distros: Ubuntu 22.04+, Debian 12+, Fedora 41+, Arch (rolling), Rocky/Alma 9+, openSUSE 15.6+.

### SELinux blocks operations (Fedora/RHEL)

Check for AVC denials:

```bash
sudo ausearch -m AVC -ts recent
```

Temporary workaround (reverts on reboot):

```bash
sudo setenforce 0
```

---

## APT & Packages

### `apt-get update` fails with GPG signature errors inside pod

**Cause:** Host apt sources (CUDA, Chrome, VirtualBox, etc.) leak into the pod via OverlayFS. Their GPG keys aren't installed in the overlay, causing signature verification failures.

**Automatic fix:** envpod already handles this for web display and desktop setup — it removes third-party sources from `/etc/apt/sources.list.d/`, clears `/var/lib/apt/lists/*`, and runs `dpkg --configure -a` before `apt-get update`.

**Manual fix** (if you encounter this in custom `setup:` commands):

```bash
# Add to the start of your setup: block
setup:
  - rm -f /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources
  - rm -rf /var/lib/apt/lists/*
  - dpkg --configure -a
  - apt-get update
  - apt-get install -y <your-packages>
```

### `dpkg: error: status database locked`

A previous apt operation was interrupted inside the pod.

```bash
# Inside pod (via envpod run):
sudo rm -f /var/lib/dpkg/lock-frontend /var/lib/dpkg/lock /var/cache/apt/archives/lock
sudo dpkg --configure -a
sudo apt-get update
```

---

## Filesystem & Overlay

### `mount overlayfs failed: EINVAL`

**Cause:** Nested overlayfs — you're running envpod inside Docker, which itself uses overlayfs as the storage driver.

**Fix:** Mount a real filesystem (ext4/xfs) for envpod's data:

```bash
docker run -v /tmp/envpod-data:/var/lib/envpod --privileged envpod-test
```

### Host file changes not visible in pod

**By design.** Bind mounts are read-only. Writes go to the COW overlay, never directly to the host. This is the foundation of the governance model.

If you need current host files, destroy and re-create the pod, or use `envpod mount` to add new bind mounts.

### `envpod diff` shows unexpected system file changes

When using `system_access: advanced` or `dangerous`, system directories get their own COW overlays. Agent writes to `/usr/`, `/bin/`, etc. go to `sys_upper/`, not the host.

```bash
# Review all changes including system files
envpod diff my-pod --include-system

# Commit only user changes (default, safe)
envpod commit my-pod

# Commit everything including system changes
envpod commit my-pod --include-system
```

### `envpod commit` doesn't include system directory changes

By design — system changes are filtered by default (protected paths). Use `--include-system` to explicitly include them:

```bash
envpod commit my-pod --include-system
```

Or export to a specific directory:

```bash
envpod commit my-pod --output /path/to/export
```

---

## Network & DNS

### DNS resolution fails inside pod (IP works fine)

**Cause:** DNS server hasn't started yet, or upstream DNS isn't detected.

```bash
# Check DNS server from inside pod
dig @10.200.1.1 google.com

# If no response, re-enter pod after 1-2 seconds
exit
sudo envpod run my-pod -- bash
```

### Domain hangs during setup (never resolves)

**Cause:** Domain not in the DNS whitelist.

```bash
# Check which domains were denied
sudo envpod audit my-pod | grep -i dns

# Add the domain to your pod.yaml
network:
  dns:
    mode: whitelist
    allow:
      - "missing-domain.com"
```

### systemd-resolved (127.0.0.53) breaks DNS

**Cause:** Ubuntu's stub resolver doesn't work from network namespaces.

**Automatic fix:** envpod detects `127.0.0.53` and reads the real upstream servers from `/run/systemd/resolve/resolv.conf` instead. Fallback chain: systemd non-stub → `/etc/resolv.conf` → Google DNS (8.8.8.8).

If DNS still fails, check the upstream:

```bash
cat /run/systemd/resolve/resolv.conf   # should show real nameservers
```

### Pod can't reach `*.pods.local` (pod discovery)

Requirements for pod discovery:
1. Central daemon must be running: `sudo envpod dns-daemon`
2. Target pod: `network.allow_discovery: true`
3. Source pod: `network.allow_pods: ["target-name"]` (or `["*"]` for all)

Verify daemon is running:

```bash
ls /var/lib/envpod/dns.sock   # socket must exist
```

If daemon is not running, `*.pods.local` returns NXDOMAIN (fail-safe).

---

## cgroups & Resources

### `cgroups v2 not active`

envpod requires cgroups v2 (unified hierarchy).

```bash
# Check if cgroups v2 is active
grep -w cgroup2 /proc/mounts
# Should show: cgroup2 /sys/fs/cgroup cgroup2 ...

# Check available controllers
cat /sys/fs/cgroup/cgroup.controllers
# Should list: cpuset cpu memory io pids
```

**Fix for Raspberry Pi 4:**

Edit `/boot/firmware/cmdline.txt` (Bookworm) or `/boot/cmdline.txt` (Bullseye) and add to the end of the single line:

```
cgroup_memory=1 cgroup_enable=memory systemd.unified_cgroup_hierarchy=1
```

Then reboot.

**Fix for other systems:**

Add to kernel boot parameters (GRUB: edit `/etc/default/grub`, update `GRUB_CMDLINE_LINUX`):

```
cgroup_no_v1=all systemd.unified_cgroup_hierarchy=1
```

Then `sudo update-grub && sudo reboot`.

### `cgroup write: Not supported (os error 95)`

Running inside Docker without host cgroup namespace. Start Docker with:

```bash
docker run --privileged --cgroupns=host ...
```

### Resource limits (CPU/memory) don't enforce

Check that cgroup controllers are enabled:

```bash
cat /sys/fs/cgroup/cgroup.controllers
# Must list: cpu memory
```

If controllers are missing, you may need to enable them in the parent cgroup:

```bash
echo "+cpu +memory +io +pids" > /sys/fs/cgroup/cgroup.subtree_control
```

### Disk full during package install (e.g. PyTorch)

**Cause:** `/tmp` is a 100MB tmpfs by default. Large package downloads (pip, npm) use `/tmp` and run out of space.

**Fix:** Increase `tmp_size` in pod.yaml:

```yaml
processor:
  tmp_size: "4GB"
```

Then destroy and reinit the pod. For an existing pod without reinit, use a workaround:

```bash
TMPDIR=/opt/tmp pip install torch    # redirect pip's temp dir to overlay
```

### Pod fills host disk

**Cause:** Without `disk_size`, the overlay upper dir writes directly to the host filesystem with no limit.

**Fix:** Set `disk_size` to cap overlay storage:

```yaml
processor:
  disk_size: "20GB"
```

Takes effect on `envpod init` only (creates a loopback ext4 device). The pod gets a real `ENOSPC` error when full instead of filling the host disk.

---

## Web Display & noVNC

### Black screen in browser (port 6080)

If `desktop_env` is set in pod.yaml, the desktop auto-starts — you should see it immediately after `envpod start`. If not using `desktop_env`, launch a GUI application manually:

```bash
sudo envpod run my-pod -- xeyes                        # quick test
sudo envpod run my-pod -- google-chrome --no-sandbox    # browser
```

If `envpod start` shows a black screen despite `desktop_env` being set, you can manually start the desktop as a fallback:

```bash
sudo envpod run my-pod -b -- startxfce4                # XFCE (manual fallback)
sudo envpod run my-pod -b -- openbox-session           # Openbox (manual fallback)
```

### Display services crash with `--user agent`

**Cause:** Display services (Xvfb, x11vnc, websockify) need root for `/dev/shm` and `/proc` access. When `--user` is set, early versions ran everything as the non-root user.

**Fix (applied automatically):** Display services always run as root. User commands drop privileges via `runuser -u $ENVPOD_RUN_USER`. The `ENVPOD_RUN_USER` env var controls which user runs the agent command.

### Xvfb crashes on NVIDIA hosts

**Cause:** Host NVIDIA EGL libraries leak into pod, causing GPU driver conflicts with software rendering.

**Fix (applied automatically):** Display services set `__EGL_VENDOR_LIBRARY_FILENAMES=""` to block NVIDIA library loading. Xvfb uses mesa software rendering.

### `Could not connect to bus` error

D-Bus session bus not started.

**Fix (applied automatically):** The display services daemon starts `dbus-daemon --session` and exports `DBUS_SESSION_BUS_ADDRESS`.

Manual workaround if running outside `envpod run`:

```bash
export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/envpod-dbus
```

### x11vnc dies with `shmget: Operation not permitted`

SysV shared memory blocked by seccomp.

**Fix (applied automatically):** The supervisor starts x11vnc with `-noshm`.

Manual workaround:

```bash
x11vnc -noshm -display :99
```

### Port 6080 not accessible from another machine

By default, the display port is localhost-only (via `ports`). To access remotely:

```yaml
# pod.yaml — use public_ports instead
public_ports:
  - "6080:6080"
```

**Warning:** This triggers security finding W-02 (HIGH). The VNC stream is unencrypted — only use on trusted networks or through an SSH tunnel.

Verify iptables rules:

```bash
sudo iptables -t nat -L OUTPUT -n | grep 6080
sudo iptables -t nat -L PREROUTING -n | grep 6080   # only with public_ports
```

### Ctrl+V does not paste into the pod desktop

**Cause:** Browsers restrict direct clipboard access to canvas elements. Ctrl+V on the noVNC canvas is blocked by the browser.

**Fix:** Use the **sidebar clipboard panel** -- click the clipboard icon on the left side of the noVNC interface, paste your text into the panel with Ctrl+V, and it is sent to the VNC clipboard automatically.

For terminals inside the pod, use Ctrl+Shift+V (xfce4-terminal) or middle-click (xterm) after pasting via the sidebar panel.

### Display terminal blocks user terminal

**Cause:** Old versions ran display services in the foreground.

**Fix (applied automatically):** Display services now run as a background daemon (`envpod-display-services`) with auto-restart loops. Multiple simultaneous `envpod run` sessions work independently.

---

## Devices & GPU

### GPU not visible in pod

GPU passthrough is off by default. Enable it in pod.yaml:

```yaml
devices:
  gpu: true
```

### NVIDIA GPU: `nvidia-smi` returns error in pod

Check that the host has working NVIDIA drivers first:

```bash
nvidia-smi   # run on host, should work
```

If host works but pod doesn't, ensure `/dev/nvidia*` devices exist and `gpu: true` is set. envpod bind-mounts `/dev/nvidia*` and `/dev/dri/*` into the pod.

### Jetson Orin DLA cores not detected

When `devices.gpu: true`, envpod automatically passes through `/dev/nvhost-nvdla0` and `/dev/nvhost-nvdla1` on Jetson devices. No additional config needed.

### Hailo HAT+ not found on Raspberry Pi 5

When `devices.gpu: true`, envpod detects and passes through `/dev/hailo0`. Ensure the Hailo driver is loaded on the host first.

### Chrome/Firefox won't launch with GPU in pod

Use the `--no-sandbox` flag for Chrome inside a pod (the pod itself is the sandbox):

```bash
google-chrome --no-sandbox --ozone-platform=wayland   # Wayland
google-chrome --no-sandbox                              # X11
```

---

## Pod Lifecycle

### `envpod destroy` fails or requires multiple calls

**Cause:** Processes still holding file handles, or system COW overlays not unmounted in correct order.

**Fix (applied automatically):** Destroy now follows a robust sequence:
1. SIGKILL all processes in the pod's cgroup
2. Wait up to 5 seconds (50 x 100ms polls) for processes to exit
3. Unmount system overlays (sub-overlays) before main overlay
4. Remove pod directory (retries once after 500ms if needed)

If destroy still fails:

```bash
# Check what's still mounted
grep "my-pod" /proc/mounts

# Manual unmount (last resort)
sudo umount /var/lib/envpod/pods/my-pod/merged
sudo rm -rf /var/lib/envpod/pods/my-pod
```

### Stale iptables rules after destroy

`envpod destroy` cleans up iptables rules, but in rare cases rules may persist.

```bash
# Check for stale rules
sudo iptables -t nat -L -n | grep envpod

# Batch cleanup
sudo envpod gc
```

### Too many stopped pods accumulating

After batch operations or experiments, stopped pods can accumulate. Use `prune` to clean up:

```bash
# Remove all stopped/created pods (preserves running and frozen pods)
sudo envpod prune

# Also remove unreferenced base pods
sudo envpod prune --bases

# Or just prune orphaned bases
sudo envpod base prune
```

### `envpod run` shows "pod not found"

The pod wasn't initialized or was destroyed.

```bash
# List existing pods
sudo envpod ls

# Re-initialize if needed
sudo envpod init my-pod -c pod.yaml
```

### Pod won't start after `envpod stop`

If `envpod start` fails after a previous `envpod stop`, the pod may have stale mounts or processes:

```bash
# Check what's still mounted
grep "my-pod" /proc/mounts

# Try destroying and re-creating
sudo envpod destroy my-pod
sudo envpod init my-pod -c pod.yaml
sudo envpod start my-pod
```

### Queue socket permission denied

**Cause:** Agent user (uid 60000) can't write to the action queue socket.

**Fix (applied automatically):** Socket permissions set to 0o777.

---

## Port Forwarding

### Port forward not working

Check which scope you're using:

| Scope | YAML key | Accessible from |
|-------|----------|-----------------|
| Localhost | `ports` | Same machine only |
| Public | `public_ports` | All network interfaces |
| Pod-to-pod | `internal_ports` | Other pods only |

Verify iptables rules:

```bash
# Localhost ports
sudo iptables -t nat -L OUTPUT -n | grep <port>

# Public ports
sudo iptables -t nat -L PREROUTING -n | grep <port>
```

### `Address already in use`

Another process is using the host port.

```bash
sudo lsof -i :<port>    # find the process
```

Use a different host port in your mapping: `"9090:3000"` instead of `"3000:3000"`.

### Port accessible from outside (unintended)

You're using `public_ports` instead of `ports`. Security finding N-04 (LOW) fires for this.

```yaml
# Localhost only (safe default)
ports:
  - "8080:3000"

# All interfaces (intentional exposure)
public_ports:
  - "8080:3000"
```

---

## Base Pods & Cloning

### Base pod not created after init

As of v0.1, base pods are no longer auto-created. Use the `--create-base` flag:

```bash
sudo envpod init my-agent -c pod.yaml --create-base
sudo envpod init my-agent -c pod.yaml --create-base custom-name
```

### Base name collision

`--create-base` auto-increments on collision: if `my-agent` exists, creates `my-agent-2`, then `my-agent-3`, etc. The actual name is printed after creation.

### Clone is slow (expected ~130ms)

Cloning from a base should take ~130ms (10x faster than init). If slower:

- Ensure you're cloning from a base, not using `--current` (which copies the full current overlay)
- Check disk I/O: `iostat -x 1` for high utilization

```bash
# Clone from base (fast, uses symlinks)
sudo envpod clone my-agent new-pod

# Clone from current state (slower, copies upper/)
sudo envpod clone my-agent new-pod --current
```

---

## Docker Testing

Running envpod inside Docker (for testing) requires special flags:

```bash
docker run -it --privileged \
  --cgroupns=host \
  -v /tmp/envpod-data:/var/lib/envpod \
  ubuntu:24.04 bash
```

### IP forwarding disabled

Pod network requires IP forwarding:

```bash
echo 1 > /proc/sys/net/ipv4/ip_forward
```

### DNS/curl hangs inside pod (Docker)

1. Enable IP forwarding (above)
2. Wait 1-2 seconds after creating the pod before entering
3. Verify DNS: `dig @10.200.1.1 google.com`

### No curl/nslookup in Docker container

Base `ubuntu:24.04` images are minimal. Install tools first:

```bash
apt-get update && apt-get install -y curl dnsutils iproute2
```

---

## Nested envpod (envpod-in-envpod)

Running envpod inside a pod is **not supported**. Like Docker-in-Docker, nested
container runtimes require relaxing isolation to the point where the inner pod
has no meaningful governance:

- **seccomp** blocks `unshare`/`clone` (namespace creation)
- **Nested OverlayFS** is fragile (kernel 5.11+ only, known bugs)
- **cgroups** require delegation from the outer pod
- **Network namespaces** need `CAP_NET_ADMIN` to create veth pairs

**Recommended alternative:** Run multiple pods side-by-side on the host. Use
[pod discovery](USER-GUIDE.md) (`network.allow_pods`) so pods can communicate
without nesting. If your agent spawns sub-agents, give each sub-agent its own
first-class pod with full governance.

---

## Getting Help

```bash
# Security audit your pod configuration
sudo envpod audit my-pod --security

# View audit log for a running pod
sudo envpod audit my-pod

# Check pod status
sudo envpod ls

# Full CLI reference
envpod --help
envpod <subcommand> --help
```

If you encounter an issue not covered here, [open an issue](https://github.com/markamo/envpod-ce/issues).
