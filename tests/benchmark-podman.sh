#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Podman vs Docker vs Envpod comparison benchmark.
# Requires: sudo, Podman, Docker, envpod release build or installed binary.
#
# Usage:
#   sudo ./tests/benchmark-podman.sh              # default: 10 iterations
#   sudo ./tests/benchmark-podman.sh 50           # 50 iterations
#
# Apples-to-apples comparison:
#   - "docker run --rm" creates a container from an image, runs, destroys.
#   - "podman run --rm" creates a container from an image, runs, destroys.
#   - "envpod clone + run + destroy" creates a pod from a base, runs, destroys.
#   All start from a cached base (container image / envpod base pod).

set -euo pipefail

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
ITERATIONS="${1:-10}"

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

# ---------------------------------------------------------------------------
# Require root + Docker + Podman
# ---------------------------------------------------------------------------
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
# Detect NVIDIA GPU
# ---------------------------------------------------------------------------
HAS_GPU=false
if command -v nvidia-smi &>/dev/null && nvidia-smi --query-gpu=name --format=csv,noheader &>/dev/null; then
    HAS_GPU=true
    GPU_NAME=$(nvidia-smi --query-gpu=name --format=csv,noheader | head -1)
    GPU_COUNT=$(nvidia-smi --query-gpu=name --format=csv,noheader | wc -l)
fi

# ---------------------------------------------------------------------------
# Container images — pull if needed
# ---------------------------------------------------------------------------
DOCKER_IMG="ubuntu:24.04"
PODMAN_IMG="docker.io/library/ubuntu:24.04"
DOCKER_GPU_IMG="nvidia/cuda:12.0.0-base-ubuntu22.04"
PODMAN_GPU_IMG="docker.io/nvidia/cuda:12.0.0-base-ubuntu22.04"

echo ""
info "Preparing container images..."

# Docker
if ! docker image inspect "$DOCKER_IMG" &>/dev/null; then
    dim "  Pulling $DOCKER_IMG (Docker)..."
    docker pull "$DOCKER_IMG" >/dev/null 2>&1
fi
dim "  Docker $DOCKER_IMG: ready"

# Podman
if ! podman image inspect "$PODMAN_IMG" &>/dev/null; then
    dim "  Pulling $PODMAN_IMG (Podman)..."
    podman pull "$PODMAN_IMG" >/dev/null 2>&1
fi
dim "  Podman $PODMAN_IMG: ready"

if $HAS_GPU; then
    if ! docker image inspect "$DOCKER_GPU_IMG" &>/dev/null; then
        dim "  Pulling $DOCKER_GPU_IMG (Docker)..."
        docker pull "$DOCKER_GPU_IMG" >/dev/null 2>&1
    fi
    dim "  Docker $DOCKER_GPU_IMG: ready"
    if ! podman image inspect "$PODMAN_GPU_IMG" &>/dev/null; then
        dim "  Pulling $PODMAN_GPU_IMG (Podman)..."
        podman pull "$PODMAN_GPU_IMG" >/dev/null 2>&1
    fi
    dim "  Podman $PODMAN_GPU_IMG: ready"
fi
echo ""

# ---------------------------------------------------------------------------
# Envpod state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-podman-bench-XXXXXX)
export ENVPOD_DIR="$BENCH_DIR"
trap 'rm -rf "$BENCH_DIR"' EXIT

# ---------------------------------------------------------------------------
# Envpod pod configs
# ---------------------------------------------------------------------------
POD_YAML="$BENCH_DIR/pod.yaml"
cat > "$POD_YAML" << 'YAML'
name: bench-pod
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
name: bench-gpu-pod
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

# ---------------------------------------------------------------------------
# Timing helper
# ---------------------------------------------------------------------------
time_ms() {
    local start end
    start=$(date +%s%N)
    "$@" >/dev/null 2>&1
    end=$(date +%s%N)
    echo $(( (end - start) / 1000000 ))
}

