#!/bin/bash
# envpod multi-distro test suite v2
# Tests the FULL user experience across 9 Linux distributions:
#   Phase 1: Download and extract tarball
#   Phase 2: Portable binary test (./envpod from extracted folder)
#   Phase 3: install.sh
#   Phase 4: Governance tests
#
# Usage:
#   sudo ./test-distros-v2.sh              # test all distros
#   sudo ./test-distros-v2.sh ubuntu       # test matching distros
#   sudo ./test-distros-v2.sh --verbose    # show full output

set -euo pipefail

ENVPOD_URL="https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz"

# ─── Distro definitions ──────────────────────────────────────────────
# Prerequisites:
#   curl + ca-certificates  (HTTPS download from GitHub)
#   tar                     (extract tarball)
#   iptables, iproute2      (install.sh requires)
#   procps                  (sysctl)
#   kmod                    (modprobe)
DISTROS=(
  # Tier 1
  "ubuntu-24.04|ubuntu:24.04|apt-get update && apt-get install -y --no-install-recommends curl ca-certificates tar iptables iproute2 procps kmod"
  "debian-12|debian:12|apt-get update && apt-get install -y --no-install-recommends curl ca-certificates tar iptables iproute2 procps kmod"
  "fedora-41|fedora:41|dnf install -y curl tar iptables iproute procps-ng kmod && mkdir -p /etc/sysctl.d"

  # Tier 2
  "arch|archlinux:latest|pacman -Sy --noconfirm curl tar iptables iproute2 procps-ng kmod"
  "ubuntu-22.04|ubuntu:22.04|apt-get update && apt-get install -y --no-install-recommends curl ca-certificates tar iptables iproute2 procps kmod"
  "rocky-9|rockylinux:9|dnf install -y --allowerasing curl tar iptables iproute procps-ng kmod && mkdir -p /etc/sysctl.d"
  "alma-9|almalinux:9|dnf install -y --allowerasing curl tar iptables iproute procps-ng kmod && mkdir -p /etc/sysctl.d"
  "opensuse|opensuse/leap:15.6|zypper install -y curl tar gzip iptables iproute2 procps kmod && mkdir -p /etc/sysctl.d"

  # Tier 3
  "amazon-linux|amazonlinux:2023|dnf install -y --allowerasing curl tar gzip iptables iproute procps-ng kmod && mkdir -p /etc/sysctl.d"
)

# ─── Colors ───────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

# ─── Results tracking ────────────────────────────────────────────────
PASS=0
FAIL=0
RESULTS=()
VERBOSE=0

