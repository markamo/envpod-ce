#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Docker vs Envpod comparison benchmark.
# Requires: sudo, Docker, envpod release build or installed binary.
#
# Usage:
#   sudo ./tests/benchmark-docker.sh              # default: 10 iterations
#   sudo ./tests/benchmark-docker.sh 50           # 50 iterations
#   sudo ./tests/benchmark-docker.sh 10 --json    # JSON output
#
# Apples-to-apples comparison:
#   - "docker run --rm" creates a container from an image, runs, destroys.
#   - "envpod clone + run + destroy" creates a pod from a base, runs, destroys.
#   Both start from a cached base (Docker image / envpod base pod).

set -euo pipefail

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
ITERATIONS="${1:-10}"
JSON_OUTPUT=false
[[ "${2:-}" == "--json" ]] && JSON_OUTPUT=true

# ---------------------------------------------------------------------------
# Color helpers
# ---------------------------------------------------------------------------
if [ -t 1 ] && ! $JSON_OUTPUT; then
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

info()  { $JSON_OUTPUT || echo -e "${BOLD}$*${RESET}"; }
dim()   { $JSON_OUTPUT || echo -e "${DIM}$*${RESET}"; }
warn()  { $JSON_OUTPUT || echo -e "${YELLOW}$*${RESET}"; }

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
# Require root + Docker
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    echo "Error: must run as root (sudo $0)" >&2
    exit 1
fi

if ! command -v docker &>/dev/null; then
    echo "Error: docker not found." >&2
    exit 1
fi

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
# Docker images — pull if needed
# ---------------------------------------------------------------------------
DOCKER_IMG="ubuntu:24.04"
DOCKER_GPU_IMG="nvidia/cuda:12.0.0-base-ubuntu22.04"

$JSON_OUTPUT || echo ""
info "Preparing Docker images..."
if ! docker image inspect "$DOCKER_IMG" &>/dev/null; then
    dim "  Pulling $DOCKER_IMG..."
    docker pull "$DOCKER_IMG" >/dev/null 2>&1
fi
dim "  $DOCKER_IMG: ready"

if $HAS_GPU; then
    if ! docker image inspect "$DOCKER_GPU_IMG" &>/dev/null; then
        dim "  Pulling $DOCKER_GPU_IMG..."
        docker pull "$DOCKER_GPU_IMG" >/dev/null 2>&1
    fi
    dim "  $DOCKER_GPU_IMG: ready"
fi
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Envpod state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-docker-bench-XXXXXX)
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
    local envpod_med=$1 docker_med=$2
    local diff=$((envpod_med - docker_med))
    if (( diff < 0 )); then
        printf "${GREEN}$(fmt_ms $((diff * -1))) faster${RESET}"
    elif (( diff == 0 )); then
        printf "same"
    else
        printf "+$(fmt_ms $diff)"
    fi
}

# ---------------------------------------------------------------------------
# Header
# ---------------------------------------------------------------------------
DOCKER_VER=$(docker --version | sed 's/Docker version //' | sed 's/,.*//')

info "envpod vs Docker benchmark"
dim "  envpod:     $ENVPOD"
dim "  docker:     $DOCKER_VER"
dim "  iterations: $ITERATIONS"
if $HAS_GPU; then
    dim "  GPU:        $GPU_NAME x$GPU_COUNT"
fi
dim "  state dir:  $BENCH_DIR"
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Warmup
# ---------------------------------------------------------------------------
info "Warmup..."
docker run --rm "$DOCKER_IMG" /bin/true >/dev/null 2>&1
"$ENVPOD" init warmup -c "$POD_YAML" >/dev/null 2>&1
"$ENVPOD" run warmup --root -- /bin/true >/dev/null 2>&1
"$ENVPOD" destroy warmup --base >/dev/null 2>&1
if $HAS_GPU; then
    docker run --rm --gpus all "$DOCKER_GPU_IMG" nvidia-smi >/dev/null 2>&1
    "$ENVPOD" init warmup-gpu -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run warmup-gpu --root -- nvidia-smi >/dev/null 2>&1
    "$ENVPOD" destroy warmup-gpu --base >/dev/null 2>&1
