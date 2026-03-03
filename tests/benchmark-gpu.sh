#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# GPU benchmark script for envpod — measures GPU passthrough overhead.
# Requires: sudo, NVIDIA GPU, nvidia-smi, a release build or installed envpod binary.
#
# Usage:
#   sudo ./tests/benchmark-gpu.sh              # default: 10 iterations
#   sudo ./tests/benchmark-gpu.sh 50           # 50 iterations
#   sudo ./tests/benchmark-gpu.sh 10 --json    # JSON output
#
# Measures:
#   1. nvidia-smi on host (baseline)
#   2. nvidia-smi inside pod with GPU passthrough (overhead)
#   3. envpod init with GPU vs without GPU (init overhead)
#   4. CUDA device query inside pod (if nvidia-cuda-toolkit available)
#
# Shows the real-world overhead of GPU passthrough through namespaces.

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
# Locate binary
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
# Require root + NVIDIA GPU
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    echo "Error: must run as root (sudo $0)" >&2
    exit 1
fi

if ! command -v nvidia-smi &>/dev/null; then
    echo "Error: nvidia-smi not found. NVIDIA GPU required for this benchmark." >&2
    exit 1
fi

# Verify GPU is accessible
if ! nvidia-smi --query-gpu=name --format=csv,noheader &>/dev/null; then
    echo "Error: nvidia-smi failed. Check NVIDIA driver." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# GPU info
# ---------------------------------------------------------------------------
GPU_NAME=$(nvidia-smi --query-gpu=name --format=csv,noheader | head -1)
GPU_COUNT=$(nvidia-smi --query-gpu=name --format=csv,noheader | wc -l)
DRIVER_VER=$(nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -1)
GPU_MEM=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader | head -1)

# Check for CUDA
HAS_CUDA=false
CUDA_DEVICE_QUERY=""
if command -v /usr/local/cuda/extras/demo_suite/deviceQuery &>/dev/null; then
    HAS_CUDA=true
    CUDA_DEVICE_QUERY="/usr/local/cuda/extras/demo_suite/deviceQuery"
elif command -v deviceQuery &>/dev/null; then
    HAS_CUDA=true
    CUDA_DEVICE_QUERY="deviceQuery"
fi

# Check for python3 + torch (for a real GPU compute test)
HAS_TORCH=false
if python3 -c "import torch; torch.cuda.is_available()" &>/dev/null; then
    HAS_TORCH=true
fi

# ---------------------------------------------------------------------------
# Use temp state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-gpu-bench-XXXXXX)
export ENVPOD_DIR="$BENCH_DIR"
trap 'rm -rf "$BENCH_DIR"' EXIT

# ---------------------------------------------------------------------------
# Timing helper — returns milliseconds
# ---------------------------------------------------------------------------
time_ms() {
    local start end
    start=$(date +%s%N)
    "$@" >/dev/null 2>&1
    end=$(date +%s%N)
    echo $(( (end - start) / 1000000 ))
}

# ---------------------------------------------------------------------------
# Stats helpers (same as benchmark.sh)
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

# ---------------------------------------------------------------------------
# Pod configs — one with GPU, one without
# ---------------------------------------------------------------------------
GPU_YAML="$BENCH_DIR/gpu-pod.yaml"
cat > "$GPU_YAML" << 'YAML'
name: bench-gpu
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

NOGPU_YAML="$BENCH_DIR/nogpu-pod.yaml"
cat > "$NOGPU_YAML" << 'YAML'
name: bench-nogpu
type: standard
backend: native
network:
  mode: Isolated
  dns:
    mode: Whitelist
    allow: []
devices:
  gpu: false
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
# Header
# ---------------------------------------------------------------------------
$JSON_OUTPUT || echo ""
info "envpod GPU benchmark"
dim "  binary:     $ENVPOD"
dim "  iterations: $ITERATIONS"
dim "  GPU:        $GPU_NAME ($GPU_MEM)"
dim "  GPUs:       $GPU_COUNT"
dim "  driver:     $DRIVER_VER"
dim "  CUDA:       $($HAS_CUDA && echo "available" || echo "not found")"
dim "  PyTorch:    $($HAS_TORCH && echo "available" || echo "not found")"
dim "  state dir:  $BENCH_DIR"
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Warmup
# ---------------------------------------------------------------------------
info "Warmup run..."
nvidia-smi --query-gpu=name --format=csv,noheader >/dev/null 2>&1
"$ENVPOD" init warmup-gpu -c "$GPU_YAML" >/dev/null 2>&1
"$ENVPOD" run warmup-gpu --root -- nvidia-smi >/dev/null 2>&1
"$ENVPOD" destroy warmup-gpu >/dev/null 2>&1
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 1. Baseline: nvidia-smi on host
# ---------------------------------------------------------------------------
info "Benchmarking 'nvidia-smi' on host ($ITERATIONS iterations)..."
declare -a host_smi_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms nvidia-smi --query-gpu=name,memory.used,temperature.gpu,utilization.gpu --format=csv,noheader)
    host_smi_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 2. nvidia-smi inside pod (GPU passthrough overhead)
