#!/usr/bin/env bash
#
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1
# envpod container installer
# Sets up envpod inside a Docker container for testing/evaluation.
# Handles Docker-specific workarounds automatically.
#
# Usage (inside a container started with the right flags):
#   bash install-container.sh
#
# Container must be started with:
#   docker run -it --privileged --cgroupns=host \
#     -v /tmp/envpod-test:/var/lib/envpod \
#     -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
#     ubuntu:24.04
#
# This script will:
#   1. Detect distro and install prerequisites
#   2. Download envpod from GitHub releases
#   3. Run install.sh
#   4. Apply Docker-specific network fixes
#   5. Verify everything works
#
set -euo pipefail

ENVPOD_VERSION="0.1.1"
ENVPOD_URL="https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────────┐"
echo "  │    envpod container installer v${ENVPOD_VERSION}      │"
echo "  │    For Docker testing environments       │"
echo "  └──────────────────────────────────────────┘"
echo -e "${NC}"

# ---------------------------------------------------------------------------
# 0. Check we're in a suitable container
# ---------------------------------------------------------------------------

step "Checking container environment"

if [[ $EUID -ne 0 ]]; then
    fail "Must run as root inside the container"
fi

# Check for required Docker flags
if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
    echo ""
    fail "cgroups v2 not available. Start container with: --privileged --cgroupns=host"
fi
info "cgroups v2 available (--cgroupns=host working)"

if [[ ! -w /sys/fs/cgroup ]]; then
    warn "/sys/fs/cgroup not writable. Start container with: -v /sys/fs/cgroup:/sys/fs/cgroup:rw"
fi

# Check if /var/lib/envpod is a real filesystem (not nested overlayfs)
ENVPOD_FS=$(stat -f -c %T /var/lib/envpod 2>/dev/null || echo "unknown")
if [[ "$ENVPOD_FS" == "overlayfs" ]]; then
    echo ""
    warn "/var/lib/envpod is on overlayfs (Docker's filesystem)."
    echo "  envpod needs a real filesystem for its overlays."
    echo "  Start container with: -v /tmp/envpod-test:/var/lib/envpod"
    fail "Mount a volume at /var/lib/envpod to avoid nested overlayfs"
fi
info "/var/lib/envpod filesystem: ${ENVPOD_FS}"

# ---------------------------------------------------------------------------
# 1. Detect distro and install prerequisites
# ---------------------------------------------------------------------------

step "Installing prerequisites"

# Detect distro
DISTRO="unknown"
if [[ -f /etc/os-release ]]; then
    . /etc/os-release
    DISTRO_NAME="${NAME:-unknown}"
    DISTRO_ID="${ID:-unknown}"
    DISTRO_VERSION="${VERSION_ID:-unknown}"
    info "Detected: ${DISTRO_NAME} ${DISTRO_VERSION}"
else
    warn "Could not detect distro"
fi

case "$DISTRO_ID" in
    ubuntu|debian)
        apt-get update -qq
        apt-get install -y --no-install-recommends \
            curl ca-certificates tar gzip \
            iptables iproute2 procps kmod >/dev/null 2>&1
        info "Prerequisites installed (apt)"
        ;;
    fedora)
        dnf install -y -q \
            curl tar gzip \
            iptables iproute procps-ng kmod >/dev/null 2>&1
        mkdir -p /etc/sysctl.d
        info "Prerequisites installed (dnf)"
        ;;
    rocky|almalinux|amzn)
        dnf install -y -q --allowerasing \
            curl tar gzip \
            iptables iproute procps-ng kmod >/dev/null 2>&1
        mkdir -p /etc/sysctl.d
        info "Prerequisites installed (dnf --allowerasing)"
        ;;
    arch|archlinux)
        pacman -Sy --noconfirm --quiet \
            curl tar gzip \
            iptables iproute2 procps-ng kmod >/dev/null 2>&1
        info "Prerequisites installed (pacman)"
        ;;
    opensuse*)
        zypper install -y --quiet \
            curl tar gzip \
            iptables iproute2 procps kmod >/dev/null 2>&1
        mkdir -p /etc/sysctl.d
        info "Prerequisites installed (zypper)"
        ;;
    *)
        warn "Unknown distro '${DISTRO_ID}' — attempting to continue"
        warn "If install fails, manually install: curl tar gzip iptables iproute2"
        ;;
esac

# ---------------------------------------------------------------------------
# 2. Docker-specific network setup
# ---------------------------------------------------------------------------

step "Applying Docker network fixes"

# Enable IP forwarding (Docker disables inside containers)
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true
info "IP forwarding enabled"

# Pre-create sysctl.d for install.sh (missing in some container images)
mkdir -p /etc/sysctl.d
info "/etc/sysctl.d ready"

# ---------------------------------------------------------------------------
# 3. Download envpod
# ---------------------------------------------------------------------------

step "Downloading envpod v${ENVPOD_VERSION}"

cd /tmp
if curl -fsSL "${ENVPOD_URL}" | tar xz; then
    info "Downloaded and extracted"
else
    fail "Download failed. Check internet access and try: curl -fsSL ${ENVPOD_URL} | tar xz"
fi

ENVPOD_DIR=$(ls -d envpod-*-linux-x86_64 2>/dev/null | head -1)
if [[ -z "$ENVPOD_DIR" ]]; then
    fail "Extracted directory not found"
fi
cd "$ENVPOD_DIR"
info "Found $ENVPOD_DIR"

# ---------------------------------------------------------------------------
# 4. Run install.sh
# ---------------------------------------------------------------------------

step "Running install.sh"

if bash install.sh; then
    info "install.sh completed"
else
    warn "install.sh had errors — attempting to continue"
fi

# ---------------------------------------------------------------------------
# 5. Verify
# ---------------------------------------------------------------------------

step "Verifying installation"

if command -v envpod &>/dev/null; then
    info "$(envpod --version 2>&1)"
else
    fail "envpod not found on PATH after install"
fi

# Quick smoke test
if envpod init __verify_test -c /usr/local/share/envpod/examples/basic-internet.yaml >/dev/null 2>&1; then
    info "init works"
    OUTPUT=$(envpod run __verify_test -- echo "container-verified" 2>&1) || true
    if echo "$OUTPUT" | grep -q "container-verified"; then
        info "run works"
    else
        warn "run failed: $OUTPUT"
    fi
    envpod destroy __verify_test >/dev/null 2>&1 || true
else
    warn "Smoke test init failed — envpod may need additional container flags"
fi

echo ""
echo -e "${GREEN}${BOLD}Container setup complete!${NC}"
echo ""
echo "  Quick start:"
echo "    envpod init my-agent -c /usr/local/share/envpod/examples/basic-internet.yaml"
echo "    envpod run my-agent -- bash"
echo "    envpod diff my-agent"
echo ""
echo "  Example configs:  ls /usr/local/share/envpod/examples/"
echo ""
echo -e "  ${YELLOW}Note: This is a testing environment. For production, run envpod"
echo -e "  directly on Linux without Docker. See docs/INSTALL.md${NC}"
echo ""
