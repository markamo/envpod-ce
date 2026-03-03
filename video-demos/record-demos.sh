#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# record-demos.sh — Generate asciinema recordings for envpod demos
#
# Usage:
#   sudo bash video-demos/record-demos.sh              # record all demos
#   sudo bash video-demos/record-demos.sh 1             # record demo 1 only
#   sudo bash video-demos/record-demos.sh 1 2 5         # record demos 1, 2, 5
#   sudo bash video-demos/record-demos.sh --list        # list available demos
#   sudo bash video-demos/record-demos.sh --clean       # destroy demo pods
#
# Output: video-demos/recordings/*.cast (asciinema v2 format)
#
# Play:   asciinema play video-demos/recordings/01-install-first-pod.cast
# Upload: asciinema upload video-demos/recordings/01-install-first-pod.cast
# GIF:    agg video-demos/recordings/01-install-first-pod.cast demo1.gif
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Config ───────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SCRIPT_PATH="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${SCRIPT_DIR}/recordings"
COLS=100
ROWS=30
TYPE_DELAY=0.04        # seconds between keystrokes
CMD_PAUSE=1.0          # pause after typing before execute
OUTPUT_PAUSE=2.0       # pause after command output
SECTION_PAUSE=3.0      # pause between sections
NARRATION_PAUSE=3.0    # time to read narration text

# Colors
BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────────────────────

# Simulate typing a command character by character
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

# Type and execute a command
run() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual"
    local rc=$?
    sleep "$OUTPUT_PAUSE"
    return $rc
}

# Type and execute, don't fail on error
run_ok() {
    local display="$1"
    local actual="${2:-$1}"
    type_cmd "$display"
    eval "$actual" || true
    sleep "$OUTPUT_PAUSE"
}

# Convenience: show "sudo envpod ..." but run "envpod ..." (we're already root)
srun() {
    run "sudo $1" "$1"
}

# Same but don't fail
srun_ok() {
    run_ok "sudo $1" "$1"
}

# Show narration text (dimmed, like a voiceover caption)
narrate() {
    echo ""
    echo -e "${DIM}# $1${NC}"
    sleep "$NARRATION_PAUSE"
    echo ""
}

# Show a section header
section() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━ $1 ━━━${NC}"
    echo ""
    sleep "$SECTION_PAUSE"
}

