#!/usr/bin/env bash
#
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1
# envpod installer — pre-built static binary.
# Auto-detects distro, installs prerequisites, sets up envpod.
#
# Usage:
#   sudo bash install.sh              # interactive — prompts if prereqs missing
#   sudo bash install.sh --auto-deps  # auto-install missing prerequisites
#   sudo bash install.sh --no-deps    # skip prereq check entirely
#
# Tested on: Ubuntu 24.04, Ubuntu 22.04, Debian 12, Fedora 41,
#            Arch Linux, Rocky Linux 9, AlmaLinux 9, openSUSE Leap 15.6,
#            Amazon Linux 2023
#
set -euo pipefail

ENVPOD_VERSION="0.1.1"
INSTALL_DIR="/usr/local/bin"
STATE_DIR="/var/lib/envpod"
EXAMPLES_DIR="${ENVPOD_EXAMPLES_DIR:-/usr/local/share/envpod/examples}"
SHARE_DIR="/usr/local/share/envpod"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

AUTO_DEPS=0
SKIP_DEPS=0

# Argument parsing
while [[ $# -gt 0 ]]; do
    case "$1" in
        --auto-deps)      AUTO_DEPS=1; shift ;;
        --no-deps)        SKIP_DEPS=1; shift ;;
        --examples-dir)   EXAMPLES_DIR="$2"; shift 2 ;;
        --examples-dir=*) EXAMPLES_DIR="${1#*=}"; shift ;;
        --no-examples)    EXAMPLES_DIR=""; shift ;;
        --help|-h)
            echo "Usage: sudo bash install.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --auto-deps            Automatically install missing prerequisites"
            echo "  --no-deps              Skip prerequisite checks entirely"
            echo "  --examples-dir <path>  Install examples to <path>"
            echo "                         (default: /usr/local/share/envpod/examples)"
            echo "  --no-examples          Skip examples installation"
            echo "  -h, --help             Show this help"
            echo ""
            echo "Environment variables:"
            echo "  ENVPOD_EXAMPLES_DIR    Override default examples directory"
            exit 0 ;;
        *) echo "Unknown argument: $1. Use --help for usage."; exit 1 ;;
    esac
done

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

# ---------------------------------------------------------------------------
# Detect distro
# ---------------------------------------------------------------------------
DISTRO_ID="unknown"
DISTRO_NAME="Unknown Linux"
DISTRO_VERSION=""

if [[ -f /etc/os-release ]]; then
    . /etc/os-release
    DISTRO_ID="${ID:-unknown}"
    DISTRO_NAME="${NAME:-Unknown Linux}"
    DISTRO_VERSION="${VERSION_ID:-}"
fi

# Map to package manager
PKG_MGR="unknown"
case "$DISTRO_ID" in
    ubuntu|debian|pop|linuxmint)
        PKG_MGR="apt" ;;
    fedora)
        PKG_MGR="dnf" ;;
    rocky|almalinux|rhel|centos|amzn)
        PKG_MGR="dnf-allowerasing" ;;
    arch|archlinux|endeavouros|manjaro)
        PKG_MGR="pacman" ;;
    opensuse*|sles)
        PKG_MGR="zypper" ;;
esac

