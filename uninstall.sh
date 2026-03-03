#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1-only

# uninstall.sh — Remove envpod CE from this system
#
# Removes:
#   /usr/local/bin/envpod          binary
#   /usr/local/share/envpod/       docs and examples
#   /etc/bash_completion.d/envpod  shell completions (if installed)
#   /usr/share/zsh/vendor-completions/_envpod
#
# Does NOT remove (your data):
#   /var/lib/envpod/               pod state, vaults, audit logs
#
# Use --purge to also remove all pod state and data.
#
# Usage:
#   sudo bash uninstall.sh           # remove binary + files, keep data
#   sudo bash uninstall.sh --purge   # remove everything including pod data

set -uo pipefail

INSTALL_DIR="/usr/local/bin"
SHARE_DIR="/usr/local/share/envpod"
STATE_DIR="/var/lib/envpod"
PURGE=false

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

for arg in "$@"; do
    case "$arg" in
        --purge) PURGE=true ;;
        --help|-h)
            echo "Usage: sudo bash uninstall.sh [--purge]"
            echo ""
            echo "  (no flags)  Remove binary and files. Keep pod data in $STATE_DIR"
            echo "  --purge     Remove everything including all pod state, vaults, and audit logs"
            exit 0
            ;;
    esac
done

if [[ $EUID -ne 0 ]]; then
    fail "Must be run as root: sudo bash uninstall.sh"
fi

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │       envpod CE uninstaller           │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# Verify this is the CE binary before removing
if [[ -f "$INSTALL_DIR/envpod" ]]; then
    installed_ver="$("$INSTALL_DIR/envpod" --version 2>/dev/null || echo "")"
    if echo "$installed_ver" | grep -qv "CE"; then
        warn "Installed binary does not appear to be envpod CE (got: $installed_ver)"
        warn "This may be the private/premium envpod binary."
        read -r -p "  Remove it anyway? [y/N] " confirm
        [[ "${confirm,,}" == "y" ]] || { echo "Aborted."; exit 0; }
    fi
fi

# ---------------------------------------------------------------------------
step "Removing binary"
# ---------------------------------------------------------------------------
if [[ -f "$INSTALL_DIR/envpod" ]]; then
    rm -f "$INSTALL_DIR/envpod"
    info "Removed $INSTALL_DIR/envpod"
else
    warn "$INSTALL_DIR/envpod not found — already removed?"
fi

# ---------------------------------------------------------------------------
step "Removing docs and examples"
# ---------------------------------------------------------------------------
if [[ -d "$SHARE_DIR" ]]; then
    rm -rf "$SHARE_DIR"
    info "Removed $SHARE_DIR"
else
    warn "$SHARE_DIR not found — already removed?"
fi

# ---------------------------------------------------------------------------
step "Removing shell completions"
# ---------------------------------------------------------------------------
removed_completions=false
for f in \
    /etc/bash_completion.d/envpod \
    /usr/share/bash-completion/completions/envpod \
    /usr/share/zsh/vendor-completions/_envpod \
    /usr/local/share/zsh/site-functions/_envpod \
    /usr/share/fish/completions/envpod.fish; do
    if [[ -f "$f" ]]; then
        rm -f "$f"
        info "Removed $f"
        removed_completions=true
    fi
done
[[ "$removed_completions" == false ]] && warn "No shell completions found — already removed?"

# ---------------------------------------------------------------------------
step "Pod data"
# ---------------------------------------------------------------------------
if [[ "$PURGE" == true ]]; then
    if [[ -d "$STATE_DIR" ]]; then
        warn "Removing all pod data: $STATE_DIR"
        warn "This deletes all pods, vaults, audit logs — this cannot be undone."
        read -r -p "  Type 'delete' to confirm: " confirm
        if [[ "$confirm" == "delete" ]]; then
            rm -rf "$STATE_DIR"
            info "Removed $STATE_DIR"
        else
            warn "Skipped — $STATE_DIR left intact"
        fi
    else
        info "$STATE_DIR not found — nothing to remove"
    fi
else
    if [[ -d "$STATE_DIR" ]]; then
        warn "Pod data kept at $STATE_DIR (use --purge to remove)"
    fi
fi

# ---------------------------------------------------------------------------
echo ""
echo -e "${BOLD}──────────────────────────────────────────────${NC}"
echo -e "${GREEN}  envpod CE uninstalled${NC}"
if [[ "$PURGE" == false && -d "$STATE_DIR" ]]; then
    echo ""
    echo "  Pod data retained at: $STATE_DIR"
    echo "  Remove manually with: sudo rm -rf $STATE_DIR"
fi
echo -e "${BOLD}──────────────────────────────────────────────${NC}"
echo ""