# Show title card
title_card() {
    local w=50
    local bar
    bar=$(printf '═%.0s' $(seq 1 $w))
    clear
    echo ""
    echo ""
    printf "${BOLD}${CYAN}  ╔%s╗${NC}\n" "$bar"
    printf "${BOLD}${CYAN}  ║%-${w}s║${NC}\n" ""
    # printf pads by bytes, not display width. Multi-byte UTF-8 chars need extra padding.
    local byte_len
    byte_len=$(printf '%s' "$1" | wc -c)
    local char_len=${#1}
    local extra=$(( byte_len - char_len ))
    printf "${BOLD}${CYAN}  ║${NC}${BOLD}   %-$((w - 3 + extra))s${CYAN}║${NC}\n" "$1"
    printf "${BOLD}${CYAN}  ║${NC}${DIM}   %-$((w - 3))s${CYAN}║${NC}\n" "$2"
    printf "${BOLD}${CYAN}  ║%-${w}s║${NC}\n" ""
    printf "${BOLD}${CYAN}  ╚%s╝${NC}\n" "$bar"
    echo ""
    echo ""
    sleep "$SECTION_PAUSE"
}

# End card
end_card() {
    local w=50
    local bar
    bar=$(printf '─%.0s' $(seq 1 $w))
    echo ""
    echo ""
    printf "${BOLD}${GREEN}  ┌%s┐${NC}\n" "$bar"
    printf "${BOLD}${GREEN}  │%-${w}s│${NC}\n" ""
    printf "${BOLD}${GREEN}  │${NC}${BOLD}   %-$((w - 3))s${GREEN}│${NC}\n" "envpod.dev"
    printf "${BOLD}${GREEN}  │${NC}${BOLD}   %-$((w - 3))s${GREEN}│${NC}\n" "github.com/markamo/envpod-ce"
    printf "${BOLD}${GREEN}  │${NC}${DIM}   %-$((w - 3))s${GREEN}│${NC}\n" "Free. Open Source. BSL 1.1."
    printf "${BOLD}${GREEN}  │%-${w}s│${NC}\n" ""
    printf "${BOLD}${GREEN}  └%s┘${NC}\n" "$bar"
    echo ""
    sleep 4
}

# Destroy demo pods silently
cleanup_pods() {
    local pods=("hello" "claude-code" "claude-code-2" "openclaw" "browser" "aider" "demo-fleet-1" "demo-fleet-2" "demo-fleet-3")
    for pod in "${pods[@]}"; do
        envpod destroy "$pod" --full 2>/dev/null || true
    done
}

# Record a demo function into a .cast file
# Uses asciinema -c to re-invoke THIS script with --run-func <func_name>
record_demo() {
    local name="$1"
    local func="$2"
    local title="$3"
    local outfile="${OUT_DIR}/${name}.cast"

    echo -e "${BOLD}Recording: ${title}${NC}"
    echo -e "  Output: ${outfile}"
    echo ""

    mkdir -p "$OUT_DIR"

    asciinema rec \
        --cols "$COLS" \
        --rows "$ROWS" \
        --title "$title" \
        --overwrite \
        -c "bash '${SCRIPT_PATH}' --run-func '${func}'" \
        "$outfile"

    echo ""
    echo -e "${GREEN}[OK]${NC} Saved: ${outfile}"
    echo -e "     Play:   asciinema play ${outfile}"
    echo -e "     Upload: asciinema upload ${outfile}"
    echo ""
}

# ── Demo 1: Install to First Pod (60s) ──────────────────────────────

demo_01() {
    title_card "envpod — Install to First Pod" "Zero-trust governance in 60 seconds"

    narrate "Single binary. 5 megs. No dependencies. x86 and ARM64."

    section "See what's available"
    run "envpod presets"

    section "Create a pod with a preset"
    srun "envpod init hello --preset devbox"

    narrate "One command. Pick a preset. Pod created."

    section "Run a command inside the pod"
    srun "envpod run hello -- bash -c \"echo 'the agent wrote this' > /home/agent/hello.txt && echo 'done'\""

    narrate "The agent thinks it wrote to your filesystem. It didn't."

    section "Review the changes"
    srun "envpod diff hello"

    narrate "Every change goes to a copy-on-write overlay. You review before anything touches the host."

    section "Accept the changes"
    srun "envpod commit hello"

    narrate "Commit what you want. Roll back the rest. That's governance."

    section "Audit trail"
    srun "envpod audit hello"

    narrate "Every action logged. Append-only. Free and open source."

    # Cleanup
    envpod destroy hello --full 2>/dev/null || true

    end_card
}

# ── Demo 2: Claude Code Governed (~3 min) ────────────────────────────

demo_02() {
    title_card "Claude Code — Governed" "AI coding agent with full isolation"

    narrate "Claude Code has full access to your filesystem, API keys, git credentials. Let's fix that."

    section "18 built-in presets"
    run "envpod presets"

    narrate "Claude Code is pre-configured — DNS whitelist, resource limits, browser seccomp."

    section "Create the pod"
    srun "envpod init claude-code --preset claude-code"

    narrate "Setup runs automatically — installs Claude CLI inside the overlay."

    section "Store the API key in the encrypted vault"
    type_cmd "sudo envpod vault claude-code set ANTHROPIC_API_KEY"
    echo -e "${DIM}Enter value for ANTHROPIC_API_KEY: ••••••••••••••••••••${NC}"
    echo -e "${DIM}✓ Secret stored (ChaCha20-Poly1305 encrypted)${NC}"
    sleep "$OUTPUT_PAUSE"

    narrate "The key goes into an encrypted vault. The agent gets it as an env var — never touches disk in plaintext."

    section "Check the pod status"
    srun "envpod status claude-code"

    section "Review security posture"
    srun "envpod audit claude-code --security"

    narrate "Static analysis of the pod config. Shows exactly what attack surface you're exposing."

    section "Clone the pod — 10x faster than init"
    srun "envpod clone claude-code claude-code-2"
    srun "envpod run claude-code-2 -- echo 'I am a separate, independent pod'"

    narrate "Clone in 130ms. Same setup, independent overlay. Scale your agents."

    section "The governance loop: run → diff → commit or rollback"

    srun "envpod run claude-code -- bash -c \"mkdir -p /opt/project && echo 'def validate(x): return x > 0' > /opt/project/validator.py && echo 'wrote validator.py'\""
    srun "envpod diff claude-code"

    narrate "Everything the agent changed. Nothing reached the real filesystem yet."

    srun "envpod commit claude-code"

    narrate "Now it's on the host. If you didn't like it — rollback. Zero risk."

    section "Full audit trail"
    srun "envpod audit claude-code"

    # Cleanup
    envpod destroy claude-code --full 2>/dev/null || true
    envpod destroy claude-code-2 --full 2>/dev/null || true

    end_card
}

# ── Demo 3: OpenClaw Governed (~2.5 min) ─────────────────────────────

demo_03() {
    title_card "OpenClaw — Messaging Agent, Governed" "WhatsApp + Telegram + LLMs, safely"

    narrate "OpenClaw connects to WhatsApp, Telegram, Discord — and talks to LLMs. That's a lot of power. Let's govern it."

    section "Interactive wizard — pick a preset, customize resources"

    # Simulate the wizard since it needs stdin
    type_cmd "sudo envpod init openclaw"
    echo ""
    echo -e "  Select a preset (or 'custom' for blank config):"
    echo ""
    echo -e "   ${BOLD}Coding Agents${NC}"
    echo "    1  claude-code    Anthropic Claude Code CLI"
    echo "    2  codex          OpenAI Codex CLI"
    echo "    3  gemini-cli     Google Gemini CLI"
    echo "    4  opencode       OpenCode terminal agent"
    echo "    5  aider          Aider AI pair programmer"
    echo "    6  swe-agent      SWE-agent autonomous coder"
    echo ""
    echo -e "   ${BOLD}Frameworks${NC}"
    echo "    7  langgraph      LangGraph workflows"
    echo "    8  google-adk     Google Agent Development Kit"
    echo "    9  openclaw       OpenClaw messaging assistant"
    echo ""
    echo -e "   ${BOLD}Browser Agents${NC}"
    echo "   10  browser-use    Browser-use web automation"
    echo "   11  playwright     Playwright browser automation"
    echo "   12  browser        Headless Chrome sandbox"
    echo ""
    echo -e "   ${BOLD}Environments${NC}"
    echo "   13  devbox         General dev sandbox"
    echo "   14  python-env     Python data science"
    echo "   15  nodejs         Node.js environment"
    echo "   16  desktop        XFCE desktop via noVNC"
    echo "   17  vscode         VS Code in browser"
    echo "   18  web-display    noVNC web display"
    echo ""
    sleep 2
    echo -e "  > ${BOLD}9${NC}"
    sleep 1
    echo ""
    echo "  Customize resources:"
    sleep 0.5
    echo -e "    CPU cores [2.0]: ${BOLD}2${NC}"
    sleep 0.5
    echo -e "    Memory [1GB]: ${BOLD}2GB${NC}"
    sleep 0.5
    echo -e "    Need GPU? [y/N]: ${BOLD}n${NC}"
    sleep 1
    echo ""
    echo -e "  ${GREEN}✓${NC} Created pod 'openclaw' (openclaw preset, 2 cores, 2GB)"
    echo ""
    sleep "$SECTION_PAUSE"

    narrate "Don't know config files? The wizard lets you pick a preset, customize, and go."

    # Actually create the pod (silently, to have a real pod for the rest)
    envpod destroy openclaw --full 2>/dev/null || true
    envpod init openclaw --preset openclaw >/dev/null 2>&1

    section "Store credentials in the encrypted vault"

    type_cmd "sudo envpod vault openclaw set ANTHROPIC_API_KEY"
    echo -e "${DIM}Enter value for ANTHROPIC_API_KEY: ••••••••••••••••••••${NC}"
    echo -e "${DIM}✓ Secret stored${NC}"
    sleep 1

    type_cmd "sudo envpod vault openclaw set OPENAI_API_KEY"
    echo -e "${DIM}Enter value for OPENAI_API_KEY: ••••••••••••••••••••${NC}"
    echo -e "${DIM}✓ Secret stored${NC}"
    sleep "$OUTPUT_PAUSE"

    narrate "Each key encrypted separately. The agent gets them at runtime — never in config files."

    section "Check pod status and security"
    srun "envpod status openclaw"
    srun "envpod audit openclaw --security"

    section "Snapshot the overlay — checkpoint the state"
    srun "envpod run openclaw -- bash -c \"echo 'agent data' > /home/agent/output.txt\""
    srun "envpod snapshot openclaw create -n after-first-run"
    srun "envpod snapshot openclaw ls"

    narrate "Snapshot at any point. Restore later if something breaks."

    section "Diff and audit"
    srun "envpod diff openclaw"
    srun "envpod audit openclaw"

    # Cleanup
    envpod destroy openclaw --full 2>/dev/null || true

    end_card
}

# ── Demo 4: Chrome Wayland + GPU (90s) ───────────────────────────────

demo_04() {
    title_card "Chrome in a Pod — Wayland + GPU" "Browser agent with display, audio, and GPU"

    narrate "A browser agent needs GPU, display, and audio. Here's how envpod does it without Docker."

    section "Three browser presets"
    run_ok "envpod presets | grep -iE 'browser|playwright'"

    section "The Wayland config"
    run "cat ${REPO_DIR}/examples/browser-wayland.yaml"

    narrate "GPU, Wayland display, PipeWire audio, browser seccomp, 4GB RAM."

    section "Create the pod"
    srun "envpod init browser -c ${REPO_DIR}/examples/browser-wayland.yaml"

    section "Security comparison: Wayland vs X11"
    srun "envpod audit --security -c ${REPO_DIR}/examples/browser-wayland.yaml"

    narrate "Wayland + PipeWire: display is LOW risk, audio is MEDIUM."
    narrate "Compare that to X11 — CRITICAL, because X11 allows keylogging across windows."

    srun "envpod audit --security -c ${REPO_DIR}/examples/browser.yaml"

    section "Launch Chrome (command shown — needs display)"
    type_cmd "sudo envpod run browser -d -a -- google-chrome --no-sandbox --ozone-platform=wayland https://youtube.com"
    echo ""
    echo -e "${DIM}# Chrome window appears on the desktop with GPU acceleration"
    echo -e "# YouTube plays with full PipeWire audio"
    echo -e "# All inside a governed pod${NC}"
    sleep "$SECTION_PAUSE"

    section "Review what Chrome did"
    srun_ok "envpod diff browser"
    srun "envpod audit browser"

    narrate "Every file Chrome wrote — cookies, cache, preferences — captured in the overlay."

    section "Rollback everything"
    srun "envpod rollback browser"

    narrate "One command. All traces gone. No base image. No container OS. Just your host, governed."

    # Cleanup
    envpod destroy browser --full 2>/dev/null || true

    end_card
}

# ── Demo 5: Dashboard Fleet Control (90s) ────────────────────────────

demo_05() {
    title_card "Dashboard — Fleet Control" "Manage all your agents from the browser"

    narrate "Spin up a fleet using presets. Each agent fully isolated and governed."

    section "Create a fleet of agents"
    srun "envpod init claude-code --preset claude-code"
    srun "envpod init openclaw --preset openclaw"
    srun "envpod init aider --preset aider"

    narrate "Three agents, three presets, three commands."

    section "Generate some activity"
    srun "envpod run claude-code -- bash -c \"echo 'agent output' > /home/agent/result.txt\""
    srun "envpod run aider -- bash -c \"mkdir -p /opt/code && echo 'def hello(): pass' > /opt/code/app.py\""

    section "Fleet overview"
    srun "envpod ls"

    narrate "Every pod at a glance. Status, IP, base image."

    section "Per-pod inspection"
    srun "envpod status claude-code"
    srun "envpod diff claude-code"
    srun "envpod audit claude-code"

    section "Snapshots"
    srun "envpod snapshot claude-code create -n checkpoint-1"
    srun "envpod snapshot claude-code ls"

    narrate "Checkpoint the overlay. Restore any time if something goes wrong."

    section "Start the web dashboard"
    type_cmd "sudo envpod dashboard"
    echo ""
    echo -e "  ${GREEN}✓${NC} Dashboard running at ${BOLD}http://localhost:9090${NC}"
    echo ""
    echo -e "${DIM}# Fleet overview: pod cards with status, CPU/memory, diff counts"
    echo -e "# Click any pod → Overview, Audit, Diff, Resources, Snapshots tabs"
    echo -e "# Commit, rollback, freeze, resume — all from the browser"
    echo -e "# Create and restore snapshots with one click${NC}"
    sleep "$SECTION_PAUSE"

    section "Freeze a pod"
    srun_ok "envpod lock claude-code"

    narrate "Instant freeze. Every process suspended. Inspect, then resume or kill."

    srun_ok "envpod unlock claude-code"

    # Cleanup
    envpod destroy claude-code --full 2>/dev/null || true
    envpod destroy claude-code-2 --full 2>/dev/null || true
    envpod destroy openclaw --full 2>/dev/null || true
    envpod destroy aider --full 2>/dev/null || true

    end_card
}

# ── Main ─────────────────────────────────────────────────────────────

# Internal: asciinema calls this script with --run-func to execute a demo
# inside the recorded session (functions are defined in the same file).
if [[ "${1:-}" == "--run-func" ]]; then
    func="${2:?missing function name}"
    "$func"
    exit $?
fi

# Must run as root (envpod needs it)
if [[ $EUID -ne 0 ]]; then
    echo "Error: must run as root (sudo)"
    echo "Usage: sudo bash $0 [1 2 3 4 5]"
    exit 1
fi

# Check dependencies
if ! command -v asciinema &>/dev/null; then
    echo "Error: asciinema not found. Install: sudo apt install asciinema"
    exit 1
fi
if ! command -v envpod &>/dev/null; then
    echo "Error: envpod not found."
    exit 1
fi

# Parse args
if [[ "${1:-}" == "--list" ]]; then
    echo "Available demos:"
    echo "  1  Install to First Pod (60s teaser)"
    echo "  2  Claude Code Governed (3 min)"
    echo "  3  OpenClaw Governed (2.5 min)"
    echo "  4  Chrome Wayland + GPU (90s)"
    echo "  5  Dashboard Fleet Control (90s)"
    exit 0
fi

if [[ "${1:-}" == "--clean" ]]; then
    echo "Cleaning up demo pods..."
    cleanup_pods
    echo "Done."
    exit 0
fi

# Select which demos to record
DEMOS=("${@:-1 2 3 4 5}")
if [[ ${#@} -eq 0 ]]; then
    DEMOS=(1 2 3 4 5)
fi

mkdir -p "$OUT_DIR"

echo ""
echo -e "${BOLD}envpod demo recorder${NC}"
echo -e "Output directory: ${OUT_DIR}"
echo -e "Recording demos: ${DEMOS[*]}"
echo ""

# Clean up any leftover pods from previous recordings
cleanup_pods

for d in "${DEMOS[@]}"; do
    case "$d" in
        1) record_demo "01-install-first-pod"    "demo_01" "envpod — Install to First Pod in 60s"   ;;
        2) record_demo "02-claude-code-governed"  "demo_02" "Claude Code — Governed"                 ;;
        3) record_demo "03-openclaw-governed"     "demo_03" "OpenClaw — Messaging Agent, Governed"   ;;
        4) record_demo "04-chrome-wayland-gpu"    "demo_04" "Chrome in a Pod — Wayland + GPU"        ;;
        5) record_demo "05-dashboard-fleet-ctrl"  "demo_05" "Dashboard — Fleet Control"              ;;
        *) echo "Unknown demo: $d (use 1-5)"; exit 1 ;;
    esac
done

echo ""
echo -e "${BOLD}${GREEN}All recordings complete!${NC}"
echo ""
echo "Files:"
ls -lh "$OUT_DIR"/*.cast 2>/dev/null
echo ""
echo "Play:     asciinema play ${OUT_DIR}/<file>.cast"
echo "Upload:   asciinema upload ${OUT_DIR}/<file>.cast"
echo "GIF:      agg ${OUT_DIR}/<file>.cast output.gif"
echo "SVG:      svg-term --in ${OUT_DIR}/<file>.cast --out output.svg"
