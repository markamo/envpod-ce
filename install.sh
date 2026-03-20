#!/usr/bin/env bash
#
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1
# envpod universal installer
#
# Works two ways:
#   1. Standalone (downloads everything):
#      curl -fsSL https://envpod.dev/install.sh | sudo bash
#
#   2. From release tarball (uses local binary):
#      cd envpod-0.1.1-linux-x86_64 && sudo bash install.sh
#
# Auto-detects: distro, package manager, architecture, container vs bare metal.
# Installs prerequisites, downloads binary if needed, sets up envpod.
#
# Options:
#   --auto-deps    Auto-install missing prerequisites (no prompt)
#   --no-deps      Skip prerequisite checks
#   --no-examples  Skip example configs
#   -h, --help     Show help
#
# Tested on: Ubuntu 24.04, Ubuntu 22.04, Debian 12, Fedora 41,
#            Arch Linux, Rocky Linux 9, AlmaLinux 9, openSUSE Leap 15.6,
#            Amazon Linux 2023 — bare metal and Docker containers.
#
set -euo pipefail

ENVPOD_CURRENT_VERSION="0.1.1"
ENVPOD_REPO="https://github.com/markamo/envpod-ce"

INSTALL_DIR="/usr/local/bin"
STATE_DIR="/var/lib/envpod"
EXAMPLES_DIR="${ENVPOD_EXAMPLES_DIR:-/usr/local/share/envpod/examples}"
SHARE_DIR="/usr/local/share/envpod"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}" 2>/dev/null)" && pwd 2>/dev/null || echo "/tmp")"

AUTO_DEPS=0
SKIP_DEPS=0

ENVPOD_VERSION="${ENVPOD_VERSION:-}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)        ENVPOD_VERSION="$2"; shift 2 ;;
        --version=*)      ENVPOD_VERSION="${1#*=}"; shift ;;
        --auto-deps)      AUTO_DEPS=1; shift ;;
        --no-deps)        SKIP_DEPS=1; shift ;;
        --examples-dir)   EXAMPLES_DIR="$2"; shift 2 ;;
        --examples-dir=*) EXAMPLES_DIR="${1#*=}"; shift ;;
        --no-examples)    EXAMPLES_DIR=""; shift ;;
        --help|-h)
            echo "envpod installer v${ENVPOD_CURRENT_VERSION}"
            echo ""
            echo "Usage:"
            echo "  curl -fsSL https://envpod.dev/install.sh | sudo bash                    # latest"
            echo "  curl -fsSL https://envpod.dev/install.sh | sudo bash -s -- --version 0.2.0  # specific"
            echo "  cd envpod-*-linux-x86_64 && sudo bash install.sh                        # from tarball"
            echo "  sudo bash install.sh --auto-deps                                         # no prompts"
            echo ""
            echo "Options:"
            echo "  --version <ver>        Download and install a specific version (e.g. 0.2.0)"
            echo "  --auto-deps            Auto-install missing prerequisites"
            echo "  --no-deps              Skip prerequisite checks"
            echo "  --examples-dir <path>  Install examples to custom path"
            echo "  --no-examples          Skip examples installation"
            echo "  -h, --help             Show this help"
            exit 0 ;;
        *) echo "Unknown argument: $1. Use --help for usage."; exit 1 ;;
    esac
done

# Set download URL based on version
if [[ -n "$ENVPOD_VERSION" ]]; then
    ENVPOD_RELEASES="${ENVPOD_REPO}/releases/download/v${ENVPOD_VERSION}"
else
    ENVPOD_VERSION="$ENVPOD_CURRENT_VERSION"
    ENVPOD_RELEASES="${ENVPOD_REPO}/releases/latest/download"
fi

# ═══════════════════════════════════════════════════════════════════════
# Output helpers
# ═══════════════════════════════════════════════════════════════════════

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

# ═══════════════════════════════════════════════════════════════════════
# macOS detection — envpod requires Linux
# ═══════════════════════════════════════════════════════════════════════