log()  { echo -e "${BLUE}[envpod-test]${NC} $1"; }
pass() { echo -e "${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); RESULTS+=("PASS|$1"); }
fail() { echo -e "${RED}[FAIL]${NC} $1: $2"; FAIL=$((FAIL+1)); RESULTS+=("FAIL|$1"); }

# ─── Test script that runs inside each container ─────────────────────
generate_test_script() {
    local url="$1"
    cat << TESTEOF
#!/bin/bash
set -e

ERRORS=0
TOTAL=0
DETAILS=""
error() { echo "  FAIL: \$1"; ERRORS=\$((ERRORS+1)); TOTAL=\$((TOTAL+1)); DETAILS="\${DETAILS}  FAIL: \$1\n"; }
ok()    { echo "  OK: \$1"; TOTAL=\$((TOTAL+1)); }

echo "=== envpod distro test v2 ==="
echo "Distro: \$(cat /etc/os-release 2>/dev/null | grep PRETTY_NAME | cut -d= -f2 | tr -d '"')"
echo "Kernel: \$(uname -r)"
echo ""

# ══════════════════════════════════════════════════════════════════════
# Phase 1: Download and Extract
# ══════════════════════════════════════════════════════════════════════

echo "=== Phase 1: Download ==="
cd /tmp
if curl -fsSL "${url}" | tar xz; then
    ok "download and extract"
else
    error "download failed"
    exit 1
fi

ENVPOD_DIR=\$(ls -d envpod-*-linux-x86_64 2>/dev/null | head -1)
if [ -z "\$ENVPOD_DIR" ]; then
    error "extracted directory not found"
    exit 1
fi
cd "\$ENVPOD_DIR"
ok "found \$ENVPOD_DIR"

# ══════════════════════════════════════════════════════════════════════
# Phase 2: Portable Binary (no install)
# ══════════════════════════════════════════════════════════════════════

echo ""
echo "=== Phase 2: Portable binary ==="

if [ -f ./envpod ]; then
    ok "binary exists"
else
    error "binary not found in extracted folder"
    exit 1
fi

if [ -x ./envpod ]; then
    ok "binary is executable"
else
    chmod +x ./envpod
    ok "binary made executable"
fi

PORTABLE_VER=\$(./envpod --version 2>&1) || true
if echo "\$PORTABLE_VER" | grep -q "envpod"; then
    ok "portable run: \$PORTABLE_VER"
else
    error "portable binary failed: \$PORTABLE_VER"
fi

# Quick portable test — init and run without install
if ./envpod init portable-test -c examples/basic-internet.yaml 2>&1; then
    ok "portable init"
else
    error "portable init failed"
fi

POUT=\$(./envpod run portable-test -- echo "portable-governed" 2>&1) || true
if echo "\$POUT" | grep -q "portable-governed"; then
    ok "portable run"
else
    error "portable run failed: \$POUT"
fi

./envpod destroy portable-test 2>&1 || true

# ══════════════════════════════════════════════════════════════════════
# Phase 3: install.sh
# ══════════════════════════════════════════════════════════════════════

echo ""
echo "=== Phase 3: install.sh ==="

if bash install.sh 2>&1; then
    ok "install.sh completed"
else
    error "install.sh failed"
    # Continue anyway to see what did install
fi

# Verify install results
if command -v envpod &>/dev/null; then
    ok "envpod on PATH: \$(envpod --version 2>&1)"
else
    error "envpod not on PATH after install"
    exit 1
fi

if [ -f /usr/local/bin/envpod ]; then
    ok "binary at /usr/local/bin/envpod"
else
    error "binary not at /usr/local/bin/envpod"
fi

if [ -d /var/lib/envpod ]; then
    ok "state dir /var/lib/envpod exists"
else
    error "state dir missing"
fi

if [ -d /usr/local/share/envpod/examples ]; then
    EXAMPLE_COUNT=\$(ls /usr/local/share/envpod/examples/*.yaml 2>/dev/null | wc -l)
    ok "examples installed (\${EXAMPLE_COUNT} configs)"
else
    error "examples not installed"
fi

if [ -f /etc/bash_completion.d/envpod ]; then
    ok "bash completions installed"
else
    error "bash completions missing"
fi

if [ -f /usr/local/share/envpod/uninstall.sh ]; then
    ok "uninstall script installed"
else
    error "uninstall script missing"
fi

# ══════════════════════════════════════════════════════════════════════
# Phase 4: Governance Tests (using installed envpod)
# ══════════════════════════════════════════════════════════════════════

echo ""
echo "=== Phase 4: Governance tests ==="

echo ""
echo "--- Test 1: init ---"
if envpod init test-pod -c /usr/local/share/envpod/examples/basic-internet.yaml 2>&1; then
    ok "init"
else
    error "init failed"
fi

echo ""
echo "--- Test 2: run ---"
OUTPUT=\$(envpod run test-pod -- echo "governed-output" 2>&1) || true
if echo "\$OUTPUT" | grep -q "governed-output"; then
    ok "run"
else
    error "run failed: \$OUTPUT"
fi

echo ""
echo "--- Test 3: diff ---"
envpod run test-pod -- bash -c "mkdir -p /home/agent && echo 'test-data' > /home/agent/test-file.txt" 2>&1 || true
DIFF=\$(envpod diff test-pod --all 2>&1) || true
if echo "\$DIFF" | grep -q "test-file"; then
    ok "diff shows changes"
else
    error "diff failed: \$DIFF"
fi

echo ""
echo "--- Test 4: rollback ---"
if envpod rollback test-pod 2>&1; then
    ok "rollback"
else
    error "rollback failed"
fi

echo ""
echo "--- Test 5: commit ---"
envpod run test-pod -- bash -c "mkdir -p /home/agent && echo 'commit-data' > /home/agent/commit-test.txt" 2>&1 || true
if envpod commit test-pod 2>&1; then
    ok "commit"
else
    error "commit failed"
fi

echo ""
echo "--- Test 6: snapshot ---"
if envpod snapshot test-pod create 2>&1; then
    ok "snapshot create"
else
    error "snapshot create failed"
fi
SNAP_LS=\$(envpod snapshot test-pod ls 2>&1) || true
if echo "\$SNAP_LS" | grep -qi "snap\|auto\|20"; then
    ok "snapshot list"
else
    error "snapshot list failed: \$SNAP_LS"
fi

echo ""
echo "--- Test 7: vault ---"
if echo "test-secret-value" | envpod vault test-pod set TEST_KEY 2>&1; then
    ok "vault set"
else
    error "vault set failed"
fi
VAULT_LS=\$(envpod vault test-pod list 2>&1) || true
if echo "\$VAULT_LS" | grep -qi "TEST_KEY"; then
    ok "vault list"
else
    error "vault list failed: \$VAULT_LS"
fi

echo ""
echo "--- Test 8: ls ---"
LS_OUT=\$(envpod ls 2>&1) || true
if echo "\$LS_OUT" | grep -q "test-pod"; then
    ok "ls shows pod"
else
    error "ls failed: \$LS_OUT"
fi

echo ""
echo "--- Test 9: clone ---"
START_NS=\$(date +%s%N)
if envpod clone test-pod clone-target 2>&1; then
    END_NS=\$(date +%s%N)
    CLONE_MS=\$(( (END_NS - START_NS) / 1000000 ))
    ok "clone in \${CLONE_MS}ms"
    envpod destroy clone-target 2>&1 || true
else
    error "clone failed"
fi

echo ""
echo "--- Test 10: destroy ---"
if envpod destroy test-pod 2>&1; then
    ok "destroy"
else
    error "destroy failed"
fi

# ══════════════════════════════════════════════════════════════════════
# Summary
# ══════════════════════════════════════════════════════════════════════

echo ""
echo "================================"
PASSED=\$((TOTAL - ERRORS))
echo "\$PASSED/\$TOTAL tests passed"
if [ \$ERRORS -eq 0 ]; then
    echo "ALL TESTS PASSED"
    exit 0
else
    echo "\$ERRORS TEST(S) FAILED"
    echo ""
    echo -e "\$DETAILS"
    exit 1
fi
TESTEOF
}

# ─── Run test for a single distro ────────────────────────────────────
test_distro() {
    local entry="$1"
    local name=$(echo "$entry" | cut -d'|' -f1)
    local image=$(echo "$entry" | cut -d'|' -f2)
    local pkg_setup=$(echo "$entry" | cut -d'|' -f3)

    log "Testing ${name} (${image})..."

    rm -rf "/tmp/envpod-distro-test-${name}"
    mkdir -p "/tmp/envpod-distro-test-${name}"

    local tmpdir=$(mktemp -d)
    generate_test_script "$ENVPOD_URL" > "${tmpdir}/test.sh"
    chmod +x "${tmpdir}/test.sh"

    local full_cmd="${pkg_setup} && bash /opt/test.sh"

    local output
    local exit_code=0
    output=$(timeout 300 docker run --rm \
        --privileged \
        --cgroupns=host \
        -v "/tmp/envpod-distro-test-${name}:/var/lib/envpod" \
        -v "/sys/fs/cgroup:/sys/fs/cgroup:rw" \
        -v "${tmpdir}/test.sh:/opt/test.sh:ro" \
        "${image}" \
        bash -c "${full_cmd}" 2>&1) || exit_code=$?

    if [ $exit_code -eq 0 ]; then
        pass "${name}"
    elif [ $exit_code -eq 124 ]; then
        fail "${name}" "timed out after 300s"
    else
        fail "${name}" "exit code ${exit_code}"
    fi

    if [ $exit_code -ne 0 ] || [ $VERBOSE -eq 1 ]; then
        echo "$output" | grep -E "(OK|FAIL|error|Error|\[✓\]|\[✗\]|\[!\])" | head -30
        echo "---"
        echo "$output" | tail -10
    fi

    rm -rf "${tmpdir}"
    rm -rf "/tmp/envpod-distro-test-${name}"

    echo ""
}

# ─── Main ─────────────────────────────────────────────────────────────
main() {
    local filter="all"

    for arg in "$@"; do
        if [ "$arg" = "--verbose" ]; then
            VERBOSE=1
        else
            filter="$arg"
        fi
    done

    echo ""
    echo "╔═══════════════════════════════════════════════════════╗"
    echo "║      envpod multi-distro test suite v2               ║"
    echo "║      download → portable → install → govern          ║"
    echo "╚═══════════════════════════════════════════════════════╝"
    echo ""

    local tested=0
    local start_time=$(date +%s)

    for entry in "${DISTROS[@]}"; do
        local name=$(echo "$entry" | cut -d'|' -f1)

        if [ "$filter" != "all" ] && [[ "$name" != *"$filter"* ]]; then
            continue
        fi

        test_distro "$entry"
        tested=$((tested+1))
    done

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    if [ $tested -eq 0 ]; then
        echo "No distros matched filter: ${filter}"
        exit 1
    fi

    echo ""
    echo "═══════════════════════════════════════════════════════"
    echo "  RESULTS SUMMARY (${duration}s)"
    echo "═══════════════════════════════════════════════════════"
    for result in "${RESULTS[@]}"; do
        local status=$(echo "$result" | cut -d'|' -f1)
        local distro=$(echo "$result" | cut -d'|' -f2)
        case $status in
            PASS) echo -e "  ${GREEN}✓${NC} ${distro}" ;;
            FAIL) echo -e "  ${RED}✗${NC} ${distro}" ;;
        esac
    done
    echo ""
    echo "  Passed: ${PASS}  Failed: ${FAIL}"
    echo "═══════════════════════════════════════════════════════"

    [ $FAIL -eq 0 ]
}

main "$@"