# ---------------------------------------------------------------------------
info "Benchmarking 'nvidia-smi' inside pod ($ITERATIONS iterations)..."
"$ENVPOD" init bench-smi -c "$GPU_YAML" >/dev/null 2>&1
# First run warms up (cold start)
"$ENVPOD" run bench-smi --root -- /bin/true >/dev/null 2>&1
declare -a pod_smi_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-smi --root -- nvidia-smi --query-gpu=name,memory.used,temperature.gpu,utilization.gpu --format=csv,noheader)
    pod_smi_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 3. nvidia-smi --list-gpus inside pod (device enumeration)
# ---------------------------------------------------------------------------
info "Benchmarking 'nvidia-smi --list-gpus' inside pod ($ITERATIONS iterations)..."
declare -a pod_list_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-smi --root -- nvidia-smi --list-gpus)
    pod_list_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
"$ENVPOD" destroy bench-smi >/dev/null 2>&1 || true
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 4. Init overhead: GPU-enabled vs no-GPU
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod init' with GPU ($ITERATIONS iterations)..."
declare -a init_gpu_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-ginit-$i"
    ms=$(time_ms "$ENVPOD" init "$pod_name" -c "$GPU_YAML")
    init_gpu_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

info "Benchmarking 'envpod init' without GPU ($ITERATIONS iterations)..."
declare -a init_nogpu_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-ninit-$i"
    ms=$(time_ms "$ENVPOD" init "$pod_name" -c "$NOGPU_YAML")
    init_nogpu_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 5. GPU run vs non-GPU run (namespace entry overhead)
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod run' with GPU vs without ($ITERATIONS iterations)..."
"$ENVPOD" init bench-grun -c "$GPU_YAML" >/dev/null 2>&1
"$ENVPOD" init bench-nrun -c "$NOGPU_YAML" >/dev/null 2>&1
# Warm up both
"$ENVPOD" run bench-grun --root -- /bin/true >/dev/null 2>&1
"$ENVPOD" run bench-nrun --root -- /bin/true >/dev/null 2>&1

declare -a run_gpu_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-grun --root -- /bin/true)
    run_gpu_times+=("$ms")
done
dim "  GPU pod:    done"

declare -a run_nogpu_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-nrun --root -- /bin/true)
    run_nogpu_times+=("$ms")
done
dim "  no-GPU pod: done"
"$ENVPOD" destroy bench-grun >/dev/null 2>&1 || true
"$ENVPOD" destroy bench-nrun >/dev/null 2>&1 || true
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# 6. CUDA device query (if available)
# ---------------------------------------------------------------------------
declare -a cuda_host_times=()
declare -a cuda_pod_times=()
if $HAS_CUDA; then
    info "Benchmarking CUDA deviceQuery on host ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms $CUDA_DEVICE_QUERY)
        cuda_host_times+=("$ms")
        dim "  [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""

    info "Benchmarking CUDA deviceQuery inside pod ($ITERATIONS iterations)..."
    "$ENVPOD" init bench-cuda -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run bench-cuda --root -- /bin/true >/dev/null 2>&1
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms "$ENVPOD" run bench-cuda --root -- $CUDA_DEVICE_QUERY)
        cuda_pod_times+=("$ms")
        dim "  [$i/$ITERATIONS] ${ms}ms"
    done
    "$ENVPOD" destroy bench-cuda >/dev/null 2>&1 || true
    $JSON_OUTPUT || echo ""
fi

# ---------------------------------------------------------------------------
# 7. PyTorch GPU test (if available) — small tensor op
# ---------------------------------------------------------------------------
declare -a torch_host_times=()
declare -a torch_pod_times=()
if $HAS_TORCH; then
    TORCH_SCRIPT='import torch; t = torch.randn(1000, 1000, device="cuda"); r = t @ t; torch.cuda.synchronize()'

    info "Benchmarking PyTorch CUDA matmul on host ($ITERATIONS iterations)..."
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms python3 -c "$TORCH_SCRIPT")
        torch_host_times+=("$ms")
        dim "  [$i/$ITERATIONS] ${ms}ms"
    done
    $JSON_OUTPUT || echo ""

    info "Benchmarking PyTorch CUDA matmul inside pod ($ITERATIONS iterations)..."
    "$ENVPOD" init bench-torch -c "$GPU_YAML" >/dev/null 2>&1
    "$ENVPOD" run bench-torch --root -- /bin/true >/dev/null 2>&1
    for i in $(seq 1 "$ITERATIONS"); do
        ms=$(time_ms "$ENVPOD" run bench-torch --root -- python3 -c "$TORCH_SCRIPT")
        torch_pod_times+=("$ms")
        dim "  [$i/$ITERATIONS] ${ms}ms"
    done
    "$ENVPOD" destroy bench-torch >/dev/null 2>&1 || true
    $JSON_OUTPUT || echo ""
