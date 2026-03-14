#!/usr/bin/env bash
#
# test-all-examples.sh — Init all example pod configs and report pass/fail.
#
# Usage:
#   sudo ./tests/test-all-examples.sh                          # test all examples
#   sudo ./tests/test-all-examples.sh aider swe-agent browser  # test specific examples
#   sudo ./tests/test-all-examples.sh --cleanup                # destroy all test pods
#   sudo ./tests/test-all-examples.sh --skip-desktop           # skip desktop configs (slow)
#
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
EXAMPLES_DIR="$REPO_DIR/examples"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# Configs that aren't pod configs
SKIP_FILES=(
    "monitoring-policy.yaml"    # monitoring policy, not a pod config
)

# Configs that need specific hardware/platform
SKIP_PLATFORM=(
    "jetson-orin.yaml"          # ARM64 + NVIDIA Jetson only
    "raspberry-pi.yaml"         # ARM64 only

)

# Desktop configs (slow — 2-5 min each due to apt-get install)
DESKTOP_CONFIGS=(
    "desktop.yaml"
    "desktop-openbox.yaml"
    "desktop-sway.yaml"
    "desktop-web.yaml"
    "desktop-user.yaml"
    "workstation.yaml"
    "workstation-full.yaml"
    "workstation-gpu.yaml"
    "gimp.yaml"
    "vscode.yaml"
    "web-display-novnc.yaml"
)

CLEANUP=0
SKIP_DESKTOP=0
ONLY_EXAMPLES=()
for arg in "$@"; do
    case "$arg" in
        --cleanup)       CLEANUP=1 ;;
        --skip-desktop)  SKIP_DESKTOP=1 ;;
        --help|-h)
            echo "Usage: sudo $0 [--cleanup] [--skip-desktop] [example ...]"
            echo ""
            echo "  --cleanup       Destroy all test pods created by this script"
            echo "  --skip-desktop  Skip desktop configs (slow, 2-5 min each)"
            echo "  example ...     Test only these examples (name without .yaml)"
            echo ""
            echo "Examples:"
            echo "  sudo $0 aider swe-agent browser    # test only these three"
            echo "  sudo $0 --skip-desktop              # test all, skip desktop"
            exit 0
            ;;
        --*)
            echo "Unknown option: $arg"
            exit 1
            ;;
        *)
            # Strip .yaml suffix if provided
            ONLY_EXAMPLES+=("${arg%.yaml}")
            ;;
    esac
done

if [[ $EUID -ne 0 ]]; then
    echo -e "${RED}Run as root: sudo $0${NC}"
    exit 1
fi

# Pod name: "test-" + filename without .yaml
pod_name() {
    local file="$1"
    echo "test-$(basename "$file" .yaml)"
}

is_skipped() {
    local file="$1"
    local base
    base=$(basename "$file")

    for s in "${SKIP_FILES[@]}"; do
        [[ "$base" == "$s" ]] && return 0
    done
    for s in "${SKIP_PLATFORM[@]}"; do
        [[ "$base" == "$s" ]] && return 0
    done
    if [[ "$SKIP_DESKTOP" -eq 1 ]]; then
        for s in "${DESKTOP_CONFIGS[@]}"; do
            [[ "$base" == "$s" ]] && return 0
        done
    fi
    return 1
}

# ─── Cleanup mode ───────────────────────────────────────────────────

