#!/bin/bash
# fix-cloud-image.sh — recover envpod pods from cloud-image Ubuntu rootfs bugs.
#
# Cloud-image Ubuntu (AWS EC2, GCP, OpenStack/Nova/Shadeform, Azure) ships
# with a minimized rootfs that breaks envpod pod setup in two independent
# ways. This script fixes both.
#
# Usage (inside the pod, via `sudo envpod run <pod> -- bash`):
#
#   curl -fsSL https://envpod.dev/fix-cloud-image.sh | bash
#
# Or run from the host before `envpod setup`:
#
#   sudo envpod run <pod> -- bash -c "$(curl -fsSL https://envpod.dev/fix-cloud-image.sh)"
#
# What it fixes (all idempotent — safe to re-run):
#
#   1. Missing Ubuntu archive signing keys
#      Symptom: "InRelease is not signed" on apt-get update.
#      Fix: relax date window, re-fetch keys from keyserver.ubuntu.com.
#
#   2. Pruned /var/lib data directories
#      Symptom: dpkg postinst failures for docutils-common, sgml-base,
#      xfonts-utils, cascading to python3-docutils, python3-oslo.*, novnc.
#      Cause: cloud images ship /usr/sbin/update-xmlcatalog,
#      update-catalog, and xfonts-utils binaries but strip their
#      /var/lib/{xml-core,sgml-base,xfonts}/ data dirs to save space.
#      Fix: recreate the dirs, then apt-get -f install -y to finish.
#
#   3. Broken VS Code install (Microsoft CDN throttles cloud IPs)
#      Symptom: `gpg: no valid OpenPGP data found` + apt-get update
#      stalls on packages.microsoft.com for minutes, then
#      `E: Unable to locate package code`.
#      Cause: `wget -qO- | gpg --dearmor` silently produces empty
#      keyrings on truncated fetches. Only runs if a half-installed
#      VS Code repo is detected OR `code` is in the apt history.
#      Fix: re-fetch key with curl -fsSL, retry apt-get update, fall
#      back to the direct .deb from update.code.visualstudio.com.
#
# The same fixes ship automatically in envpod 0.1.13+ (CE) and 0.1.17+
# (Premium). Use this script on older releases or when you need to
# recover an already-broken pod without re-init.
#
# Copyright 2026 Xtellix Inc. — BSL 1.1 / Apache-2.0 (this script)

set -euo pipefail