# ---------------------------------------------------------------------------
# Install prerequisites for detected distro
# ---------------------------------------------------------------------------
install_prereqs() {
    step "Installing prerequisites for ${DISTRO_NAME}"

    case "$PKG_MGR" in
        apt)
            apt-get update -qq
            apt-get install -y --no-install-recommends \
                iptables iproute2 procps kmod 2>&1
            info "Installed: iptables iproute2 procps kmod"
            ;;
        dnf)
            dnf install -y \
                iptables iproute procps-ng kmod 2>&1
            mkdir -p /etc/sysctl.d
            info "Installed: iptables iproute procps-ng kmod"
            ;;
        dnf-allowerasing)
            dnf install -y --allowerasing \
                iptables iproute procps-ng kmod 2>&1
            mkdir -p /etc/sysctl.d
            info "Installed: iptables iproute procps-ng kmod"
            ;;
        pacman)
            pacman -Sy --noconfirm \
                iptables iproute2 procps-ng kmod 2>&1
            info "Installed: iptables iproute2 procps-ng kmod"
            ;;
        zypper)
            zypper install -y \
                iptables iproute2 procps kmod 2>&1
            mkdir -p /etc/sysctl.d
            info "Installed: iptables iproute2 procps kmod"
            ;;
        *)
            warn "Unknown package manager for '${DISTRO_ID}'"
            warn "Please manually install: iptables iproute2"
            return 1
            ;;
    esac
}

# Human-readable install hint for error messages
pkg_hint() {
    local pkg="$1"
    case "$PKG_MGR" in
        apt)              echo "apt-get install -y ${pkg}" ;;
        dnf)              echo "dnf install -y ${pkg}" ;;
        dnf-allowerasing) echo "dnf install -y --allowerasing ${pkg}" ;;
        pacman)           echo "pacman -S ${pkg}" ;;
        zypper)           echo "zypper install -y ${pkg}" ;;
        *)                echo "your package manager to install ${pkg}" ;;
    esac
}

# Map generic package names to distro-specific names
pkg_name() {
    local generic="$1"
    case "$PKG_MGR" in
        apt)
            case "$generic" in
                iproute2) echo "iproute2" ;;
                procps)   echo "procps" ;;
                *)        echo "$generic" ;;
            esac ;;
        dnf|dnf-allowerasing)
            case "$generic" in
                iproute2) echo "iproute" ;;
                procps)   echo "procps-ng" ;;
                *)        echo "$generic" ;;
            esac ;;
        pacman)
            case "$generic" in
                procps)   echo "procps-ng" ;;
                *)        echo "$generic" ;;
            esac ;;
        *)
            echo "$generic" ;;
    esac
}

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │       envpod installer v${ENVPOD_VERSION}        │"
echo "  │    Zero-trust governance for AI      │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"
echo "  Detected: ${DISTRO_NAME} ${DISTRO_VERSION} (${PKG_MGR})"

if [[ $EUID -ne 0 ]]; then
    fail "This installer must be run as root (sudo bash install.sh)"
fi

# ---------------------------------------------------------------------------
# 1. Prerequisites
# ---------------------------------------------------------------------------

step "Checking prerequisites"

# Kernel version
KVER=$(uname -r | cut -d. -f1-2)
KMAJOR=$(echo "$KVER" | cut -d. -f1)
KMINOR=$(echo "$KVER" | cut -d. -f2)
if [[ "$KMAJOR" -lt 5 ]] || { [[ "$KMAJOR" -eq 5 ]] && [[ "$KMINOR" -lt 11 ]]; }; then
    fail "Kernel $KVER is too old. envpod requires Linux 5.11+ (found: $(uname -r))"
fi
info "Kernel $(uname -r) (>= 5.11)"

# cgroup v2
if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
    echo ""
    warn "cgroups v2 not active."
    echo "  Raspberry Pi OS: add 'systemd.unified_cgroup_hierarchy=1' to /boot/firmware/cmdline.txt"
    echo "  Other distros:   boot with 'systemd.unified_cgroup_hierarchy=1' in kernel cmdline"
    echo "  Then reboot."
    fail "cgroups v2 required"
fi
info "cgroups v2 available"

# OverlayFS
if ! modprobe -n overlay 2>/dev/null && ! grep -q overlay /proc/filesystems 2>/dev/null; then
    warn "OverlayFS not loaded — trying modprobe overlay..."
    modprobe overlay 2>/dev/null || fail "OverlayFS not available. Run: modprobe overlay"
fi
info "OverlayFS available"

