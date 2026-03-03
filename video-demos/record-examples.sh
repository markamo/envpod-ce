#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# record-examples.sh — Generate asciinema recordings for all example configs
#
# Records a demo for each of the 31 example YAML configs, showing:
#   1. Config file contents (cat)
#   2. envpod init (create pod)
#   3. envpod status (pod details)
#   4. envpod audit --security (security findings)
#   5. envpod run (quick command inside pod)
#   6. envpod diff (show changes)
#   7. envpod destroy (cleanup)
#
# Usage:
#   sudo bash video-demos/record-examples.sh                     # all examples
#   sudo bash video-demos/record-examples.sh claude-code browser  # specific ones
#   sudo bash video-demos/record-examples.sh --list               # list available
#   sudo bash video-demos/record-examples.sh --presets-only       # only preset configs
#   sudo bash video-demos/record-examples.sh --clean              # destroy all demo pods
#
# Output: video-demos/recordings/examples/*.cast
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="${SCRIPT_DIR}/recordings/examples"
EXAMPLES_DIR="${REPO_DIR}/examples"
COLS=110
ROWS=35
TYPE_DELAY=0.03
CMD_PAUSE=0.8
OUTPUT_PAUSE=1.5
SECTION_PAUSE=2.0

# Colors
BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────────────────────

type_cmd() {
    local display="$1"
    printf "${GREEN}\$ ${NC}"
    for (( i=0; i<${#display}; i++ )); do
        printf "%s" "${display:$i:1}"
        sleep "$TYPE_DELAY"
    done
    sleep "$CMD_PAUSE"
    printf "\n"
}

run() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual"
    sleep "$OUTPUT_PAUSE"
}

run_ok() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual" || true
    sleep "$OUTPUT_PAUSE"
}

srun() {
    run "sudo $1" "$1"
}

srun_ok() {
    run_ok "sudo $1" "$1"
}

narrate() {
    echo ""
    echo -e "${DIM}# $1${NC}"
    sleep 1.5
    echo ""
}

title_card() {
    clear
    echo ""
    echo -e "${BOLD}${CYAN}  ╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BOLD}${CYAN}  ║                                                              ║${NC}"
    printf  "${BOLD}${CYAN}  ║${NC}${BOLD}   %-58s${CYAN}║${NC}\n" "$1"
    printf  "${BOLD}${CYAN}  ║${NC}${DIM}   %-58s${CYAN}║${NC}\n" "$2"
    echo -e "${BOLD}${CYAN}  ║                                                              ║${NC}"
    echo -e "${BOLD}${CYAN}  ╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    sleep "$SECTION_PAUSE"
}

end_card() {
    echo ""
    echo -e "${BOLD}${GREEN}  envpod.dev  ·  github.com/markamo/envpod-ce  ·  BSL 1.1${NC}"
    echo ""
    sleep 3
}

# ── Example categories ───────────────────────────────────────────────

# Map example name to a human-readable description
describe_example() {
    case "$1" in
        aider)              echo "Aider AI pair programmer" ;;
        basic-cli)          echo "Minimal CLI sandbox" ;;
        basic-internet)     echo "CLI with internet access" ;;
        browser)            echo "Headless Chrome (X11/auto)" ;;
        browser-use)        echo "Browser-use web automation agent" ;;
        browser-wayland)    echo "Chrome with Wayland + PipeWire (secure)" ;;
        claude-code)        echo "Anthropic Claude Code CLI agent" ;;
        codex)              echo "OpenAI Codex CLI agent" ;;
        coding-agent)       echo "Generic coding agent template" ;;
        demo-pod)           echo "Demo/testing pod" ;;
        desktop)            echo "XFCE desktop via noVNC" ;;
        devbox)             echo "General development sandbox" ;;
        discovery-client)   echo "Pod discovery client example" ;;
        discovery-service)  echo "Pod discovery service example" ;;
        fuse-agent)         echo "FUSE filesystem agent" ;;
        gemini-cli)         echo "Google Gemini CLI agent" ;;
        google-adk)         echo "Google Agent Development Kit" ;;
        hardened-sandbox)   echo "Maximum security sandbox" ;;
        jetson-orin)        echo "NVIDIA Jetson Orin (ARM64 GPU)" ;;
        langgraph)          echo "LangGraph workflow framework" ;;
        ml-training)        echo "ML training with GPU" ;;
        monitoring-policy)  echo "Monitoring + policy example" ;;
        nodejs)             echo "Node.js 22 environment" ;;
        openclaw)           echo "OpenClaw messaging assistant" ;;
        opencode)           echo "OpenCode terminal agent" ;;
        playwright)         echo "Playwright browser automation" ;;
        python-env)         echo "Python data science environment" ;;
        raspberry-pi)       echo "Raspberry Pi (ARM64)" ;;
        swe-agent)          echo "SWE-agent autonomous coder" ;;
        vscode)             echo "VS Code in browser (code-server)" ;;
        web-display-novnc)  echo "noVNC web display" ;;
        *)                  echo "$1" ;;
    esac
}

