# Platform Support

> **EnvPod v0.1.1** — The zero-trust governance layer for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Where envpod runs, what it needs, and how to set it up on each platform.

## Quick Reference

| Platform | Works? | Privileged? | Notes |
|----------|--------|------------|-------|
| **Bare metal Linux** | Native | sudo | Primary target. Full performance. |
| **Cloud VM** (EC2, GCP, Azure, DigitalOcean) | Native | sudo | Full kernel — same as bare metal |
| **WSL2** (Windows) | Native | sudo | Real Linux kernel inside Windows |
| **OrbStack** (macOS) | Native | sudo | Lightweight Linux VM |
| **Lima / UTM** (macOS) | Native | sudo | Alternative macOS VMs |
| **Proxmox VM** | Native | sudo | KVM — full kernel |
| **Firecracker microVM** | Native | sudo | AWS Lambda infrastructure |
| **Docker container** | Yes | `--privileged` | Nested namespaces |
| **Podman container** | Yes | `--privileged` | Same as Docker |
| **LXC / LXD container** | Yes | Nesting + privileged | Community tested |
| **Incus container** | Yes | Nesting + privileged | LXD fork, same approach |
| **systemd-nspawn** | Yes | `--capability=all` | Niche |
| **Kubernetes pod** | Yes | Privileged + SYS_ADMIN | Enterprise sidecar |

**The rule:** If you have a real Linux kernel (bare metal, VM, WSL2), envpod runs natively with just sudo. Inside another container runtime, you need `--privileged` so envpod can create its own namespaces.

## Requirements

All platforms need:
- Linux kernel 5.11+
- cgroups v2
- OverlayFS (`modprobe overlay`)
- iptables + iproute2
- Root access (sudo)

## Native Platforms

### Bare Metal Linux

The primary target. Full features, full performance.

```bash
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

Tested on: Ubuntu 24.04, Debian 12, Fedora 40, Arch Linux.

### Cloud VMs (EC2, GCP, Azure)

Cloud VMs run full Linux kernels — envpod works identically to bare metal.

```bash
# On any cloud VM running Ubuntu 24.04:
curl -fsSL https://envpod.dev/install.sh | sudo bash
envpod init my-agent -c pod.yaml
```

Recommended instance types:
- **AWS:** `t3.medium` (general), `g5.xlarge` (GPU), `c6i.metal` (bare metal for cloud envpod)
- **GCP:** `e2-medium` (general), `a2-highgpu-1g` (GPU)
- **Azure:** `Standard_D2s_v5` (general), `Standard_NC6s_v3` (GPU)

### Windows (WSL2)

WSL2 runs a real Linux kernel. Full envpod features.

```powershell
# PowerShell (Admin):
wsl --install Ubuntu-24.04
```

```bash
# Inside Ubuntu terminal:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

