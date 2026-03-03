#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Disk footprint comparison: Docker vs Podman vs Envpod.
# Requires: sudo, Docker, Podman, envpod release build or installed binary.
#
# Usage:
#   sudo ./tests/benchmark-size.sh

set -e

# ---------------------------------------------------------------------------
# Color helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
    GREEN='\033[32m'
    CYAN='\033[36m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'
else
    GREEN='' CYAN='' BOLD='' DIM='' RESET=''
fi

info()  { echo -e "${BOLD}$*${RESET}"; }
dim()   { echo -e "${DIM}$*${RESET}"; }

# ---------------------------------------------------------------------------
# Locate envpod binary
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -x "$REPO_ROOT/target/release/envpod" ]; then
    ENVPOD="$REPO_ROOT/target/release/envpod"
elif command -v envpod &>/dev/null; then
    ENVPOD="$(command -v envpod)"
elif [ -x "$REPO_ROOT/target/debug/envpod" ]; then
    ENVPOD="$REPO_ROOT/target/debug/envpod"
else
    echo "Error: envpod binary not found. Run 'cargo build --release' first." >&2
    exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: must run as root (sudo $0)" >&2
    exit 1
fi

for cmd in docker podman; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "Error: $cmd not found." >&2
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Size helpers
# ---------------------------------------------------------------------------
# Actual on-disk size in bytes (does NOT follow symlinks inside the tree).
dir_bytes() {
    du -sb "$1" 2>/dev/null | awk '{print $1}' || echo "0"
}

# Get the actual pod directory for a named pod.
# Pods are stored by UUID under {ENVPOD_DIR}/pods/{uuid}/, not by name.
# The name→UUID mapping is in {ENVPOD_DIR}/state/{name}.json.
pod_dir_for() {
    local name=$1
    local state_file="$BENCH_DIR/state/${name}.json"
    if [ -f "$state_file" ]; then
        sed -n 's/.*"pod_dir" *: *"\([^"]*\)".*/\1/p' "$state_file" | head -1
    fi
}

# Parse human-readable size string (e.g. "78.1MB") to bytes.
parse_human_size() {
    local s="$1"
    local num unit
    num=$(echo "$s" | grep -oP '[0-9.]+' | head -1)
    unit=$(echo "$s" | grep -oP '[A-Za-z]+' | head -1)
    case "$unit" in
        B)  echo "$num" | awk '{printf "%.0f", $1}' ;;
        KB|kB) echo "$num" | awk '{printf "%.0f", $1 * 1024}' ;;
        MB) echo "$num" | awk '{printf "%.0f", $1 * 1048576}' ;;
        GB) echo "$num" | awk '{printf "%.0f", $1 * 1073741824}' ;;
        *)  echo "0" ;;
    esac
}

fmt_size() {
    local bytes=${1:-0}
    if (( bytes >= 1073741824 )); then
        printf "%.1f GB" "$(echo "scale=1; $bytes / 1073741824" | bc)"
    elif (( bytes >= 1048576 )); then
        printf "%.1f MB" "$(echo "scale=1; $bytes / 1048576" | bc)"
    elif (( bytes >= 1024 )); then
        printf "%.1f KB" "$(echo "scale=1; $bytes / 1024" | bc)"
    else
        printf "%d B" "$bytes"
    fi
}

# ---------------------------------------------------------------------------
# Detect GPU
# ---------------------------------------------------------------------------
HAS_GPU=false
if command -v nvidia-smi &>/dev/null && nvidia-smi --query-gpu=name --format=csv,noheader &>/dev/null 2>&1; then
    HAS_GPU=true
fi

# ---------------------------------------------------------------------------
# Images
# ---------------------------------------------------------------------------
DOCKER_IMG="ubuntu:24.04"
PODMAN_IMG="docker.io/library/ubuntu:24.04"
DOCKER_GPU_IMG="nvidia/cuda:12.0.0-base-ubuntu22.04"
PODMAN_GPU_IMG="docker.io/nvidia/cuda:12.0.0-base-ubuntu22.04"

echo ""
info "Preparing images..."

# Pull if needed
docker image inspect "$DOCKER_IMG" &>/dev/null || docker pull "$DOCKER_IMG" >/dev/null 2>&1
podman image inspect "$PODMAN_IMG" &>/dev/null || podman pull "$PODMAN_IMG" >/dev/null 2>&1
if $HAS_GPU; then
    docker image inspect "$DOCKER_GPU_IMG" &>/dev/null || docker pull "$DOCKER_GPU_IMG" >/dev/null 2>&1
    podman image inspect "$PODMAN_GPU_IMG" &>/dev/null || podman pull "$PODMAN_GPU_IMG" >/dev/null 2>&1
fi

