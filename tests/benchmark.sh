#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# Benchmark script for envpod pod startup times.
# Requires: sudo, a release build or installed envpod binary.
#
# Usage:
#   sudo ./tests/benchmark.sh              # default: 10 iterations
#   sudo ./tests/benchmark.sh 50           # 50 iterations
#   sudo ./tests/benchmark.sh 10 --json    # JSON output
#
# Measures the wall-clock time to execute:
#   envpod init  (pod creation + overlayfs + cgroup + netns setup)
#   envpod run   (namespace entry + command execution)
#   envpod diff      (overlay scan)
#   envpod rollback  (discard overlay changes)
#   envpod destroy   (full teardown)
#
# Each benchmark creates a fresh pod, runs /bin/true (exits instantly),
# so the measured time is pure envpod overhead.

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
# Use temp state dir so we don't pollute real pods
# ---------------------------------------------------------------------------
BENCH_DIR=$(mktemp -d /tmp/envpod-bench-XXXXXX)
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

    # Median (sort + pick middle)
    local sorted
    sorted=($(printf '%s\n' "${_arr[@]}" | sort -n))
    local mid=$((count / 2))
    local median
    if (( count % 2 == 0 )); then
        median=$(( (sorted[mid-1] + sorted[mid]) / 2 ))
    else
        median=${sorted[$mid]}
    fi

    # p95
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
# Pod config (inline minimal YAML)
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
# Warmup
# ---------------------------------------------------------------------------
$JSON_OUTPUT || echo ""
info "envpod benchmark"
dim "  binary:     $ENVPOD"
dim "  iterations: $ITERATIONS"
dim "  state dir:  $BENCH_DIR"
$JSON_OUTPUT || echo ""

info "Warmup run..."
"$ENVPOD" init warmup -c "$POD_YAML" >/dev/null 2>&1
"$ENVPOD" run warmup -- /bin/true >/dev/null 2>&1
"$ENVPOD" diff warmup >/dev/null 2>&1
"$ENVPOD" destroy warmup >/dev/null 2>&1
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: init
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod init' ($ITERATIONS iterations)..."
declare -a init_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-init-$i"
    ms=$(time_ms "$ENVPOD" init "$pod_name" -c "$POD_YAML")
    init_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
    # Cleanup
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1 || true
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: run (create pod once, run N times)
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod run -- /bin/true' ($ITERATIONS iterations)..."
"$ENVPOD" init bench-run -c "$POD_YAML" >/dev/null 2>&1
declare -a run_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-run -- /bin/true)
    run_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: diff
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod diff' ($ITERATIONS iterations)..."
# Create a file so diff has something to scan
"$ENVPOD" run bench-run -- /bin/sh -c "echo hello > /root/testfile" >/dev/null 2>&1
declare -a diff_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" diff bench-run)
    diff_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: rollback
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod rollback' ($ITERATIONS iterations)..."
declare -a rollback_times=()
for i in $(seq 1 "$ITERATIONS"); do
    # Create a file so rollback has something to discard
    "$ENVPOD" run bench-run -- /bin/sh -c "echo rollback-$i > /root/rollback-test" >/dev/null 2>&1
    ms=$(time_ms "$ENVPOD" rollback bench-run)
    rollback_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: run --root (compare root vs agent user)
# ---------------------------------------------------------------------------
info "Benchmarking 'envpod run --root -- /bin/true' ($ITERATIONS iterations)..."
declare -a run_root_times=()
for i in $(seq 1 "$ITERATIONS"); do
    ms=$(time_ms "$ENVPOD" run bench-run --root -- /bin/true)
    run_root_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
"$ENVPOD" destroy bench-run >/dev/null 2>&1 || true
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Benchmark: full lifecycle (init + run + diff + destroy)
# ---------------------------------------------------------------------------
info "Benchmarking full lifecycle ($ITERATIONS iterations)..."
declare -a lifecycle_times=()
for i in $(seq 1 "$ITERATIONS"); do
    pod_name="bench-life-$i"
    start=$(date +%s%N)
    "$ENVPOD" init "$pod_name" -c "$POD_YAML" >/dev/null 2>&1
    "$ENVPOD" run "$pod_name" -- /bin/true >/dev/null 2>&1
    "$ENVPOD" diff "$pod_name" >/dev/null 2>&1
    "$ENVPOD" destroy "$pod_name" >/dev/null 2>&1
    end=$(date +%s%N)
    ms=$(( (end - start) / 1000000 ))
    lifecycle_times+=("$ms")
    dim "  [$i/$ITERATIONS] ${ms}ms"