if [[ "$(uname -s)" == "Darwin" ]]; then
    echo ""
    echo "  envpod requires a Linux kernel (namespaces, cgroups, OverlayFS)."
    echo ""
    echo "  On macOS, use a lightweight Linux VM:"
    echo ""
    echo "    brew install orbstack"
    echo "    orb create ubuntu envpod-vm"
    echo "    orb shell envpod-vm"
    echo "    curl -fsSL https://envpod.dev/install.sh | sudo bash"
    echo ""
    echo "  Or use Lima, UTM, or any Linux VM with kernel 5.11+."
    echo ""
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════════
# Detect: architecture, distro, package manager, container
# ═══════════════════════════════════════════════════════════════════════

# Architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64)   ARCH="x86_64" ; TARBALL_ARCH="x86_64" ;;
    aarch64|arm64)   ARCH="aarch64"; TARBALL_ARCH="aarch64" ;;
    *)               fail "Unsupported architecture: $ARCH (envpod supports x86_64 and arm64)" ;;
esac

# Distro
DISTRO_ID="unknown"
DISTRO_NAME="Unknown Linux"
DISTRO_VERSION=""
if [[ -f /etc/os-release ]]; then
    . /etc/os-release
    DISTRO_ID="${ID:-unknown}"
    DISTRO_NAME="${NAME:-Unknown Linux}"
    DISTRO_VERSION="${VERSION_ID:-}"
fi

# Package manager
PKG_MGR="unknown"
case "$DISTRO_ID" in
    ubuntu|debian|pop|linuxmint|raspbian)
        PKG_MGR="apt" ;;
    fedora)
        PKG_MGR="dnf" ;;
    rocky|almalinux|rhel|centos|amzn)
        PKG_MGR="dnf-allowerasing" ;;
    arch|archlinux|endeavouros|manjaro|cachyos)
        PKG_MGR="pacman" ;;
    opensuse*|sles)
        PKG_MGR="zypper" ;;
esac

# Container detection
detect_container() {
    [[ -f /.dockerenv ]] && return 0
    [[ -f /run/.containerenv ]] && return 0
    grep -qE "docker|lxc|kubepods|containerd|libpod" /proc/1/cgroup 2>/dev/null && return 0
    if command -v systemd-detect-virt &>/dev/null; then
        local virt
        virt=$(systemd-detect-virt --container 2>/dev/null) || true
        [[ -n "$virt" && "$virt" != "none" ]] && return 0
    fi
    grep -q "container=" /proc/1/environ 2>/dev/null && return 0
    return 1
}

IN_CONTAINER=0
CONTAINER_TYPE="none"
if detect_container; then
    IN_CONTAINER=1
    if [[ -f /.dockerenv ]]; then
        CONTAINER_TYPE="docker"
    elif [[ -f /run/.containerenv ]]; then
        CONTAINER_TYPE="podman"
    else
        CONTAINER_TYPE="container"
    fi
fi