# ---------------------------------------------------------------------------
# Stats helpers
# ---------------------------------------------------------------------------
calc_stats() {
    local -n _arr=$1
    local sum=0 min=999999 max=0 count=${#_arr[@]}

    for v in "${_arr[@]}"; do
        sum=$((sum + v))
        (( v < min )) && min=$v
        (( v > max )) && max=$v
    done

    local avg=$((sum / count))

    local sorted
    sorted=($(printf '%s\n' "${_arr[@]}" | sort -n))
    local mid=$((count / 2))
    local median
    if (( count % 2 == 0 )); then
        median=$(( (sorted[mid-1] + sorted[mid]) / 2 ))
    else
        median=${sorted[$mid]}
    fi

    local p95_idx=$(( (count * 95 + 99) / 100 - 1 ))
    (( p95_idx >= count )) && p95_idx=$((count - 1))
    local p95=${sorted[$p95_idx]}

    echo "$avg $median $min $max $p95"
}

fmt_ms() {
    local ms=$1
    if (( ms >= 1000 )); then
        printf "%d.%03ds" $((ms / 1000)) $((ms % 1000))
    else
        printf "%dms" "$ms"
    fi
}

diff_str() {
    local envpod_med=$1 other_med=$2
    local diff=$((envpod_med - other_med))
    if (( diff < 0 )); then
        printf "${GREEN}$(fmt_ms $((diff * -1))) faster${RESET}"
    elif (( diff == 0 )); then
        printf "same"
    else
        printf "${RED}+$(fmt_ms $diff)${RESET}"
    fi
}

# ---------------------------------------------------------------------------
# Header
# ---------------------------------------------------------------------------
DOCKER_VER=$(docker --version | sed 's/Docker version //' | sed 's/,.*//')
PODMAN_VER=$(podman --version | sed 's/podman version //')

info "envpod vs Docker vs Podman benchmark"
dim "  envpod:     $ENVPOD"
dim "  docker:     $DOCKER_VER"
dim "  podman:     $PODMAN_VER"
dim "  iterations: $ITERATIONS"
if $HAS_GPU; then
    dim "  GPU:        $GPU_NAME x$GPU_COUNT"
fi
dim "  state dir:  $BENCH_DIR"
echo ""

# ---------------------------------------------------------------------------
# Warmup
# ---------------------------------------------------------------------------
info "Warmup..."
docker run --rm "$DOCKER_IMG" /bin/true >/dev/null 2>&1
podman run --rm "$PODMAN_IMG" /bin/true >/dev/null 2>&1
"$ENVPOD" init warmup -c "$POD_YAML" >/dev/null 2>&1
"$ENVPOD" run warmup --root -- /bin/true >/dev/null 2>&1
"$ENVPOD" destroy warmup --base >/dev/null 2>&1
if $HAS_GPU; then
    docker run --rm --gpus all "$DOCKER_GPU_IMG" nvidia-smi >/dev/null 2>&1
    podman run --rm --device nvidia.com/gpu=all "$PODMAN_GPU_IMG" nvidia-smi >/dev/null 2>&1
    "$ENVPOD" init warmup-gpu -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run warmup-gpu --root -- nvidia-smi >/dev/null 2>&1
    "$ENVPOD" destroy warmup-gpu --base >/dev/null 2>&1
fi
echo ""

# ===========================================================================
# Setup: create envpod base pod + long-running containers for warm tests
# ===========================================================================
info "Creating base instances for cloning/exec..."
"$ENVPOD" init bench-source -c "$POD_YAML" >/dev/null 2>&1

DOCKER_EXEC_CID=$(docker run -d "$DOCKER_IMG" sleep 3600)
PODMAN_EXEC_CID=$(podman run -d "$PODMAN_IMG" sleep 3600)
"$ENVPOD" init bench-persistent -c "$POD_YAML" >/dev/null 2>&1
"$ENVPOD" run bench-persistent --root -- /bin/true >/dev/null 2>&1

if $HAS_GPU; then
    "$ENVPOD" init bench-gpu-source -c "$GPU_YAML" >/dev/null 2>&1
    DOCKER_GPU_EXEC_CID=$(docker run -d --gpus all "$DOCKER_GPU_IMG" sleep 3600)
    PODMAN_GPU_EXEC_CID=$(podman run -d --device nvidia.com/gpu=all "$PODMAN_GPU_IMG" sleep 3600)
    "$ENVPOD" init bench-gpu-persistent -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run bench-gpu-persistent --root -- /bin/true >/dev/null 2>&1
fi
echo ""

# ===========================================================================
# TEST 1: Fresh instance — run /bin/true
# ===========================================================================
info "Test 1: Fresh instance — run /bin/true"
dim "  Docker:  docker run --rm"
dim "  Podman:  podman run --rm"
dim "  Envpod:  clone + run + destroy"
echo ""

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_fresh_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker run --rm "$DOCKER_IMG" /bin/true)
    docker_fresh_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Podman ($ITERATIONS iterations)..."
