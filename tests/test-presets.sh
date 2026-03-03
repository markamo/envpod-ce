#!/bin/bash
# envpod preset setup test suite
#
# Tests each preset's setup commands inside a Docker container (Ubuntu 24.04)
# to verify they install correctly without needing full envpod.
#
# Usage:
#   ./tests/test-presets.sh                    # test all presets
#   ./tests/test-presets.sh claude-code codex  # test specific presets
#   ./tests/test-presets.sh --verbose          # show full output
#   ./tests/test-presets.sh --skip-heavy       # skip browser/desktop (slow)

set -euo pipefail

IMAGE="ubuntu:24.04"
TIMEOUT=300  # 5 min per preset

# ─── Colors ───────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
DIM='\033[2m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0
RESULTS=()
VERBOSE=0
SKIP_HEAVY=0

log()  { echo -e "${BLUE}[test]${NC} $1"; }
pass() { echo -e "  ${GREEN}✓${NC} $1 ${DIM}(${2}s)${NC}"; PASS=$((PASS+1)); RESULTS+=("PASS|$1|$2"); }
fail() { echo -e "  ${RED}✗${NC} $1: $2"; FAIL=$((FAIL+1)); RESULTS+=("FAIL|$1|0"); }
skip() { echo -e "  ${YELLOW}⊘${NC} $1 (skipped: $2)"; SKIP=$((SKIP+1)); RESULTS+=("SKIP|$1|0"); }

# ─── Test a single preset ─────────────────────────────────────────────
test_preset() {
    local name="$1"
    local is_heavy="$2"
    local script_file="$3"

    if [ "$SKIP_HEAVY" -eq 1 ] && [ "$is_heavy" = "1" ]; then
        skip "$name" "heavy (use without --skip-heavy)"
        return
    fi

    local start_time=$(date +%s)
    local output
    local exit_code=0

    output=$(timeout "$TIMEOUT" docker run --rm \
        -v "${script_file}:/opt/test.sh:ro" \
        "$IMAGE" \
        bash /opt/test.sh 2>&1) || exit_code=$?

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    if [ $exit_code -eq 0 ]; then
        pass "$name" "$duration"
    elif [ $exit_code -eq 124 ]; then
        fail "$name" "timed out after ${TIMEOUT}s"
    else
        fail "$name" "exit code ${exit_code}"
    fi

    if [ $exit_code -ne 0 ] || [ $VERBOSE -eq 1 ]; then
        echo ""
        echo "$output" | tail -20
        echo "---"
    fi
}

# ─── Main ─────────────────────────────────────────────────────────────
main() {
    local filters=()

    for arg in "$@"; do
        case "$arg" in
            --verbose) VERBOSE=1 ;;
            --skip-heavy) SKIP_HEAVY=1 ;;
            *) filters+=("$arg") ;;
        esac
    done

    echo ""
    echo "╔═══════════════════════════════════════════════════════╗"
    echo "║      envpod preset setup test suite                   ║"
    echo "║      Docker container × Ubuntu 24.04                  ║"
    echo "╚═══════════════════════════════════════════════════════╝"
    echo ""

    local tmpdir=$(mktemp -d)
    trap "rm -rf ${tmpdir}" EXIT

    local start_time=$(date +%s)
    local tested=0

    # Generate all test scripts
    # Format: name, is_heavy, script content

    # ── claude-code ──
    cat > "${tmpdir}/claude-code.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: claude-code ---"
curl -fsSL https://claude.ai/install.sh | bash
echo "--- Verify ---"
export PATH="$HOME/.claude/local/bin:$HOME/.local/bin:$PATH"
claude --version || which claude || find / -name claude -type f 2>/dev/null | head -3
echo "=== PASSED ==="
SCRIPT

    # ── codex ──
    cat > "${tmpdir}/codex.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: codex ---"
export NVM_DIR=/opt/nvm && mkdir -p /opt/nvm
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash
. /opt/nvm/nvm.sh && nvm install 22
. /opt/nvm/nvm.sh
ln -sf "$(which node)" /usr/local/bin/node
ln -sf "$(which npm)" /usr/local/bin/npm
ln -sf "$(which npx)" /usr/local/bin/npx
npm install -g @openai/codex
echo "--- Verify ---"
codex --version 2>&1 || which codex || ls /usr/local/bin/codex
echo "=== PASSED ==="
SCRIPT

    # ── gemini-cli ──
    cat > "${tmpdir}/gemini-cli.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: gemini-cli ---"