# Detect install mode: local tarball or online download
INSTALL_MODE="online"
if [[ -f "$SCRIPT_DIR/envpod" ]]; then
    INSTALL_MODE="local"

    # Verify tarball is complete
    TARBALL_MISSING=()
    [[ -f "$SCRIPT_DIR/envpod" ]]       || TARBALL_MISSING+=("envpod")
    [[ -f "$SCRIPT_DIR/LICENSE" ]]       || TARBALL_MISSING+=("LICENSE")
    [[ -f "$SCRIPT_DIR/README.md" ]]     || TARBALL_MISSING+=("README.md")
    [[ -f "$SCRIPT_DIR/uninstall.sh" ]]  || TARBALL_MISSING+=("uninstall.sh")
    [[ -d "$SCRIPT_DIR/docs" ]]          || TARBALL_MISSING+=("docs/")
    [[ -d "$SCRIPT_DIR/examples" ]]      || TARBALL_MISSING+=("examples/")

    if [[ ${#TARBALL_MISSING[@]} -gt 0 ]]; then
        warn "Release archive is incomplete. Missing: ${TARBALL_MISSING[*]}"
        echo ""
        if [[ "$AUTO_DEPS" -eq 1 ]]; then
            info "Re-downloading complete release..."
            INSTALL_MODE="online"
        else
            read -r -p "  Re-download the complete release? [Y/n] " response
            response=${response:-Y}
            case "$response" in
                [yY]|[yY][eE][sS])
                    INSTALL_MODE="online"
                    ;;
                *)
                    warn "Continuing with incomplete archive — some features may not install"
                    ;;
            esac
        fi
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
# Package helpers
# ═══════════════════════════════════════════════════════════════════════

install_download_prereqs() {
    step "Installing download prerequisites"
    case "$PKG_MGR" in
        apt)
            apt-get update -qq
            apt-get install -y --no-install-recommends curl ca-certificates tar gzip 2>&1
            ;;
        dnf)
            dnf install -y curl tar gzip 2>&1
            ;;
        dnf-allowerasing)
            dnf install -y --allowerasing curl tar gzip 2>&1
            ;;
        pacman)
            pacman -Sy --noconfirm curl tar gzip 2>&1
            ;;
        zypper)
            zypper install -y curl tar gzip 2>&1
            ;;
        *)
            fail "Cannot install download tools — unknown package manager. Manually install: curl tar gzip ca-certificates"
            ;;
    esac
    info "Download prerequisites installed"
}

install_runtime_prereqs() {
    step "Installing runtime prerequisites"
    case "$PKG_MGR" in
        apt)
            apt-get update -qq
            apt-get install -y --no-install-recommends iptables iproute2 procps kmod 2>&1
            ;;
        dnf)
            dnf install -y iptables iproute procps-ng kmod 2>&1
            ;;
        dnf-allowerasing)
            dnf install -y --allowerasing iptables iproute procps-ng kmod 2>&1
            ;;
        pacman)
            pacman -Sy --noconfirm iptables iproute2 procps-ng kmod 2>&1
            ;;
        zypper)
            zypper install -y iptables iproute2 procps kmod 2>&1
            ;;
        *)
            warn "Unknown package manager — manually install: iptables iproute2"
            return 1
            ;;
    esac
    # Ensure sysctl.d exists (missing on some distros in containers)
    mkdir -p /etc/sysctl.d
    info "Runtime prerequisites installed"
}

pkg_hint() {
    local pkg="$1"
    case "$PKG_MGR" in
        apt)              echo "sudo apt-get install -y ${pkg}" ;;
        dnf)              echo "sudo dnf install -y ${pkg}" ;;
        dnf-allowerasing) echo "sudo dnf install -y ${pkg}" ;;
        pacman)           echo "sudo pacman -S ${pkg}" ;;
        zypper)           echo "sudo zypper install -y ${pkg}" ;;
        *)                echo "install ${pkg} using your package manager" ;;
    esac
}

iproute_pkg() {
    case "$PKG_MGR" in
        dnf|dnf-allowerasing) echo "iproute" ;;
        *) echo "iproute2" ;;
    esac
}

# ═══════════════════════════════════════════════════════════════════════
# Banner
# ═══════════════════════════════════════════════════════════════════════

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │       envpod installer v${ENVPOD_VERSION}        │"
echo "  │    Zero-trust governance for AI      │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"
echo "  OS:          ${DISTRO_NAME} ${DISTRO_VERSION} (${ARCH})"
echo "  Package mgr: ${PKG_MGR}"
if [[ "$IN_CONTAINER" -eq 1 ]]; then
echo "  Environment: ${CONTAINER_TYPE} container"
else
echo "  Environment: bare metal"
fi
echo "  Install mode: ${INSTALL_MODE}"

if [[ $EUID -ne 0 ]]; then
    fail "Run as root: sudo bash install.sh (or pipe to sudo bash)"
fi

# ═══════════════════════════════════════════════════════════════════════
# Container-specific checks
# ═══════════════════════════════════════════════════════════════════════

