<!-- type-delay 0.03 -->
# Quickstart Tutorial

> **EnvPod v0.1.0** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

This tutorial takes you from zero to a governed AI agent in about 10 minutes. You'll create pods, see filesystem isolation in action, test DNS filtering, inspect audit trails, and run a real AI agent — all on your local machine.

<p align="center">
  <img src="../assets/demos/01-quickstart.gif" alt="envpod quickstart" width="720">
</p>

**Prerequisites:** envpod installed and working (`sudo envpod ls` prints an empty list). See [Installation](INSTALL.md) if you haven't set up yet.

---

## 1. Your First Pod

Create a pod using a built-in preset, the interactive wizard, or a config file:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Option A: Use a built-in preset (18 available — run `envpod presets` to see all)
sudo envpod init tutorial --preset devbox

# Option B: Interactive wizard — shows presets by category, lets you customize
sudo envpod init tutorial

# Option C: Use a config file directly
sudo envpod init tutorial -c examples/basic-cli.yaml
```

Verify it exists:

<!-- no-exec -->
```bash
sudo envpod ls
```

<!-- output -->
```
NAME       BACKEND  STATUS   CREATED
tutorial   native   created  2026-02-26T10:00:00Z
```
<!-- pause 2 -->

Run a single command inside the pod:

<!-- no-exec -->
```bash
sudo envpod run tutorial -- echo "Hello from inside the pod"
```

Or drop into an interactive shell (every command you type runs inside the pod):

<!-- no-exec -->
```bash
sudo envpod run tutorial -- /bin/bash
```

You're now inside the pod. Try `hostname` (shows `tutorial`), `cat /proc/1/cmdline`, or explore the filesystem. Press **Ctrl+Z** to detach (pod continues in background), or type `exit` to leave. Use `sudo envpod fg tutorial` to reattach to a detached pod.

You can also start a pod in the background with `-b`:

<!-- no-exec -->
```bash
sudo envpod run tutorial -b -- python3 long_task.py
sudo envpod fg tutorial  # reattach later
```

Check pod status:

<!-- no-exec -->
```bash
sudo envpod status tutorial
```

The pod is fully isolated — separate PID namespace, mount namespace, network namespace, and cgroup limits (1 CPU core, 512 MB memory as defined in `basic-cli.yaml`).

---

## 2. Filesystem Isolation (Copy-on-Write)

Every file the agent writes goes to a COW overlay — the host filesystem is never modified directly. Let's see this in action.

Write a file inside the pod (use `/opt/` — not `/tmp`, which is a fresh tmpfs that bypasses the overlay):

<!-- no-exec -->
```bash
sudo envpod run tutorial -- sh -c 'echo "agent output" > /opt/result.txt'
sudo envpod run tutorial -- sh -c 'mkdir -p /opt/data && echo "more data" > /opt/data/notes.txt'
```

See what changed:

<!-- no-exec -->
```bash
sudo envpod diff tutorial
```

<!-- output -->
```
  Added    /opt/result.txt
  Added    /opt/data/
  Added    /opt/data/notes.txt
```
<!-- pause 2 -->

The host filesystem is untouched. Now reject the changes:

<!-- no-exec -->
```bash
sudo envpod rollback tutorial
sudo envpod diff tutorial        # empty — changes discarded
```

Write again and this time accept:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod run tutorial -- sh -c 'echo "final output" > /opt/result.txt; echo "extra" > /opt/extra.txt'
sudo envpod diff tutorial        # shows /opt/result.txt and /opt/extra.txt

# Commit just one file (selective commit)
sudo envpod commit tutorial /opt/result.txt
sudo envpod diff tutorial        # only /opt/extra.txt remains

# Commit the rest
sudo envpod commit tutorial      # applies remaining changes to host
```

This is the core safety loop: **run → diff → commit or rollback**. Every file change is reviewable before it touches the real filesystem.

---

## 3. Network Isolation & DNS Filtering

<p align="center">
  <img src="../assets/demos/04-network-isolation.gif" alt="envpod network isolation" width="720">
</p>

> **Note:** The `tutorial` pod from section 1 has no allowed domains (`allow: []`) — it cannot reach the internet by design. This section creates a separate pod with a PyPI-only whitelist.

Create a network-enabled pod using the `python-env.yaml` config (this also runs setup commands — installing numpy, pandas, and other data science packages):

<!-- no-exec -->
```bash
sudo envpod init pynet -c examples/python-env.yaml
```

Test DNS resolution inside the **pynet** pod:

<!-- no-exec -->
```bash
# Allowed — pypi.org is on the whitelist
sudo envpod run pynet -- nslookup pypi.org

# Blocked — google.com is not on the whitelist
sudo envpod run pynet -- nslookup google.com
```

