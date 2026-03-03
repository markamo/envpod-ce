#!/bin/bash
# envpod multi-distro test runner
# Tests basic envpod functionality across popular Linux distributions
#
# Usage:
#   ./test-distros.sh              # test all distros
#   ./test-distros.sh ubuntu       # test one distro
#   ./test-distros.sh --build-only # just build images, don't test

set -euo pipefail

# ─── Distro definitions ──────────────────────────────────────────────
# Format: name|image|package_manager_setup
DISTROS=(
  # Tier 1 — must work on launch day
  "ubuntu-24.04|ubuntu:24.04|apt-get update && apt-get install -y curl iptables iproute2 net-tools dnsutils"
  "debian-12|debian:12|apt-get update && apt-get install -y curl iptables iproute2 net-tools dnsutils"
  "fedora-41|fedora:41|dnf install -y curl iptables iproute net-tools bind-utils"

  # Tier 2 — catch the vocal minority
  "arch|archlinux:latest|pacman -Sy --noconfirm curl iptables iproute2 net-tools bind"
  "ubuntu-22.04|ubuntu:22.04|apt-get update && apt-get install -y curl iptables iproute2 net-tools dnsutils"
  "rocky-9|rockylinux:9|dnf install -y curl iptables iproute net-tools bind-utils"
  "alma-9|almalinux:9|dnf install -y curl iptables iproute net-tools bind-utils"
  "opensuse|opensuse/leap:15.6|zypper install -y curl iptables iproute2 net-tools bind-utils"

  # Tier 3 — nice to have
  "amazon-linux|amazonlinux:2023|dnf install -y curl iptables iproute net-tools bind-utils"
)

# ─── Colors ───────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# ─── Results tracking ────────────────────────────────────────────────
PASS=0
FAIL=0
SKIP=0
RESULTS=()