if [[ "$IN_CONTAINER" -eq 1 ]]; then
    step "Container environment checks"

    if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
        fail "cgroups v2 not available. Start container with: --privileged --cgroupns=host"
    fi
    info "cgroups v2 available"

    if [[ "$INSTALL_MODE" == "local" ]]; then
        # Check overlayfs only if envpod data dir exists on overlay
        ENVPOD_FS=$(stat -f -c %T /var/lib/envpod 2>/dev/null || echo "unknown")
        if [[ "$ENVPOD_FS" == "overlayfs" || "$ENVPOD_FS" == "overlay" ]]; then
            warn "/var/lib/envpod is on overlayfs — nested overlays will fail"
            echo "  Start container with: -v /tmp/envpod-test:/var/lib/envpod"
            fail "Mount a volume at /var/lib/envpod"
        fi
        info "Filesystem OK"
    fi

    echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true
    info "IP forwarding enabled"
    mkdir -p /etc/sysctl.d

    # Auto-install deps in containers (no tty for prompts)
    if [[ "$SKIP_DEPS" -eq 0 && "$AUTO_DEPS" -eq 0 ]]; then
        AUTO_DEPS=1
        info "Container detected — will auto-install missing prerequisites"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
# Online mode: download binary
# ═══════════════════════════════════════════════════════════════════════

if [[ "$INSTALL_MODE" == "online" ]]; then
    step "Downloading envpod v${ENVPOD_VERSION} (${ARCH})"

    # Check for download tools
    NEED_DOWNLOAD_TOOLS=0
    if ! command -v curl &>/dev/null; then NEED_DOWNLOAD_TOOLS=1; fi
    if ! command -v tar &>/dev/null; then NEED_DOWNLOAD_TOOLS=1; fi
    if ! command -v gzip &>/dev/null; then NEED_DOWNLOAD_TOOLS=1; fi

    if [[ "$NEED_DOWNLOAD_TOOLS" -eq 1 ]]; then
        if [[ "$AUTO_DEPS" -eq 1 ]]; then
            install_download_prereqs
        elif [[ "$PKG_MGR" != "unknown" ]]; then
            echo ""
            warn "Missing download tools (curl, tar, or gzip)"
            echo ""
            read -r -p "  Install download prerequisites? [Y/n] " response
            response=${response:-Y}
            case "$response" in
                [yY]|[yY][eE][sS]) install_download_prereqs ;;
                *) fail "Cannot download without curl, tar, gzip. Install them manually." ;;
            esac
        else
            fail "Missing curl/tar/gzip and cannot detect package manager. Install manually."
        fi
    fi

    # Download and extract
    TARBALL_NAME="envpod-linux-${TARBALL_ARCH}.tar.gz"
    DOWNLOAD_URL="${ENVPOD_RELEASES}/${TARBALL_NAME}"

    TMPDIR=$(mktemp -d)
    cd "$TMPDIR"

    info "Downloading from ${DOWNLOAD_URL}"
    if curl -fsSL "$DOWNLOAD_URL" | tar xz; then
        info "Downloaded and extracted"
    else
        rm -rf "$TMPDIR"
        fail "Download failed. Check your internet connection and try again."
    fi

    # Find extracted directory
    ENVPOD_DIR=$(ls -d envpod-*-linux-${TARBALL_ARCH} 2>/dev/null | head -1)
    if [[ -z "$ENVPOD_DIR" ]]; then
        rm -rf "$TMPDIR"
        fail "Could not find extracted envpod directory"
    fi

    # Update SCRIPT_DIR to point at downloaded tarball
    SCRIPT_DIR="${TMPDIR}/${ENVPOD_DIR}"
    cd "$SCRIPT_DIR"
    info "Found ${ENVPOD_DIR}"
fi

# ═══════════════════════════════════════════════════════════════════════
# 1. Prerequisites
# ═══════════════════════════════════════════════════════════════════════

step "Checking prerequisites"

# Kernel
KVER=$(uname -r | cut -d. -f1-2)
KMAJOR=$(echo "$KVER" | cut -d. -f1)
KMINOR=$(echo "$KVER" | cut -d. -f2)
if [[ "$KMAJOR" -lt 5 ]] || { [[ "$KMAJOR" -eq 5 ]] && [[ "$KMINOR" -lt 11 ]]; }; then
    fail "Kernel $KVER too old. envpod requires 5.11+ (found: $(uname -r))"