done
$JSON_OUTPUT || echo ""

# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------
read init_avg init_med init_min init_max init_p95 <<< "$(calc_stats init_times)"
read run_avg run_med run_min run_max run_p95 <<< "$(calc_stats run_times)"
read diff_avg diff_med diff_min diff_max diff_p95 <<< "$(calc_stats diff_times)"
read rb_avg rb_med rb_min rb_max rb_p95 <<< "$(calc_stats rollback_times)"
read root_avg root_med root_min root_max root_p95 <<< "$(calc_stats run_root_times)"
read life_avg life_med life_min life_max life_p95 <<< "$(calc_stats lifecycle_times)"

if $JSON_OUTPUT; then
    cat << ENDJSON
{
  "binary": "$ENVPOD",
  "iterations": $ITERATIONS,
  "results": {
    "init": { "avg_ms": $init_avg, "median_ms": $init_med, "min_ms": $init_min, "max_ms": $init_max, "p95_ms": $init_p95 },
    "run": { "avg_ms": $run_avg, "median_ms": $run_med, "min_ms": $run_min, "max_ms": $run_max, "p95_ms": $run_p95 },
    "run_root": { "avg_ms": $root_avg, "median_ms": $root_med, "min_ms": $root_min, "max_ms": $root_max, "p95_ms": $root_p95 },
    "diff": { "avg_ms": $diff_avg, "median_ms": $diff_med, "min_ms": $diff_min, "max_ms": $diff_max, "p95_ms": $diff_p95 },
    "rollback": { "avg_ms": $rb_avg, "median_ms": $rb_med, "min_ms": $rb_min, "max_ms": $rb_max, "p95_ms": $rb_p95 },
    "lifecycle": { "avg_ms": $life_avg, "median_ms": $life_med, "min_ms": $life_min, "max_ms": $life_max, "p95_ms": $life_p95 }
  }
}
ENDJSON
else
    info "Results ($ITERATIONS iterations)"
    echo ""
    printf "  ${BOLD}%-20s %8s %8s %8s %8s %8s${RESET}\n" "COMMAND" "AVG" "MEDIAN" "MIN" "MAX" "P95"
    printf "  %-20s %8s %8s %8s %8s %8s\n" "────────────────────" "────────" "────────" "────────" "────────" "────────"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod init"       "$(fmt_ms $init_avg)" "$(fmt_ms $init_med)" "$(fmt_ms $init_min)" "$(fmt_ms $init_max)" "$(fmt_ms $init_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod run"        "$(fmt_ms $run_avg)"  "$(fmt_ms $run_med)"  "$(fmt_ms $run_min)"  "$(fmt_ms $run_max)"  "$(fmt_ms $run_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod run --root" "$(fmt_ms $root_avg)" "$(fmt_ms $root_med)" "$(fmt_ms $root_min)" "$(fmt_ms $root_max)" "$(fmt_ms $root_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod diff"       "$(fmt_ms $diff_avg)" "$(fmt_ms $diff_med)" "$(fmt_ms $diff_min)" "$(fmt_ms $diff_max)" "$(fmt_ms $diff_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "envpod rollback"   "$(fmt_ms $rb_avg)"   "$(fmt_ms $rb_med)"   "$(fmt_ms $rb_min)"   "$(fmt_ms $rb_max)"   "$(fmt_ms $rb_p95)"
    printf "  ${CYAN}%-20s${RESET} %8s %8s %8s %8s %8s\n" "full lifecycle"    "$(fmt_ms $life_avg)" "$(fmt_ms $life_med)" "$(fmt_ms $life_min)" "$(fmt_ms $life_max)" "$(fmt_ms $life_p95)"
    echo ""
    dim "  lifecycle = init + run + diff + destroy"
    echo ""
fi