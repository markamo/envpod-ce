#!/bin/bash
# fix-apt-keyrings.sh — recover from "InRelease is not signed" apt failures
# inside an envpod pod whose base rootfs (cloud-image Ubuntu) ships with
# incomplete /usr/share/keyrings/ coverage.
#
# Usage (inside the pod, via `sudo envpod run <pod> -- bash`):
#
#   curl -fsSL https://envpod.dev/fix-apt-keyrings.sh | bash
#
# Or run it on the host side before `envpod setup`:
#
#   sudo envpod run <pod> -- bash -c "$(curl -fsSL https://envpod.dev/fix-apt-keyrings.sh)"
#
# What it does (idempotent — safe to re-run):
#   1. Makes sure gnupg + curl are installed so the refresh tools exist.
#   2. Relaxes apt's date-window check (tolerates clock drift on fresh VPS).
#   3. Re-fetches the Ubuntu archive signing keys from keyserver.ubuntu.com.
#   4. Retries apt-get update.
#
# The same fix ships automatically in envpod 0.1.12+ (CE) and 0.1.17+
# (Premium). Use this script on older releases or when you need to recover
# an already-broken pod without re-init.
#
# Copyright 2026 Xtellix Inc. — BSL 1.1 / Apache-2.0 (this script)

set -euo pipefail

step() { echo "[fix-apt-keyrings] $*"; }
die()  { echo "[fix-apt-keyrings] ERROR: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "must run as root (try: sudo envpod run <pod> -- bash)"

# ---------------------------------------------------------------------------
# 1. Ensure gnupg + curl (required by every later step)
# ---------------------------------------------------------------------------
missing=""
command -v gpg >/dev/null 2>&1 || missing="$missing gnupg"
command -v curl >/dev/null 2>&1 || missing="$missing curl"
if [ -n "$missing" ]; then
    step "installing prerequisites:$missing"
    # Use --allow-unauthenticated because this is exactly the apt state
    # we're trying to repair.
    DEBIAN_FRONTEND=noninteractive apt-get \
        -o Acquire::AllowInsecureRepositories=true \
        -o Acquire::Check-Valid-Until=false \
        install -y --allow-unauthenticated $missing 2>/dev/null || \
        die "could not install $missing — check DNS / setup_allow for the mirror"
fi

# ---------------------------------------------------------------------------
# 2. Try a permissive apt-get update first. If it succeeds, we're done.
# ---------------------------------------------------------------------------
step "attempting apt-get update with relaxed date check..."
if DEBIAN_FRONTEND=noninteractive \
       apt-get -o Acquire::Check-Valid-Until=false update -qq 2>/dev/null; then
    step "success — apt-get update worked with relaxed date check only."
    step "no keyring refresh needed."
    exit 0
fi

# ---------------------------------------------------------------------------
# 3. Refresh the Ubuntu archive signing keys from keyserver.ubuntu.com.
#    Keys cover: Ubuntu Archive 2018 (871920D1991BC93C),
#                Ubuntu Archive Master 2012 (40976EAF437D05B5),
#                Ubuntu CD Image (3B4FE6ACC0B21F32).
# ---------------------------------------------------------------------------
step "apt-get update failed — refreshing Ubuntu archive keyring..."

tmp="/tmp/ubuntu-archive-keyring.gpg.new"
rm -f "$tmp"

for KEY in 871920D1991BC93C 40976EAF437D05B5 3B4FE6ACC0B21F32; do
    step "  fetching key 0x${KEY}..."
    curl -sSLf "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x${KEY}" \
        | gpg --dearmor 2>/dev/null \
        >> "$tmp" 2>/dev/null || {
            step "  WARN: could not fetch 0x${KEY} (keyserver unreachable?)"
        }
done

if [ ! -s "$tmp" ]; then
    rm -f "$tmp"
    die "keyring fetch failed — keyserver.ubuntu.com unreachable. \
If DNS is Allowlist mode, add 'keyserver.ubuntu.com' to network.dns.setup_allow."
fi

mv "$tmp" /usr/share/keyrings/ubuntu-archive-keyring.gpg
step "  keyring refreshed."

# ---------------------------------------------------------------------------
# 4. Retry apt-get update with the fresh keyring.
# ---------------------------------------------------------------------------
step "retrying apt-get update..."
if DEBIAN_FRONTEND=noninteractive \
       apt-get -o Acquire::Check-Valid-Until=false update -qq; then
    step "success — apt-get update works with refreshed keyring."
    exit 0
fi

die "apt-get update still failing after keyring refresh. \
See docs/TROUBLESHOOTING.md → 'InRelease is not signed' for further steps."