The second lookup fails because envpod runs an embedded DNS server per pod. Each pod's `/etc/resolv.conf` is rewritten to point to envpod's resolver, which only resolves domains on the whitelist. In `Isolated` mode, iptables rules inside the pod's network namespace also block DNS to any other server, preventing bypass.

You can mutate DNS policy on a running pod without restarting:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Allow a new domain
sudo envpod dns pynet --allow google.com

# Verify it resolves now
sudo envpod run pynet -- nslookup google.com

# Remove it again
sudo envpod dns pynet --remove-allow google.com
```

---

## 4. Audit Trail

Every action inside a pod is logged. View the audit trail for the `pynet` pod:

<!-- no-exec -->
```bash
sudo envpod audit pynet
```

<!-- output -->
```
TIME                 ACTION              DETAILS
2026-02-26T10:05:00  create              backend=native
2026-02-26T10:05:01  start               pid=1234, cmd=nslookup pypi.org
2026-02-26T10:05:01  dns_query           domain=pypi.org. type=A decision=allow
2026-02-26T10:05:01  stop
2026-02-26T10:05:02  start               pid=1235, cmd=nslookup google.com
2026-02-26T10:05:02  dns_query           domain=google.com. type=A decision=deny
2026-02-26T10:05:02  stop
...
```
<!-- pause 2 -->

For machine-readable output:

<!-- no-exec -->
```bash
sudo envpod audit pynet --json
```

Each entry includes a timestamp, action type, details, and pod ID — everything you need for compliance and post-incident review.

---

## 5. Running a Real AI Agent

Let's run Claude Code (Anthropic's CLI coding agent) inside a governed pod.

First, store the API key in the pod's credential vault so it's never exposed in the agent's context:

<!-- no-exec -->
```bash
sudo envpod init claude-code --preset claude-code
sudo envpod vault claude-code set ANTHROPIC_API_KEY
```

The vault prompts for the value interactively — it never appears in shell history or audit logs.

Run the agent. Use `-w` to mount the current working directory so the agent can see your project files (writes go to the COW overlay):

<!-- no-exec -->
```bash
sudo envpod run claude-code -w -- claude
```

The agent runs with full isolation:
- **Filesystem:** All file writes go to the COW overlay. With `-w`, the agent sees your project directory but changes are staged for review via `envpod diff`/`commit`
- **Network:** Only `api.anthropic.com`, `github.com`, and package registries are reachable (see `examples/claude-code.yaml` for the full whitelist)
- **Process:** Capped at 2 CPU cores and 4 GB memory
- **Audit:** Every action is logged

After the session, review and decide:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod diff claude-code       # what did it change?
sudo envpod commit claude-code     # accept all changes
sudo envpod commit claude-code /opt/output.txt  # or accept specific files
# or
sudo envpod rollback claude-code   # reject everything
```

### Vault Proxy Injection (v0.2)

For maximum security, use vault proxy injection — the agent never sees the real API key:

<!-- no-exec -->
```bash
# Bind the key to the API domain (auto-enables proxy)
sudo envpod vault claude-code bind ANTHROPIC_API_KEY api.anthropic.com "Authorization: Bearer {value}"

# Run — proxy injects real key transparently
sudo envpod run claude-code -- claude
```

With proxy injection, the agent can make normal HTTPS requests to `api.anthropic.com`, but the real key is injected at the transport layer by a proxy running outside the pod's namespace. Even a compromised agent cannot read or exfiltrate the key. See [Pod Config — Vault](POD-CONFIG.md#vault) for details.

All 18 presets include auto-setup commands — dependencies install automatically during `envpod init`. Use `envpod presets` to see the full list, or run `sudo envpod init <name>` for an interactive wizard.

<!-- output -->
| Preset | Agent | Auto-Setup |
|--------|-------|------------|
| `claude-code` | Anthropic Claude Code | `curl` installer |
| `codex` | OpenAI Codex CLI | nvm + `npm install -g @openai/codex` |
| `gemini-cli` | Google Gemini CLI | nvm + `npm install -g @google/gemini-cli` |
| `opencode` | OpenCode terminal agent | `curl` installer |
| `aider` | Aider pair programmer | `pip install aider-chat` |
| `swe-agent` | SWE-agent autonomous coder | `pip install sweagent` |
| `langgraph` | LangGraph workflows | `pip install langgraph langchain-openai` |
| `google-adk` | Google ADK | `pip install google-adk` |
| `openclaw` | OpenClaw messaging | nvm + `npm install -g openclaw` |
| `browser-use` | Browser-use agent | `pip install browser-use playwright` + Chromium |
| `playwright` | Playwright automation | `pip install playwright` + Chromium |
| `browser` | Headless Chrome | Chrome (check host, else install) |
| `python-env` | Python environment | numpy, pandas, matplotlib, scipy, scikit-learn |
| `nodejs` | Node.js environment | nvm + Node.js 22 |
| `desktop` | XFCE desktop via noVNC | `desktop_env: xfce` + Chrome (~550MB) |
| `vscode` | VS Code in browser | code-server |