fi

# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------
read hsmi_avg hsmi_med hsmi_min hsmi_max hsmi_p95 <<< "$(calc_stats host_smi_times)"
read psmi_avg psmi_med psmi_min psmi_max psmi_p95 <<< "$(calc_stats pod_smi_times)"
read plist_avg plist_med plist_min plist_max plist_p95 <<< "$(calc_stats pod_list_times)"
read ig_avg ig_med ig_min ig_max ig_p95 <<< "$(calc_stats init_gpu_times)"
read in_avg in_med in_min in_max in_p95 <<< "$(calc_stats init_nogpu_times)"
read rg_avg rg_med rg_min rg_max rg_p95 <<< "$(calc_stats run_gpu_times)"
read rn_avg rn_med rn_min rn_max rn_p95 <<< "$(calc_stats run_nogpu_times)"

if $JSON_OUTPUT; then
    cat << ENDJSON
{
  "binary": "$ENVPOD",
  "iterations": $ITERATIONS,
  "gpu": {
    "name": "$GPU_NAME",
    "count": $GPU_COUNT,
    "driver": "$DRIVER_VER",
    "memory": "$GPU_MEM"
  },
  "results": {
    "nvidia_smi_host": { "avg_ms": $hsmi_avg, "median_ms": $hsmi_med, "min_ms": $hsmi_min, "max_ms": $hsmi_max, "p95_ms": $hsmi_p95 },
    "nvidia_smi_pod": { "avg_ms": $psmi_avg, "median_ms": $psmi_med, "min_ms": $psmi_min, "max_ms": $psmi_max, "p95_ms": $psmi_p95 },
    "nvidia_smi_list_pod": { "avg_ms": $plist_avg, "median_ms": $plist_med, "min_ms": $plist_min, "max_ms": $plist_max, "p95_ms": $plist_p95 },
    "init_gpu": { "avg_ms": $ig_avg, "median_ms": $ig_med, "min_ms": $ig_min, "max_ms": $ig_max, "p95_ms": $ig_p95 },
    "init_no_gpu": { "avg_ms": $in_avg, "median_ms": $in_med, "min_ms": $in_min, "max_ms": $in_max, "p95_ms": $in_p95 },
    "run_gpu": { "avg_ms": $rg_avg, "median_ms": $rg_med, "min_ms": $rg_min, "max_ms": $rg_max, "p95_ms": $rg_p95 },
    "run_no_gpu": { "avg_ms": $rn_avg, "median_ms": $rn_med, "min_ms": $rn_min, "max_ms": $rn_max, "p95_ms": $rn_p95 }$(
    $HAS_CUDA && echo ",
    \"cuda_device_query_host\": { \"avg_ms\": $(calc_stats cuda_host_times | cut -d' ' -f1), \"median_ms\": $(calc_stats cuda_host_times | cut -d' ' -f2) },
    \"cuda_device_query_pod\": { \"avg_ms\": $(calc_stats cuda_pod_times | cut -d' ' -f1), \"median_ms\": $(calc_stats cuda_pod_times | cut -d' ' -f2) }"
    )$(
    $HAS_TORCH && echo ",
    \"torch_matmul_host\": { \"avg_ms\": $(calc_stats torch_host_times | cut -d' ' -f1), \"median_ms\": $(calc_stats torch_host_times | cut -d' ' -f2) },
    \"torch_matmul_pod\": { \"avg_ms\": $(calc_stats torch_pod_times | cut -d' ' -f1), \"median_ms\": $(calc_stats torch_pod_times | cut -d' ' -f2) }"
    )
  }
}
ENDJSON
else
    info "Results ($ITERATIONS iterations)"
    dim "  GPU: $GPU_NAME x$GPU_COUNT ($GPU_MEM, driver $DRIVER_VER)"
    echo ""

    # --- nvidia-smi comparison ---
    info "  nvidia-smi (host vs pod)"
    printf "  ${BOLD}%-28s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
    printf "  %-28s %8s %8s %8s %8s %8s\n" "────────────────────────────" "────────" "────────" "────────" "────────" "────────"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "nvidia-smi (host)"         "$(fmt_ms $hsmi_avg)" "$(fmt_ms $hsmi_med)" "$(fmt_ms $hsmi_min)" "$(fmt_ms $hsmi_max)" "$(fmt_ms $hsmi_p95)"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "nvidia-smi (pod)"          "$(fmt_ms $psmi_avg)" "$(fmt_ms $psmi_med)" "$(fmt_ms $psmi_min)" "$(fmt_ms $psmi_max)" "$(fmt_ms $psmi_p95)"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "nvidia-smi --list-gpus"    "$(fmt_ms $plist_avg)" "$(fmt_ms $plist_med)" "$(fmt_ms $plist_min)" "$(fmt_ms $plist_max)" "$(fmt_ms $plist_p95)"

    # Overhead calculation
    if (( hsmi_med > 0 )); then
        overhead_ms=$((psmi_med - hsmi_med))
        echo ""
        dim "  overhead: ${overhead_ms}ms (envpod namespace entry, not GPU)"
    fi
    echo ""

    # --- Init comparison ---
    info "  envpod init (GPU vs no-GPU)"
    printf "  ${BOLD}%-28s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
    printf "  %-28s %8s %8s %8s %8s %8s\n" "────────────────────────────" "────────" "────────" "────────" "────────" "────────"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "init (gpu: true)"          "$(fmt_ms $ig_avg)" "$(fmt_ms $ig_med)" "$(fmt_ms $ig_min)" "$(fmt_ms $ig_max)" "$(fmt_ms $ig_p95)"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "init (gpu: false)"         "$(fmt_ms $in_avg)" "$(fmt_ms $in_med)" "$(fmt_ms $in_min)" "$(fmt_ms $in_max)" "$(fmt_ms $in_p95)"
    echo ""

    # --- Run comparison ---
    info "  envpod run /bin/true (GPU vs no-GPU)"
    printf "  ${BOLD}%-28s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
    printf "  %-28s %8s %8s %8s %8s %8s\n" "────────────────────────────" "────────" "────────" "────────" "────────" "────────"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "run (gpu: true)"           "$(fmt_ms $rg_avg)" "$(fmt_ms $rg_med)" "$(fmt_ms $rg_min)" "$(fmt_ms $rg_max)" "$(fmt_ms $rg_p95)"
    printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "run (gpu: false)"          "$(fmt_ms $rn_avg)" "$(fmt_ms $rn_med)" "$(fmt_ms $rn_min)" "$(fmt_ms $rn_max)" "$(fmt_ms $rn_p95)"
    echo ""

    # --- CUDA device query (optional) ---
    if $HAS_CUDA; then
        read ch_avg ch_med ch_min ch_max ch_p95 <<< "$(calc_stats cuda_host_times)"
        read cp_avg cp_med cp_min cp_max cp_p95 <<< "$(calc_stats cuda_pod_times)"
        info "  CUDA deviceQuery (host vs pod)"
        printf "  ${BOLD}%-28s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
        printf "  %-28s %8s %8s %8s %8s %8s\n" "────────────────────────────" "────────" "────────" "────────" "────────" "────────"
        printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "deviceQuery (host)"        "$(fmt_ms $ch_avg)" "$(fmt_ms $ch_med)" "$(fmt_ms $ch_min)" "$(fmt_ms $ch_max)" "$(fmt_ms $ch_p95)"
        printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "deviceQuery (pod)"         "$(fmt_ms $cp_avg)" "$(fmt_ms $cp_med)" "$(fmt_ms $cp_min)" "$(fmt_ms $cp_max)" "$(fmt_ms $cp_p95)"
        echo ""
    fi

    # --- PyTorch matmul (optional) ---
    if $HAS_TORCH; then
        read th_avg th_med th_min th_max th_p95 <<< "$(calc_stats torch_host_times)"
        read tp_avg tp_med tp_min tp_max tp_p95 <<< "$(calc_stats torch_pod_times)"
        info "  PyTorch CUDA matmul 1000x1000 (host vs pod)"
        printf "  ${BOLD}%-28s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
        printf "  %-28s %8s %8s %8s %8s %8s\n" "────────────────────────────" "────────" "────────" "────────" "────────" "────────"
        printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "torch matmul (host)"       "$(fmt_ms $th_avg)" "$(fmt_ms $th_med)" "$(fmt_ms $th_min)" "$(fmt_ms $th_max)" "$(fmt_ms $th_p95)"
        printf "  ${CYAN}%-28s${RESET} %8s %8s %8s %8s %8s\n" "torch matmul (pod)"        "$(fmt_ms $tp_avg)" "$(fmt_ms $tp_med)" "$(fmt_ms $tp_min)" "$(fmt_ms $tp_max)" "$(fmt_ms $tp_p95)"
        echo ""
    fi

    dim "  Note: pod overhead is namespace entry (~20ms), not GPU latency."
    dim "  GPU device access is zero-copy bind-mount — no virtualization layer."
    echo ""
fi