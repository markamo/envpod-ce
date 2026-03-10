#!/usr/bin/env bash
#
# build-release.sh — Build envpod and assemble self-contained release folders.
#
# Output:
#   release/envpod-0.1.0-linux-x86_64/    (x86_64 release, default)
#   release/envpod-0.1.0-linux-aarch64/   (ARM64: Raspberry Pi / Jetson Orin)
#
# Usage:
#   ./build-release.sh              # x86_64 only (default)
#   ./build-release.sh --arch arm64 # aarch64 only
#   ./build-release.sh --all        # both architectures
#
# Prerequisites (x86_64):
#   rustup target add x86_64-unknown-linux-musl
#   apt install musl-tools
#
# Prerequisites (arm64) — choose one:
#   Option A (recommended): cargo install cross   [requires Docker]
#   Option B: cargo install cargo-zigbuild && snap install zig --classic --beta
#   Option C: install aarch64-linux-musl-gcc from musl.cc prebuilt toolchain
#
set -euo pipefail

VERSION="0.1.0"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

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
# Parse arguments
# ---------------------------------------------------------------------------

BUILD_X86=true
BUILD_ARM64=false

for arg in "$@"; do
    case "$arg" in
        --arch=x86_64|--arch=amd64|--arch\ x86_64|--arch\ amd64) BUILD_X86=true;  BUILD_ARM64=false ;;
        --arch=arm64|--arch=aarch64)                              BUILD_X86=false; BUILD_ARM64=true  ;;
        --arch)  : ;;  # handled in pair below
        arm64|aarch64)  BUILD_X86=false; BUILD_ARM64=true  ;;
        x86_64|amd64)   BUILD_X86=true;  BUILD_ARM64=false ;;
        --all)  BUILD_X86=true; BUILD_ARM64=true ;;
        --help|-h)
            echo "Usage: $0 [--arch x86_64|arm64] [--all]"
            exit 0
            ;;
        *) fail "Unknown argument: $arg" ;;
    esac
done

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │      envpod release builder v${VERSION}     │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

ARCH_LIST=""
${BUILD_X86}   && ARCH_LIST="${ARCH_LIST} x86_64"
${BUILD_ARM64} && ARCH_LIST="${ARCH_LIST} aarch64"
echo "  Architectures:${ARCH_LIST}"
echo ""

# ---------------------------------------------------------------------------
# build_arch <rust_target> <arch_label> <build_tool>
#
#   rust_target  e.g. x86_64-unknown-linux-musl
#   arch_label   e.g. x86_64 or aarch64
#   build_tool   cargo | cross | zigbuild
# ---------------------------------------------------------------------------

build_arch() {
    local RUST_TARGET="$1"
    local ARCH_LABEL="$2"
    local BUILD_TOOL="$3"

    local RELEASE_NAME="envpod-${VERSION}-linux-${ARCH_LABEL}"
    local RELEASE_DIR="${SCRIPT_DIR}/release/${RELEASE_NAME}"

    # -----------------------------------------------------------------------
    # 1. Build static binary
    # -----------------------------------------------------------------------

    step "Building ${ARCH_LABEL} static binary (${RUST_TARGET})"

    if ! rustup target list --installed | grep -q "${RUST_TARGET}"; then
        echo "  Adding rustup target ${RUST_TARGET}..."
        rustup target add "${RUST_TARGET}"
    fi

    case "${BUILD_TOOL}" in
        cross)
            if ! command -v cross &>/dev/null; then
                fail "'cross' not found. Install with: cargo install cross  (requires Docker)"
            fi
            cross build --release --target "${RUST_TARGET}"
            ;;
        zigbuild)
            if ! command -v cargo-zigbuild &>/dev/null; then
                fail "'cargo-zigbuild' not found. Install with: cargo install cargo-zigbuild"
            fi
            # musl targets don't use glibc versioning — no .2.17 suffix
            cargo zigbuild --release --target "${RUST_TARGET}"
            ;;
        cargo)
            cargo build --release --target "${RUST_TARGET}"
            ;;
        *)
            fail "Unknown build tool: ${BUILD_TOOL}"
            ;;
    esac

    local BINARY="${SCRIPT_DIR}/target/${RUST_TARGET}/release/envpod"
    if [[ ! -f "${BINARY}" ]]; then
        fail "Build failed — binary not found at ${BINARY}"
    fi
    info "Binary built: ${BINARY} ($(du -h "${BINARY}" | cut -f1))"

    # -----------------------------------------------------------------------
    # 2. Create release directory
    # -----------------------------------------------------------------------

    step "Assembling release directory for ${ARCH_LABEL}"

    rm -rf "${RELEASE_DIR}"
    mkdir -p "${RELEASE_DIR}/docs" "${RELEASE_DIR}/examples"

    cp "${BINARY}" "${RELEASE_DIR}/envpod"
    chmod 755 "${RELEASE_DIR}/envpod"
    info "Binary copied"

    # -----------------------------------------------------------------------
    # 3. Copy universal install.sh
    # -----------------------------------------------------------------------

    cp "${SCRIPT_DIR}/install.sh" "${RELEASE_DIR}/install.sh"
    chmod 755 "${RELEASE_DIR}/install.sh"
    info "install.sh copied (universal installer)"

    # -----------------------------------------------------------------------
    # 4. Generate README.md
    # -----------------------------------------------------------------------

    cat > "${RELEASE_DIR}/README.md" << README_EOF