fi
$JSON_OUTPUT || echo ""

# ===========================================================================
# Setup: create envpod base pod for cloning
# ===========================================================================
info "Creating envpod base pod for cloning..."
"$ENVPOD" init bench-source -c "$POD_YAML" >/dev/null 2>&1
if $HAS_GPU; then
    "$ENVPOD" init bench-gpu-source -c "$GPU_YAML" >/dev/null 2>&1
fi
$JSON_OUTPUT || echo ""

# Also create a long-running Docker container for exec comparison
DOCKER_EXEC_CID=$(docker run -d "$DOCKER_IMG" sleep 3600)
# And a persistent envpod pod for run comparison
"$ENVPOD" init bench-persistent -c "$POD_YAML" >/dev/null 2>&1
"$ENVPOD" run bench-persistent --root -- /bin/true >/dev/null 2>&1  # warm up

if $HAS_GPU; then
    DOCKER_GPU_EXEC_CID=$(docker run -d --gpus all "$DOCKER_GPU_IMG" sleep 3600)
    "$ENVPOD" init bench-gpu-persistent -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run bench-gpu-persistent --root -- /bin/true >/dev/null 2>&1
fi

# ===========================================================================
# TEST 1: Fresh instance — docker run --rm vs clone+run+destroy
# ===========================================================================
info "Test 1: Fresh instance — run /bin/true"
dim "  Docker:  docker run --rm (create container from image, run, destroy)"
dim "  Envpod:  clone + run + destroy (create pod from base, run, destroy)"
$JSON_OUTPUT || echo ""

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_fresh_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker run --rm "$DOCKER_IMG" /bin/true)
    docker_fresh_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

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
$JSON_OUTPUT || echo ""

# ===========================================================================
# TEST 2: Warm run — docker exec vs envpod run (reuse existing)
# ===========================================================================
info "Test 2: Warm run — /bin/true in existing container/pod"
dim "  Docker:  docker exec (command in running container)"
dim "  Envpod:  envpod run (command in existing pod)"
$JSON_OUTPUT || echo ""

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_warm_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker exec "$DOCKER_EXEC_CID" /bin/true)
    docker_warm_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

info "  Envpod ($ITERATIONS iterations)..."
declare -a envpod_warm_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-persistent --root -- /bin/true)
    envpod_warm_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ===========================================================================
# TEST 3: File I/O — write + read 1MB inside container/pod
# ===========================================================================
info "Test 3: File I/O — write + read 1MB (fresh instance)"
dim "  Docker:  docker run --rm"
dim "  Envpod:  clone + run + destroy"
$JSON_OUTPUT || echo ""

info "  Docker ($ITERATIONS iterations)..."
declare -a docker_file_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms docker run --rm "$DOCKER_IMG" /bin/sh -c "dd if=/dev/zero of=/tmp/testfile bs=1M count=1 2>/dev/null && cat /tmp/testfile >/dev/null")
    docker_file_times+=("$ms")
    dim "    [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

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
$JSON_OUTPUT || echo ""

