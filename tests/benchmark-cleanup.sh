#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# tests/benchmark-cleanup.sh
#
# Emergency cleanup for resources left behind by benchmark scripts that
# crashed, were killed, or exited before their trap handlers fired.
#
# Covers all six benchmark scripts:
#   benchmark.sh, benchmark-clone.sh, benchmark-scale.sh,
#   benchmark-podman.sh, benchmark-size.sh, benchmark-gpu.sh
#
# Idempotent — safe to run multiple times. Never uses set -e.
# Always exits 0 (failures are logged, not fatal).
#
# Usage:
#   sudo bash tests/benchmark-cleanup.sh           # full cleanup
#   sudo bash tests/benchmark-cleanup.sh --dry-run # show what would run
#   sudo bash tests/benchmark-cleanup.sh --help

set -uo pipefail

# ─── Flags ────────────────────────────────────────────────────────────────────
DRY_RUN=false
VERBOSE=false

for arg in "$@"; do
  case "$arg" in
    --dry-run|-n) DRY_RUN=true ;;
    --verbose|-v) VERBOSE=true ;;
    --help|-h)
      cat <<'EOF'
Usage: sudo bash tests/benchmark-cleanup.sh [--dry-run] [--verbose]

Cleans up all resources that benchmark scripts can leave behind:

  1. envpod pods in benchmark temp dirs    (ENVPOD_DIR=/tmp/envpod-*bench*)
  2. envpod pods in the system store       (bench-*, clone-*, scale-*, warmup)
  3. Docker containers                     (bench-d-*, bench-p-*, sleep 3600)
  4. Podman containers                     (bench-p-*, bench-d-*, sleep 3600)
  5. Orphaned cgroups                      (/sys/fs/cgroup/envpod/bench-*)
  6. Orphaned network namespaces           (/run/netns/envpod-*)
  7. Stale overlay/tmpfs mounts            (in /tmp/envpod-*bench* paths)
  8. Stale iptables rules                  (10.200.x.x with no live pod)
  9. Temp directories                      (/tmp/envpod-*bench*)

Options:
  --dry-run, -n    Print what would be done without making any changes
  --verbose, -v    Print all skipped items as well as actions taken
  --help, -h       Show this message and exit

Must be run as root (sudo).
EOF
      exit 0
      ;;
    *)
      echo "Unknown flag: $arg  (use --help)" >&2
      exit 1
      ;;
  esac
done

# ─── Colour helpers ───────────────────────────────────────────────────────────
RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

info()   { echo -e "${CYAN}[info]${NC}   $*"; }
ok()     { echo -e "${GREEN}[ok]${NC}     $*"; }
warn()   { echo -e "${YELLOW}[warn]${NC}   $*"; }
act()    { echo -e "${BOLD}[clean]${NC}  $*"; }
dbg()    { [[ "$VERBOSE" == true ]] && echo -e "         ${NC}$*" || true; }
section(){ echo ""; echo -e "${BOLD}── $* ──────────────────────────────────────────────────────${NC}"; }

# ─── Root check ───────────────────────────────────────────────────────────────
if [[ $EUID -ne 0 ]]; then
  echo -e "${RED}error: must be run as root (sudo bash $0)${NC}" >&2
  exit 1
fi

if [[ "$DRY_RUN" == true ]]; then
  echo -e "${YELLOW}DRY-RUN mode — no changes will be made${NC}"
fi

# ─── run: execute a command unless --dry-run ──────────────────────────────────
run() {
  if [[ "$DRY_RUN" == true ]]; then
    echo -e "         ${YELLOW}would run:${NC} $*"
  else
    "$@" 2>/dev/null || true
  fi
}

# ─── Pod names used across all benchmark scripts ──────────────────────────────
# Exact names (no prefix — must match literally)
BENCH_EXACT=(
  warmup
  warmup-gpu
  bench-pod
  bench-run
  clone-source
  bench-source
  bench-persistent
  bench-gpu-source
  bench-gpu-persistent
  scale-base
)

# Prefixes — any pod whose name starts with one of these is a benchmark pod
BENCH_PREFIXES=(
  bench-init-
  bench-life-
  bench-clone-
  bench-clonecur-
  bench-ir-
  bench-cr-
  bench-fresh-
  bench-file-
  bench-gpuf-
  clone-        # clone-1, clone-2, … from scale + clone scripts
)

# Returns 0 if the given pod name looks like a benchmark pod
is_bench_pod() {
  local name="$1"
  for exact in "${BENCH_EXACT[@]}"; do
    [[ "$name" == "$exact" ]] && return 0
  done
  for prefix in "${BENCH_PREFIXES[@]}"; do
    [[ "$name" == "$prefix"* ]] && return 0
  done
  return 1
}