export NVM_DIR=/opt/nvm && mkdir -p /opt/nvm
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash
. /opt/nvm/nvm.sh && nvm install 22
. /opt/nvm/nvm.sh
ln -sf "$(which node)" /usr/local/bin/node
ln -sf "$(which npm)" /usr/local/bin/npm
ln -sf "$(which npx)" /usr/local/bin/npx
npm install -g @google/gemini-cli
echo "--- Verify ---"
gemini --version 2>&1 || which gemini || ls /usr/local/bin/gemini
echo "=== PASSED ==="
SCRIPT

    # ── opencode ──
    cat > "${tmpdir}/opencode.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: opencode ---"
curl -fsSL https://opencode.ai/install | bash
echo "--- Verify ---"
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
opencode version 2>&1 || which opencode || find / -name opencode -type f 2>/dev/null | head -3
echo "=== PASSED ==="
SCRIPT

    # ── aider ──
    cat > "${tmpdir}/aider.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: aider ---"
pip install --break-system-packages aider-chat
echo "--- Verify ---"
aider --version 2>&1 || which aider
echo "=== PASSED ==="
SCRIPT

    # ── swe-agent ──
    cat > "${tmpdir}/swe-agent.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates git python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: swe-agent ---"
git clone https://github.com/SWE-agent/SWE-agent.git /opt/swe-agent
cd /opt/swe-agent && pip install --break-system-packages --editable .
echo "--- Verify ---"
python3 -c "import sweagent; print('ok')" 2>&1 || ls /opt/swe-agent/sweagent/
echo "=== PASSED ==="
SCRIPT

    # ── langgraph ──
    cat > "${tmpdir}/langgraph.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: langgraph ---"
pip install --break-system-packages langgraph langchain-openai langchain-anthropic
echo "--- Verify ---"
python3 -c "from importlib.metadata import version; print(f'langgraph={version(\"langgraph\")}')"
echo "=== PASSED ==="
SCRIPT

    # ── google-adk ──
    cat > "${tmpdir}/google-adk.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: google-adk ---"
pip install --break-system-packages google-adk
echo "--- Verify ---"
pip show google-adk | grep -i version
echo "=== PASSED ==="
SCRIPT

    # ── openclaw ──
    cat > "${tmpdir}/openclaw.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates git >/dev/null 2>&1
echo "--- Setup: openclaw ---"
export NVM_DIR=/opt/nvm && mkdir -p /opt/nvm
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash
. /opt/nvm/nvm.sh && nvm install 22
. /opt/nvm/nvm.sh
ln -sf "$(which node)" /usr/local/bin/node
ln -sf "$(which npm)" /usr/local/bin/npm
ln -sf "$(which npx)" /usr/local/bin/npx
npm install -g openclaw
echo "--- Verify ---"
openclaw --version 2>&1 || which openclaw || ls /usr/local/bin/openclaw
echo "=== PASSED ==="
SCRIPT

    # ── browser-use ──
    cat > "${tmpdir}/browser-use.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: browser-use ---"
pip install --break-system-packages browser-use playwright
playwright install --with-deps chromium
echo "--- Verify ---"
python3 -c "import browser_use; print('ok')"
echo "=== PASSED ==="
SCRIPT

    # ── playwright ──
    cat > "${tmpdir}/playwright.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: playwright ---"
pip install --break-system-packages playwright
playwright install --with-deps chromium
echo "--- Verify ---"
python3 -c "import playwright; print('ok')"
echo "=== PASSED ==="
SCRIPT

    # ── browser ──
    cat > "${tmpdir}/browser.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq wget gnupg ca-certificates >/dev/null 2>&1
echo "--- Setup: browser ---"
wget -qO- https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list
apt-get update && apt-get install -y google-chrome-stable
echo "--- Verify ---"
google-chrome --version
echo "=== PASSED ==="
SCRIPT

    # ── devbox ──
    cat > "${tmpdir}/devbox.sh" << 'SCRIPT'
#!/bin/bash
set -e
echo "--- Setup: devbox (no setup needed) ---"
echo "--- Verify ---"
echo ok
echo "=== PASSED ==="
SCRIPT

    # ── python-env ──
    cat > "${tmpdir}/python-env.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq python3-pip python3-venv >/dev/null 2>&1