fi
info "Kernel $(uname -r) (>= 5.11)"

# cgroup v2
if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
    warn "cgroups v2 not active."
    echo "  Raspberry Pi: add 'systemd.unified_cgroup_hierarchy=1' to /boot/firmware/cmdline.txt"
    echo "  Other:        add 'systemd.unified_cgroup_hierarchy=1' to kernel cmdline, then reboot"
    fail "cgroups v2 required"
fi
info "cgroups v2 available"

# OverlayFS
if ! modprobe -n overlay 2>/dev/null && ! grep -q overlay /proc/filesystems 2>/dev/null; then
    warn "OverlayFS not loaded — trying modprobe..."
    modprobe overlay 2>/dev/null || fail "OverlayFS not available. Run: modprobe overlay"
fi
info "OverlayFS available"

# Runtime tools
if [[ "$SKIP_DEPS" -eq 1 ]]; then
    info "Prerequisite check skipped (--no-deps)"
else
    MISSING=()
    MISSING_HINTS=()

    if ! command -v iptables &>/dev/null; then
        MISSING+=("iptables")
        MISSING_HINTS+=("$(pkg_hint iptables)")
    else
        info "iptables found"
    fi

    if ! command -v ip &>/dev/null; then
        MISSING+=("iproute2")
        MISSING_HINTS+=("$(pkg_hint "$(iproute_pkg)")")
    else
        info "iproute2 found"
    fi

    if [[ ${#MISSING[@]} -gt 0 ]]; then
        warn "Missing: ${MISSING[*]}"

        if [[ "$AUTO_DEPS" -eq 1 ]]; then
            install_runtime_prereqs
        elif [[ "$PKG_MGR" != "unknown" ]]; then
            echo ""
            echo "  envpod can install these automatically."
            read -r -p "  Install missing prerequisites? [Y/n] " response
            response=${response:-Y}
            case "$response" in
                [yY]|[yY][eE][sS]) install_runtime_prereqs ;;
                *)
                    echo "  Install manually:"
                    for hint in "${MISSING_HINTS[@]}"; do echo "    $hint"; done
                    fail "Missing prerequisites: ${MISSING[*]}"
                    ;;
            esac
        else
            echo "  Install manually:"
            for hint in "${MISSING_HINTS[@]}"; do echo "    $hint"; done
            fail "Missing prerequisites: ${MISSING[*]}"
        fi

        # Re-verify
        command -v iptables &>/dev/null || fail "iptables still not found"
        command -v ip &>/dev/null || fail "iproute2 still not found"
        info "All prerequisites installed"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
# 2. Install binary
# ═══════════════════════════════════════════════════════════════════════

step "Installing binary"

if [[ ! -f "$SCRIPT_DIR/envpod" ]]; then
    fail "envpod binary not found in $SCRIPT_DIR"
fi

cp "$SCRIPT_DIR/envpod" "$INSTALL_DIR/envpod"
chmod 755 "$INSTALL_DIR/envpod"
info "Installed to $INSTALL_DIR/envpod"

# ═══════════════════════════════════════════════════════════════════════
# 3. Envpod group (run without sudo)
# ═══════════════════════════════════════════════════════════════════════

ENVPOD_GROUP_ADDED=0
REAL_USER="${SUDO_USER:-$(whoami)}"

if [[ "$REAL_USER" != "root" && "$IN_CONTAINER" -eq 0 ]]; then
    step "Envpod group setup"
    echo ""
    echo "  Run envpod without sudo?"
    echo "  This adds '$REAL_USER' to the 'envpod' group (like Docker)."
    echo ""
    read -p "  Add $REAL_USER to envpod group? [Y/n] " ENVPOD_GROUP_CHOICE </dev/tty 2>/dev/null || ENVPOD_GROUP_CHOICE="y"

    if [[ "$ENVPOD_GROUP_CHOICE" != "n" && "$ENVPOD_GROUP_CHOICE" != "N" ]]; then
        groupadd -f envpod
        usermod -aG envpod "$REAL_USER"
        chgrp envpod "$INSTALL_DIR/envpod"
        chmod g+s "$INSTALL_DIR/envpod"
        ENVPOD_GROUP_ADDED=1
        info "Added $REAL_USER to envpod group"
        info "Binary set to setgid envpod"
    else
        info "Skipped — use sudo to run envpod"
    fi
else
    if [[ "$IN_CONTAINER" -eq 1 ]]; then
        step "Envpod group"
        info "Container — skipped (run as root)"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
# 4. State directories
# ═══════════════════════════════════════════════════════════════════════

step "Creating state directories"
mkdir -p "$STATE_DIR/state" "$STATE_DIR/pods"
# Make state dir writable by envpod group
if [[ "$ENVPOD_GROUP_ADDED" -eq 1 ]]; then
    chgrp -R envpod "$STATE_DIR"
    chmod -R g+rwx "$STATE_DIR"
fi
info "$STATE_DIR/{state,pods}"

# ═══════════════════════════════════════════════════════════════════════
# 5. Shell completions
# ═══════════════════════════════════════════════════════════════════════

step "Installing shell completions"

REAL_USER="${SUDO_USER:-root}"
REAL_HOME=$(eval echo "~$REAL_USER")

install_bash_completions() {
    local d="/etc/bash_completion.d"
    mkdir -p "$d"
    "$INSTALL_DIR/envpod" completions bash > "$d/envpod"
    info "Bash → $d/envpod"
}

install_zsh_completions() {
    local d="$REAL_HOME/.zfunc"
    mkdir -p "$d"
    "$INSTALL_DIR/envpod" completions zsh > "$d/_envpod"
    local rc="$REAL_HOME/.zshrc"
    if [[ -f "$rc" ]] && ! grep -q '.zfunc' "$rc" 2>/dev/null; then
        echo 'fpath=(~/.zfunc $fpath)' >> "$rc"
    fi
    chown -R "$REAL_USER":"$REAL_USER" "$d" 2>/dev/null || true
    info "Zsh → $d/_envpod"
}

install_fish_completions() {
    local d="$REAL_HOME/.config/fish/completions"
    mkdir -p "$d"
    "$INSTALL_DIR/envpod" completions fish > "$d/envpod.fish"
    chown -R "$REAL_USER":"$REAL_USER" "$d" 2>/dev/null || true
    info "Fish → $d/envpod.fish"
}

REAL_SHELL=$(getent passwd "$REAL_USER" 2>/dev/null | cut -d: -f7 || echo "/bin/bash")
case "$REAL_SHELL" in
    */zsh)  install_zsh_completions; install_bash_completions ;;
    */fish) install_fish_completions ;;
    *)      install_bash_completions ;;
