<!-- type-delay 0.03 -->
# Installing envpod

envpod is a single static binary with no runtime dependencies. It runs on any Linux distribution with kernel 5.11+ and cgroup v2.

## One-Line Install (any distro)

<!-- no-exec -->
```bash
curl -fsSL https://envpod.dev/install.sh | sudo bash
```

Auto-detects your distro, installs prerequisites, downloads the correct binary (x86_64 or ARM64), and sets everything up. Works on Ubuntu, Debian, Fedora, Arch, Rocky, Alma, openSUSE, and Amazon Linux.

Add `--auto-deps` to skip the interactive prompt:

<!-- no-exec -->
```bash
curl -fsSL https://envpod.dev/install.sh | sudo bash -s -- --auto-deps
```

## Install from Tarball

If you already downloaded the release:

<!-- no-exec -->
```bash
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

The same `install.sh` detects whether the binary is present locally or needs to be downloaded.

## Portable (no install)

Download, extract, run. No install step, no PATH modification, no state directories. Just the binary and examples in one folder.

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo ./envpod init my-agent -c examples/basic-internet.yaml
sudo ./envpod run my-agent -- bash
```

The portable binary requires `iptables` and `iproute2` at runtime.

## Docker (testing/evaluation)

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
docker run -it --privileged --cgroupns=host \
  -v /tmp/envpod-test:/var/lib/envpod \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  ubuntu:24.04

# Inside the container — install.sh detects Docker automatically:
curl -fsSL https://envpod.dev/install.sh | bash
```

See `docs/DOCKER-TESTING.md` for details.

## Per-Distro Notes

The one-line installer handles prerequisites automatically. These notes are for manual installation or troubleshooting.

### Ubuntu 24.04 / 22.04

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites
sudo apt-get update
sudo apt-get install -y curl ca-certificates tar gzip iptables iproute2

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

### Debian 12 (Bookworm)

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites
sudo apt-get update
sudo apt-get install -y curl ca-certificates tar gzip iptables iproute2

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

### Fedora 41+

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites
sudo dnf install -y curl tar gzip iptables iproute

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

Note: Fedora runs SELinux in enforcing mode by default. If you encounter permission errors during `envpod run`, check `ausearch -m AVC -ts recent` for denials. Workaround: `sudo setenforce 0` (temporary, reverts on reboot). SELinux policy module support is planned.

### Arch Linux

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites
sudo pacman -S curl tar gzip iptables iproute2

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

### Rocky Linux 9 / AlmaLinux 9

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites (--allowerasing replaces curl-minimal with curl)
sudo dnf install -y --allowerasing curl tar gzip iptables iproute

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

Note: Rocky and AlmaLinux minimal images ship `curl-minimal` which conflicts with `curl`. The `--allowerasing` flag replaces it. On a full server install this is not needed.

### openSUSE Leap 15.6

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites
sudo zypper install -y curl tar gzip iptables iproute2

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

### Amazon Linux 2023

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Prerequisites (--allowerasing replaces curl-minimal with curl)
sudo dnf install -y --allowerasing curl tar gzip iptables iproute

# Install envpod
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh
```

### Raspberry Pi OS (64-bit) / ARM64

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Ensure cgroup v2 is enabled
# Add to /boot/firmware/cmdline.txt:
#   systemd.unified_cgroup_hierarchy=1
# Then reboot

# Prerequisites
sudo apt-get update
sudo apt-get install -y curl ca-certificates tar gzip iptables iproute2

# Install envpod (ARM64 binary)
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-arm64.tar.gz | tar xz
cd envpod-*-linux-arm64
sudo bash install.sh
```

## What install.sh does

The install script performs these steps:

1. **Checks prerequisites** — kernel ≥ 5.11, cgroup v2 active, overlayfs available, iptables and iproute2 present
2. **Copies binary** to `/usr/local/bin/envpod`
3. **Creates state directories** at `/var/lib/envpod/{state,pods}`
4. **Installs shell completions** — bash, zsh, or fish (auto-detected)
5. **Enables IP forwarding** — runtime and persisted to `/etc/sysctl.d/99-envpod.conf`
6. **Installs example configs** to `/usr/local/share/envpod/examples/`
7. **Installs uninstall script** to `/usr/local/share/envpod/uninstall.sh`

## Verify installation

<!-- no-exec -->
```bash
envpod --version
sudo envpod ls
```

## Uninstall

<!-- no-exec -->
```bash
sudo bash /usr/local/share/envpod/uninstall.sh
```

## System requirements

<!-- output -->
| Requirement | Minimum | Notes |
|---|---|---|
| Kernel | 5.11+ | `uname -r` to check |
| cgroup | v2 (unified hierarchy) | Check: `ls /sys/fs/cgroup/cgroup.controllers` |
| Architecture | x86_64 or ARM64 (aarch64) | |
| Filesystem | overlayfs support | Mainline since 3.18 |
| Download tools | curl, tar, gzip, ca-certificates | Only needed to download envpod |
| Network tools | iptables, iproute2 | Required at runtime |
| Disk | ~12 MB (binary) + pod storage | |
| RAM | ~4 MB per idle pod | |

## Troubleshooting

**`curl: (77) error setting certificate file`**
Missing `ca-certificates`. Install it: `apt install ca-certificates` (Debian/Ubuntu) or `dnf install ca-certificates` (Fedora/RHEL).

**`tar: gzip: Cannot exec: No such file or directory`**
Missing `gzip`. Install it: `dnf install gzip` (RHEL-based) or `zypper install gzip` (openSUSE).

**`problem with installed package curl-minimal`** (Rocky/Alma/Amazon Linux)
These distros ship `curl-minimal` which conflicts with `curl`. Use `dnf install -y --allowerasing curl` to replace it.

**`cgroup write: Not supported (os error 95)`**
Running inside Docker without `--cgroupns=host`. See `docs/DOCKER-TESTING.md`.

**`mount overlayfs failed: EINVAL`**
Running inside Docker (nested overlayfs). Mount a volume for envpod's data. See `docs/DOCKER-TESTING.md`.

**`Kernel X.X is too old`**
envpod requires kernel 5.11+. Check with `uname -r`. Upgrade your kernel or use a newer distro release.

**`cgroups v2 not active`**
Boot with cgroup v2 enabled. Raspberry Pi: add `systemd.unified_cgroup_hierarchy=1` to `/boot/firmware/cmdline.txt`. Other: add `cgroup_no_v1=all systemd.unified_cgroup_hierarchy=1` to kernel boot parameters.

## Tested distributions

<!-- output -->
| Distro | Version | Portable | Install | Governance |
|--------|---------|----------|---------|------------|
| Ubuntu | 24.04 LTS | ✓ | ✓ | ✓ |
| Ubuntu | 22.04 LTS | ✓ | ✓ | ✓ |
| Debian | 12 (Bookworm) | ✓ | ✓ | ✓ |
| Fedora | 41 | ✓ | ✓ | ✓ |
| Arch Linux | rolling | ✓ | ✓ | ✓ |
| Rocky Linux | 9 | ✓ | ✓ | ✓ |
| AlmaLinux | 9 | ✓ | ✓ | ✓ |
| openSUSE Leap | 15.6 | ✓ | ✓ | ✓ |
| Amazon Linux | 2023 | ✓ | ✓ | ✓ |

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
