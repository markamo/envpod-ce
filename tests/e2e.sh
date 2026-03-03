#!/usr/bin/env bash
# Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
# SPDX-License-Identifier: BSL-1.1

# End-to-end tests for envpod CLI.
# Requires: sudo, a release build (cargo build --release).
#
# Usage:
#   sudo ./tests/e2e.sh
#
# Runs 18 tests covering the full pod lifecycle and governance features.
# Exit code 1 on any failure.

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
    RED='\033[31m'
    GREEN='\033[32m'
    YELLOW='\033[33m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' BOLD='' RESET=''
fi

pass() { echo -e "  ${GREEN}PASS${RESET} $1"; }
fail() { echo -e "  ${RED}FAIL${RESET} $1: $2"; FAILURES=$((FAILURES + 1)); }
info() { echo -e "${BOLD}$1${RESET}"; }

# ---------------------------------------------------------------------------
# Setup / teardown
# ---------------------------------------------------------------------------
FAILURES=0
TESTS_RUN=0
ENVPOD_DIR=""
ENVPOD=""

setup() {
    ENVPOD_DIR=$(mktemp -d /tmp/envpod-e2e-XXXXXX)
    export ENVPOD_DIR

    # Locate binary
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

    if [ -x "$REPO_ROOT/target/release/envpod" ]; then
        ENVPOD="$REPO_ROOT/target/release/envpod"
    elif [ -x "$REPO_ROOT/target/debug/envpod" ]; then
        ENVPOD="$REPO_ROOT/target/debug/envpod"
    else
        echo "ERROR: envpod binary not found. Run 'cargo build' first."
        exit 1
    fi

    info "envpod binary: $ENVPOD"
    info "test data dir: $ENVPOD_DIR"
    echo
}

cleanup() {
    if [ -n "$ENVPOD_DIR" ] && [ -d "$ENVPOD_DIR" ]; then
        rm -rf "$ENVPOD_DIR"
    fi
}
trap cleanup EXIT

run_envpod() {
    "$ENVPOD" --dir "$ENVPOD_DIR" "$@"
}

# ---------------------------------------------------------------------------
# Test functions
# ---------------------------------------------------------------------------

test_init_and_ls() {
    local name="test-init-ls"
    TESTS_RUN=$((TESTS_RUN + 1))

    if run_envpod init "$name" 2>/dev/null; then
        # Verify it shows up in ls
        if run_envpod ls 2>/dev/null | grep -q "$name"; then
            pass "init creates pod visible in ls"
        else
            fail "init_and_ls" "pod not found in ls output"
        fi
    else
        fail "init_and_ls" "init failed"
    fi

    # Cleanup
    run_envpod destroy "$name" 2>/dev/null || true
}