esac

# ═══════════════════════════════════════════════════════════════════════
# 6. IP forwarding
# ═══════════════════════════════════════════════════════════════════════

if [[ "$IN_CONTAINER" -eq 0 ]]; then
    step "Enabling IP forwarding"
    CURRENT_FWD=$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo "0")
    if [[ "$CURRENT_FWD" == "1" ]]; then
        info "Already enabled"
    else
        sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 || warn "Could not enable (may need reboot)"
        info "Enabled (runtime)"
    fi
    SYSCTL_CONF="/etc/sysctl.d/99-envpod.conf"
    if [[ ! -f "$SYSCTL_CONF" ]]; then
        mkdir -p "$(dirname "$SYSCTL_CONF")"
        echo "net.ipv4.ip_forward = 1" > "$SYSCTL_CONF"
        info "Persisted → $SYSCTL_CONF"
    fi
else
    step "IP forwarding"
    info "Container — already enabled"
    mkdir -p /etc/sysctl.d
    echo "net.ipv4.ip_forward = 1" > /etc/sysctl.d/99-envpod.conf 2>/dev/null || true
fi

# ═══════════════════════════════════════════════════════════════════════
# 7. Examples
# ═══════════════════════════════════════════════════════════════════════

step "Installing examples"

if [[ -z "$EXAMPLES_DIR" ]]; then
    info "Skipped (--no-examples)"
