#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Scale benchmark: create N instances and measure creation time, disk usage,
# and cleanup time.  Compares Docker vs Podman vs Envpod clone scaling.
#
# Usage:
#   sudo ./tests/benchmark-scale.sh [COUNT]
#   Default COUNT is 50.

set -euo pipefail

COUNT=${1:-50}

# ---------------------------------------------------------------------------
# Color helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
    RED='\033[31m'
    GREEN='\033[32m'
    CYAN='\033[36m'
    YELLOW='\033[33m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'
else
    RED='' GREEN='' CYAN='' YELLOW='' BOLD='' DIM='' RESET=''
fi

info()  { echo -e "${BOLD}$*${RESET}"; }
dim()   { echo -e "${DIM}$*${RESET}"; }
warn()  { echo -e "${YELLOW}$*${RESET}"; }

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
# Helpers
# ---------------------------------------------------------------------------
fmt_ms() {
    local ms=$1
    if (( ms >= 1000 )); then
        printf "%.1fs" "$(echo "scale=1; $ms / 1000" | bc)"
    else
        printf "%dms" "$ms"
    fi
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

now_ms() { echo $(( $(date +%s%N) / 1000000 )); }

# ---------------------------------------------------------------------------
# Images
# ---------------------------------------------------------------------------
DOCKER_IMG="ubuntu:24.04"
PODMAN_IMG="docker.io/library/ubuntu:24.04"

echo ""
info "Scale Benchmark — $COUNT instances"
echo ""

info "Preparing images..."
docker image inspect "$DOCKER_IMG" &>/dev/null || docker pull "$DOCKER_IMG" >/dev/null 2>&1
dim "  Docker $DOCKER_IMG: ready"
podman image inspect "$PODMAN_IMG" &>/dev/null || podman pull "$PODMAN_IMG" >/dev/null 2>&1
dim "  Podman $PODMAN_IMG: ready"

# ---------------------------------------------------------------------------
# Envpod state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-scale-bench-XXXXXX)
export ENVPOD_DIR="$BENCH_DIR"
trap 'rm -rf "$BENCH_DIR"' EXIT

POD_YAML="$BENCH_DIR/pod.yaml"
cat > "$POD_YAML" << 'YAML'
name: scale-base
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

info "Creating envpod base pod..."
"$ENVPOD" init scale-base -c "$POD_YAML" >/dev/null 2>&1
dim "  envpod base: ready"
echo ""

# ===========================================================================
# Test 1: Create N instances
# ===========================================================================
info "Test 1: Create $COUNT instances"
echo ""

# --- Docker ---
dim "  Creating $COUNT Docker containers..."
DOCKER_CIDS=()
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    cid=$(docker create --name "bench-d-$i" "$DOCKER_IMG" /bin/true 2>/dev/null)
    DOCKER_CIDS+=("$cid")
done
t1=$(now_ms)
DOCKER_CREATE_MS=$(( t1 - t0 ))
DOCKER_PER_MS=$(( DOCKER_CREATE_MS / COUNT ))
dim "    Done: $(fmt_ms $DOCKER_CREATE_MS) total, $(fmt_ms $DOCKER_PER_MS)/instance"

# --- Podman ---
dim "  Creating $COUNT Podman containers..."
PODMAN_CIDS=()
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    cid=$(podman create --name "bench-p-$i" "$PODMAN_IMG" /bin/true 2>/dev/null)
    PODMAN_CIDS+=("$cid")
done
t1=$(now_ms)
PODMAN_CREATE_MS=$(( t1 - t0 ))
PODMAN_PER_MS=$(( PODMAN_CREATE_MS / COUNT ))
dim "    Done: $(fmt_ms $PODMAN_CREATE_MS) total, $(fmt_ms $PODMAN_PER_MS)/instance"

# --- Envpod ---
dim "  Creating $COUNT envpod clones..."
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    "$ENVPOD" clone scale-base "clone-$i" >/dev/null 2>&1
done
t1=$(now_ms)
ENVPOD_CREATE_MS=$(( t1 - t0 ))
ENVPOD_PER_MS=$(( ENVPOD_CREATE_MS / COUNT ))
dim "    Done: $(fmt_ms $ENVPOD_CREATE_MS) total, $(fmt_ms $ENVPOD_PER_MS)/instance"
echo ""

# ===========================================================================
# Test 2: Total disk overhead for N instances
# ===========================================================================
info "Test 2: Total disk overhead for $COUNT instances"
echo ""

# Docker: sum of SizeRw for all containers
dim "  Measuring Docker container layers..."
DOCKER_DISK=0
for i in $(seq 1 "$COUNT"); do
    raw=$(docker inspect -s "bench-d-$i" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
    size=${raw//[^0-9]/}
    DOCKER_DISK=$(( DOCKER_DISK + ${size:-0} ))
done

# Podman: sum of SizeRw for all containers (podman uses --size, not -s)
dim "  Measuring Podman container layers..."
PODMAN_DISK=0
for i in $(seq 1 "$COUNT"); do
    raw=$(podman inspect --size "bench-p-$i" --format '{{.SizeRw}}' 2>/dev/null || echo "0")
    size=${raw//[^0-9]/}
    PODMAN_DISK=$(( PODMAN_DISK + ${size:-0} ))
done

# Envpod: total size of pods/ directory (all clones + base pod)
dim "  Measuring envpod pod directories..."
ENVPOD_DISK=$(du -sb "$BENCH_DIR/pods/" 2>/dev/null | awk '{print $1}')
ENVPOD_DISK=${ENVPOD_DISK:-0}
echo ""

# ===========================================================================
# Test 3: Run /bin/true in ALL N instances
# ===========================================================================
info "Test 3: Run /bin/true in all $COUNT instances"
echo ""

# Docker — start all containers (they were created with /bin/true)
dim "  Running in $COUNT Docker containers..."
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    docker start "bench-d-$i" >/dev/null 2>&1
done
t1=$(now_ms)
DOCKER_RUN_ALL_MS=$(( t1 - t0 ))
DOCKER_RUN_PER_MS=$(( DOCKER_RUN_ALL_MS / COUNT ))
dim "    Done: $(fmt_ms $DOCKER_RUN_ALL_MS) total, $(fmt_ms $DOCKER_RUN_PER_MS)/instance"

# Podman — start all containers
dim "  Running in $COUNT Podman containers..."
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    podman start "bench-p-$i" >/dev/null 2>&1
done
t1=$(now_ms)
PODMAN_RUN_ALL_MS=$(( t1 - t0 ))
PODMAN_RUN_PER_MS=$(( PODMAN_RUN_ALL_MS / COUNT ))
dim "    Done: $(fmt_ms $PODMAN_RUN_ALL_MS) total, $(fmt_ms $PODMAN_RUN_PER_MS)/instance"

# Envpod — run /bin/true in all clones (first run triggers deferred network setup)
dim "  Running in $COUNT envpod clones..."
t0=$(now_ms)
for i in $(seq 1 "$COUNT"); do
    "$ENVPOD" run "clone-$i" --root -- /bin/true >/dev/null 2>&1
done
t1=$(now_ms)
ENVPOD_RUN_ALL_MS=$(( t1 - t0 ))
ENVPOD_RUN_PER_MS=$(( ENVPOD_RUN_ALL_MS / COUNT ))
dim "    Done: $(fmt_ms $ENVPOD_RUN_ALL_MS) total, $(fmt_ms $ENVPOD_RUN_PER_MS)/instance"
echo ""

# ===========================================================================
# Test 4: Cleanup time (destroy all N instances)
# ===========================================================================
info "Test 4: Destroy all $COUNT instances"
echo ""

# Build name lists for batch operations
DOCKER_NAMES=()
PODMAN_NAMES=()
ENVPOD_NAMES=()
for i in $(seq 1 "$COUNT"); do
    DOCKER_NAMES+=("bench-d-$i")
    PODMAN_NAMES+=("bench-p-$i")
    ENVPOD_NAMES+=("clone-$i")
done

# Docker (batch rm)
dim "  Destroying $COUNT Docker containers..."
t0=$(now_ms)
docker rm -f "${DOCKER_NAMES[@]}" >/dev/null 2>&1 || true
t1=$(now_ms)
DOCKER_DESTROY_MS=$(( t1 - t0 ))
dim "    Done: $(fmt_ms $DOCKER_DESTROY_MS)"

# Podman (batch rm)
dim "  Destroying $COUNT Podman containers..."
t0=$(now_ms)
podman rm -f "${PODMAN_NAMES[@]}" >/dev/null 2>&1 || true
t1=$(now_ms)
PODMAN_DESTROY_MS=$(( t1 - t0 ))
dim "    Done: $(fmt_ms $PODMAN_DESTROY_MS)"

# Envpod (batch destroy — single process invocation)
dim "  Destroying $COUNT envpod clones..."
t0=$(now_ms)
"$ENVPOD" destroy "${ENVPOD_NAMES[@]}" >/dev/null 2>&1 || true
t1=$(now_ms)
ENVPOD_DESTROY_MS=$(( t1 - t0 ))
dim "    Done: $(fmt_ms $ENVPOD_DESTROY_MS)"

# Envpod gc (clean up stale iptables rules left by fast destroy)
dim "  Running envpod gc (iptables cleanup)..."
t0=$(now_ms)
"$ENVPOD" gc >/dev/null 2>&1 || true
t1=$(now_ms)
ENVPOD_GC_MS=$(( t1 - t0 ))
dim "    Done: $(fmt_ms $ENVPOD_GC_MS)"

ENVPOD_DESTROY_PLUS_GC_MS=$(( ENVPOD_DESTROY_MS + ENVPOD_GC_MS ))

# Clean up base
"$ENVPOD" destroy scale-base --base >/dev/null 2>&1 || true

# ===========================================================================
# Results
# ===========================================================================
DOCKER_VER=$(docker --version | sed 's/Docker version //' | sed 's/,.*//')
PODMAN_VER=$(podman --version | sed 's/podman version //')

echo ""
info "═══════════════════════════════════════════════════════════════════════"
info "  Scale Benchmark — $COUNT instances"
info "═══════════════════════════════════════════════════════════════════════"
dim "  Docker $DOCKER_VER, Podman $PODMAN_VER, envpod (native Linux backend)"
echo ""

# --- Creation ---
info "  Create $COUNT instances"
echo ""
printf "  ${BOLD}%-20s %12s %12s${RESET}\n" "RUNTIME" "TOTAL" "PER INSTANCE"
printf "  %-20s %12s %12s\n" "────────────────────" "────────────" "────────────"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Docker" "$(fmt_ms $DOCKER_CREATE_MS)" "$(fmt_ms $DOCKER_PER_MS)"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Podman" "$(fmt_ms $PODMAN_CREATE_MS)" "$(fmt_ms $PODMAN_PER_MS)"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Envpod (clone)" "$(fmt_ms $ENVPOD_CREATE_MS)" "$(fmt_ms $ENVPOD_PER_MS)"

if (( DOCKER_CREATE_MS > ENVPOD_CREATE_MS )); then
    SPEEDUP_D=$(echo "scale=1; $DOCKER_CREATE_MS / $ENVPOD_CREATE_MS" | bc)
    SPEEDUP_P=$(echo "scale=1; $PODMAN_CREATE_MS / $ENVPOD_CREATE_MS" | bc)
    printf "\n  ${GREEN}Envpod is ${SPEEDUP_D}x faster than Docker, ${SPEEDUP_P}x faster than Podman${RESET}\n"
fi
echo ""

# --- Disk ---
info "  Total disk overhead ($COUNT instances)"
echo ""
printf "  ${BOLD}%-20s %12s${RESET}\n" "RUNTIME" "TOTAL"
printf "  %-20s %12s\n" "────────────────────" "────────────"
printf "  ${CYAN}%-20s${RESET} %12s\n" "Docker" "$(fmt_size $DOCKER_DISK)"
printf "  ${CYAN}%-20s${RESET} %12s\n" "Podman" "$(fmt_size $PODMAN_DISK)"
printf "  ${CYAN}%-20s${RESET} %12s\n" "Envpod" "$(fmt_size $ENVPOD_DISK)"
echo ""

# --- Run all ---
info "  Run /bin/true in all $COUNT instances"
echo ""
printf "  ${BOLD}%-20s %12s %12s${RESET}\n" "RUNTIME" "TOTAL" "PER INSTANCE"
printf "  %-20s %12s %12s\n" "────────────────────" "────────────" "────────────"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Docker" "$(fmt_ms $DOCKER_RUN_ALL_MS)" "$(fmt_ms $DOCKER_RUN_PER_MS)"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Podman" "$(fmt_ms $PODMAN_RUN_ALL_MS)" "$(fmt_ms $PODMAN_RUN_PER_MS)"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Envpod" "$(fmt_ms $ENVPOD_RUN_ALL_MS)" "$(fmt_ms $ENVPOD_RUN_PER_MS)"
echo ""
dim "  Note: envpod first run triggers deferred network setup (cgroup + netns)."
echo ""

# --- Cleanup ---
info "  Destroy $COUNT instances"
echo ""
printf "  ${BOLD}%-20s %12s %12s${RESET}\n" "RUNTIME" "DESTROY" "DESTROY + GC"
printf "  %-20s %12s %12s\n" "────────────────────" "────────────" "────────────"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Docker" "$(fmt_ms $DOCKER_DESTROY_MS)" "—"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Podman" "$(fmt_ms $PODMAN_DESTROY_MS)" "—"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Envpod" "$(fmt_ms $ENVPOD_DESTROY_MS)" "$(fmt_ms $ENVPOD_DESTROY_PLUS_GC_MS)"
echo ""
dim "  Envpod destroy defers iptables cleanup for speed. Run 'envpod gc' to"
dim "  clean up stale rules ($(fmt_ms $ENVPOD_GC_MS) for $COUNT pods). Dead rules are harmless — they"
dim "  reference non-existent interfaces and never match traffic."
echo ""

# --- Full lifecycle ---
DOCKER_TOTAL=$(( DOCKER_CREATE_MS + DOCKER_RUN_ALL_MS + DOCKER_DESTROY_MS ))
PODMAN_TOTAL=$(( PODMAN_CREATE_MS + PODMAN_RUN_ALL_MS + PODMAN_DESTROY_MS ))
ENVPOD_TOTAL=$(( ENVPOD_CREATE_MS + ENVPOD_RUN_ALL_MS + ENVPOD_DESTROY_MS ))
ENVPOD_TOTAL_GC=$(( ENVPOD_CREATE_MS + ENVPOD_RUN_ALL_MS + ENVPOD_DESTROY_PLUS_GC_MS ))

info "  Full lifecycle: create + run + destroy $COUNT instances"
echo ""
printf "  ${BOLD}%-20s %12s %12s${RESET}\n" "RUNTIME" "TOTAL" "PER INSTANCE"
printf "  %-20s %12s %12s\n" "────────────────────" "────────────" "────────────"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Docker" "$(fmt_ms $DOCKER_TOTAL)" "$(fmt_ms $(( DOCKER_TOTAL / COUNT )))"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Podman" "$(fmt_ms $PODMAN_TOTAL)" "$(fmt_ms $(( PODMAN_TOTAL / COUNT )))"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Envpod" "$(fmt_ms $ENVPOD_TOTAL)" "$(fmt_ms $(( ENVPOD_TOTAL / COUNT )))"
printf "  ${CYAN}%-20s${RESET} %12s %12s\n" "Envpod (with gc)" "$(fmt_ms $ENVPOD_TOTAL_GC)" "$(fmt_ms $(( ENVPOD_TOTAL_GC / COUNT )))"

if (( DOCKER_TOTAL > ENVPOD_TOTAL )); then
    LIFE_D=$(echo "scale=1; $DOCKER_TOTAL / $ENVPOD_TOTAL" | bc)
    LIFE_P=$(echo "scale=1; $PODMAN_TOTAL / $ENVPOD_TOTAL" | bc)
    printf "\n  ${GREEN}Envpod full lifecycle is ${LIFE_D}x faster than Docker, ${LIFE_P}x faster than Podman${RESET}\n"
fi
if (( DOCKER_TOTAL > ENVPOD_TOTAL_GC )); then
    LIFE_D_GC=$(echo "scale=1; $DOCKER_TOTAL / $ENVPOD_TOTAL_GC" | bc)
    printf "  ${GREEN}(${LIFE_D_GC}x faster than Docker even including gc)${RESET}\n"
fi
echo ""