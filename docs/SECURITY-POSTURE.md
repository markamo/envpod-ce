# Security Posture — All Example Configs

> **EnvPod v0.1.3** — Zero-trust governance environments for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Security audit results for all 42 example configs. Generated with `envpod audit --security -c <config>`.

## Test Methodology

The jailbreak test (`examples/jailbreak-test.sh`) runs 4 phases:

| Phase | What it tests | User |
|-------|--------------|------|
| **Host boundary** | Can the agent escape the pod? (filesystem, PID, network, advanced vectors) | root |
| **Pod boundary (root)** | Are the pod's walls enforced? (DNS, seccomp, hardening, cgroups, info leaks) | root |
| **Hardening (root)** | Are defense-in-depth measures active? (NO_NEW_PRIVS, seccomp, coredump) | root |
| **Pod boundary (non-root)** | Same as above but with default non-root user | agent |

**Key result:** Non-root user passes 14/17 tests (3 gaps are info leaks, not escapes). Root user passes 6/17 (known gaps from CAP_NET_ADMIN). Default non-root is recommended for all production use.

## Privilege Escalation Prevention

An agent running as non-root **cannot escalate to root**. Two independent layers:

1. **NO_NEW_PRIVS** — set for all pods (root and non-root). `sudo` refuses: *"The no new privileges flag is set, which prevents sudo from running as root."*
2. **Seccomp-BPF** — blocks `setuid`/`setgid` syscalls

The only way to run as root is `envpod run --root` from the host.

## /proc Masking

| File | Behavior |
|------|----------|
| `/proc/cpuinfo` | Model name always sanitized to "CPU". With cgroup CPU limits, filtered to allowed CPUs only. |
| `/proc/meminfo` | With cgroup memory limits, values reflect pod limits. Without limits, shows host values. |
| `/proc/stat` | Not masked (htop/top need live CPU counters). Accepted as I-03 MEDIUM info leak. |
| `/proc/acpi`, `/proc/kcore`, `/proc/keys`, etc. | Bind-mounted to /dev/null (OCI standard masking). |
| `/proc/1/root`, `/proc/1/cwd`, `/proc/1/environ` | Bind-mounted to /dev/null (escape prevention). |

## Security Matrix — All Example Configs

| Config | Boundary | Findings | CRIT | HIGH | MED | LOW | Category |
|--------|----------|----------|------|------|-----|-----|----------|
| `hardened-sandbox.yaml` | 17/17 | 0 | 0 | 0 | 0 | 0 | Security |
| `basic-cli.yaml` | 17/17 | 0 | 0 | 0 | 0 | 0 | Environment |
| `raspberry-pi.yaml` | 17/17 | 0 | 0 | 0 | 0 | 0 | Embedded |
| `coding-agent.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `aider.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `codex.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `claude-code.yaml` | 17/17 | 2 | 0 | 1 | 1 | 0 | Coding agent |
| `gemini-cli.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `opencode.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `swe-agent.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Coding agent |
| `openclaw.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Framework |
| `google-adk.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Framework |
| `langgraph.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Framework |
| `fuse-agent.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Framework |
| `demo-pod.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Environment |
| `nodejs.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Environment |
| `python-env.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Environment |
| `devbox.yaml` | 17/17 | 2 | 0 | 1 | 1 | 0 | Environment |
| `basic-internet.yaml` | 17/17 | 2 | 0 | 1 | 1 | 0 | Environment |
| `discovery-client.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Networking |
| `discovery-service.yaml` | 17/17 | 1 | 0 | 0 | 1 | 0 | Networking |
| `clone-user.yaml` | 17/17 | 3 | 0 | 0 | 3 | 0 | Environment |
| `host-apps.yaml` | 17/17 | 3 | 0 | 0 | 3 | 0 | Environment |
| `ml-training.yaml` | 17/17 | 2 | 0 | 0 | 1 | 1 | Environment |
| `jetson-orin.yaml` | 17/17 | 1 | 0 | 0 | 0 | 1 | Embedded |
| `vscode.yaml` | 17/17 | 2 | 0 | 1 | 1 | 0 | Desktop |
| `browser-use.yaml` | 17/17 | 5 | 0 | 2 | 2 | 1 | Browser agent |
| `playwright.yaml` | 17/17 | 4 | 0 | 2 | 2 | 0 | Browser agent |
| `browser-wayland.yaml` | 17/17 | 6 | 0 | 2 | 2 | 2 | Browser agent |
| `browser.yaml` | 17/17 | 6 | 1 | 3 | 1 | 1 | Browser agent |
| `nanoclaw.yaml` | 15/17 | 3 | 1 | 2 | 0 | 0 | Messaging |
| `desktop.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `desktop-openbox.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `desktop-sway.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `desktop-web.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `desktop-user.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `gimp.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `web-display-novnc.yaml` | 15/17 | 6 | 1 | 3 | 2 | 0 | Desktop |
| `workstation.yaml` | 15/17 | 7 | 1 | 3 | 2 | 1 | Desktop |
| `workstation-gpu.yaml` | 15/17 | 7 | 1 | 3 | 2 | 1 | Desktop |
| `workstation-full.yaml` | 15/17 | 7 | 1 | 3 | 2 | 1 | Desktop |

## Summary

| Boundary Score | Count | Pattern |
|---------------|-------|---------|
| **17/17** (full protection) | 30 configs | Non-root user, default seccomp |
| **15/17** (root gaps) | 12 configs | `user: root` — needed for desktop/GPU |

| Severity | Total across all configs |
|----------|------------------------|
| CRITICAL | 10 (all from `user: root` + N-05 iptables) |
| HIGH | 39 (DNS denylist bypass, raw sockets, relaxed seccomp) |
| MEDIUM | 39 (no PID limit, nested namespaces, noVNC unencrypted) |
| LOW | 8 (GPU info leak, kernel version visible) |

## Common Findings

| Finding | Severity | Configs Affected | Root Cause | Fix |
|---------|----------|-----------------|------------|-----|
| N-05 | CRITICAL | 12 (desktop/root) | `user: root` grants CAP_NET_ADMIN | Use default non-root user |
| N-03 | HIGH | 12 (denylist DNS) | Denylist mode allows direct IP queries | Use allowlist mode |
| N-06 | HIGH | 12 (desktop/root) | `user: root` allows raw sockets | Use default non-root user |
| S-03 | HIGH | 8 (browser) | `seccomp_profile: browser` is relaxed | Use `default` if no browser needed |
| C-03 | MEDIUM | 25 (no PID limit) | No `max_pids` set | Add `processor.max_pids` |
| W-01 | MEDIUM | 7 (noVNC) | VNC is unencrypted | Use localhost-only (default) |
| I-06 | LOW | 4 (GPU) | nvidia-smi exposes host GPU model | Set `devices.gpu: false` if unneeded |

## Running Security Audits

```bash
# Audit a specific config (no pod needed)
envpod audit --security -c examples/coding-agent.yaml

# Audit with JSON output
envpod audit --security --json -c examples/workstation-full.yaml

# Run the full jailbreak test suite (needs a running pod)
sudo ./examples/jailbreak-test.sh

# Audit all configs in one pass
for f in examples/*.yaml; do echo "=== $(basename $f) ===" && envpod audit --security -c "$f" 2>&1 | head -5; done
```

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