elif [[ -d "$SCRIPT_DIR/examples" ]]; then
    mkdir -p "$EXAMPLES_DIR"
    cp "$SCRIPT_DIR/examples/"*.yaml "$EXAMPLES_DIR/"
    cp "$SCRIPT_DIR/examples/"*.sh "$EXAMPLES_DIR/" 2>/dev/null || true
    EXAMPLE_COUNT=$(ls "$EXAMPLES_DIR/"*.yaml 2>/dev/null | wc -l)
    info "${EXAMPLE_COUNT} configs → $EXAMPLES_DIR/"
else
    warn "No examples directory found — skipping"
fi

# ═══════════════════════════════════════════════════════════════════════
# 8. Uninstall script
# ═══════════════════════════════════════════════════════════════════════

step "Installing uninstall script"

mkdir -p "$SHARE_DIR"
if [[ -f "$SCRIPT_DIR/uninstall.sh" ]]; then
    cp "$SCRIPT_DIR/uninstall.sh" "$SHARE_DIR/uninstall.sh"
    chmod 755 "$SHARE_DIR/uninstall.sh"
    info "Uninstall → $SHARE_DIR/uninstall.sh"
else
    warn "uninstall.sh not found in release archive"
fi

# ═══════════════════════════════════════════════════════════════════════
# 9. Verify
# ═══════════════════════════════════════════════════════════════════════

step "Verifying installation"

INSTALLED_VERSION=$("$INSTALL_DIR/envpod" --version 2>&1 || true)
if [[ -z "$INSTALLED_VERSION" ]]; then
    fail "envpod binary not working"
fi
info "$INSTALLED_VERSION"

"$INSTALL_DIR/envpod" ls >/dev/null 2>&1 && info "envpod ls — OK" || warn "envpod ls — needs sudo"

# Cleanup temp dir if we downloaded
if [[ "$INSTALL_MODE" == "online" && -n "${TMPDIR:-}" ]]; then
    rm -rf "$TMPDIR"
fi

# ═══════════════════════════════════════════════════════════════════════
# Done
# ═══════════════════════════════════════════════════════════════════════

echo ""
echo -e "${GREEN}${BOLD}Installation complete!${NC}"
echo ""

if [[ "$ENVPOD_GROUP_ADDED" -eq 1 ]]; then
    echo -e "  ${YELLOW}⚠ Log out and back in for the envpod group to take effect.${NC}"
    echo "  Then run envpod without sudo:"
    echo ""
    echo "  Quick start:"
    if [[ -n "$EXAMPLES_DIR" ]]; then
    echo "    envpod init my-agent -c ${EXAMPLES_DIR}/basic-internet.yaml"
    else
    echo "    envpod init my-agent -c pod.yaml"
    fi
    echo "    envpod run my-agent -- bash"
    echo "    envpod diff my-agent"
else
    echo "  Quick start:"
    if [[ -n "$EXAMPLES_DIR" ]]; then
    echo "    sudo envpod init my-agent -c ${EXAMPLES_DIR}/basic-internet.yaml"
    else
    echo "    sudo envpod init my-agent -c pod.yaml"
    fi
    echo "    sudo envpod run my-agent -- bash"
    echo "    sudo envpod diff my-agent"
fi
echo ""
if [[ -n "$EXAMPLES_DIR" ]]; then
echo "  Examples:     ls $EXAMPLES_DIR/"
fi
echo "  Docs:         ${ENVPOD_REPO}/tree/main/docs"
echo "  Uninstall:    sudo bash $SHARE_DIR/uninstall.sh"

if [[ "$IN_CONTAINER" -eq 1 ]]; then
echo ""
echo -e "  ${YELLOW}Running in ${CONTAINER_TYPE}. For production, install"
echo -e "  directly on Linux. See docs/INSTALL.md${NC}"
fi
echo ""