declare -a podman_fresh_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms podman run --rm "$PODMAN_IMG" /bin/true)
    podman_fresh_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Envpod ($ITERATIONS iterations)..."
declare -a envpod_fresh_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-fresh-$i"
    start=$(date +%s%N)
    "$ENVPOD" clone bench-source "$pod_name" >/dev/null 2>&1
    "$ENVPOD" run "$pod_name" --root -- /bin/true >/dev/null 2>&1
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    envpod_fresh_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

# ===========================================================================
# TEST 2: Warm run — exec/run in existing instance
# ===========================================================================
info "Test 2: Warm run — /bin/true in existing container/pod"
dim "  Docker:  docker exec"
dim "  Podman:  podman exec"
dim "  Envpod:  envpod run"
echo ""

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_warm_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker exec "$DOCKER_EXEC_CID" /bin/true)
    docker_warm_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Podman ($ITERATIONS iterations)..."
declare -a podman_warm_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms podman exec "$PODMAN_EXEC_CID" /bin/true)
    podman_warm_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Envpod ($ITERATIONS iterations)..."
declare -a envpod_warm_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-persistent --root -- /bin/true)
    envpod_warm_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

# ===========================================================================
# TEST 3: File I/O — write + read 1MB (fresh instance)
# ===========================================================================
info "Test 3: File I/O — write + read 1MB (fresh instance)"
dim "  Docker:  docker run --rm"
dim "  Podman:  podman run --rm"
dim "  Envpod:  clone + run + destroy"
echo ""

FILE_CMD='/bin/sh -c "dd if=/dev/zero of=/tmp/testfile bs=1M count=1 2>/dev/null && cat /tmp/testfile >/dev/null"'

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_file_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker run --rm "$DOCKER_IMG" /bin/sh -c "dd if=/dev/zero of=/tmp/testfile bs=1M count=1 2>/dev/null && cat /tmp/testfile >/dev/null")
    docker_file_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Podman ($ITERATIONS iterations)..."
declare -a podman_file_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms podman run --rm "$PODMAN_IMG" /bin/sh -c "dd if=/dev/zero of=/tmp/testfile bs=1M count=1 2>/dev/null && cat /tmp/testfile >/dev/null")
    podman_file_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

info "  Envpod ($ITERATIONS iterations)..."
declare -a envpod_file_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-file-$i"
    start=$(date +%s%N)
    "$ENVPOD" clone bench-source "$pod_name" >/dev/null 2>&1
    "$ENVPOD" run "$pod_name" --root -- /bin/sh -c "dd if=/dev/zero of=/tmp/testfile bs=1M count=1 2>/dev/null && cat /tmp/testfile >/dev/null" >/dev/null 2>&1
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    envpod_file_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
echo ""

# ===========================================================================
# TEST 4: GPU — nvidia-smi (if available)
# ===========================================================================
declare -a docker_gpu_fresh_times=()
declare -a podman_gpu_fresh_times=()
declare -a envpod_gpu_fresh_times=()
declare -a docker_gpu_warm_times=()
declare -a podman_gpu_warm_times=()
declare -a envpod_gpu_warm_times=()