# Check required runtime tools
if [[ "$SKIP_DEPS" -eq 1 ]]; then
    info "Prerequisite check skipped (--no-deps)"
else
    MISSING=()

    if ! command -v iptables &>/dev/null; then
        MISSING+=("iptables")
    else
        info "iptables found"
    fi

    if ! command -v ip &>/dev/null; then
        MISSING+=("iproute2")
    else
        info "iproute2 found"
    fi

    if [[ ${#MISSING[@]} -gt 0 ]]; then
        echo ""
        warn "Missing prerequisites: ${MISSING[*]}"

        if [[ "$AUTO_DEPS" -eq 1 ]]; then
            install_prereqs
        elif [[ "$PKG_MGR" != "unknown" ]]; then
            echo ""
            echo "  envpod can install these automatically."
            echo ""
            read -r -p "  Install missing prerequisites? [Y/n] " response
            response=${response:-Y}
            case "$response" in
                [yY]|[yY][eE][sS])
                    install_prereqs
                    ;;
                *)
                    echo ""
                    echo "  Install manually:"
                    for pkg in "${MISSING[@]}"; do
                        echo "    $(pkg_hint "$(pkg_name "$pkg")")"
                    done
                    echo ""
                    fail "Missing prerequisites: ${MISSING[*]}"
                    ;;
            esac
        else
            echo ""
            echo "  Install manually:"
            for pkg in "${MISSING[@]}"; do
                echo "    $(pkg_hint "$(pkg_name "$pkg")")"
            done
            echo ""
            fail "Missing prerequisites: ${MISSING[*]}"
        fi

        # Re-verify after install
        if ! command -v iptables &>/dev/null; then
            fail "iptables still not found after install attempt"
        fi
        if ! command -v ip &>/dev/null; then
            fail "iproute2 still not found after install attempt"
        fi
        info "All prerequisites installed"
    fi
fi

# ---------------------------------------------------------------------------
# 2. Install binary
# ---------------------------------------------------------------------------

step "Installing binary"

if [[ ! -f "$SCRIPT_DIR/envpod" ]]; then
    fail "envpod binary not found in $SCRIPT_DIR. Re-extract the release archive."
fi

cp "$SCRIPT_DIR/envpod" "$INSTALL_DIR/envpod"
chmod 755 "$INSTALL_DIR/envpod"
info "Installed to $INSTALL_DIR/envpod"

# ---------------------------------------------------------------------------
# 3. Create state directories
# ---------------------------------------------------------------------------

step "Creating state directories"
mkdir -p "$STATE_DIR/state" "$STATE_DIR/pods"
info "$STATE_DIR/{state,pods} created"

# ---------------------------------------------------------------------------
# 4. Shell completions
# ---------------------------------------------------------------------------

step "Installing shell completions"

REAL_USER="${SUDO_USER:-root}"
REAL_HOME=$(eval echo "~$REAL_USER")

install_bash_completions() {
    local comp_dir="/etc/bash_completion.d"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions bash > "$comp_dir/envpod"
    info "Bash completions → $comp_dir/envpod"
}

install_zsh_completions() {
    local comp_dir="$REAL_HOME/.zfunc"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions zsh > "$comp_dir/_envpod"
    local zshrc="$REAL_HOME/.zshrc"
    if [[ -f "$zshrc" ]] && ! grep -q '.zfunc' "$zshrc" 2>/dev/null; then
        echo 'fpath=(~/.zfunc $fpath)' >> "$zshrc"
    fi
    chown -R "$REAL_USER":"$REAL_USER" "$comp_dir" 2>/dev/null || true
    info "Zsh completions → $comp_dir/_envpod"
}

install_fish_completions() {
    local comp_dir="$REAL_HOME/.config/fish/completions"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions fish > "$comp_dir/envpod.fish"
    chown -R "$REAL_USER":"$REAL_USER" "$comp_dir" 2>/dev/null || true
    info "Fish completions → $comp_dir/envpod.fish"
}

REAL_SHELL=$(getent passwd "$REAL_USER" 2>/dev/null | cut -d: -f7 || echo "/bin/bash")
case "$REAL_SHELL" in
    */zsh)  install_zsh_completions; install_bash_completions ;;
    */fish) install_fish_completions ;;
    *)      install_bash_completions ;;