# ── Per-example recording function ───────────────────────────────────

record_example() {
    local name="$1"
    local yaml="${EXAMPLES_DIR}/${name}.yaml"
    local desc
    desc="$(describe_example "$name")"
    local pod_name="demo-${name}"

    if [[ ! -f "$yaml" ]]; then
        echo -e "${RED}[SKIP]${NC} ${yaml} not found"
        return 1
    fi

    echo -e "${BOLD}Recording: ${name}${NC} — ${desc}"

    # Create a wrapper script for this example
    local tmpscript
    tmpscript=$(mktemp /tmp/envpod-record-XXXXXX.sh)
    cat > "$tmpscript" << 'HELPERS'
type_cmd() {
    local display="$1"
    printf "\033[0;32m\$ \033[0m"
    for (( i=0; i<${#display}; i++ )); do
        printf "%s" "${display:$i:1}"
        sleep 0.03
    done
    sleep 0.8
    printf "\n"
}
run() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual"
    sleep 1.5
}
run_ok() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual" || true
    sleep 1.5
}
srun() { run "sudo $1" "$1"; }
srun_ok() { run_ok "sudo $1" "$1"; }
narrate() {
    echo ""
    echo -e "\033[2m# $1\033[0m"
    sleep 1.5
    echo ""
}
HELPERS

    cat >> "$tmpscript" << DEMOSCRIPT
# Title
clear
echo ""
echo -e "\033[1m\033[0;36m  ╔══════════════════════════════════════════════════════════════╗\033[0m"
echo -e "\033[1m\033[0;36m  ║\033[0m\033[1m   envpod example: ${name}\033[0;36m$(printf '%*s' $((43 - ${#name})) '')║\033[0m"
echo -e "\033[1m\033[0;36m  ║\033[0m\033[2m   ${desc}\033[0;36m$(printf '%*s' $((43 - ${#desc})) '')║\033[0m"
echo -e "\033[1m\033[0;36m  ╚══════════════════════════════════════════════════════════════╝\033[0m"
echo ""
sleep 2

# Show config
narrate "Pod configuration"
run "cat ${yaml}"

# Init
narrate "Create the pod"
srun "envpod init ${pod_name} -c ${yaml}"

# Status
narrate "Pod status"
srun "envpod status ${pod_name}"

# Security audit
narrate "Security audit"
srun "envpod audit ${pod_name} --security"

# Run a command
narrate "Run a command inside the pod"
srun "envpod run ${pod_name} -- bash -c \"echo 'hello from ${name}' > /home/agent/test.txt && ls -la /home/agent/ && echo 'Process info:' && cat /proc/self/cgroup | head -5\""

# Diff
narrate "Review changes (copy-on-write overlay)"
srun "envpod diff ${pod_name}"

# Audit log
narrate "Audit trail"
srun "envpod audit ${pod_name}"

# Cleanup
narrate "Destroy the pod"
srun "envpod destroy ${pod_name} --full"

# End
echo ""
echo -e "\033[1m\033[0;32m  envpod.dev  ·  github.com/markamo/envpod-ce  ·  BSL 1.1\033[0m"
echo ""
sleep 3
DEMOSCRIPT

    chmod +x "$tmpscript"

    # Destroy any leftover pod
    envpod destroy "$pod_name" --full 2>/dev/null || true

    local outfile="${OUT_DIR}/${name}.cast"
    asciinema rec \
        --cols "$COLS" \
        --rows "$ROWS" \
        --title "envpod example: ${name} — ${desc}" \
        --overwrite \
        -c "bash $tmpscript" \
        "$outfile"

    rm -f "$tmpscript"

    # Cleanup pod in case it survived
    envpod destroy "$pod_name" --full 2>/dev/null || true

    echo -e "${GREEN}[OK]${NC} ${outfile}"
}

# ── Preset-only list ─────────────────────────────────────────────────

PRESETS=(
    claude-code codex gemini-cli opencode aider swe-agent
    langgraph google-adk openclaw
    browser-use playwright browser
    devbox python-env nodejs desktop vscode web-display-novnc
)

ALL_EXAMPLES=(
    aider basic-cli basic-internet browser browser-use browser-wayland
    claude-code codex coding-agent demo-pod desktop devbox
    discovery-client discovery-service fuse-agent gemini-cli google-adk
    hardened-sandbox jetson-orin langgraph ml-training monitoring-policy
    nodejs openclaw opencode playwright python-env raspberry-pi
    swe-agent vscode web-display-novnc
)

# ── Main ─────────────────────────────────────────────────────────────

if [[ $EUID -ne 0 ]]; then
    echo "Error: must run as root (sudo)"
    echo "Usage: sudo bash $0 [example-names...]"
    exit 1
fi

if ! command -v asciinema &>/dev/null; then
    echo "Error: asciinema not found. Install: sudo apt install asciinema"
    exit 1
fi
if ! command -v envpod &>/dev/null; then
    echo "Error: envpod not found."
    exit 1
fi

case "${1:-}" in
    --list)
        echo "Available examples (31):"
        echo ""
        for ex in "${ALL_EXAMPLES[@]}"; do
            printf "  %-22s %s\n" "$ex" "$(describe_example "$ex")"
        done
        echo ""
        echo "Presets (18):"
        echo ""
        for ex in "${PRESETS[@]}"; do
            printf "  %-22s %s\n" "$ex" "$(describe_example "$ex")"
        done
        exit 0
        ;;
    --clean)
        echo "Cleaning up demo pods..."
        for ex in "${ALL_EXAMPLES[@]}"; do
            envpod destroy "demo-${ex}" --full 2>/dev/null || true
        done
        echo "Done."
        exit 0
        ;;
    --presets-only)
        shift
        TARGETS=("${PRESETS[@]}")
        ;;
    "")
        TARGETS=("${ALL_EXAMPLES[@]}")
        ;;
    *)
        TARGETS=("$@")
        ;;
esac

mkdir -p "$OUT_DIR"

echo ""
echo -e "${BOLD}envpod example recorder${NC}"
echo -e "Output: ${OUT_DIR}/"
echo -e "Recording ${#TARGETS[@]} example(s)"
echo ""

PASSED=0
FAILED=0
SKIPPED=0

for ex in "${TARGETS[@]}"; do
    echo ""
    echo -e "${BOLD}━━━ ${ex} ━━━${NC}"
    if record_example "$ex"; then
        PASSED=$((PASSED + 1))
    else
        FAILED=$((FAILED + 1))
    fi
    echo ""
done

echo ""
echo -e "${BOLD}${GREEN}Recording complete!${NC}"
echo -e "  Passed: ${PASSED}  Failed: ${FAILED}"
echo ""
echo "Files:"
ls -lh "$OUT_DIR"/*.cast 2>/dev/null || echo "  (none)"
echo ""
echo "Play:     asciinema play ${OUT_DIR}/<name>.cast"
echo "Upload:   asciinema upload ${OUT_DIR}/<name>.cast"
echo "GIF:      agg ${OUT_DIR}/<name>.cast <name>.gif"