# ===========================================================================
# TEST 4: GPU — nvidia-smi (if available)
# ===========================================================================
declare -a docker_gpu_fresh_times=()
declare -a envpod_gpu_fresh_times=()
declare -a docker_gpu_warm_times=()
declare -a envpod_gpu_warm_times=()
if $HAS_GPU; then
    info "Test 4a: GPU fresh — nvidia-smi (fresh instance)"
    dim "  Docker:  docker run --rm --gpus all"
    dim "  Envpod:  clone + run + destroy"
    $JSON_OUTPUT || echo ""

    info "  Docker ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms docker run --rm --gpus all "$DOCKER_GPU_IMG" nvidia-smi --query-gpu=name,memory.used,temperature.gpu --format=csv,noheader)
        docker_gpu_fresh_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""

    info "  Envpod ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        pod_name="bench-gpuf-$i"
        start=$(date +%s%N)
        "$ENVPOD" clone bench-gpu-source "$pod_name" >/dev/null 2>&1
        "$ENVPOD" run "$pod_name" --root -- nvidia-smi --query-gpu=name,memory.used,temperature.gpu --format=csv,noheader >/dev/null 2>&1
        "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1
        end=$(date +%s%N)
        ms=$(( (end - start) / 1000000 ))
        envpod_gpu_fresh_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""

    info "Test 4b: GPU warm — nvidia-smi (existing container/pod)"
    dim "  Docker:  docker exec"
    dim "  Envpod:  envpod run"
    $JSON_OUTPUT || echo ""

    info "  Docker ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms docker exec "$DOCKER_GPU_EXEC_CID" nvidia-smi --query-gpu=name,memory.used,temperature.gpu --format=csv,noheader)
        docker_gpu_warm_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""

    info "  Envpod ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms "$ENVPOD" run bench-gpu-persistent --root -- nvidia-smi --query-gpu=name,memory.used,temperature.gpu --format=csv,noheader)
        envpod_gpu_warm_times+=("$ms")
        dim "    [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""
fi

# ===========================================================================
# Cleanup
# ===========================================================================
docker rm -f "$DOCKER_EXEC_CID" >/dev/null 2>&1 || true
"$ENVPOD" destroy bench-persistent --base >/dev/null 2>&1 || true
"$ENVPOD" destroy bench-source --base >/dev/null 2>&1 || true
if $HAS_GPU; then
    docker rm -f "$DOCKER_GPU_EXEC_CID" >/dev/null 2>&1 || true
    "$ENVPOD" destroy bench-gpu-persistent --base >/dev/null 2>&1 || true
    "$ENVPOD" destroy bench-gpu-source --base >/dev/null 2>&1 || true
fi

# ===========================================================================
# Results
# ===========================================================================
read dfr_avg dfr_med dfr_min dfr_max dfr_p95 <<< "$(calc_stats docker_fresh_times)"
read efr_avg efr_med efr_min efr_max efr_p95 <<< "$(calc_stats envpod_fresh_times)"
read dw_avg dw_med dw_min dw_max dw_p95 <<< "$(calc_stats docker_warm_times)"
read ew_avg ew_med ew_min ew_max ew_p95 <<< "$(calc_stats envpod_warm_times)"
read df_avg df_med df_min df_max df_p95 <<< "$(calc_stats docker_file_times)"
read ef_avg ef_med ef_min ef_max ef_p95 <<< "$(calc_stats envpod_file_times)"

if $HAS_GPU; then
    read dgf_avg dgf_med dgf_min dgf_max dgf_p95 <<< "$(calc_stats docker_gpu_fresh_times)"
    read egf_avg egf_med egf_min egf_max egf_p95 <<< "$(calc_stats envpod_gpu_fresh_times)"
    read dgw_avg dgw_med dgw_min dgw_max dgw_p95 <<< "$(calc_stats docker_gpu_warm_times)"
    read egw_avg egw_med egw_min egw_max egw_p95 <<< "$(calc_stats envpod_gpu_warm_times)"
fi