You can also use config files directly for full control: `sudo envpod init my-agent -c examples/browser-wayland.yaml`

---

## 6. Browser Pod with Display & Audio

Run a full GUI browser inside a governed pod:

<!-- no-exec -->
```bash
sudo envpod init browser-pod -c examples/browser.yaml
sudo envpod run browser-pod -- useradd -m browseruser
sudo envpod run browser-pod -d -a --user browseruser -- google-chrome https://youtube.com
```

The `-d` flag auto-detects Wayland or X11 and sets the correct environment. The `-a` flag auto-detects PipeWire or PulseAudio. The `browser.yaml` config enables GPU, display, and audio passthrough plus the browser seccomp profile.

**For maximum security** (Wayland + PipeWire), use the dedicated config:

<!-- no-exec -->
```bash
sudo envpod init browser-secure -c examples/browser-wayland.yaml
sudo envpod run browser-secure -d -a --user browseruser -- google-chrome --ozone-platform=wayland https://youtube.com
```

Chrome needs `--ozone-platform=wayland` to use Wayland natively (it defaults to X11 otherwise).

> **Note:** Firefox is a snap on Ubuntu 24.04 and doesn't work inside namespace pods. Use Chrome (deb package) instead.

Compare security findings with `sudo envpod audit --security -c examples/browser.yaml` vs `examples/browser-wayland.yaml`:

<!-- output -->
| Config | I-04 Display | I-05 Audio |
|--------|-------------|------------|
| `browser.yaml` (X11/auto) | **CRITICAL** | **HIGH** |
| `browser-wayland.yaml` (Wayland + PipeWire) | LOW | MEDIUM |
<!-- pause 2 -->

**Alternative: Browser Desktop via noVNC** — No host display needed. Run a full desktop in the browser:

```bash
sudo envpod init web-demo --preset desktop
sudo envpod start web-demo
# Open http://localhost:6080 — auto-connects, envpod-branded, with audio + file upload
```

The noVNC interface includes an upload button (files → `/tmp/uploads/`) and an audio toggle (speaker icon). Works on headless servers and SSH sessions.

---

## 7. Verify Isolation (Jailbreak Test)

Envpod ships a jailbreak test script that probes all isolation boundaries from inside the pod. Run it against your tutorial pod to verify the walls are holding:

<!-- no-exec -->
```bash
sudo envpod run tutorial -- bash /usr/local/share/envpod/examples/jailbreak-test.sh
```

Pods run as the non-root `agent` user by default, giving full pod boundary protection. You'll see a colored pass/fail report across 49 tests:

<!-- output -->
```
envpod jailbreak test v0.1.0
Probing isolation boundaries...

=== Filesystem Wall (F-01 to F-10) ===
  PASS   F-01  [MEDIUM]  Write to overlay (not host root)
  PASS   F-02  [HIGH]    Access overlay upper dir
  PASS   F-03  [CRITICAL] Mount new filesystem
  ...

=== Summary ===

  Host boundary:   16/16 passed — agent cannot escape
  Pod boundary  (agent):  17/17 passed — walls enforced
  Pod hardening (agent):  12/16 passed (4 gaps)

Known gaps (v0.1):
  I-03  [MEDIUM]   /proc/stat leaks host CPU counters
```
<!-- pause 2 -->

The default non-root user passes all 17 pod boundary tests. Running with `--root` exposes 2 additional gaps (N-05 iptables, N-06 raw sockets). Use `--json` for machine-readable output, or `--category seccomp` to test a single category.

---

## 8. Clone & Base Pods (Fast Pod Creation)

`envpod init` takes ~1.3s because it builds a rootfs from scratch. Once you have a working pod, **clone** it in ~130ms — 10x faster.

### Cloning from a Pod

Use `--create-base` during `envpod init` to create a **base snapshot** (the state after init + setup). Clone from it:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Create a coding agent with a base snapshot (takes ~1.3s + setup time)
sudo envpod init coder -c examples/coding-agent.yaml --create-base

# Clone it — 10x faster (~130ms), inherits all setup
sudo envpod clone coder coder-2
sudo envpod clone coder coder-3

