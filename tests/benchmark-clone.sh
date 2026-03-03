#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Benchmark: envpod clone vs envpod init
# Measures the wall-clock time difference between creating a pod from scratch
# (init) versus cloning from an existing pod (clone).
#
# Usage:
#   sudo ./tests/benchmark-clone.sh              # default: 10 iterations
#   sudo ./tests/benchmark-clone.sh 50           # 50 iterations
#   sudo ./tests/benchmark-clone.sh 10 --json    # JSON output

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
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'
else
    RED='' GREEN='' CYAN='' BOLD='' DIM='' RESET=''
fi

info()  { $JSON_OUTPUT || echo -e "${BOLD}$*${RESET}"; }
dim()   { $JSON_OUTPUT || echo -e "${DIM}$*${RESET}"; }

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
# Require root
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    echo "Error: must run as root (sudo $0)" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Temp state dir
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-bench-clone-XXXXXX)
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

# ---------------------------------------------------------------------------
# Pod config
# ---------------------------------------------------------------------------
POD_YAML="$BENCH_DIR/bench.yaml"
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

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------
$JSON_OUTPUT || echo ""
info "envpod clone vs init benchmark"
dim "  binary:     $ENVPOD"
dim "  iterations: $ITERATIONS"
dim "  state dir:  $BENCH_DIR"
$JSON_OUTPUT || echo ""

# Create the source pod that clones will come from
info "Creating source pod..."
"$ENVPOD" init clone-source -c "$POD_YAML" >/dev/null 2>&1
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: init (from scratch)
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod init' ($ITERATIONS iterations)..."
declare -a init_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-init-$i"
    ms=$(time_ms "$ENVPOD" init "$pod_name" -c "$POD_YAML")
    init_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: clone (from base snapshot)
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod clone' ($ITERATIONS iterations)..."
declare -a clone_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-clone-$i"
    ms=$(time_ms "$ENVPOD" clone clone-source "$pod_name")
    clone_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: clone --current
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod clone --current' ($ITERATIONS iterations)..."
# Write some data into the source pod so --current has agent state to copy
"$ENVPOD" run clone-source --root -- /bin/sh -c "echo agent-data > /root/testfile" >/dev/null 2>&1
declare -a clone_current_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-clonecur-$i"
    ms=$(time_ms "$ENVPOD" clone clone-source "$pod_name" --current)
    clone_current_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: clone + run (end-to-end usability)
# ---------------------------------------------------------------------------
info "Benchmarking clone+run vs init+run ($ITERATIONS iterations)..."
declare -a init_run_times=()
declare -a clone_run_times=()
for i in $(seq 1 "$ITERATIONS"); do
    # init + run
    pod_name="bench-ir-$i"
    start=$(date +%s%N)
    "$ENVPOD" init "$pod_name" -c "$POD_YAML" >/dev/null 2>&1
    "$ENVPOD" run "$pod_name" -- /bin/true >/dev/null 2>&1
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    init_run_times+=("$ms")
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true

    # clone + run
    pod_name="bench-cr-$i"
    start=$(date +%s%N)
    "$ENVPOD" clone clone-source "$pod_name" >/dev/null 2>&1
    "$ENVPOD" run "$pod_name" -- /bin/true >/dev/null 2>&1
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    clone_run_times+=("$ms")
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true

    dim "  [$i/$ITERATIONS] init+run=$(fmt_ms ${init_run_times[-1]})  clone+run=$(fmt_ms ${clone_run_times[-1]})"
done
$JSON_OUTPUT || echo ""

# Cleanup source pod and its base
"$ENVPOD" destroy clone-source --base >/dev/null 2>&1 || true

# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------
read init_avg init_med init_min init_max init_p95 <<< "$(calc_stats init_times)"
read clone_avg clone_med clone_min clone_max clone_p95 <<< "$(calc_stats clone_times)"
read cc_avg cc_med cc_min cc_max cc_p95 <<< "$(calc_stats clone_current_times)"
read ir_avg ir_med ir_min ir_max ir_p95 <<< "$(calc_stats init_run_times)"
read cr_avg cr_med cr_min cr_max cr_p95 <<< "$(calc_stats clone_run_times)"

# Speedup calculation
if (( clone_med > 0 )); then
    speedup_x=$(( init_med * 100 / clone_med ))
    speedup_whole=$((speedup_x / 100))
    speedup_frac=$((speedup_x % 100))
    speedup="${speedup_whole}.${speedup_frac}x"
else
    speedup="N/A"
fi

if $JSON_OUTPUT; then
    cat << ENDJSON
{
  "binary": "$ENVPOD",
  "iterations": $ITERATIONS,
  "results": {
    "init": { "avg_ms": $init_avg, "median_ms": $init_med, "min_ms": $init_min, "max_ms": $init_max, "p95_ms": $init_p95 },
    "clone": { "avg_ms": $clone_avg, "median_ms": $clone_med, "min_ms": $clone_min, "max_ms": $clone_max, "p95_ms": $clone_p95 },
    "clone_current": { "avg_ms": $cc_avg, "median_ms": $cc_med, "min_ms": $cc_min, "max_ms": $cc_max, "p95_ms": $cc_p95 },
    "init_plus_run": { "avg_ms": $ir_avg, "median_ms": $ir_med, "min_ms": $ir_min, "max_ms": $ir_max, "p95_ms": $ir_p95 },
    "clone_plus_run": { "avg_ms": $cr_avg, "median_ms": $cr_med, "min_ms": $cr_min, "max_ms": $cr_max, "p95_ms": $cr_p95 },
    "speedup": "$speedup"
  }
}
ENDJSON
else
    info "Results ($ITERATIONS iterations)"
    echo ""
    printf "  ${BOLD}%-20s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
    printf "  %-20s %8s %8s %8s %8s %8s\n" "────────────────────" "────────" "────────" "────────" "────────" "────────"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod init"          "$(fmt_ms $init_avg)"  "$(fmt_ms $init_med)"  "$(fmt_ms $init_min)"  "$(fmt_ms $init_max)"  "$(fmt_ms $init_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod clone"         "$(fmt_ms $clone_avg)" "$(fmt_ms $clone_med)" "$(fmt_ms $clone_min)" "$(fmt_ms $clone_max)" "$(fmt_ms $clone_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod clone --curr." "$(fmt_ms $cc_avg)"    "$(fmt_ms $cc_med)"    "$(fmt_ms $cc_min)"    "$(fmt_ms $cc_max)"    "$(fmt_ms $cc_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "init + run"           "$(fmt_ms $ir_avg)"    "$(fmt_ms $ir_med)"    "$(fmt_ms $ir_min)"    "$(fmt_ms $ir_max)"    "$(fmt_ms $ir_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "clone + run"          "$(fmt_ms $cr_avg)"    "$(fmt_ms $cr_med)"    "$(fmt_ms $cr_min)"    "$(fmt_ms $cr_max)"    "$(fmt_ms $cr_p95)"
    echo ""
    printf "  ${GREEN}Speedup: clone is %s faster than init (median)${RESET}\n" "$speedup"
    echo ""
    dim "  Saved $(fmt_ms $((init_med - clone_med))) per pod creation"
    echo ""
fi