if [[ "$CLEANUP" -eq 1 ]]; then
    echo -e "${BOLD}Destroying all test pods...${NC}"
    echo ""
    count=0
    for yaml in "$EXAMPLES_DIR"/*.yaml; do
        if is_skipped "$yaml"; then continue; fi
        name=$(pod_name "$yaml")
        if envpod ls 2>/dev/null | grep -q "$name"; then
            echo -n "  $name ... "
            if envpod destroy "$name" 2>/dev/null; then
                echo -e "${GREEN}destroyed${NC}"
            else
                echo -e "${YELLOW}not found${NC}"
            fi
            count=$((count + 1))
        fi
    done
    echo ""
    echo -e "${GREEN}Cleaned up $count pods${NC}"
    exit 0
fi

# ─── Init mode ──────────────────────────────────────────────────────

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │    envpod example config test suite   │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

total=0
passed=0
failed=0
skipped=0
PASSED_LIST=()
FAILED_LIST=()
SKIPPED_LIST=()

for yaml in "$EXAMPLES_DIR"/*.yaml; do
    base=$(basename "$yaml")
    name_no_ext="${base%.yaml}"

    # If specific examples requested, skip everything else
    if [[ ${#ONLY_EXAMPLES[@]} -gt 0 ]]; then
        match=0
        for ex in "${ONLY_EXAMPLES[@]}"; do
            [[ "$ex" == "$name_no_ext" ]] && match=1 && break
        done
        [[ "$match" -eq 0 ]] && continue
    fi

    if is_skipped "$yaml"; then
        echo -e "  ${DIM}SKIP  $base${NC}"
        skipped=$((skipped + 1))
        SKIPPED_LIST+=("$base")
        continue
    fi

    total=$((total + 1))
    name=$(pod_name "$yaml")

    # Destroy existing pod with same name (from previous run)
    envpod destroy "$name" 2>/dev/null || true

    echo ""
    echo -e "  ${BOLD}[$total] $base → $name${NC}"
    echo "  ────────────────────────────────────────"

    # Run init with live output so user sees progress
    if envpod init "$name" -c "$yaml" 2>&1 | tee /tmp/envpod-test-output.log; then
        # Check if setup actually completed
        if grep -q "Setup failed\|Setup Incomplete" /tmp/envpod-test-output.log; then
            echo -e "  ${RED}✗ FAIL${NC}  Setup incomplete"
            fail_line=$(grep -o 'Setup failed at step.*' /tmp/envpod-test-output.log | head -1)
            failed=$((failed + 1))
            FAILED_LIST+=("$base: $fail_line")
        else
            echo -e "  ${GREEN}✓ PASS${NC}"
            passed=$((passed + 1))
            PASSED_LIST+=("$base")
        fi
    else
        echo -e "  ${RED}✗ FAIL${NC}  Init failed"
        fail_line=$(grep -iE 'error|fail|fatal' /tmp/envpod-test-output.log | head -1)
        failed=$((failed + 1))
        FAILED_LIST+=("$base: ${fail_line:-unknown error}")
    fi
done

# ─── Summary ────────────────────────────────────────────────────────

echo ""
echo "  ════════════════════════════════════════"
echo -e "  ${GREEN}Passed: $passed${NC}  ${RED}Failed: $failed${NC}  ${DIM}Skipped: $skipped${NC}  Total: $total"
echo "  ════════════════════════════════════════"

if [[ ${#PASSED_LIST[@]} -gt 0 ]]; then
    echo ""
    echo -e "  ${GREEN}${BOLD}Passed ($passed):${NC}"
    for p in "${PASSED_LIST[@]}"; do
        echo -e "    ${GREEN}✓${NC} $p"
    done
fi

if [[ ${#FAILED_LIST[@]} -gt 0 ]]; then
    echo ""
    echo -e "  ${RED}${BOLD}Failed ($failed):${NC}"
    for f in "${FAILED_LIST[@]}"; do
        echo -e "    ${RED}✗${NC} $f"
    done
fi

if [[ ${#SKIPPED_LIST[@]} -gt 0 ]]; then
    echo ""
    echo -e "  ${DIM}${BOLD}Skipped ($skipped):${NC}"
    for s in "${SKIPPED_LIST[@]}"; do
        echo -e "    ${DIM}–${NC} $s"
    done
fi

echo ""
echo -e "  Cleanup: sudo $0 --cleanup"
echo ""

exit "$failed"