# envpod v${VERSION}

> **EnvPod v${VERSION}** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL 1.1

**Docker isolates. Envpod governs.**

Every AI agent runs inside a **pod** — an isolated environment with a foundation (OverlayFS COW), four walls (processor, network, memory, devices), and a governance ceiling (credential vault, action queue, monitoring, remote control, audit).

## What's in This Release

\`\`\`
${RELEASE_NAME}/
├── envpod          Static binary for ${ARCH_LABEL} Linux (no dependencies)
├── install.sh      Universal installer (distro detection, online/offline)
├── README.md       This file
├── LICENSE         BSL 1.1
├── docs/           Documentation
│   ├── INSTALL.md
│   ├── QUICKSTART.md
│   ├── USER-GUIDE.md
│   ├── FAQ.md
│   ├── BENCHMARKS.md
│   ├── SECURITY.md
│   ├── TUTORIALS.md
│   ├── POD-CONFIG.md
│   ├── CAPABILITIES.md
│   ├── ROADMAP.md
│   └── EMBEDDED.md     (Raspberry Pi / Jetson Orin guide)
└── examples/       Pod configs (24 YAML) + jailbreak-test.sh
\`\`\`

## Quick Start

\`\`\`bash
# Install
sudo bash install.sh

# Create a pod from an example config
sudo envpod init my-agent -c examples/coding-agent.yaml

# Run a command inside the pod (fully isolated)
sudo envpod run my-agent -- /bin/bash

# See what the agent changed
sudo envpod diff my-agent

# Accept or reject changes
sudo envpod commit my-agent              # apply all changes to host
sudo envpod commit my-agent /opt/a       # commit specific paths only
sudo envpod rollback my-agent            # discard everything

# View audit trail
sudo envpod audit my-agent

# Security analysis
sudo envpod audit my-agent --security
\`\`\`

See [docs/INSTALL.md](docs/INSTALL.md), [docs/QUICKSTART.md](docs/QUICKSTART.md),
[docs/POD-CONFIG.md](docs/POD-CONFIG.md), [docs/TUTORIALS.md](docs/TUTORIALS.md),
[docs/CAPABILITIES.md](docs/CAPABILITIES.md), [docs/ROADMAP.md](docs/ROADMAP.md),
[docs/BENCHMARKS.md](docs/BENCHMARKS.md), [docs/SECURITY.md](docs/SECURITY.md),
[docs/FAQ.md](docs/FAQ.md), and [docs/EMBEDDED.md](docs/EMBEDDED.md).

## Features

**Filesystem Isolation** — OverlayFS copy-on-write. Agent writes go to an overlay, never the host. Review with diff, accept with commit, discard with rollback.

**Network Isolation** — Each pod gets its own network namespace. Embedded DNS resolver per pod with whitelist, blacklist, or monitor modes. Every DNS query is logged.

**Process Isolation** — PID namespace, cgroups v2 (CPU, memory, PID limits), seccomp-BPF syscall filtering.

**Credential Vault** — Secrets stored encrypted (ChaCha20-Poly1305). Vault proxy injection available: agent never sees real API keys.

**Pod-to-Pod Discovery** — Pods can discover each other by name (\`<name>.pods.local\`) via the central envpod-dns daemon. Policy-controlled, bilateral access.

**Action Queue** — Actions classified by reversibility: immediate, delayed, staged (human approval), blocked.

**Audit Trail** — Append-only JSONL logs. Static security analysis via \`envpod audit --security\`.

**Monitoring Agent** — Background policy engine can autonomously freeze or restrict a pod.

**Remote Control** — Freeze, resume, kill, or restrict a running pod via \`envpod remote\`.

**Display + Audio** — GPU passthrough, Wayland/X11, PipeWire/PulseAudio forwarding for GUI agents.

**Web Dashboard** — \`envpod dashboard\` on localhost:9090 — fleet overview, live resource usage, audit timeline, diff/commit from browser.

**Embedded Systems** — Runs on Raspberry Pi 4/5 and NVIDIA Jetson Orin (ARM64 static binary). See [docs/EMBEDDED.md](docs/EMBEDDED.md).

## CLI Commands

| Command | Description |
|---------|-------------|
| \`envpod init <name> [-c config.yaml]\` | Create a new pod |
| \`envpod setup <name>\` | Re-run setup commands |
| \`envpod run <name> [--root] [-d] [-a] -- <cmd>\` | Run a command inside a pod |
| \`envpod diff <name>\` | Show filesystem changes |
| \`envpod commit <name> [paths...] [--exclude ...]\` | Apply changes to host |
| \`envpod rollback <name>\` | Discard all overlay changes |
| \`envpod audit <name> [--security] [--json]\` | Audit log or security analysis |
| \`envpod status <name>\` | Pod status and resource usage |
| \`envpod lock <name>\` | Freeze pod state |
| \`envpod kill <name>\` | Stop and rollback |
| \`envpod destroy <names...> [--base]\` | Remove pod(s) |
| \`envpod clone <source> <name> [--current]\` | Clone a pod (fast) |
| \`envpod base create/ls/destroy\` | Manage base pods |
| \`envpod ls [--json]\` | List all pods |
| \`envpod vault <name> set/get/remove/bind/unbind\` | Manage credentials + proxy |
| \`envpod ports <name> -p/-P/-i/--remove\` | Live port forwarding mutations |
| \`envpod discover <name> --on/--off/--add-pod\` | Live discovery mutations |
| \`envpod dns-daemon [--socket]\` | Start central DNS daemon |
| \`envpod queue/approve/cancel <name>\` | Action staging queue |
| \`envpod undo <name>\` | Undo last reversible action |
| \`envpod dns <name>\` | Update DNS policy live |
| \`envpod remote <name> <cmd>\` | Remote control |
| \`envpod monitor <name>\` | Monitoring policy |
| \`envpod dashboard [--port 9090]\` | Web dashboard |
| \`envpod gc\` | Clean up orphaned resources |

## System Requirements

- Linux ${ARCH_LABEL}, kernel 5.11+
- cgroups v2 (see [docs/EMBEDDED.md](docs/EMBEDDED.md) for Pi-specific setup)
- OverlayFS (\`modprobe overlay\`)
- iptables, iproute2

## License

Copyright 2026 Xtellix Inc. All rights reserved.

Licensed under BSL 1.1. Converts to AGPL-3.0 on 2030-03-07. See [LICENSE](LICENSE) for the full text.

**Author:** Mark Amoboateng, Xtellix Inc. (mark@envpod.dev)
**Patent:** Provisional patent filed February 22, 2026.
README_EOF
    info "README.md generated"

    # -----------------------------------------------------------------------
    # 5. Generate LICENSE
    # -----------------------------------------------------------------------

    cp "${SCRIPT_DIR}/LICENSE" "${RELEASE_DIR}/LICENSE"
    info "LICENSE copied"

    # -----------------------------------------------------------------------
    # 5b. Copy uninstall.sh
    # -----------------------------------------------------------------------

    if [[ -f "${SCRIPT_DIR}/uninstall.sh" ]]; then
        cp "${SCRIPT_DIR}/uninstall.sh" "${RELEASE_DIR}/uninstall.sh"
        chmod 755 "${RELEASE_DIR}/uninstall.sh"
        info "uninstall.sh copied"
    fi

    # -----------------------------------------------------------------------
    # 6. Copy docs and examples from repo
    # -----------------------------------------------------------------------

    for doc in INSTALL.md QUICKSTART.md USER-GUIDE.md FAQ.md BENCHMARKS.md \
               SECURITY.md TUTORIALS.md POD-CONFIG.md CAPABILITIES.md \
               ROADMAP.md EMBEDDED.md; do
        if [[ -f "${SCRIPT_DIR}/docs/${doc}" ]]; then
            cp "${SCRIPT_DIR}/docs/${doc}" "${RELEASE_DIR}/docs/${doc}"
        else
            echo "  Warning: docs/${doc} not found — skipping"
        fi
    done
    info "Documentation copied"

    cp "${SCRIPT_DIR}/examples/"*.yaml "${RELEASE_DIR}/examples/"
    cp "${SCRIPT_DIR}/examples/"*.sh "${RELEASE_DIR}/examples/" 2>/dev/null || true
    local EXAMPLE_COUNT
    EXAMPLE_COUNT=$(ls -1 "${RELEASE_DIR}/examples/"*.yaml 2>/dev/null | wc -l)
    local SCRIPT_COUNT
    SCRIPT_COUNT=$(ls -1 "${RELEASE_DIR}/examples/"*.sh 2>/dev/null | wc -l)
    info "Examples copied (${EXAMPLE_COUNT} YAML configs, ${SCRIPT_COUNT} scripts)"

    # -----------------------------------------------------------------------
    # 7. Create tarball
    # -----------------------------------------------------------------------

    step "Creating tarball for ${ARCH_LABEL}"

    local TARBALL="${SCRIPT_DIR}/${RELEASE_NAME}.tar.gz"
    tar czf "${TARBALL}" -C "${SCRIPT_DIR}/release" "${RELEASE_NAME}"
    info "Created ${TARBALL}"

    # Create unversioned copy for stable download URL (envpod.dev/install.sh)
    local LATEST_TARBALL="${SCRIPT_DIR}/envpod-linux-${ARCH_LABEL}.tar.gz"
    cp "${TARBALL}" "${LATEST_TARBALL}"
    info "Created ${LATEST_TARBALL} (unversioned copy for install script)"

    # -----------------------------------------------------------------------
    # 8. Summary for this arch
    # -----------------------------------------------------------------------

    step "Release summary — ${ARCH_LABEL}"

    echo ""
    echo "  Release directory: ${RELEASE_DIR}/"
    echo ""
    ls -lh "${RELEASE_DIR}/"
    echo ""

    local TARBALL_SIZE
    TARBALL_SIZE=$(du -h "${TARBALL}" | cut -f1)
    local TARBALL_SHA
    TARBALL_SHA=$(sha256sum "${TARBALL}")

    echo -e "  ${BOLD}Tarball:${NC}  ${RELEASE_NAME}.tar.gz (${TARBALL_SIZE})"
    echo -e "  ${BOLD}SHA-256:${NC} ${TARBALL_SHA}"
    echo ""
    info "Done! Distribute ${RELEASE_NAME}.tar.gz to any ${ARCH_LABEL} Linux system."
}

# ---------------------------------------------------------------------------
# Main: build requested architectures
# ---------------------------------------------------------------------------

# Detect ARM64 build tool preference (zigbuild preferred — cross has GLIBC issues)
ARM64_TOOL="cargo"
if command -v cross &>/dev/null; then
    ARM64_TOOL="cross"
fi
if command -v cargo-zigbuild &>/dev/null; then
    ARM64_TOOL="zigbuild"
fi

if ${BUILD_X86}; then
    build_arch "x86_64-unknown-linux-musl" "x86_64" "cargo"
fi

if ${BUILD_ARM64}; then
    echo ""
    echo -e "${BOLD}ARM64 build tool: ${ARM64_TOOL}${NC}"
    echo "  (override: ARM64_TOOL=cargo|cross|zigbuild ./build-release.sh --arch arm64)"
    echo ""
    # Allow override via environment
    ARM64_TOOL="${ARM64_TOOL_OVERRIDE:-${ARM64_TOOL}}"
    build_arch "aarch64-unknown-linux-musl" "aarch64" "${ARM64_TOOL}"
fi

echo ""
echo -e "${GREEN}${BOLD}All builds complete!${NC}"
