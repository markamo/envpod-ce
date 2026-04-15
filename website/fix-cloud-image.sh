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
# What it fixes (both idempotent — safe to re-run):
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
#      Fix: recreate the dirs, seed the xfonts exclusion file, then
#      reconfigure any packages that failed during the last apt run.
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

step "[2/2] recreating pruned /var/lib data dirs"

mkdir -p \
    /var/lib/xml-core \
    /var/lib/sgml-base \
    /var/lib/xfonts \
    /etc/xml \
    /etc/sgml \
    2>/dev/null

[ -e /var/lib/xfonts/excluded-aliases ] || \
    touch /var/lib/xfonts/excluded-aliases 2>/dev/null

step "  dirs ready: /var/lib/{xml-core,sgml-base,xfonts}, /etc/{xml,sgml}"

# If the last apt run left packages half-configured, recover them now.
# dpkg --configure -a picks up where the previous run failed and re-runs
# postinst scripts — now that the data dirs exist, they'll succeed.
if dpkg -l 2>/dev/null | awk '/^iU|^iF|^iW/ { found=1 } END { exit !found }'; then
    step "  reconfiguring half-configured packages..."
    DEBIAN_FRONTEND=noninteractive dpkg --configure -a 2>&1 | tail -20 || \
        step "  WARN: dpkg --configure -a reported errors; check output above"
fi

# Check whether the canonical packages (xml-core, sgml-base, xfonts-utils)
# are installed. If yes, reconfigure them to prime their data dirs.
for pkg in xml-core sgml-base xfonts-utils; do
    if dpkg -s "$pkg" >/dev/null 2>&1; then
        step "  reconfiguring $pkg to prime data dir..."
        DEBIAN_FRONTEND=noninteractive dpkg-reconfigure "$pkg" 2>/dev/null || true
    fi
done

step "done. If the original setup run failed part-way, re-run:"
step "  sudo envpod setup <pod-name>"