log()  { echo -e "${BLUE}[envpod-test]${NC} $1"; }
pass() { echo -e "${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); RESULTS+=("PASS|$1"); }
fail() { echo -e "${RED}[FAIL]${NC} $1"; FAIL=$((FAIL+1)); RESULTS+=("FAIL|$1"); }
skip() { echo -e "${YELLOW}[SKIP]${NC} $1"; SKIP=$((SKIP+1)); RESULTS+=("SKIP|$1"); }

# ─── Test script that runs inside each container ─────────────────────
generate_test_script() {
    cat << 'TESTEOF'
#!/bin/bash
set -e

ERRORS=0
error() { echo "  FAIL: $1"; ERRORS=$((ERRORS+1)); }
ok()    { echo "  OK: $1"; }

echo "=== envpod distro test ==="
echo "Distro: $(cat /etc/os-release | grep PRETTY_NAME | cut -d= -f2 | tr -d '"')"
echo "Kernel: $(uname -r)"
echo ""

# Enable IP forwarding
echo 1 > /proc/sys/net/ipv4/ip_forward

# Install envpod
echo "--- Installing envpod ---"
cd /tmp
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
./install.sh
envpod --version && ok "envpod installed" || error "envpod install failed"

# Test 1: init
echo ""
echo "--- Test 1: envpod init ---"
envpod init test-pod -c examples/basic-internet.yaml && ok "init" || error "init failed"

# Test 2: run a simple command
echo ""
echo "--- Test 2: envpod run ---"
OUTPUT=$(envpod run test-pod -- echo "governed-output" 2>&1)
echo "$OUTPUT" | grep -q "governed-output" && ok "run" || error "run failed: $OUTPUT"

# Test 3: filesystem overlay — write and diff
echo ""
echo "--- Test 3: filesystem overlay ---"
envpod run test-pod -- bash -c "echo 'test-data' > /tmp/test-file.txt" 2>&1
DIFF=$(envpod diff test-pod 2>&1)
echo "$DIFF" | grep -q "test-file.txt" && ok "diff shows changes" || error "diff failed: $DIFF"

# Test 4: rollback
echo ""
echo "--- Test 4: rollback ---"
envpod rollback test-pod && ok "rollback" || error "rollback failed"

# Test 5: commit
echo ""
echo "--- Test 5: commit ---"
envpod run test-pod -- bash -c "echo 'commit-data' > /tmp/commit-test.txt" 2>&1
envpod commit test-pod && ok "commit" || error "commit failed"

# Test 6: snapshot
echo ""
echo "--- Test 6: snapshot ---"
envpod snapshot test-pod create snap1 && ok "snapshot create" || error "snapshot create failed"
envpod snapshot test-pod ls 2>&1 | grep -q "snap1" && ok "snapshot list" || error "snapshot list failed"

# Test 7: vault
echo ""
echo "--- Test 7: credential vault ---"
echo "test-secret-value" | envpod vault test-pod set TEST_KEY 2>&1 && ok "vault set" || error "vault set failed"
envpod vault test-pod list 2>&1 | grep -q "TEST_KEY" && ok "vault list" || error "vault list failed"

# Test 8: network — curl by IP from inside pod
echo ""
echo "--- Test 8: network (IP) ---"
NET_OUT=$(envpod run test-pod -- curl -s --max-time 10 http://142.251.32.174 2>&1)
echo "$NET_OUT" | grep -qi "moved\|google\|html" && ok "network IP" || error "network IP failed (may be Docker networking): $NET_OUT"

# Test 9: network — DNS resolution from inside pod
echo ""
echo "--- Test 9: network (DNS) ---"
DNS_OUT=$(envpod run test-pod -- curl -s --max-time 10 http://google.com 2>&1)
echo "$DNS_OUT" | grep -qi "moved\|google\|html" && ok "network DNS" || error "network DNS failed: $DNS_OUT"

# Test 10: destroy
echo ""
echo "--- Test 10: destroy ---"
envpod destroy test-pod && ok "destroy" || error "destroy failed"

# Test 11: clone speed
echo ""
echo "--- Test 11: clone ---"
envpod init clone-source -c examples/basic-internet.yaml 2>&1
START=$(date +%s%N)
envpod clone clone-source clone-target 2>&1
END=$(date +%s%N)
CLONE_MS=$(( (END - START) / 1000000 ))
ok "clone completed in ${CLONE_MS}ms"
envpod destroy clone-source 2>&1
envpod destroy clone-target 2>&1

# Summary
echo ""
echo "================================"
if [ $ERRORS -eq 0 ]; then
    echo "ALL TESTS PASSED"
    exit 0
else
    echo "$ERRORS TEST(S) FAILED"
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

    # Create temp dir for test script
    local tmpdir=$(mktemp -d)
    generate_test_script > "${tmpdir}/test.sh"
    chmod +x "${tmpdir}/test.sh"

    # Build inline test command
    local full_cmd="${pkg_setup} && bash /opt/test.sh"

    # Run container with timeout
    local output
    if output=$(timeout 300 docker run --rm \
        --privileged \
        --cgroupns=host \
        -v /tmp/envpod-distro-test-${name}:/var/lib/envpod \
        -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
        -v "${tmpdir}/test.sh:/opt/test.sh:ro" \
        "${image}" \
        bash -c "${full_cmd}" 2>&1); then
        pass "${name}"
    else
        local exit_code=$?
        if [ $exit_code -eq 124 ]; then
            fail "${name} (timed out after 300s)"
        else
            fail "${name} (exit code ${exit_code})"
        fi
        # Print last 20 lines of output on failure
        echo "$output" | tail -20
    fi

    # Cleanup
    rm -rf "${tmpdir}"
    docker volume rm "envpod-distro-test-${name}" 2>/dev/null || true
    rm -rf "/tmp/envpod-distro-test-${name}"

    echo ""
}

# ─── Main ─────────────────────────────────────────────────────────────
main() {
    local filter="${1:-all}"

    echo ""
    echo "╔═══════════════════════════════════════════════════════╗"
    echo "║        envpod multi-distro test suite                ║"
    echo "╚═══════════════════════════════════════════════════════╝"
    echo ""

    local start_time=$(date +%s)

    for entry in "${DISTROS[@]}"; do
        local name=$(echo "$entry" | cut -d'|' -f1)

        if [ "$filter" != "all" ] && [[ "$name" != *"$filter"* ]]; then
            continue
        fi

        test_distro "$entry"
    done

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    # Print summary table
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
            SKIP) echo -e "  ${YELLOW}○${NC} ${distro}" ;;
        esac
    done
    echo ""
    echo "  Passed: ${PASS}  Failed: ${FAIL}  Skipped: ${SKIP}"
    echo "═══════════════════════════════════════════════════════"

    # Exit with failure if any tests failed
    [ $FAIL -eq 0 ]
}

main "$@"