if $HAS_GPU; then
    GPU_CMD="nvidia-smi --query-gpu=name,memory.used,temperature.gpu --format=csv,noheader"

    info "Test 4a: GPU fresh — nvidia-smi (fresh instance)"
    dim "  Docker:  docker run --rm --gpus all"
    dim "  Podman:  podman run --rm --device nvidia.com/gpu=all"
    dim "  Envpod:  clone + run + destroy"
    echo ""

    info "  Docker ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms docker run --rm --gpus all "$DOCKER_GPU_IMG" $GPU_CMD)
        docker_gpu_fresh_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""

    info "  Podman ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms podman run --rm --device nvidia.com/gpu=all "$PODMAN_GPU_IMG" $GPU_CMD)
        podman_gpu_fresh_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""

    info "  Envpod ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        pod_name="bench-gpuf-$i"
        start=$(date +%s%N)
        "$ENVPOD" clone bench-gpu-source "$pod_name" >/dev/null 2>&1
        "$ENVPOD" run "$pod_name" --root -- $GPU_CMD >/dev/null 2>&1
        "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1
        end=$(date +%s%N)
        ms=$(( (end - start) / 1000000 ))
        envpod_gpu_fresh_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""

    info "Test 4b: GPU warm — nvidia-smi (existing container/pod)"
    dim "  Docker:  docker exec"
    dim "  Podman:  podman exec"
    dim "  Envpod:  envpod run"
    echo ""

    info "  Docker ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms docker exec "$DOCKER_GPU_EXEC_CID" $GPU_CMD)
        docker_gpu_warm_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""

    info "  Podman ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms podman exec "$PODMAN_GPU_EXEC_CID" $GPU_CMD)
        podman_gpu_warm_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""

    info "  Envpod ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms "$ENVPOD" run bench-gpu-persistent --root -- $GPU_CMD)
        envpod_gpu_warm_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    echo ""
fi

# ===========================================================================
# Cleanup
# ===========================================================================
docker rm -f "$DOCKER_EXEC_CID" >/dev/null 2>&1 || true
podman rm -f "$PODMAN_EXEC_CID" >/dev/null 2>&1 || true
"$ENVPOD" destroy bench-persistent --base >/dev/null 2>&1 || true
"$ENVPOD" destroy bench-source --base >/dev/null 2>&1 || true
if $HAS_GPU; then
    docker rm -f "$DOCKER_GPU_EXEC_CID" >/dev/null 2>&1 || true
    podman rm -f "$PODMAN_GPU_EXEC_CID" >/dev/null 2>&1 || true
    "$ENVPOD" destroy bench-gpu-persistent --base >/dev/null 2>&1 || true
    "$ENVPOD" destroy bench-gpu-source --base >/dev/null 2>&1 || true
fi

# ===========================================================================
# Results
# ===========================================================================
read dfr_avg dfr_med dfr_min dfr_max dfr_p95 <<< "$(calc_stats docker_fresh_times)"
read pfr_avg pfr_med pfr_min pfr_max pfr_p95 <<< "$(calc_stats podman_fresh_times)"
read efr_avg efr_med efr_min efr_max efr_p95 <<< "$(calc_stats envpod_fresh_times)"
read dw_avg dw_med dw_min dw_max dw_p95 <<< "$(calc_stats docker_warm_times)"
read pw_avg pw_med pw_min pw_max pw_p95 <<< "$(calc_stats podman_warm_times)"
read ew_avg ew_med ew_min ew_max ew_p95 <<< "$(calc_stats envpod_warm_times)"
read df_avg df_med df_min df_max df_p95 <<< "$(calc_stats docker_file_times)"
read pf_avg pf_med pf_min pf_max pf_p95 <<< "$(calc_stats podman_file_times)"
read ef_avg ef_med ef_min ef_max ef_p95 <<< "$(calc_stats envpod_file_times)"

