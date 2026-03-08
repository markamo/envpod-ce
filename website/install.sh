#!/usr/bin/env sh
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

#
# envpod installer — https://envpod.dev/install.sh
#
# Downloads the right pre-built binary for your system, then runs the bundled
# installer with sudo. No Rust toolchain required.
#
# Usage:
#   curl -fsSL https://envpod.dev/install.sh | sh
#   curl -fsSL https://envpod.dev/install.sh | sh -s -- --examples-dir /opt/myproject/examples
#   curl -fsSL https://envpod.dev/install.sh | sh -s -- --no-examples
#   ENVPOD_EXAMPLES_DIR=/opt/myproject/examples curl -fsSL https://envpod.dev/install.sh | sh
#
# Supports: Linux x86_64 and ARM64 (Raspberry Pi, Jetson Orin, etc.)
#
set -e

REPO="markamo/envpod-ce"
BASE_URL="https://github.com/${REPO}/releases/latest/download"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { printf "${GREEN}[✓]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
fail()  { printf "${RED}[✗]${NC} %s\n" "$*"; exit 1; }

printf "${BOLD}"
printf "  ┌──────────────────────────────────────┐\n"
printf "  │      envpod — downloading installer  │\n"
printf "  │    https://envpod.dev                │\n"
printf "  └──────────────────────────────────────┘\n"
printf "${NC}\n"

# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------

if [ "$(uname -s)" != "Linux" ]; then
    fail "envpod requires Linux. Detected: $(uname -s)"
fi

MACHINE=$(uname -m)
case "$MACHINE" in
    x86_64|amd64)   ARCH="x86_64" ;;
    aarch64|arm64)  ARCH="arm64"  ;;
    *)
        fail "Unsupported architecture: $MACHINE (envpod supports x86_64 and arm64)"
        ;;
esac
info "Architecture: ${ARCH}"

if ! command -v sudo >/dev/null 2>&1; then
    fail "sudo is required. Install sudo or run as root."
fi

if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
    fail "curl or wget is required. Install one and retry."
fi

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------

TARBALL="envpod-linux-${ARCH}.tar.gz"
URL="${BASE_URL}/${TARBALL}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "${TMPDIR}"' EXIT

info "Downloading ${TARBALL}..."
if command -v curl >/dev/null 2>&1; then
    curl -fSL -o "${TMPDIR}/${TARBALL}" "${URL}" || fail "Download failed: ${URL}"
else
    wget -qO "${TMPDIR}/${TARBALL}" "${URL}" || fail "Download failed: ${URL}"
fi
info "Downloaded ${TARBALL}"

# ---------------------------------------------------------------------------
# Extract
# ---------------------------------------------------------------------------

info "Extracting..."
tar xzf "${TMPDIR}/${TARBALL}" -C "${TMPDIR}"

RELEASE_DIR=$(find "${TMPDIR}" -maxdepth 1 -type d -name "envpod-*" | head -1)
if [ -z "${RELEASE_DIR}" ]; then
    fail "Could not find extracted release directory"
fi
info "Extracted to ${RELEASE_DIR}"

# ---------------------------------------------------------------------------
# Install (requires root — sudo prompts here, after context is shown)
# ---------------------------------------------------------------------------

printf "\n${BOLD}Root access required to install to /usr/local/bin${NC}\n\n"

# Pass ENVPOD_EXAMPLES_DIR env var and any CLI args through to the bundled installer
EXTRA_ARGS=""
if [ -n "${ENVPOD_EXAMPLES_DIR:-}" ]; then
    EXTRA_ARGS="--examples-dir ${ENVPOD_EXAMPLES_DIR}"
fi

sudo bash "${RELEASE_DIR}/install.sh" "$@" ${EXTRA_ARGS}