if $JSON_OUTPUT; then
    cat << ENDJSON
{
  "envpod_binary": "$ENVPOD",
  "docker_version": "$DOCKER_VER",
  "iterations": $ITERATIONS,
  "has_gpu": $HAS_GPU,$(
  $HAS_GPU && echo "
  \"gpu\": \"$GPU_NAME x$GPU_COUNT\","
  )
  "results": {
    "fresh_instance": {
      "docker_run_rm": { "avg_ms": $dfr_avg, "median_ms": $dfr_med, "min_ms": $dfr_min, "max_ms": $dfr_max, "p95_ms": $dfr_p95 },
      "envpod_clone_run_destroy": { "avg_ms": $efr_avg, "median_ms": $efr_med, "min_ms": $efr_min, "max_ms": $efr_max, "p95_ms": $efr_p95 }
    },
    "warm_run": {
      "docker_exec": { "avg_ms": $dw_avg, "median_ms": $dw_med, "min_ms": $dw_min, "max_ms": $dw_max, "p95_ms": $dw_p95 },
      "envpod_run": { "avg_ms": $ew_avg, "median_ms": $ew_med, "min_ms": $ew_min, "max_ms": $ew_max, "p95_ms": $ew_p95 }
    },
    "file_io": {
      "docker_run_rm": { "avg_ms": $df_avg, "median_ms": $df_med, "min_ms": $df_min, "max_ms": $df_max, "p95_ms": $df_p95 },
      "envpod_clone_run_destroy": { "avg_ms": $ef_avg, "median_ms": $ef_med, "min_ms": $ef_min, "max_ms": $ef_max, "p95_ms": $ef_p95 }
    }$(
    $HAS_GPU && echo ",
    \"gpu_fresh\": {
      \"docker_run_rm\": { \"avg_ms\": $dgf_avg, \"median_ms\": $dgf_med, \"min_ms\": $dgf_min, \"max_ms\": $dgf_max, \"p95_ms\": $dgf_p95 },
      \"envpod_clone_run_destroy\": { \"avg_ms\": $egf_avg, \"median_ms\": $egf_med, \"min_ms\": $egf_min, \"max_ms\": $egf_max, \"p95_ms\": $egf_p95 }
    },
    \"gpu_warm\": {
      \"docker_exec\": { \"avg_ms\": $dgw_avg, \"median_ms\": $dgw_med, \"min_ms\": $dgw_min, \"max_ms\": $dgw_max, \"p95_ms\": $dgw_p95 },
      \"envpod_run\": { \"avg_ms\": $egw_avg, \"median_ms\": $egw_med, \"min_ms\": $egw_min, \"max_ms\": $egw_max, \"p95_ms\": $egw_p95 }
    }"
    )
  }
}
ENDJSON
else
    $JSON_OUTPUT || echo ""
    info "═══════════════════════════════════════════════════════════════"
    info "  Results ($ITERATIONS iterations)"
    info "═══════════════════════════════════════════════════════════════"
    dim "  Docker $DOCKER_VER vs envpod (native Linux backend)"
    if $HAS_GPU; then
        dim "  GPU: $GPU_NAME x$GPU_COUNT"
    fi
    echo ""

    printf "  ${BOLD}%-40s %10s %10s %10s${RESET}\n" "TEST" "DOCKER" "ENVPOD" "DIFF"
    printf "  %-40s %10s %10s %10s\n" "────────────────────────────────────────" "──────────" "──────────" "──────────"

    # Fresh instance
    printf "  ${CYAN}%-40s${RESET} %10s %10s %b\n" \
        "fresh: run /bin/true" \
        "$(fmt_ms $dfr_med)" "$(fmt_ms $efr_med)" "$(diff_str $efr_med $dfr_med)"

    # Warm run
    printf "  ${CYAN}%-40s${RESET} %10s %10s %b\n" \
        "warm: run /bin/true" \
        "$(fmt_ms $dw_med)" "$(fmt_ms $ew_med)" "$(diff_str $ew_med $dw_med)"

    # File I/O
    printf "  ${CYAN}%-40s${RESET} %10s %10s %b\n" \
        "fresh: file I/O (write+read 1MB)" \
        "$(fmt_ms $df_med)" "$(fmt_ms $ef_med)" "$(diff_str $ef_med $df_med)"

    # GPU
    if $HAS_GPU; then
        printf "  ${CYAN}%-40s${RESET} %10s %10s %b\n" \
            "fresh: GPU nvidia-smi" \
            "$(fmt_ms $dgf_med)" "$(fmt_ms $egf_med)" "$(diff_str $egf_med $dgf_med)"
        printf "  ${CYAN}%-40s${RESET} %10s %10s %b\n" \
            "warm: GPU nvidia-smi" \
            "$(fmt_ms $dgw_med)" "$(fmt_ms $egw_med)" "$(diff_str $egw_med $dgw_med)"
    fi

    echo ""
    dim "  fresh = create from base + run + destroy  (docker run --rm / envpod clone+run+destroy)"
    dim "  warm  = run in existing instance           (docker exec / envpod run)"
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
fi