**GPU note:** WSL2 supports NVIDIA GPU passthrough. Install the [Windows NVIDIA driver](https://developer.nvidia.com/cuda/wsl) — CUDA works inside envpod pods on WSL2.

### macOS (via OrbStack) — Beta

macOS can't run Linux namespaces natively. OrbStack provides a lightweight Linux VM.

```bash
# Install OrbStack:
brew install orbstack

# Create a Linux VM:
orb create ubuntu envpod-vm

# Inside the VM:
orb shell envpod-vm
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

Also works with [Lima](https://lima-vm.io/) and [UTM](https://mac.getutm.app/).

**Limitations:**
- `host_user.clone_host` clones the VM user, not the macOS user
- `mount_cwd` mounts the VM's filesystem, not macOS directories (OrbStack auto-mounts `/Users/` but paths differ)
- GPU passthrough not available (no NVIDIA on macOS)

### ARM64 (Raspberry Pi, Jetson)

Native ARM64 static binary. No emulation.

```bash
# Raspberry Pi 4/5 (64-bit OS) or Jetson Orin:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

See [EMBEDDED.md](EMBEDDED.md) for detailed setup, cgroups v2 enablement, and GPU configuration.

## Container Platforms

Running envpod inside another container requires `--privileged` because envpod creates its own Linux namespaces (PID, mount, network) which need `CAP_SYS_ADMIN`.

### Docker

```bash
docker run -it --privileged --cgroupns=host \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  ubuntu:24.04

# Inside the container:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

Or use the container-specific installer:

```bash
bash install-container.sh
```

See [DOCKER-TESTING.md](DOCKER-TESTING.md) for details.

### Podman

Same flags as Docker:

```bash
podman run -it --privileged --cgroupns=host \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  ubuntu:24.04

# Inside the container:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

**Note:** Rootless Podman may not work — nested user namespaces can conflict. Use `sudo podman` if rootless fails.

### LXC / LXD

Enable nesting and privileged mode:

```bash
# LXD:
lxc launch ubuntu:24.04 envpod-host -c security.nesting=true -c security.privileged=true
lxc exec envpod-host -- bash

# Inside the container:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

**Proxmox LXC:** In the container config (`/etc/pve/lxc/<id>.conf`):

```
unprivileged: 0
features: nesting=1
```

Community tested — OpenClaw deployed in under 3 minutes on constrained LXC hardware.

### Incus

LXD fork, same approach:

```bash
incus launch images:ubuntu/24.04 envpod-host -c security.nesting=true -c security.privileged=true
incus exec envpod-host -- bash
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

### systemd-nspawn

```bash
sudo systemd-nspawn --boot --capability=all \
  --bind=/sys/fs/cgroup \
  -D /var/lib/machines/envpod-host

# Inside:
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

### Kubernetes

Run envpod as a privileged pod or sidecar:

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: envpod-host
spec:
  containers:
  - name: envpod
    image: ubuntu:24.04
    securityContext:
      privileged: true
    command: ["sleep", "infinity"]
```

```bash
kubectl exec -it envpod-host -- bash
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

**Note:** This is for evaluation/testing. Production Kubernetes deployment of envpod is on the roadmap.

## Feature Availability by Platform

| Feature | Native | Docker/Podman | LXC | WSL2 | macOS (OrbStack) |
|---------|--------|--------------|-----|------|-------------------|
| COW filesystem | Full | Full | Full | Full | Full |
| Diff/commit/rollback | Full | Full | Full | Full | Full |
| DNS filtering | Full | Full | Full | Full | Full |
| Credential vault | Full | Full | Full | Full | Full |
| cgroups v2 limits | Full | Full | Full | Full | Full |
| Network namespace | Full | Full | Full | Full | Full |
| GPU passthrough | Full | Host GPU needed | Host GPU needed | NVIDIA WSL driver | Not available |
| noVNC desktop | Full | Full | Full | Full | Full |
| `mount_cwd` | Full | Mount from host | Mount from host | WSL filesystem | OrbStack filesystem |
| `clone_host` | Full | Container user | Container user | WSL user | VM user |
| Seccomp-BPF | Full | Full | Full | Full | Full |
| Live resize | Full | Full | Full | Full | Full |

## Troubleshooting

### "Operation not permitted" during init

The container runtime doesn't have enough privileges. Add `--privileged` (Docker/Podman) or `security.nesting=true` (LXC).

### "cgroups v2 not available"

The host or container isn't using cgroups v2. Check:

```bash
ls /sys/fs/cgroup/cgroup.controllers
```

If missing, the host kernel uses cgroups v1. For Docker, add `--cgroupns=host` and ensure the host has cgroups v2.

### "overlayfs: filesystem not found"

Load the overlay kernel module:

```bash
sudo modprobe overlay
```

In containers, the host must have the overlay module loaded.

### Pod networking fails inside a container

Ensure the container has `--cgroupns=host` and access to `/sys/fs/cgroup`. The container also needs `CAP_NET_ADMIN` for iptables rules (included in `--privileged`).

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