# Each clone is independent — separate overlay, network, cgroup
sudo envpod run coder-2 -- echo "I'm clone 2"
sudo envpod run coder-3 -- echo "I'm clone 3"
```

Clones share the base rootfs via symlink (~1 KB unique data per clone). This is analogous to Docker's image→container model.

### Clone Current State

By default, `clone` copies the base snapshot (after init+setup, before any agent changes). To clone the current state including agent modifications:

<!-- no-exec -->
```bash
sudo envpod run coder -- sh -c 'echo "agent work" > /opt/result.txt'
sudo envpod clone coder coder-fork --current    # includes /opt/result.txt
```

### Standalone Base Pods

For managing reusable snapshots independently:

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
# Create a base pod (no instance needed)
sudo envpod base create python-base -c examples/python-env.yaml

# List base pods
sudo envpod base ls

# Clone from a base pod
sudo envpod clone python-base worker-1
sudo envpod clone python-base worker-2

# Destroy a base pod
sudo envpod base destroy python-base
```

### At Scale

Creating 50 agent pods from a base takes **407ms total** (8ms each) vs 6.3s with Docker. See [Benchmarks](BENCHMARKS.md#scale-test) for full numbers.

<!-- no-exec -->
```bash
# Spin up 50 agents
for i in $(seq 1 50); do
    sudo envpod clone coder "agent-$i"
done
```

---

## 9. Security Audit

Check the security posture of any pod config without creating a pod:

<!-- no-exec -->
```bash
sudo envpod audit --security -c examples/browser.yaml
```

<!-- output -->
```
  envpod security audit · browser-agent

  Pod boundary  17/17 with default user

  ⚠ 6 security notes:

  I-04  [CRITICAL] X11 display access — keylogging possible
  N-03  [HIGH]     Direct DNS bypass possible
  S-03  [HIGH]     Relaxed seccomp profile
  I-05  [HIGH]     Microphone access available
  P-03  [MEDIUM]   Nested namespaces possible
  I-06  [LOW]      GPU information leakage
```
<!-- pause 2 -->

Compare with the secure Wayland config:

<!-- no-exec -->
```bash
sudo envpod audit --security -c examples/browser-wayland.yaml
```

The Wayland config drops I-04 from CRITICAL to LOW and I-05 from HIGH to MEDIUM. See [Security Report](SECURITY.md) for a full audit of all example configs.

For machine-readable output:

<!-- no-exec -->
```bash
sudo envpod audit --security --json -c examples/coding-agent.yaml
```

---

## 10. Bonus Features

### Action Queue

View and manage staged actions that need human approval:

<!-- no-exec -->
```bash
sudo envpod queue claude-code          # list pending actions
sudo envpod approve claude-code <id>   # approve an action
sudo envpod cancel claude-code <id>    # reject an action
```

### Lock & Undo

Freeze a pod instantly (all processes paused, state preserved):

<!-- no-exec -->
```bash
sudo envpod lock claude-code           # freeze
sudo envpod undo claude-code           # list undo-able actions
sudo envpod undo claude-code <id>      # undo a specific action
```

### Live DNS Mutation

Update network policy without restarting:

<!-- no-exec -->
```bash
sudo envpod dns claude-code --allow newdomain.com
sudo envpod dns claude-code --deny suspicious.io
```

### Snapshots

Save and restore pod state at any point:

```bash
sudo envpod snapshot claude-code create before-refactor
sudo envpod snapshot claude-code ls
sudo envpod snapshot claude-code restore <id>
```

---

## 11. Cleanup

Destroy all tutorial pods (batch destroy — single command):

<!-- no-exec -->
```bash
sudo envpod destroy tutorial pynet claude-code coder coder-2 coder-3
```

For a fully clean teardown (including iptables rules), use `--full`:

<!-- no-exec -->
```bash
sudo envpod destroy tutorial --full
```

Or destroy with the default fast path and clean up afterward:

<!-- no-exec -->
```bash
sudo envpod destroy tutorial pynet claude-code
sudo envpod gc    # removes stale iptables, orphaned netns, cgroups, pod dirs
```

Verify:

<!-- no-exec -->
```bash
sudo envpod ls    # empty
```

---

## What's Next

- `envpod --help` — full command reference
- `envpod dashboard` — start the web dashboard for browser-based fleet management (v0.2)
- `examples/` — pre-built configs for popular AI agents
- [Pod Config Reference](POD-CONFIG.md) — every pod.yaml option explained (including vault proxy)
- [Tutorials](TUTORIALS.md) — detailed use-case guides (browser, audio, GPU, coding agents)
- [Benchmarks](BENCHMARKS.md) — performance deep-dive
- [Security Report](SECURITY.md) — security audit of all example configs
- [FAQ](FAQ.md) — common questions answered
- [README](../README.md) — architecture overview and feature list

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