if $HAS_GPU; then
    read dgf_avg dgf_med dgf_min dgf_max dgf_p95 <<< "$(calc_stats docker_gpu_fresh_times)"
    read pgf_avg pgf_med pgf_min pgf_max pgf_p95 <<< "$(calc_stats podman_gpu_fresh_times)"
    read egf_avg egf_med egf_min egf_max egf_p95 <<< "$(calc_stats envpod_gpu_fresh_times)"
    read dgw_avg dgw_med dgw_min dgw_max dgw_p95 <<< "$(calc_stats docker_gpu_warm_times)"
    read pgw_avg pgw_med pgw_min pgw_max pgw_p95 <<< "$(calc_stats podman_gpu_warm_times)"
    read egw_avg egw_med egw_min egw_max egw_p95 <<< "$(calc_stats envpod_gpu_warm_times)"
fi

echo ""
info "═══════════════════════════════════════════════════════════════════════"
info "  Results ($ITERATIONS iterations)"
info "═══════════════════════════════════════════════════════════════════════"
dim "  Docker $DOCKER_VER vs Podman $PODMAN_VER vs envpod (native Linux backend)"
if $HAS_GPU; then
    dim "  GPU: $GPU_NAME x$GPU_COUNT"
fi
echo ""

printf "  ${BOLD}%-36s %10s %10s %10s %14s %14s${RESET}\n" \
    "TEST" "DOCKER" "PODMAN" "ENVPOD" "vs DOCKER" "vs PODMAN"
printf "  %-36s %10s %10s %10s %14s %14s\n" \
    "────────────────────────────────────" "──────────" "──────────" "──────────" "──────────────" "──────────────"

# Fresh instance
printf "  ${CYAN}%-36s${RESET} %10s %10s %10s %b %b\n" \
    "fresh: run /bin/true" \
    "$(fmt_ms $dfr_med)" "$(fmt_ms $pfr_med)" "$(fmt_ms $efr_med)" \
    "$(diff_str $efr_med $dfr_med)" "$(diff_str $efr_med $pfr_med)"

# Warm run
printf "  ${CYAN}%-36s${RESET} %10s %10s %10s %b %b\n" \
    "warm: run /bin/true" \
    "$(fmt_ms $dw_med)" "$(fmt_ms $pw_med)" "$(fmt_ms $ew_med)" \
    "$(diff_str $ew_med $dw_med)" "$(diff_str $ew_med $pw_med)"

# File I/O
printf "  ${CYAN}%-36s${RESET} %10s %10s %10s %b %b\n" \
    "fresh: file I/O (write+read 1MB)" \
    "$(fmt_ms $df_med)" "$(fmt_ms $pf_med)" "$(fmt_ms $ef_med)" \
    "$(diff_str $ef_med $df_med)" "$(diff_str $ef_med $pf_med)"

# GPU
if $HAS_GPU; then
    printf "  ${CYAN}%-36s${RESET} %10s %10s %10s %b %b\n" \
        "fresh: GPU nvidia-smi" \
        "$(fmt_ms $dgf_med)" "$(fmt_ms $pgf_med)" "$(fmt_ms $egf_med)" \
        "$(diff_str $egf_med $dgf_med)" "$(diff_str $egf_med $pgf_med)"
    printf "  ${CYAN}%-36s${RESET} %10s %10s %10s %b %b\n" \
        "warm: GPU nvidia-smi" \
        "$(fmt_ms $dgw_med)" "$(fmt_ms $pgw_med)" "$(fmt_ms $egw_med)" \
        "$(diff_str $egw_med $dgw_med)" "$(diff_str $egw_med $pgw_med)"
fi

echo ""
dim "  fresh = create from base + run + destroy"
dim "  warm  = run in existing instance"
echo ""
info "  What envpod adds (zero extra cost):"
dim "    + COW filesystem (diff/commit/rollback)"
dim "    + Per-pod DNS filtering with query logging"
dim "    + Action-level audit trail (JSONL)"
dim "    + seccomp-BPF syscall filtering"
dim "    + Credential vault"
dim "    + Remote control (freeze/kill/restrict)"
dim "    + Undo registry"
echo ""