# ─── Temp dir patterns for all six scripts ────────────────────────────────────
TEMP_PATTERNS=(
  "/tmp/envpod-bench-*"
  "/tmp/envpod-scale-bench-*"
  "/tmp/envpod-podman-bench-*"
  "/tmp/envpod-bench-clone-*"
  "/tmp/envpod-size-bench-*"
  "/tmp/envpod-gpu-bench-*"
)

# Collect all existing benchmark temp dirs into BENCH_TMPDIRS array
BENCH_TMPDIRS=()
for pattern in "${TEMP_PATTERNS[@]}"; do
  for d in $pattern; do
    [[ -d "$d" ]] && BENCH_TMPDIRS+=("$d")
  done
done

TOTAL_CLEANED=0

# ═══════════════════════════════════════════════════════════════════════════════
# 1. envpod pods inside benchmark temp dirs  (ENVPOD_DIR=<tmpdir>)
# ═══════════════════════════════════════════════════════════════════════════════
section "1/9  envpod pods in benchmark temp dirs"

if [[ ${#BENCH_TMPDIRS[@]} -eq 0 ]]; then
  info "no benchmark temp directories found"
else
  ENVPOD_BIN="$(command -v envpod 2>/dev/null || true)"

  for tmpdir in "${BENCH_TMPDIRS[@]}"; do
    store="$tmpdir/store"
    if [[ ! -d "$store" ]]; then
      dbg "no store/ in $tmpdir — skipping"
      continue
    fi

    info "scanning $tmpdir/store/"
    while IFS= read -r pod_dir; do
      pod_name="$(basename "$pod_dir")"
      if [[ -z "$ENVPOD_BIN" ]]; then
        warn "envpod not in PATH — cannot destroy pod '$pod_name' in $tmpdir"
        continue
      fi
      act "ENVPOD_DIR=$tmpdir envpod destroy $pod_name"
      if [[ "$DRY_RUN" != true ]]; then
        ENVPOD_DIR="$tmpdir" "$ENVPOD_BIN" destroy "$pod_name" 2>/dev/null || true
        ((TOTAL_CLEANED++)) || true
      fi
    done < <(find "$store" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort)
  done
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 2. envpod pods in the system store  (default ENVPOD_DIR)
# ═══════════════════════════════════════════════════════════════════════════════
section "2/9  envpod pods in system store"

SYSTEM_STORE="/var/lib/envpod/store"
ENVPOD_BIN="$(command -v envpod 2>/dev/null || true)"

if [[ -z "$ENVPOD_BIN" ]]; then
  warn "envpod not in PATH — skipping system store cleanup"
elif [[ ! -d "$SYSTEM_STORE" ]]; then
  dbg "system store $SYSTEM_STORE not found"
  info "no system store — skipping"
else
  FOUND_SYSTEM=0
  while IFS= read -r pod_dir; do
    pod_name="$(basename "$pod_dir")"
    if is_bench_pod "$pod_name"; then
      act "envpod destroy $pod_name"
      run "$ENVPOD_BIN" destroy "$pod_name"
      ((FOUND_SYSTEM++)) || true
      ((TOTAL_CLEANED++)) || true
    else
      dbg "skip non-bench pod: $pod_name"
    fi
  done < <(find "$SYSTEM_STORE" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort)

  if [[ "$FOUND_SYSTEM" -eq 0 ]]; then
    info "no benchmark pods in system store"
  fi

  # Run gc to purge any leftover artifacts (orphaned dirs, stale state)
  act "envpod gc"
  run "$ENVPOD_BIN" gc
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 3. Docker containers
# ═══════════════════════════════════════════════════════════════════════════════
section "3/9  Docker containers"

if ! command -v docker &>/dev/null; then
  info "docker not in PATH — skipping"
elif ! docker info &>/dev/null 2>&1; then
  info "docker daemon not running — skipping"
else
  DOCKER_REMOVED=0

  # Named bench-d-* and bench-p-* containers (from scale + podman scripts)
  while IFS= read -r name; do
    [[ -z "$name" ]] && continue
    act "docker rm -f $name"
    run docker rm -f "$name"
    ((DOCKER_REMOVED++)) || true
  done < <(docker ps -a --format '{{.Names}}' 2>/dev/null | grep -E '^bench-[dp]-' || true)

  # Containers running "sleep 3600" — benchmark size/podman scripts leave these
  while IFS= read -r cid; do
    [[ -z "$cid" ]] && continue
    act "docker rm -f $cid  (sleep 3600)"
    run docker rm -f "$cid"
    ((DOCKER_REMOVED++)) || true
  done < <(docker ps --format '{{.ID}}\t{{.Command}}' 2>/dev/null \
           | awk -F'\t' '$2 ~ /sleep.*3600/ {print $1}' || true)

  if [[ "$DOCKER_REMOVED" -eq 0 ]]; then
    info "no benchmark Docker containers found"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 4. Podman containers
# ═══════════════════════════════════════════════════════════════════════════════
section "4/9  Podman containers"

if ! command -v podman &>/dev/null; then
  info "podman not in PATH — skipping"
else
  PODMAN_REMOVED=0

  # Named bench-p-* and bench-d-* containers
  while IFS= read -r name; do
    [[ -z "$name" ]] && continue
    act "podman rm -f $name"
    run podman rm -f "$name"
    ((PODMAN_REMOVED++)) || true
  done < <(podman ps -a --format '{{.Names}}' 2>/dev/null | grep -E '^bench-[dp]-' || true)

  # Containers running "sleep 3600"
  while IFS= read -r cid; do
    [[ -z "$cid" ]] && continue
    act "podman rm -f $cid  (sleep 3600)"
    run podman rm -f "$cid"
    ((PODMAN_REMOVED++)) || true
  done < <(podman ps --format '{{.ID}}\t{{.Command}}' 2>/dev/null \
           | awk -F'\t' '$2 ~ /sleep.*3600/ {print $1}' || true)

  if [[ "$PODMAN_REMOVED" -eq 0 ]]; then
    info "no benchmark Podman containers found"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 5. Orphaned cgroups
# ═══════════════════════════════════════════════════════════════════════════════
section "5/9  Orphaned cgroups (/sys/fs/cgroup/envpod/)"

CGROUP_BASE="/sys/fs/cgroup/envpod"

if [[ ! -d "$CGROUP_BASE" ]]; then
  info "no envpod cgroup hierarchy found"
else
  CG_REMOVED=0
  while IFS= read -r cg; do
    cg_name="$(basename "$cg")"
    if ! is_bench_pod "$cg_name"; then
      dbg "skip non-bench cgroup: $cg_name"
      continue
    fi

    # Kill any stray processes still inside this cgroup
    procs_file="$cg/cgroup.procs"
    if [[ -f "$procs_file" ]]; then
      while IFS= read -r pid; do
        [[ -n "$pid" ]] || continue
        if [[ "$pid" =~ ^[0-9]+$ ]] && [[ "$pid" -gt 1 ]]; then
          act "kill -9 $pid  (in cgroup $cg_name)"
          run kill -9 "$pid"
        fi
      done < "$procs_file"
    fi

    # Brief pause to let processes die before rmdir
    [[ "$DRY_RUN" != true ]] && sleep 0.05

    act "rmdir $cg"
    run rmdir "$cg"
    ((CG_REMOVED++)) || true
  done < <(find "$CGROUP_BASE" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort)

  if [[ "$CG_REMOVED" -eq 0 ]]; then
    info "no orphaned benchmark cgroups found"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 6. Orphaned network namespaces
# ═══════════════════════════════════════════════════════════════════════════════
section "6/9  Orphaned network namespaces (/run/netns/envpod-*)"

if [[ ! -d "/run/netns" ]]; then
  info "no netns directory found"
else
  NS_REMOVED=0
  while IFS= read -r ns_file; do
    ns_name="$(basename "$ns_file")"
    # Check if any process's /proc/PID/ns/net shares the same inode as this file.
    # If so, the namespace is still in use by a live pod — skip it.
    ns_inode="$(stat -Lc '%i' "$ns_file" 2>/dev/null || echo 0)"
    in_use=false
    if [[ "$ns_inode" != "0" ]]; then
      for proc_ns in /proc/[0-9]*/ns/net; do
        proc_inode="$(stat -Lc '%i' "$proc_ns" 2>/dev/null || echo 0)"
        if [[ "$proc_inode" == "$ns_inode" && "$proc_inode" != "0" ]]; then
          in_use=true
          break
        fi
      done
    fi

    if [[ "$in_use" == true ]]; then
      dbg "netns $ns_name still in use — skipping"
      continue
    fi

    act "ip netns delete $ns_name"
    run ip netns delete "$ns_name"
    ((NS_REMOVED++)) || true
  done < <(find /run/netns -maxdepth 1 -name 'envpod-*' -type f 2>/dev/null | sort)

  if [[ "$NS_REMOVED" -eq 0 ]]; then
    info "no orphaned envpod network namespaces found"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 7. Stale overlay and bind mounts inside temp dirs
# ═══════════════════════════════════════════════════════════════════════════════
section "7/9  Stale mounts in benchmark temp dirs"

MNT_REMOVED=0

# Collect all mount points that live under /tmp/envpod-*bench* paths.
# Sort in reverse order so we unmount children before parents.
while IFS= read -r mp; do
  [[ -z "$mp" ]] && continue
  act "umount -l $mp"
  run umount -l "$mp"
  ((MNT_REMOVED++)) || true
done < <(
  awk '{print $2}' /proc/mounts 2>/dev/null \
  | grep '/tmp/envpod-' \
  | grep -E 'bench' \
  | sort -r
)

if [[ "$MNT_REMOVED" -eq 0 ]]; then
  info "no stale mounts in benchmark temp dirs"
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 8. Stale iptables rules referencing dead pod IPs (10.200.x.x)
# ═══════════════════════════════════════════════════════════════════════════════
section "8/9  Stale iptables rules (10.200.x.x with no live pod)"

if ! command -v iptables &>/dev/null; then
  info "iptables not available — skipping"
else
  # Collect IPs of pods that are currently alive (have a running netns)
  declare -A LIVE_IPS
  while IFS= read -r ns_file; do
    ns_name="$(basename "$ns_file")"
    pod_ip=$(ip netns exec "$ns_name" ip -4 addr show dev eth0 2>/dev/null \
             | awk '/inet / {split($2,a,"/"); print a[1]}')
    [[ -n "$pod_ip" ]] && LIVE_IPS["$pod_ip"]=1
  done < <(find /run/netns -maxdepth 1 -name 'envpod-*' -type f 2>/dev/null)

  IPT_REMOVED=0

  for table_chain in "nat:PREROUTING" "nat:OUTPUT" "nat:POSTROUTING" "filter:FORWARD"; do
    table="${table_chain%%:*}"
    chain="${table_chain##*:}"

    # Read rules into a temp file to avoid subshell variable scope issues
    tmp_rules="$(mktemp)"
    iptables -t "$table" -S "$chain" 2>/dev/null | grep -E '10\.200\.[0-9]+\.[0-9]+' > "$tmp_rules" || true

    while IFS= read -r rule; do
      [[ -z "$rule" ]] && continue
      rule_ip="$(echo "$rule" | grep -oE '10\.200\.[0-9]+\.[0-9]+' | head -1)"
      [[ -z "$rule_ip" ]] && continue

      if [[ -n "${LIVE_IPS[$rule_ip]+_}" ]]; then
        dbg "rule for $rule_ip is live — keeping: $rule"
        continue
      fi

      # Convert "-A CHAIN spec..." to "-D CHAIN spec..."
      del_rule="${rule/-A $chain/-D $chain}"
      act "iptables -t $table $del_rule"
      if [[ "$DRY_RUN" != true ]]; then
        # Use read -ra for safe word splitting
        read -ra del_args <<< "$del_rule"
        iptables -t "$table" "${del_args[@]}" 2>/dev/null || true
        ((IPT_REMOVED++)) || true
      fi
    done < "$tmp_rules"

    rm -f "$tmp_rules"
  done

  if [[ "$IPT_REMOVED" -eq 0 && "$DRY_RUN" != true ]]; then
    info "no stale iptables rules found"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# 9. Remove benchmark temp directories
# ═══════════════════════════════════════════════════════════════════════════════
section "9/9  Benchmark temp directories"

if [[ ${#BENCH_TMPDIRS[@]} -eq 0 ]]; then
  info "no benchmark temp directories to remove"
else
  for tmpdir in "${BENCH_TMPDIRS[@]}"; do
    # Verify nothing is still mounted inside before removing
    still_mounted=$(awk '{print $2}' /proc/mounts 2>/dev/null | grep -c "^${tmpdir}" || true)
    if [[ "$still_mounted" -gt 0 ]]; then
      warn "$tmpdir has $still_mounted remaining mounts — skipping rm (re-run after umount)"
      continue
    fi
    act "rm -rf $tmpdir"
    run rm -rf "$tmpdir"
  done
fi

# ═══════════════════════════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}────────────────────────────────────────────────────────${NC}"
if [[ "$DRY_RUN" == true ]]; then
  echo -e "${YELLOW}  dry-run complete — no changes made${NC}"
  echo "  Re-run without --dry-run to apply cleanup."
else
  echo -e "${GREEN}  benchmark cleanup complete${NC}"
fi
echo -e "${BOLD}────────────────────────────────────────────────────────${NC}"
echo ""

exit 0