step() { echo "[fix-cloud-image] $*"; }
die()  { echo "[fix-cloud-image] ERROR: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "must run as root (try: sudo envpod run <pod> -- bash)"

# ---------------------------------------------------------------------------
# Part 1: Keyring / apt signature recovery
# ---------------------------------------------------------------------------

step "[1/2] apt keyring + signature recovery"

missing=""
command -v gpg >/dev/null 2>&1 || missing="$missing gnupg"
command -v curl >/dev/null 2>&1 || missing="$missing curl"
if [ -n "$missing" ]; then
    step "  installing prerequisites:$missing"
    DEBIAN_FRONTEND=noninteractive apt-get \
        -o Acquire::AllowInsecureRepositories=true \
        -o Acquire::Check-Valid-Until=false \
        install -y --allow-unauthenticated $missing 2>/dev/null || \
        die "could not install $missing — check DNS / setup_allow for the mirror"
fi

if DEBIAN_FRONTEND=noninteractive \
       apt-get -o Acquire::Check-Valid-Until=false update -qq 2>/dev/null; then
    step "  apt-get update OK with relaxed date check — no keyring refresh needed."
else
    step "  apt-get update failed — refreshing Ubuntu archive keyring..."
    tmp="/tmp/ubuntu-archive-keyring.gpg.new"
    rm -f "$tmp"
    for KEY in 871920D1991BC93C 40976EAF437D05B5 3B4FE6ACC0B21F32; do
        step "    fetching key 0x${KEY}..."
        curl -sSLf "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x${KEY}" \
            | gpg --dearmor 2>/dev/null \
            >> "$tmp" 2>/dev/null || step "    WARN: 0x${KEY} fetch failed"
    done
    if [ ! -s "$tmp" ]; then
        rm -f "$tmp"
        die "keyring fetch failed — keyserver.ubuntu.com unreachable. \
If DNS is Allowlist mode, add 'keyserver.ubuntu.com' to network.dns.setup_allow."
    fi
    mv "$tmp" /usr/share/keyrings/ubuntu-archive-keyring.gpg
    DEBIAN_FRONTEND=noninteractive \
        apt-get -o Acquire::Check-Valid-Until=false update -qq || \
        die "apt-get update still failing after keyring refresh. See docs/TROUBLESHOOTING.md."
    step "  keyring refreshed, apt-get update succeeded."
fi

# ---------------------------------------------------------------------------
# Part 2: Pruned /var/lib data directory recovery
# ---------------------------------------------------------------------------
#
# Minimum viable recipe — confirmed by independent review against the
# Shadeform setup.log:
#
#   mkdir -p /var/lib/xml-core /var/lib/sgml-base /var/lib/xfonts
#   dpkg --configure -a
#   apt-get -f install -y
#
# That's the tight fix. The extras below are defensive (/etc/{xml,sgml},
# touching excluded-aliases, explicit dpkg-reconfigure) and quiet the
# noisier warnings that surface but don't actually block installs.
# Idempotent either way.

step "[2/3] recreating pruned /var/lib data dirs"

# Core: the three dirs cited in the reported dpkg failure.
mkdir -p /var/lib/xml-core /var/lib/sgml-base /var/lib/xfonts 2>/dev/null

# Defensive: /etc/{xml,sgml} are written by the same packages' postinst
# scripts. The excluded-aliases file silences a sed warning fonts
# postinsts emit; not strictly required for a clean install.
mkdir -p /etc/xml /etc/sgml 2>/dev/null
[ -e /var/lib/xfonts/excluded-aliases ] || \
    touch /var/lib/xfonts/excluded-aliases 2>/dev/null

step "  dirs ready: /var/lib/{xml-core,sgml-base,xfonts} + defensive /etc/{xml,sgml}"

# dpkg --configure -a: picks up any half-configured packages from the
# previous run. Now that the data dirs exist, their postinst scripts
# succeed on the retry.
step "  running dpkg --configure -a..."
DEBIAN_FRONTEND=noninteractive dpkg --configure -a 2>&1 | tail -10 || \
    step "  WARN: dpkg --configure -a reported errors; check output above"

# apt-get -f install: fixes any dependency relationships the failed
# postinst left dangling (e.g. python3-docutils marked installed but
# unconfigured pulls python3-oslo.config into limbo).
step "  running apt-get -f install -y..."
DEBIAN_FRONTEND=noninteractive apt-get -f install -y 2>&1 | tail -5 || \
    step "  WARN: apt-get -f install reported errors; check output above"

# ---------------------------------------------------------------------------
# Part 3: VS Code install recovery (only runs if the pod was trying to
# install VS Code). Microsoft CDN / Azure Front Door throttles some cloud
# IP ranges down to ~20 B/s, which makes `wget -qO-` truncate silently and
# apt-get update stall on packages.microsoft.com InRelease.
# ---------------------------------------------------------------------------

step "[3/3] VS Code install recovery (if applicable)"

vscode_wanted=0
# Trigger: existing vscode.list (previous attempt left it), OR the known
# keyring file (even if empty from a bad fetch), OR `code` half-installed.
[ -f /etc/apt/sources.list.d/vscode.list ] && vscode_wanted=1
[ -f /usr/share/keyrings/packages.microsoft.gpg ] && vscode_wanted=1
dpkg -l code 2>/dev/null | grep -q '^i' && vscode_wanted=1

if [ "$vscode_wanted" -eq 0 ]; then
    step "  skipped — pod doesn't appear to install VS Code"
else
    if command -v code >/dev/null 2>&1 && code --version >/dev/null 2>&1; then
        step "  skipped — VS Code already working"
    else
        step "  wiping stale vscode.list + keyring"
        rm -f /etc/apt/sources.list.d/vscode.list \
              /usr/share/keyrings/packages.microsoft.gpg

        step "  fetching Microsoft key via curl (no silent truncation)"
        apt-get install -y curl ca-certificates gpg apt-transport-https 2>/dev/null
        install -d -m 0755 /usr/share/keyrings
        if curl -fsSL --retry 3 --retry-delay 5 \
             https://packages.microsoft.com/keys/microsoft.asc \
           | gpg --dearmor --batch --yes \
                 -o /usr/share/keyrings/packages.microsoft.gpg; then
            echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/packages.microsoft.gpg] https://packages.microsoft.com/repos/code stable main" \
                > /etc/apt/sources.list.d/vscode.list
            step "  retrying apt-get update (up to 3x)..."
            rm -rf /var/lib/apt/lists/*
            for i in 1 2 3; do
                apt-get update && apt-cache show code >/dev/null 2>&1 && break
                sleep 5
            done
            apt-get install -y code 2>/dev/null || true
        else
            step "  WARN: Microsoft key fetch failed — will try direct .deb"
        fi

        # Fallback: direct .deb from update.code.visualstudio.com (different
        # CDN path; not throttled the same way as packages.microsoft.com).
        if ! command -v code >/dev/null 2>&1; then
            step "  apt repo unusable — falling back to direct .deb"
            if curl -fsSL --retry 3 --retry-delay 5 \
                 https://update.code.visualstudio.com/latest/linux-deb-x64/stable \
                 -o /tmp/code.deb; then
                apt-get install -y /tmp/code.deb 2>&1 | tail -5 || true
                rm -f /tmp/code.deb
            else
                step "  WARN: direct .deb fetch also failed; network is very flaky"
            fi
        fi

        if command -v code >/dev/null 2>&1; then
            step "  VS Code installed: $(code --version 2>/dev/null | head -1)"
        else
            step "  WARN: VS Code install still failing — see TROUBLESHOOTING.md"
        fi
    fi
fi

step "done. If the original setup run failed part-way, re-run:"
step "  sudo envpod setup <pod-name>"