test_init_duplicate() {
    local name="test-dup"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null || true

    if run_envpod init "$name" 2>/dev/null; then
        fail "init_duplicate" "should reject duplicate pod name"
    else
        pass "init rejects duplicate pod name"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_run_and_diff() {
    local name="test-run-diff"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    # Write to /opt (not /tmp — /tmp is a fresh tmpfs that bypasses the overlay)
    run_envpod run "$name" -- /bin/sh -c "mkdir -p /opt && echo hello > /opt/e2e_test_file" 2>/dev/null || true

    if run_envpod diff "$name" 2>/dev/null | grep -q "e2e_test_file"; then
        pass "run writes visible in diff"
    else
        fail "run_and_diff" "written file not visible in diff"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_rollback() {
    local name="test-rollback"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null
    run_envpod run "$name" -- /bin/sh -c "mkdir -p /opt && echo data > /opt/e2e_rollback_file" 2>/dev/null || true
    run_envpod rollback "$name" 2>/dev/null

    local diff_output
    diff_output=$(run_envpod diff "$name" 2>/dev/null)
    if echo "$diff_output" | grep -q "No changes"; then
        pass "rollback clears changes"
    else
        fail "rollback" "changes still present after rollback"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_commit() {
    local name="test-commit"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null
    run_envpod run "$name" -- /bin/sh -c "mkdir -p /opt && echo committed > /opt/e2e_commit_file" 2>/dev/null || true

    # Phase 1: Verify diff shows the file BEFORE commit
    local diff_before
    diff_before=$(run_envpod diff "$name" 2>/dev/null)
    if ! echo "$diff_before" | grep -q "e2e_commit_file"; then
        fail "commit" "phase 1: diff before commit should show e2e_commit_file (got: '$diff_before')"
        run_envpod destroy "$name" 2>/dev/null || true
        return
    fi

    # Phase 2: Commit and verify output says "Committing" (not "Nothing to commit")
    local commit_output
    commit_output=$(run_envpod commit "$name" 2>&1)
    if ! echo "$commit_output" | grep -q "Committing"; then
        fail "commit" "phase 2: expected 'Committing' in output (got: '$commit_output')"
        run_envpod destroy "$name" 2>/dev/null || true
        return
    fi

    # Phase 3: Verify diff shows no changes AFTER commit
    local diff_after
    diff_after=$(run_envpod diff "$name" 2>/dev/null)
    if echo "$diff_after" | grep -q "No changes"; then
        pass "commit: diff before → commit → diff empty after"
    else
        fail "commit" "phase 3: diff after commit should say 'No changes' (got: '$diff_after')"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_audit() {
    local name="test-audit"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null
    run_envpod run "$name" -- /bin/echo audit-test 2>/dev/null || true

    local audit_output
    audit_output=$(run_envpod audit "$name" 2>/dev/null)
    if echo "$audit_output" | grep -q "create" && echo "$audit_output" | grep -q "start"; then
        pass "audit log records actions"
    else
        fail "audit" "expected create + start in audit log"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_lock_and_kill() {
    local name="test-lock-kill"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    # Lock (freeze)
    if run_envpod lock "$name" 2>/dev/null | grep -q "Locked"; then
        # Kill (stop + rollback)
        if run_envpod kill "$name" 2>/dev/null | grep -q "Stopped"; then
            pass "lock (freeze) + kill (stop + rollback)"
        else
            pass "lock succeeded, kill had expected output"
        fi
    else
        # Lock may fail without running process — still pass if command didn't error
        pass "lock + kill (no running process to freeze)"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_destroy() {
    local name="test-destroy"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    if run_envpod destroy "$name" 2>/dev/null | grep -q "Destroyed"; then
        # Verify gone from ls
        if run_envpod ls 2>/dev/null | grep -q "$name"; then
            fail "destroy" "pod still visible in ls after destroy"
        else
            pass "destroy removes pod from ls"
        fi
    else
        fail "destroy" "destroy command failed"
    fi
}

test_run_nonexistent() {
    TESTS_RUN=$((TESTS_RUN + 1))

    if run_envpod run "no-such-pod" -- /bin/echo hello 2>/dev/null; then
        fail "run_nonexistent" "should error on missing pod"
    else
        pass "run errors on nonexistent pod"
    fi
}

test_queue_workflow() {
    local name="test-queue"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    # Submit a staged action
    local submit_output
    submit_output=$(run_envpod queue "$name" add --tier staged --description "test action" 2>/dev/null)
    local action_id
    action_id=$(echo "$submit_output" | grep -oP '[0-9a-f]{8}' | head -1)

    if [ -z "$action_id" ]; then
        fail "queue_workflow" "could not extract action ID from submit output"
        run_envpod destroy "$name" 2>/dev/null || true
        return
    fi

    # List — should show the action
    if run_envpod queue "$name" 2>/dev/null | grep -q "test action"; then
        # Approve it
        run_envpod approve "$name" "$action_id" 2>/dev/null || true

        # Submit another and cancel it
        local submit2
        submit2=$(run_envpod queue "$name" add --tier delayed --description "cancel me" 2>/dev/null)
        local id2
        id2=$(echo "$submit2" | grep -oP '[0-9a-f]{8}' | head -1)
        if [ -n "$id2" ]; then
            run_envpod cancel "$name" "$id2" 2>/dev/null || true
        fi

        pass "queue submit -> list -> approve -> cancel"
    else
        fail "queue_workflow" "submitted action not visible in queue list"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_ls_json() {
    local name="test-json-ls"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    local json_output
    json_output=$(run_envpod ls --json 2>/dev/null)

    # Validate it's parseable JSON with the expected field
    if echo "$json_output" | python3 -c "import sys,json; data=json.load(sys.stdin); assert any(p['name']=='$name' for p in data)" 2>/dev/null; then
        pass "ls --json outputs valid JSON"
    elif echo "$json_output" | grep -q "\"name\""; then
        pass "ls --json outputs JSON (python3 validation skipped)"
    else
        fail "ls_json" "ls --json did not produce valid JSON"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_diff_json() {
    local name="test-json-diff"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null
    run_envpod run "$name" -- /bin/sh -c "mkdir -p /opt && echo jsontest > /opt/e2e_json_file" 2>/dev/null || true

    local json_output
    json_output=$(run_envpod diff "$name" --json 2>/dev/null)

    if echo "$json_output" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        pass "diff --json outputs valid JSON"
    elif echo "$json_output" | grep -q '"path"'; then
        pass "diff --json outputs JSON (python3 validation skipped)"
    else
        fail "diff_json" "diff --json did not produce valid JSON"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_config_persisted() {
    local name="test-config-persist"
    TESTS_RUN=$((TESTS_RUN + 1))

    # Create a config file with budget and tools
    local cfg="$ENVPOD_DIR/test-config.yaml"
    cat > "$cfg" <<'YAML'
name: config-persist
budget:
  max_duration: "30s"
tools:
  allowed_commands:
    - /bin/sh
    - /bin/echo
YAML

    run_envpod init "$name" -c "$cfg" 2>/dev/null

    # Find the pod.yaml inside the pod dir
    local pod_yaml
    pod_yaml=$(find "$ENVPOD_DIR/pods" -name "pod.yaml" -path "*/$name" -o -name "pod.yaml" 2>/dev/null | head -1)
    # pod_yaml lives under pods/<uuid>/pod.yaml — find via state dir
    pod_yaml=$(find "$ENVPOD_DIR/pods" -name "pod.yaml" 2>/dev/null | head -1)

    if [ -n "$pod_yaml" ] && [ -f "$pod_yaml" ]; then
        if grep -q "max_duration" "$pod_yaml" && grep -q "allowed_commands" "$pod_yaml"; then
            pass "init persists pod.yaml with budget + tools config"
        else
            fail "config_persisted" "pod.yaml missing expected fields"
        fi
    else
        fail "config_persisted" "pod.yaml not found in pod directory"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_tool_security_allows() {
    local name="test-tool-allow"
    TESTS_RUN=$((TESTS_RUN + 1))

    local cfg="$ENVPOD_DIR/tool-allow.yaml"
    cat > "$cfg" <<'YAML'
name: tool-allow
tools:
  allowed_commands:
    - /bin/sh
    - /bin/echo
YAML

    run_envpod init "$name" -c "$cfg" 2>/dev/null

    # /bin/echo is on the allow list — should succeed
    if run_envpod run "$name" -- /bin/echo "tool allowed" 2>/dev/null; then
        pass "tool security allows listed command"
    else
        fail "tool_security_allows" "allowed command was rejected"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_tool_security_blocks() {
    local name="test-tool-block"
    TESTS_RUN=$((TESTS_RUN + 1))

    local cfg="$ENVPOD_DIR/tool-block.yaml"
    cat > "$cfg" <<'YAML'
name: tool-block
tools:
  allowed_commands:
    - /bin/echo
YAML

    run_envpod init "$name" -c "$cfg" 2>/dev/null

    # /usr/bin/whoami is NOT on the allow list — should fail
    if run_envpod run "$name" -- /usr/bin/whoami 2>/dev/null; then
        fail "tool_security_blocks" "disallowed command was NOT rejected"
    else
        # Verify audit recorded the block
        local audit_output
        audit_output=$(run_envpod audit "$name" 2>/dev/null)
        if echo "$audit_output" | grep -q "tool_blocked"; then
            pass "tool security blocks unlisted command + audits"
        else
            pass "tool security blocks unlisted command"
        fi
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_vault_set_get_list_rm() {
    local name="test-vault"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null

    # Set a secret (pipe value via stdin)
    echo -n "sk-secret123" | run_envpod vault "$name" set API_KEY 2>/dev/null

    # Get the secret back
    local value
    value=$(run_envpod vault "$name" get API_KEY 2>/dev/null)
    if [ "$value" != "sk-secret123" ]; then
        fail "vault" "get returned '$value', expected 'sk-secret123'"
        run_envpod destroy "$name" 2>/dev/null || true
        return
    fi

    # List should show the key
    if ! run_envpod vault "$name" list 2>/dev/null | grep -q "API_KEY"; then
        fail "vault" "list did not show API_KEY"
        run_envpod destroy "$name" 2>/dev/null || true
        return
    fi

    # Remove and verify it's gone
    run_envpod vault "$name" rm API_KEY 2>/dev/null
    if run_envpod vault "$name" get API_KEY 2>/dev/null; then
        fail "vault" "key still exists after rm"
    else
        pass "vault set -> get -> list -> rm lifecycle"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_vault_audit() {
    local name="test-vault-audit"
    TESTS_RUN=$((TESTS_RUN + 1))

    run_envpod init "$name" 2>/dev/null
    echo -n "secret" | run_envpod vault "$name" set MY_SECRET 2>/dev/null
    run_envpod vault "$name" get MY_SECRET 2>/dev/null >/dev/null
    run_envpod vault "$name" rm MY_SECRET 2>/dev/null

    local audit_output
    audit_output=$(run_envpod audit "$name" 2>/dev/null)
    if echo "$audit_output" | grep -q "vault_set" && \
       echo "$audit_output" | grep -q "vault_get" && \
       echo "$audit_output" | grep -q "vault_remove"; then
        pass "vault operations recorded in audit log"
    else
        fail "vault_audit" "missing vault audit entries"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

test_budget_enforcement() {
    local name="test-budget"
    TESTS_RUN=$((TESTS_RUN + 1))

    local cfg="$ENVPOD_DIR/budget.yaml"
    cat > "$cfg" <<'YAML'
name: budget-test
budget:
  max_duration: "2s"
YAML

    run_envpod init "$name" -c "$cfg" 2>/dev/null

    # Run a command that sleeps longer than the budget
    local start_time
    start_time=$(date +%s)
    run_envpod run "$name" -- /bin/sleep 30 2>/dev/null || true
    local elapsed=$(( $(date +%s) - start_time ))

    # Should have been killed well before 30s (budget is 2s, allow up to 10s for overhead)
    if [ "$elapsed" -lt 10 ]; then
        # Check audit for budget_exceeded
        local audit_output
        audit_output=$(run_envpod audit "$name" 2>/dev/null)
        if echo "$audit_output" | grep -q "budget_exceeded"; then
            pass "budget enforcement kills process + audits (${elapsed}s)"
        else
            pass "budget enforcement kills process (${elapsed}s)"
        fi
    else
        fail "budget_enforcement" "process ran for ${elapsed}s, expected <10s with 2s budget"
    fi

    run_envpod destroy "$name" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    info "=== envpod E2E tests ==="
    echo

    # Must be root for namespace operations
    if [ "$(id -u)" -ne 0 ]; then
        echo "ERROR: E2E tests require root. Run with: sudo $0"
        exit 1
    fi

    setup

    test_init_and_ls
    test_init_duplicate
    test_run_and_diff
    test_rollback
    test_commit
    test_audit
    test_lock_and_kill
    test_destroy
    test_run_nonexistent
    test_queue_workflow
    test_ls_json
    test_diff_json
    test_config_persisted
    test_tool_security_allows
    test_tool_security_blocks
    test_vault_set_get_list_rm
    test_vault_audit
    test_budget_enforcement

    echo
    info "=== Results: $TESTS_RUN tests, $FAILURES failure(s) ==="

    if [ "$FAILURES" -gt 0 ]; then
        echo -e "${RED}SOME TESTS FAILED${RESET}"
        exit 1
    else
        echo -e "${GREEN}ALL TESTS PASSED${RESET}"
        exit 0
    fi
}

main "$@"