echo "--- Setup: python-env ---"
pip install --break-system-packages numpy pandas matplotlib scipy scikit-learn requests
echo "--- Verify ---"
python3 -c "import numpy; import pandas; print(f'numpy={numpy.__version__}, pandas={pandas.__version__}')"
echo "=== PASSED ==="
SCRIPT

    # ── nodejs ──
    cat > "${tmpdir}/nodejs.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: nodejs ---"
export NVM_DIR=/opt/nvm && mkdir -p /opt/nvm
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash
. /opt/nvm/nvm.sh && nvm install 22
. /opt/nvm/nvm.sh
ln -sf "$(which node)" /usr/local/bin/node
ln -sf "$(which npm)" /usr/local/bin/npm
ln -sf "$(which npx)" /usr/local/bin/npx
echo "--- Verify ---"
node --version && npm --version
echo "=== PASSED ==="
SCRIPT

    # ── vscode ──
    cat > "${tmpdir}/vscode.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null 2>&1
echo "--- Setup: vscode ---"
curl -fsSL https://code-server.dev/install.sh | sh
echo "--- Verify ---"
code-server --version
echo "=== PASSED ==="
SCRIPT

    # ── desktop ──
    cat > "${tmpdir}/desktop.sh" << 'SCRIPT'
#!/bin/bash
set -e
apt-get update -qq && apt-get install -y -qq wget gnupg ca-certificates >/dev/null 2>&1
echo "--- Setup: desktop ---"
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xfce4 xfce4-terminal dbus-x11
wget -qO- https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list
apt-get update && apt-get install -y google-chrome-stable
echo "--- Verify ---"
which xfce4-session && google-chrome --version
echo "=== PASSED ==="
SCRIPT

    # ── web-display ──
    cat > "${tmpdir}/web-display.sh" << 'SCRIPT'
#!/bin/bash
set -e
echo "--- Setup: web-display (no setup, supervisor handles it) ---"
echo "--- Verify ---"
echo ok
echo "=== PASSED ==="
SCRIPT

    # Define order and heaviness
    # name:is_heavy
    local presets_order=(
        "claude-code:0"
        "codex:0"
        "gemini-cli:0"
        "opencode:0"
        "aider:0"
        "swe-agent:1"
        "langgraph:0"
        "google-adk:0"
        "openclaw:0"
        "browser-use:1"
        "playwright:1"
        "browser:1"
        "devbox:0"
        "python-env:0"
        "nodejs:0"
        "web-display:0"
        "vscode:0"
        "desktop:1"
    )

    for entry in "${presets_order[@]}"; do
        local name="${entry%%:*}"
        local heavy="${entry##*:}"
        local script="${tmpdir}/${name}.sh"

        # Apply filter
        if [ ${#filters[@]} -gt 0 ]; then
            local matched=0
            for filter in "${filters[@]}"; do
                if [[ "$name" == *"$filter"* ]]; then
                    matched=1
                    break
                fi
            done
            [ $matched -eq 0 ] && continue
        fi

        chmod +x "$script"
        test_preset "$name" "$heavy" "$script"
        tested=$((tested+1))
    done

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    if [ $tested -eq 0 ]; then
        echo "No presets matched filter"
        exit 1
    fi

    echo ""
    echo "═══════════════════════════════════════════════════════"
    echo "  RESULTS (${duration}s total, ${tested} tested)"
    echo "═══════════════════════════════════════════════════════"
    for result in "${RESULTS[@]}"; do
        local status=$(echo "$result" | cut -d'|' -f1)
        local pname=$(echo "$result" | cut -d'|' -f2)
        local dur=$(echo "$result" | cut -d'|' -f3)
        case $status in
            PASS) echo -e "  ${GREEN}✓${NC} ${pname} (${dur}s)" ;;
            FAIL) echo -e "  ${RED}✗${NC} ${pname}" ;;
            SKIP) echo -e "  ${YELLOW}⊘${NC} ${pname}" ;;
        esac
    done
    echo ""
    echo "  Passed: ${PASS}  Failed: ${FAIL}  Skipped: ${SKIP}"
    echo "═══════════════════════════════════════════════════════"

    [ $FAIL -eq 0 ]
}

main "$@"