# ---------------------------------------------------------------------------
# Envpod state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-size-bench-XXXXXX)
export ENVPOD_DIR="$BENCH_DIR"
trap 'rm -rf "$BENCH_DIR"' EXIT

POD_YAML="$BENCH_DIR/pod.yaml"
cat > "$POD_YAML" << 'YAML'
name: size-test
type: standard
backend: native
network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow: []
processor:
  cores: 1.0
  memory: "256MB"
  max_pids: 64
budget:
  max_duration: "1m"
audit:
  action_log: true
YAML

GPU_YAML="$BENCH_DIR/gpu-pod.yaml"
cat > "$GPU_YAML" << 'YAML'
name: size-gpu-test
type: standard
backend: native
network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow: []
devices:
  gpu: true
processor:
  cores: 1.0
  memory: "256MB"
  max_pids: 64
budget:
  max_duration: "1m"
audit:
  action_log: true
YAML

# ===========================================================================
# Measure Docker image sizes
# ===========================================================================
info "Measuring Docker image sizes..."
# Docker 29+ deprecated VirtualSize and .Size returns compressed content-store size.
# Use `docker image ls --format '{{.Size}}'` which shows uncompressed size (e.g. "78.1MB").
# For Podman, `image inspect .VirtualSize` still works and returns raw bytes.
docker_image_size() {
    local img=$1
    local human
    human=$(docker image ls "$img" --format '{{.Size}}' 2>/dev/null | head -1)
    if [ -n "$human" ]; then
        parse_human_size "$human"
    else
        echo "0"
    fi
}

podman_image_size() {
    local img=$1
    local raw
    raw=$(podman image inspect "$img" --format '{{.VirtualSize}}' 2>/dev/null) || true
    if [[ "$raw" =~ ^[0-9]+$ ]] && (( raw > 0 )); then
        echo "$raw"
        return
    fi
    raw=$(podman image inspect "$img" --format '{{.Size}}' 2>/dev/null) || true
    if [[ "$raw" =~ ^[0-9]+$ ]]; then
        echo "$raw"
    else
        echo "0"
    fi
}

DOCKER_IMG_SIZE=$(docker_image_size "$DOCKER_IMG")
DOCKER_GPU_IMG_SIZE=0
if $HAS_GPU; then
    DOCKER_GPU_IMG_SIZE=$(docker_image_size "$DOCKER_GPU_IMG")
fi

# ===========================================================================
# Measure Podman image sizes
# ===========================================================================
info "Measuring Podman image sizes..."
PODMAN_IMG_SIZE=$(podman_image_size "$PODMAN_IMG")
PODMAN_GPU_IMG_SIZE=0
if $HAS_GPU; then
    PODMAN_GPU_IMG_SIZE=$(podman_image_size "$PODMAN_GPU_IMG")
fi