esac

# ---------------------------------------------------------------------------
# 5. Enable IP forwarding
# ---------------------------------------------------------------------------

step "Enabling IP forwarding"

CURRENT_FWD=$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo "0")
if [[ "$CURRENT_FWD" == "1" ]]; then
    info "IP forwarding already enabled"
else
    sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 || warn "Could not enable IP forwarding (may need reboot)"
    info "IP forwarding enabled (runtime)"
fi

# Persist across reboots
SYSCTL_CONF="/etc/sysctl.d/99-envpod.conf"
if [[ ! -f "$SYSCTL_CONF" ]]; then
    mkdir -p "$(dirname "$SYSCTL_CONF")"
    echo "net.ipv4.ip_forward = 1" > "$SYSCTL_CONF"
    info "Persisted to $SYSCTL_CONF"
else
    info "$SYSCTL_CONF already exists"
fi

# ---------------------------------------------------------------------------
# 6. Install examples
# ---------------------------------------------------------------------------

step "Installing examples"

if [[ -z "$EXAMPLES_DIR" ]]; then
    info "Examples skipped (--no-examples)"
elif [[ -d "$SCRIPT_DIR/examples" ]]; then
    mkdir -p "$EXAMPLES_DIR"
    cp "$SCRIPT_DIR/examples/"*.yaml "$EXAMPLES_DIR/"
    cp "$SCRIPT_DIR/examples/"*.sh "$EXAMPLES_DIR/" 2>/dev/null || true
    EXAMPLE_COUNT=$(ls "$EXAMPLES_DIR/"*.yaml 2>/dev/null | wc -l)
    info "${EXAMPLE_COUNT} example configs → $EXAMPLES_DIR/"
else
    warn "No examples directory found — skipping"
fi

# ---------------------------------------------------------------------------
# 7. Install uninstall script
# ---------------------------------------------------------------------------

step "Installing uninstall script"

mkdir -p "$SHARE_DIR"
if [[ -f "$SCRIPT_DIR/uninstall.sh" ]]; then
    cp "$SCRIPT_DIR/uninstall.sh" "$SHARE_DIR/uninstall.sh"
    chmod 755 "$SHARE_DIR/uninstall.sh"
    info "Uninstall script → $SHARE_DIR/uninstall.sh"
else
    warn "uninstall.sh not found in release archive"
fi

# ---------------------------------------------------------------------------
# 8. Verify
# ---------------------------------------------------------------------------

step "Verifying installation"

INSTALLED_VERSION=$("$INSTALL_DIR/envpod" --version 2>&1 || true)
if [[ -z "$INSTALLED_VERSION" ]]; then
    fail "envpod binary not working"
fi
info "$INSTALLED_VERSION"

"$INSTALL_DIR/envpod" ls >/dev/null 2>&1 && info "envpod ls — OK" || warn "envpod ls failed (state dir may need sudo)"

echo ""
echo -e "${GREEN}${BOLD}Installation complete!${NC}"
echo ""
echo "  Quick start:"
echo "    sudo envpod init my-agent -c ${EXAMPLES_DIR}/basic-internet.yaml"
echo "    sudo envpod run my-agent -- bash"
echo "    sudo envpod diff my-agent"
echo ""
if [[ -n "$EXAMPLES_DIR" ]]; then
echo "  Examples:     $EXAMPLES_DIR/"
fi
echo "  Docs:         https://github.com/markamo/envpod-ce/tree/main/docs"
echo "  Uninstall:    sudo bash $SHARE_DIR/uninstall.sh"
echo ""