# ===========================================================================
# Measure Docker container sizes (run + inspect with -s)
# ===========================================================================
info "Measuring Docker container overhead..."
DOCKER_CID=$(docker run -d "$DOCKER_IMG" /bin/true 2>/dev/null)
sleep 1  # let container finish so writable layer is populated
DOCKER_CTR_RAW=$(docker inspect -s "$DOCKER_CID" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
DOCKER_CTR_SIZE=${DOCKER_CTR_RAW//[^0-9]/}
DOCKER_CTR_SIZE=${DOCKER_CTR_SIZE:-0}
docker rm -f "$DOCKER_CID" >/dev/null 2>&1

DOCKER_GPU_CTR_SIZE=0
if $HAS_GPU; then
    DOCKER_GPU_CID=$(docker run -d --gpus all "$DOCKER_GPU_IMG" /bin/true 2>/dev/null)
    sleep 1
    DOCKER_GPU_CTR_RAW=$(docker inspect -s "$DOCKER_GPU_CID" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
    DOCKER_GPU_CTR_SIZE=${DOCKER_GPU_CTR_RAW//[^0-9]/}
    DOCKER_GPU_CTR_SIZE=${DOCKER_GPU_CTR_SIZE:-0}
    docker rm -f "$DOCKER_GPU_CID" >/dev/null 2>&1
fi

# ===========================================================================
# Measure Podman container sizes
# ===========================================================================
info "Measuring Podman container overhead..."
PODMAN_CID=$(podman run -d "$PODMAN_IMG" /bin/true 2>/dev/null)
sleep 1
PODMAN_CTR_RAW=$(podman inspect -s "$PODMAN_CID" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
PODMAN_CTR_SIZE=${PODMAN_CTR_RAW//[^0-9]/}
PODMAN_CTR_SIZE=${PODMAN_CTR_SIZE:-0}
podman rm -f "$PODMAN_CID" >/dev/null 2>&1

PODMAN_GPU_CTR_SIZE=0
if $HAS_GPU; then
    PODMAN_GPU_CID=$(podman run -d --device nvidia.com/gpu=all "$PODMAN_GPU_IMG" /bin/true 2>/dev/null)
    sleep 1
    PODMAN_GPU_CTR_RAW=$(podman inspect -s "$PODMAN_GPU_CID" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
    PODMAN_GPU_CTR_SIZE=${PODMAN_GPU_CTR_RAW//[^0-9]/}
    PODMAN_GPU_CTR_SIZE=${PODMAN_GPU_CTR_SIZE:-0}
    podman rm -f "$PODMAN_GPU_CID" >/dev/null 2>&1
fi

# ===========================================================================
# Measure envpod sizes
# ===========================================================================
info "Creating envpod pods and measuring sizes..."

# Standard pod (init creates base + pod)
"$ENVPOD" init size-test -c "$POD_YAML" 2>&1 || { echo "envpod init failed" >&2; exit 1; }

# Resolve the actual pod directory (stored by UUID, not name)
POD_DIR=$(pod_dir_for "size-test")
if [ -z "$POD_DIR" ] || [ ! -d "$POD_DIR" ]; then
    echo "Error: could not resolve pod directory for size-test" >&2
    echo "  State dir: $BENCH_DIR/state/" >&2
    ls -la "$BENCH_DIR/state/" 2>/dev/null >&2
    exit 1
fi

ENVPOD_BASE_SIZE=$(dir_bytes "$BENCH_DIR/bases/size-test")
# Pod unique overhead = pod dir WITHOUT following symlinks (rootfs is shared via symlink)
ENVPOD_POD_SIZE=$(dir_bytes "$POD_DIR")

# Breakdown
# Rootfs size — resolve symlink if present so we measure the actual rootfs dir
ENVPOD_ROOTFS_SIZE=0
ROOTFS_NOTE=""
ROOTFS_PATH="$POD_DIR/rootfs"
if [ -L "$ROOTFS_PATH" ]; then
    ROOTFS_PATH=$(readlink -f "$ROOTFS_PATH")
    ENVPOD_ROOTFS_SIZE=$(dir_bytes "$ROOTFS_PATH")
    ROOTFS_NOTE="(shared with base via symlink)"
elif [ -d "$ROOTFS_PATH" ]; then
    ENVPOD_ROOTFS_SIZE=$(dir_bytes "$ROOTFS_PATH")
    ROOTFS_NOTE="(in pod dir)"
fi

ENVPOD_UPPER_SIZE=$(dir_bytes "$POD_DIR/upper")

# Clone (should be very small — symlinked rootfs + empty upper)
"$ENVPOD" clone size-test size-clone >/dev/null 2>&1
CLONE_DIR=$(pod_dir_for "size-clone")
ENVPOD_CLONE_SIZE=0
if [ -n "$CLONE_DIR" ] && [ -d "$CLONE_DIR" ]; then
    ENVPOD_CLONE_SIZE=$(dir_bytes "$CLONE_DIR")
fi

# GPU pod
ENVPOD_GPU_BASE_SIZE=0
ENVPOD_GPU_POD_SIZE=0
if $HAS_GPU; then
    "$ENVPOD" init size-gpu-test -c "$GPU_YAML" >/dev/null 2>&1
    ENVPOD_GPU_BASE_SIZE=$(dir_bytes "$BENCH_DIR/bases/size-gpu-test")
    GPU_POD_DIR=$(pod_dir_for "size-gpu-test")
    if [ -n "$GPU_POD_DIR" ] && [ -d "$GPU_POD_DIR" ]; then
        ENVPOD_GPU_POD_SIZE=$(dir_bytes "$GPU_POD_DIR")
    fi
fi

# Measure sizes before cleanup
info "Measuring envpod sizes..."

# Cleanup
"$ENVPOD" destroy size-clone >/dev/null 2>&1 || true
"$ENVPOD" destroy size-test --base >/dev/null 2>&1 || true
if $HAS_GPU; then
    "$ENVPOD" destroy size-gpu-test --base >/dev/null 2>&1 || true
fi

# ===========================================================================
# Results
# ===========================================================================
DOCKER_VER=$(docker --version | sed 's/Docker version //' | sed 's/,.*//')
PODMAN_VER=$(podman --version | sed 's/podman version //')

echo ""
info "═══════════════════════════════════════════════════════════════════════"
info "  Disk Footprint Comparison"
info "═══════════════════════════════════════════════════════════════════════"
dim "  Docker $DOCKER_VER, Podman $PODMAN_VER, envpod (native Linux backend)"
echo ""

# --- Base images ---
info "  Base image / base pod (ubuntu 24.04)"
echo ""
printf "  ${BOLD}%-30s %12s${RESET}\n" "RUNTIME" "SIZE"
printf "  %-30s %12s\n" "──────────────────────────────" "────────────"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Docker image" "$(fmt_size $DOCKER_IMG_SIZE)"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Podman image" "$(fmt_size $PODMAN_IMG_SIZE)"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Envpod base pod" "$(fmt_size $ENVPOD_BASE_SIZE)"

if (( DOCKER_IMG_SIZE > 0 && ENVPOD_BASE_SIZE < DOCKER_IMG_SIZE )); then
    DOCKER_SAVINGS=$(( (DOCKER_IMG_SIZE - ENVPOD_BASE_SIZE) * 100 / DOCKER_IMG_SIZE ))
    printf "\n  ${GREEN}Envpod base is %d%% smaller than Docker image${RESET}\n" "$DOCKER_SAVINGS"
fi
echo ""

# --- Per-instance overhead (unique, non-shared) ---
info "  Per-instance overhead (unique disk cost per container/pod)"
echo ""
printf "  ${BOLD}%-30s %12s${RESET}\n" "RUNTIME" "SIZE"
printf "  %-30s %12s\n" "──────────────────────────────" "────────────"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Docker container layer" "$(fmt_size $DOCKER_CTR_SIZE)"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Podman container layer" "$(fmt_size $PODMAN_CTR_SIZE)"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Envpod pod (unique)" "$(fmt_size $ENVPOD_POD_SIZE)"
printf "  ${CYAN}%-30s${RESET} %12s\n" "Envpod clone (unique)" "$(fmt_size $ENVPOD_CLONE_SIZE)"
echo ""
dim "  Note: all runtimes share base image/rootfs across instances."
dim "  Sizes above are the per-instance writable layer only."
echo ""

# --- Envpod breakdown ---
info "  Envpod pod breakdown"
echo ""
printf "  ${BOLD}%-35s %12s  %s${RESET}\n" "COMPONENT" "SIZE" ""
printf "  %-35s %12s\n" "───────────────────────────────────" "────────────"
printf "  ${CYAN}%-35s${RESET} %12s  %s\n" "base pod (shared rootfs + state)" "$(fmt_size $ENVPOD_BASE_SIZE)" ""
printf "  ${CYAN}%-35s${RESET} %12s  %s\n" "  rootfs/" "$(fmt_size $ENVPOD_ROOTFS_SIZE)" "$ROOTFS_NOTE"
printf "  ${CYAN}%-35s${RESET} %12s  %s\n" "pod unique overhead" "$(fmt_size $ENVPOD_POD_SIZE)" "(pod.yaml + empty dirs)"
printf "  ${CYAN}%-35s${RESET} %12s  %s\n" "  upper/" "$(fmt_size $ENVPOD_UPPER_SIZE)" "(COW layer, grows with changes)"
printf "  ${CYAN}%-35s${RESET} %12s  %s\n" "clone unique overhead" "$(fmt_size $ENVPOD_CLONE_SIZE)" "(rootfs symlinked to base)"
echo ""

# --- GPU ---
if $HAS_GPU; then
    info "  GPU image / base pod (CUDA 12.0 ubuntu 22.04)"
    echo ""
    printf "  ${BOLD}%-30s %12s${RESET}\n" "RUNTIME" "SIZE"
    printf "  %-30s %12s\n" "──────────────────────────────" "────────────"
    printf "  ${CYAN}%-30s${RESET} %12s\n" "Docker image" "$(fmt_size $DOCKER_GPU_IMG_SIZE)"
    printf "  ${CYAN}%-30s${RESET} %12s\n" "Podman image" "$(fmt_size $PODMAN_GPU_IMG_SIZE)"
    printf "  ${CYAN}%-30s${RESET} %12s\n" "Envpod base pod (gpu: true)" "$(fmt_size $ENVPOD_GPU_BASE_SIZE)"
    if (( DOCKER_GPU_IMG_SIZE > 0 && ENVPOD_GPU_BASE_SIZE < DOCKER_GPU_IMG_SIZE )); then
        GPU_SAVINGS=$(( (DOCKER_GPU_IMG_SIZE - ENVPOD_GPU_BASE_SIZE) * 100 / DOCKER_GPU_IMG_SIZE ))
        printf "\n  ${GREEN}Envpod GPU base is %d%% smaller than Docker CUDA image${RESET}\n" "$GPU_SAVINGS"
    fi
    echo ""
fi

# --- Why envpod is smaller ---
info "  Why envpod is smaller:"
dim "    Docker/Podman: full distro userland copied into image (~120MB+ for ubuntu)"
dim "    Envpod: only /etc + apt state in rootfs; /usr /bin /lib bind-mounted from host"
dim "    Clones share the base rootfs via symlink — near-zero per-clone overhead"
echo ""