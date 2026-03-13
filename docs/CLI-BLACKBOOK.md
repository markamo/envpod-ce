# envpod CLI Black Book

> **EnvPod v0.1.1** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Dense field reference for every envpod command. Every flag, every variant, real examples, and practical use cases. No padding.

All commands require `sudo` (namespace operations need root). The state directory defaults to `/var/lib/envpod` — override with `--dir` or `ENVPOD_DIR`.

---

## Quick Index

| Command | One Line |
|---|---|
| [`init`](#init) | Create a pod from pod.yaml |
| [`setup`](#setup) | Re-run setup commands on existing pod |
| [`run`](#run) | Start pod namespace and execute a command |
| [`start`](#start) | Start a pod in the background |
| [`stop`](#stop) | Gracefully stop a running pod |
| [`diff`](#diff) | Show what the agent changed |
| [`commit`](#commit) | Apply agent changes to host filesystem |
| [`rollback`](#rollback) | Discard all agent changes |
| [`audit`](#audit) | View action log or run security scan |
| [`lock`](#lock) | Freeze pod processes |
| [`destroy`](#destroy) | Remove pod entirely |
| [`prune`](#prune) | Remove all stopped/created pods |
| [`ls`](#ls) | List all pods |
| [`status`](#status) | Pod status and resource usage |
| [`logs`](#logs) | Pod stdout/stderr |
| [`clone`](#clone) | Fast-clone a pod from its base |
| [`base`](#base) | Manage reusable base pod snapshots |
| [`snapshot`](#snapshot) | Named checkpoints of overlay state |
| [`vault`](#vault) | Encrypted credential vault |
| [`queue`](#queue) | View and manage the action queue |
| [`approve`](#approve) | Approve a staged action |
| [`cancel`](#cancel) | Cancel a queued action |
| [`undo`](#undo) | Undo an executed action |
| [`actions`](#actions) | Manage the agent action catalog |
| [`ports`](#ports) | Live port mutation (no restart) |
| [`discover`](#discover) | Live discovery mutation (no restart) |
| [`dns`](#dns) | Live DNS policy mutation |
| [`dns-daemon`](#dns-daemon) | Start central pod discovery daemon |
| [`remote`](#remote) | Remote control a running pod |
| [`monitor`](#monitor) | Monitoring policy and alerts |
| [`mount`](#mount) | Mount a host path into the overlay |
| [`unmount`](#unmount) | Unmount a path from the overlay |
| [`dashboard`](#dashboard) | Start web UI on localhost:9090 |
| [`gc`](#gc) | Clean up stale iptables rules |
| [`completions`](#completions) | Generate shell tab completions |

---

## Global Flags

```
--dir <PATH>          Base directory for all envpod state
                      Default: /var/lib/envpod
                      Env: ENVPOD_DIR
```

Example: `sudo ENVPOD_DIR=/opt/envpod envpod ls`

---

## init

Create a new pod: builds rootfs, overlay, cgroup hierarchy, network namespace, and DNS resolver. Runs `setup:` commands if defined in pod.yaml.

```
envpod init <name> [--config <pod.yaml>] [--backend native] [--create-base [base-name]] [-v]
```

| Flag | Default | Description |
|---|---|---|
| `name` | required | Pod name (alphanumeric, hyphens) |
| `-c, --config <path>` | auto-detect | Path to pod.yaml. If omitted, looks for `<name>/pod.yaml` or `pod.yaml` in CWD |
| `--backend <name>` | `native` | Isolation backend (`native` only in v0.2) |
| `--create-base [name]` | none | Create a base snapshot after init (for `envpod clone`). Uses pod name if no name given. Auto-increments on collision (e.g. `my-agent-2`). |
| `-v, --verbose` | false | Stream live output from setup commands |

**Use cases:**

```bash
# Minimal — create pod with inline config auto-detected
sudo envpod init myagent

# From explicit config file
sudo envpod init myagent -c configs/myagent.yaml

# With live setup output (useful for debugging setup: commands)
sudo envpod init myagent -c pod.yaml --verbose

# Create pod + base snapshot named "myagent" (for envpod clone)
sudo envpod init myagent -c pod.yaml --create-base

# Create pod + base snapshot with custom name
sudo envpod init myagent -c pod.yaml --create-base ubuntu-dev

# Create and immediately inspect what's there
sudo envpod init myagent -c pod.yaml && sudo envpod diff myagent
```

**After init:**
- Pod directory: `/var/lib/envpod/pods/myagent/`
- Overlay ready at `upper/`, `work/`, `merged/`
- Base snapshot created only if `--create-base` was used (for `envpod clone`)
- Security brief printed if findings exist

**pod.yaml minimum:**
```yaml
name: myagent
```

---

## setup

Re-run the `setup:` commands from pod.yaml on an already-created pod. Use this to add packages or tools after init without destroying and re-initializing.

```
envpod setup <name> [--create-base [base-name]] [-v]
```

| Flag | Description |
|---|---|
| `name` | Pod name |
| `--create-base [name]` | Create a base snapshot after setup (for `envpod clone`). Uses pod name if no name given. Auto-increments on collision. |
| `-v, --verbose` | Stream live output |

**Use cases:**

```bash
# Add a package after the pod already exists
sudo envpod setup myagent --verbose

# Re-run setup and create a base snapshot named "myagent"
sudo envpod setup myagent -v --create-base

# Re-run setup and create a base snapshot with custom name
sudo envpod setup myagent -v --create-base myagent-v2

# Typical workflow after editing pod.yaml setup block:
# Edit pod.yaml → envpod setup --create-base
sudo envpod setup myagent -v --create-base myagent-v2
```

---

## run

Start the pod's namespace and execute a command inside it. The pod's overlay is mounted, cgroup limits enforced, DNS resolver started, vault secrets injected as env vars, and the queue socket bound-mounted.

```
envpod run <name> [flags] -- <command> [args...]
```

| Flag | Short | Description |
|---|---|---|
| `name` | | Pod name |
| `--root` | | Run as root inside pod (default: non-root `agent` user, UID 60000) |
| `--user <uid|name>` | `-u` | Run as specific user inside pod (`--user root` is equivalent to `--root`) |
| `--env KEY=VALUE` | `-e` | Set extra env vars (repeatable) |
| `--background` | `-b` | Run in background (detached). Use `envpod fg` to reattach |
| `--enable-display` | `-d` | Forward display (Wayland preferred, X11 fallback) |
| `--enable-audio` | `-a` | Forward audio (PipeWire preferred, PulseAudio fallback) |
| `--mount-cwd` | `-w` | Mount working directory into pod (COW isolated). Uses `cwd_path` from init, or current CWD if not set |
| `--no-mount-cwd` | | Skip CWD mount even if `mount_cwd: true` in pod.yaml |
| `--publish host:pod` | `-p` | Port forward — localhost only (OUTPUT DNAT) |
| `--publish-all host:pod` | `-P` | Port forward — all interfaces (PREROUTING + FORWARD) |
| `--internal pod_port` | `-i` | Pod-to-pod port only (no host mapping) |

**Detach/reattach:** Press **Ctrl+Z** during an interactive run to detach — the pod continues running in the background with DNS, port forwards, and all services intact. Use `envpod fg <name>` to reattach. Starting with `-b` runs in background from the start.

**Use cases:**

```bash
# Run a script
sudo envpod run myagent -- python3 agent.py

# Interactive shell for debugging
sudo envpod run myagent -- bash

# Run as root (e.g. package install in ad-hoc session)
sudo envpod run myagent --root -- apt-get install -y ripgrep

# Run specific user
sudo envpod run myagent --user 1000 -- id

# Background mode — run detached, reattach later
sudo envpod run myagent -b -- python3 long_task.py
sudo envpod fg myagent

# Mount working directory (agent sees project files, writes go to overlay)
sudo envpod run myagent -w -- claude

# With display and audio (browser, GUI apps)
sudo envpod run myagent -d -a -- google-chrome --no-sandbox

# With display only (Wayland, explicit)
sudo envpod run myagent -d -- firefox

# Inject extra env vars
sudo envpod run myagent -e DEBUG=1 -e LOG_LEVEL=trace -- python3 agent.py

# Port forward: access pod's port 3000 at localhost:8080
sudo envpod run myagent -p 8080:3000 -- node server.js

# Multiple ports
sudo envpod run myagent -p 8080:3000 -p 5432:5432 -- ./start.sh

# Public port (all interfaces — careful, triggers N-04 security finding)
sudo envpod run myagent -P 80:8080 -- python3 -m http.server 8080

# Pod-to-pod only port (internal — other pods reach it by pod IP)
sudo envpod run myagent -i 3000 -- node server.js

# UDP port
sudo envpod run myagent -p 5353:53/udp -- dnsmasq

# Combine flags
sudo envpod run myagent -d -a -p 8080:3000 -e API_URL=http://localhost:8080 -- python3 agent.py
```

**Protocol suffix:** `/tcp` (default) or `/udp`. Example: `8080:3000/udp`

**After run exits:** Namespace torn down, iptables rules cleaned up, queue socket closed, DNS resolver stopped. Overlay changes persist until `envpod commit` or `envpod rollback`.

---

## fg

Reattach to a background or detached pod. Tails the pod's `run.log` and waits for the process to exit. Press **Ctrl+Z** to detach again (pod continues running).

```
envpod fg <name>
```

```bash
# Start in background, then attach
sudo envpod run myagent -b -- python3 agent.py
sudo envpod fg myagent

# Or: start interactive, Ctrl+Z to detach, then reattach
sudo envpod fg myagent
```

---

## start

Start a pod in the background. Services auto-start (display, desktop, audio, upload). Connect via noVNC at `http://localhost:6080` or open a shell with `envpod run <pod> -- bash`. The pod keeps running until explicitly stopped.

```
envpod start <name> [flags]
```

| Flag | Short | Description |
|---|---|---|
| `name` | | Pod name |
| `--root` | | Run as root inside pod (default: non-root `agent` user, UID 60000) |
| `--user <uid\|name>` | `-u` | Run as specific user inside pod |
| `--env KEY=VALUE` | `-e` | Set extra env vars (repeatable) |
| `--enable-display` | `-d` | Forward display (Wayland preferred, X11 fallback) |
| `--enable-audio` | `-a` | Forward audio (PipeWire preferred, PulseAudio fallback) |
| `--publish host:pod` | `-p` | Port forward — localhost only |
| `--publish-all host:pod` | `-P` | Port forward — all interfaces |
| `--internal pod_port` | `-i` | Pod-to-pod port only |

**Use cases:**

```bash
# Start a desktop pod (connect via noVNC)
sudo envpod start my-desktop

# Start with display and audio forwarding
sudo envpod start my-agent -d -a

# Start as root with extra ports
sudo envpod start my-agent --root -p 8080:3000

# Start, then connect with a shell
sudo envpod start my-agent
sudo envpod run my-agent -- bash

# Start, do work, stop, start again later
sudo envpod start my-agent
# ... work ...
sudo envpod stop my-agent
# ... later ...
sudo envpod start my-agent
```

**Difference from `run`:** `start` launches the pod in the background without executing a specific command. Services (display, audio, upload) auto-start. Use `run` to execute commands inside an already-started pod, or to run a one-shot command.

---

## stop

Gracefully stop one or more running pods. Preserves overlay data — the pod can be started again later with `envpod start`. Accepts multiple pod names for batch operations.

```
envpod stop <name> [name2...]
```

| Flag | Description |
|---|---|
| `names...` | One or more pod names to stop |

**Use cases:**

```bash
# Stop one pod
sudo envpod stop my-agent

# Stop multiple pods
sudo envpod stop agent-1 agent-2 agent-3

# Stop all running pods (shell loop)
sudo envpod ls --json | jq -r '.[] | select(.status == "running") | .name' \
  | xargs -r sudo envpod stop

# Stop and restart cycle
sudo envpod stop my-agent
sudo envpod start my-agent
```

**Difference from `destroy`:** `stop` preserves the pod's overlay, snapshots, vault, and configuration. `destroy` removes everything permanently.

**Difference from `lock`:** `lock` freezes processes in place (SIGSTOP) — they resume exactly where they left off. `stop` fully tears down the namespace and services.

---

## diff

Show what the agent wrote — every file added, modified, or deleted in the overlay since the last commit (or since init).

```
envpod diff <name> [--all] [--json]
```

| Flag | Description |
|---|---|
| `name` | Pod name |
| `--all` | Include system/ignored paths (e.g. `/proc`, `/sys`) |
| `--json` | JSON output: `[{"path": "...", "kind": "Added|Modified|Deleted", "size": N}]` |

**Output:**
```
+ /workspace/output.json       (Added, 2.1 KB)
~ /workspace/config.yaml       (Modified, 840 B)
- /workspace/old_cache.pkl     (Deleted)
```

**Use cases:**

```bash
# Human review before committing
sudo envpod diff myagent

# Machine-readable for scripts
sudo envpod diff myagent --json | jq '.[] | select(.kind == "Added")'

# Count changed files
sudo envpod diff myagent --json | jq length

# Check if anything changed (CI gate)
if sudo envpod diff myagent --json | jq -e 'length > 0' > /dev/null; then
  echo "Agent made changes — review required"
fi

# See everything including system dirs
sudo envpod diff myagent --all
```

---

## commit

Apply the agent's overlay changes to the real host filesystem. **Human review step** — always run `diff` first. The overlay is cleared after a successful commit.

```
envpod commit <name> [paths...] [--exclude path] [--output dir] [--all] [--include-system]
```

| Flag | Description |
|---|---|
| `name` | Pod name |
| `paths...` | Specific paths to commit. Commits all if omitted. |
| `--exclude <path>` | Commit everything EXCEPT these paths (repeatable) |
| `-o, --output <dir>` | Export to this directory instead of the host filesystem |
| `--all` | Commit all changes including system/ignored paths |
| `--include-system` | Also commit changes in `/usr`, `/bin`, `/lib`, etc. |

**Use cases:**

```bash
# Commit everything after review
sudo envpod diff myagent && sudo envpod commit myagent

# Commit specific files only
sudo envpod commit myagent /workspace/output.json /workspace/report.md

# Commit a whole directory
sudo envpod commit myagent /workspace/results/

# Exclude a path from commit
sudo envpod commit myagent --exclude /workspace/.cache

# Export changes to a staging directory (don't touch host yet)
sudo envpod commit myagent --output /tmp/agent-output

# Export and inspect, then manually copy
sudo envpod commit myagent --output /tmp/review
diff -r /tmp/review /workspace

# Commit system directory changes (advanced — rare)
sudo envpod commit myagent --include-system

# Commit everything including system
sudo envpod commit myagent --all
```

**If `queue.require_commit_approval: true`:** The commit is queued as a staged action — requires `envpod approve` before applying.

---

## rollback

Discard all agent changes. Wipes the overlay back to the state at the last commit (or init). Irreversible — the agent's work is gone.

```
envpod rollback <name>
```

**Use cases:**

```bash
# Agent made a mess — discard everything
sudo envpod rollback myagent

# Safe workflow: snapshot first, then run; rollback if bad
sudo envpod snapshot myagent create --name pre-run
sudo envpod run myagent -- python3 risky_agent.py
# If bad:
sudo envpod rollback myagent
# Or restore to exact pre-run state:
sudo envpod snapshot myagent restore pre-run
```

---

## audit

View the action audit log or run a static security analysis of the pod configuration.

```
envpod audit [<name>] [--json] [--security] [--config pod.yaml]
```

| Flag | Description |
|---|---|
| `name` | Pod name (required for log; optional with `--security`) |
| `--json` | JSON output |
| `--security` | Run static security audit on pod configuration |
| `-c, --config <path>` | Pod.yaml to audit (for `--security` before creating the pod) |

**Use cases:**

```bash
# Tail the audit log
sudo envpod audit myagent

# JSON for parsing
sudo envpod audit myagent --json | jq '.[] | select(.action == "vault_get")'

# Filter by action type
sudo envpod audit myagent --json | jq '.[] | select(.action | startswith("queue"))'

# Find all failed actions
sudo envpod audit myagent --json | jq '.[] | select(.success == false)'

# Security audit a live pod
sudo envpod audit myagent --security

# Security audit a yaml file before creating the pod
sudo envpod audit --security -c agent/pod.yaml

# Security audit with JSON output (for CI)
sudo envpod audit --security -c pod.yaml --json | jq '.findings[] | select(.severity == "CRITICAL")'

# Count high+ findings
sudo envpod audit --security -c pod.yaml --json \
  | jq '[.findings[] | select(.severity | test("CRITICAL|HIGH"))] | length'
```

**Security findings returned:**

| ID | Severity | What |
|---|---|---|
| N-03 | HIGH | Unsafe network mode — DNS bypass possible |
| N-04 | LOW | `public_ports` exposed to all interfaces |
| N-05 | MEDIUM | Running as root |
| N-06 | HIGH | Root + Unsafe network |
| S-03 | MEDIUM | Browser without relaxed seccomp |
| I-04 | CRITICAL/LOW | X11 display forwarding |
| I-05 | HIGH/MEDIUM | Audio forwarding |
| I-06 | HIGH | GPU passthrough |
| C-01 | LOW | No CPU limit |
| C-02 | LOW | No memory limit |
| C-03 | LOW | No PID limit |
| V-01 | MEDIUM | Vault bindings without proxy enabled |
| V-02 | HIGH | Vault proxy + Unsafe network |
| V-03 | MEDIUM | Binding domain not in DNS allow list |
| D-01 | MEDIUM | Discovery enabled without daemon |
| D-02 | MEDIUM | `allow_pods` without `allow_discovery` |

---

## lock

Freeze all processes in a pod (SIGSTOP). The pod is paused — no CPU, no network activity. Use for incident response or review.

```
envpod lock [<name>] [--all]
```

| Flag | Description |
|---|---|
| `name` | Pod name |
| `--all` | Freeze every pod simultaneously |

**Use cases:**

```bash
# Freeze one pod
sudo envpod lock myagent

# Freeze everything (incident response)
sudo envpod lock --all

# Freeze, review, then resume via remote
sudo envpod lock myagent
sudo envpod audit myagent --json | tail -20
sudo envpod remote myagent resume
```

---

## destroy

Remove a pod completely: tears down the namespace (if running), removes cgroup, deletes overlay and state. Safe to run on background pods — automatically terminates the supervisor process and in-pod processes before cleanup.

```
envpod destroy <name> [name2...] [--base] [--full]
```

| Flag | Description |
|---|---|
| `names...` | One or more pod names |
| `--base` | Also remove the associated base pod |
| `--full` | Immediately clean up iptables rules (slower; without this, use `gc` later) |

**Use cases:**

```bash
# Remove one pod
sudo envpod destroy myagent

# Remove a running background pod (stops it first)
sudo envpod destroy myagent

# Remove multiple at once
sudo envpod destroy agent1 agent2 agent3

# Remove pod and its base (clean slate)
sudo envpod destroy myagent --base

# Immediate full cleanup (no stale iptables)
sudo envpod destroy myagent --full

# Destroy all pods matching a pattern
sudo envpod ls --json | jq -r '.[].name' | grep "^test-" | xargs sudo envpod destroy
```

**Without `--full`:** Iptables rules referencing the destroyed pod's veth are left in place. They are harmless (reference non-existent interfaces) but tidy them up with `envpod gc`.

---

## prune

Remove all stopped and created (never started) pods in one pass. Running and frozen pods are preserved. Use this to clean up after batch operations or abandoned experiments.

```
envpod prune [--bases]
```

| Flag | Description |
|---|---|
| `--bases` | Also prune base pods that are not referenced by any remaining pod |

**Use cases:**

```bash
# Remove all stopped pods
sudo envpod prune

# Remove stopped pods and unreferenced bases
sudo envpod prune --bases

# Typical fleet cleanup after a batch run
sudo envpod stop worker-1 worker-2 worker-3
sudo envpod prune
sudo envpod gc

# Preview what would be pruned (check ls first)
sudo envpod ls
sudo envpod prune
```

**What it preserves:** Running pods, frozen (locked) pods, and their base pods. Only stopped and created pods are removed.

---

## ls

List all pods with status.

```
envpod ls [--json]
```

| Flag | Description |
|---|---|
| `--json` | Full JSON output with all fields |

**Output:**
```
NAME          STATUS     CPU    MEM     DIFF  BACKEND
myagent       running    2%     128MB   3     native
test-pod      stopped    —      —       0     native
```

**Use cases:**

```bash
# Quick overview
sudo envpod ls

# Get all pod names
sudo envpod ls --json | jq -r '.[].name'

# Running pods only
sudo envpod ls --json | jq '.[] | select(.status == "running") | .name'

# Pods with uncommitted changes
sudo envpod ls --json | jq '.[] | select(.diff_count > 0)'

# Script: destroy all stopped pods
sudo envpod ls --json | jq -r '.[] | select(.status == "stopped") | .name' \
  | xargs -r sudo envpod destroy
```

---

## status

Show detailed status and live resource usage for a pod.

```
envpod status <name> [--json]
```

**Output includes:** Status, PID, CPU%, memory (used/limit), I/O, PID count, network mode, diff count, vault key count.

**Use cases:**

```bash
sudo envpod status myagent

# JSON for monitoring scripts
sudo envpod status myagent --json | jq '{cpu: .cpu_pct, mem: .mem_mb}'

# Alert if memory above threshold
MEM=$(sudo envpod status myagent --json | jq '.mem_mb')
[ "$MEM" -gt 900 ] && echo "WARNING: pod using ${MEM}MB"
```

---

## logs

Show pod stdout/stderr output.

```
envpod logs <name> [-f] [-n <lines>]
```

| Flag | Default | Description |
|---|---|---|
| `name` | required | Pod name |
| `-f, --follow` | false | Stream new output as it arrives |
| `-n <N>` | 50 | Show last N lines (0 = all) |

**Use cases:**

```bash
# Last 50 lines
sudo envpod logs myagent

# Follow live
sudo envpod logs myagent -f

# All output
sudo envpod logs myagent -n 0

# Last 200 lines
sudo envpod logs myagent -n 200

# Search output
sudo envpod logs myagent -n 0 | grep ERROR
```

---

## clone

Fast-clone a pod from its base snapshot. Symlinks the rootfs (~130ms) — 10× faster than `init`.

```
envpod clone <source> <name> [--current]
```

| Flag | Description |
|---|---|
| `source` | Source pod name (must have a base snapshot) |
| `name` | Name for the new cloned pod |
| `--current` | Clone from the pod's current state (includes uncommitted overlay changes) |

**Use cases:**

```bash
# Clone a standard agent for each new task
sudo envpod clone myagent task-001
sudo envpod clone myagent task-002

# Clone 10 agents for parallel processing
for i in $(seq 1 10); do sudo envpod clone myagent worker-$i; done

# Clone current state (preserve agent's work so far)
sudo envpod clone myagent myagent-checkpoint --current

# Clone then run immediately
sudo envpod clone myagent session-$(date +%s) && sudo envpod run session-... -- python3 agent.py
```

**Requires:** Source pod must have a base snapshot (created via `--create-base` during `init`/`setup`, or manually via `envpod base create`).

---

## base

Manage reusable base pods. A base is a rootfs + overlay snapshot that clones are born from.

```
envpod base create <name> [-c pod.yaml] [-v]
envpod base ls [--json]
envpod base destroy <name> [name2...] [--force]
envpod base prune
```

### base create

Runs `init` + `setup` commands + snapshots the result. The temporary pod is removed; only the base snapshot remains.

```bash
# Create from pod.yaml
sudo envpod base create python-agent -c agent/pod.yaml

# With live output
sudo envpod base create python-agent -c pod.yaml --verbose

# After updating pod.yaml, rebuild the base
sudo envpod base destroy python-agent
sudo envpod base create python-agent -c pod.yaml -v
```

### base ls

```bash
sudo envpod base ls
# NAME           SIZE     CREATED
# python-agent   1.2 GB   2026-03-01 14:22

sudo envpod base ls --json
```

### base destroy

```bash
# Destroy one base
sudo envpod base destroy python-agent

# Destroy multiple
sudo envpod base destroy base1 base2 base3

# Force destroy even if pods still reference it
sudo envpod base destroy python-agent --force
```

### base prune

Remove all base pods that are not referenced by any existing pod. Safe cleanup — only removes orphaned bases.

```bash
# Remove all unreferenced bases
sudo envpod base prune

# See what bases exist before pruning
sudo envpod base ls
sudo envpod base prune
```

**Use cases:**

```bash
# Typical versioning workflow
sudo envpod base create agent-v1 -c v1/pod.yaml
# ... time passes, update pod.yaml ...
sudo envpod base create agent-v2 -c v2/pod.yaml
# New clones use v2; existing v1 clones keep working
sudo envpod clone agent-v2 new-session

# Build a specialized base for each project
sudo envpod base create node-agent -c configs/nodejs.yaml
sudo envpod base create python-ml -c configs/ml.yaml
sudo envpod base create browser-agent -c configs/browser.yaml
```

---

## snapshot

Named point-in-time captures of the overlay. Unlike `commit`, snapshots do not touch the host filesystem — they save the agent's in-progress state so you can restore it later.

```
envpod snapshot <name> create [--name <label>]
envpod snapshot <name> ls
envpod snapshot <name> restore <id> [-y]
envpod snapshot <name> destroy <id>
envpod snapshot <name> prune
envpod snapshot <name> promote <id> <base-name>
```

### snapshot create

```bash
# Unnamed (auto-label with timestamp)
sudo envpod snapshot myagent create

# Named label
sudo envpod snapshot myagent create --name "after-data-fetch"
sudo envpod snapshot myagent create -n "pre-refactor"
```

### snapshot ls

```bash
sudo envpod snapshot myagent ls
# ID          LABEL              CREATED               SIZE
# snap-a1b2   after-data-fetch   2026-03-01 14:22:00   45 MB
# snap-c3d4   (auto)             2026-03-01 09:10:33   38 MB
```

### snapshot restore

Pod must be stopped before restoring.

```bash
# Restore by ID prefix
sudo envpod snapshot myagent restore snap-a1b2

# Skip confirmation
sudo envpod snapshot myagent restore snap-a1b2 --yes
sudo envpod snapshot myagent restore snap-a1b2 -y
```

### snapshot destroy

```bash
sudo envpod snapshot myagent destroy snap-c3d4
```

### snapshot prune

Remove oldest auto-snapshots down to `snapshot.keep_last` in pod.yaml. Manual (labeled) snapshots are never pruned.

```bash
sudo envpod snapshot myagent prune
```

### snapshot promote

Promote a snapshot to a new base pod — allowing cloning from any historical state.

```bash
# Save today's agent state as a base for future clones
sudo envpod snapshot myagent promote snap-a1b2 agent-v2-trained

# Then clone from that exact state
sudo envpod clone agent-v2-trained new-session
```

**Use cases:**

```bash
# Checkpoint before a risky operation
sudo envpod snapshot myagent create -n "before-delete-old-data"
sudo envpod run myagent -- python3 cleanup.py
# If bad:
sudo envpod snapshot myagent restore before-delete-old-data

# Save progress mid-run: stop pod, snapshot, resume
sudo envpod lock myagent
sudo envpod snapshot myagent create -n "day-1-checkpoint"
sudo envpod remote myagent resume

# Auto-snapshot policy (pod.yaml):
# snapshot:
#   auto: true
#   keep_last: 10
#   auto_prune: true
```

---

## vault

Encrypted credential vault. Secrets are ChaCha20-Poly1305 encrypted at rest. Never stored in env files or pod.yaml.

```
envpod vault <pod> set <KEY>
envpod vault <pod> get <KEY>
envpod vault <pod> list
envpod vault <pod> rm <KEY>
envpod vault <pod> import <path> [--overwrite]
envpod vault <pod> bind <KEY> <domain> <header>
envpod vault <pod> unbind <KEY>
envpod vault <pod> bindings
```

### vault set

Value is read from **stdin** — never passed as a CLI argument (would appear in shell history).

```bash
# Interactive prompt
sudo envpod vault myagent set OPENAI_API_KEY
# Enter value: (typed, hidden)

# From pipe (for scripts)
echo "sk-..." | sudo envpod vault myagent set OPENAI_API_KEY

# From file
cat ~/.secrets/openai | sudo envpod vault myagent set OPENAI_API_KEY
```

### vault get

```bash
sudo envpod vault myagent get OPENAI_API_KEY
# Outputs value to stdout
```

### vault list

```bash
sudo envpod vault myagent list
# OPENAI_API_KEY
# SENDGRID_API_KEY
# DATABASE_URL
```

### vault rm

```bash
sudo envpod vault myagent rm OLD_KEY
```

### vault import

Import from a `.env` file (lines of `KEY=value`).

```bash
# Import all keys (skip conflicts)
sudo envpod vault myagent import .env

# Import and overwrite existing
sudo envpod vault myagent import .env --overwrite

# Import from a secrets file
sudo envpod vault myagent import /etc/agent-secrets.env
```

### vault bind (Premium)

Bind a vault key to a domain for transparent proxy injection. The agent uses a dummy key; the proxy injects the real one.

```bash
# OpenAI
sudo envpod vault myagent bind OPENAI_API_KEY api.openai.com "Authorization: Bearer {value}"

# Anthropic
sudo envpod vault myagent bind ANTHROPIC_API_KEY api.anthropic.com "Authorization: Bearer {value}"

# Custom API key header
sudo envpod vault myagent bind STRIPE_KEY api.stripe.com "Authorization: Bearer {value}"
sudo envpod vault myagent bind CUSTOM_KEY api.example.com "X-API-Key: {value}"
```

Also requires in pod.yaml:
```yaml
vault:
  proxy: true
```

### vault unbind

```bash
sudo envpod vault myagent unbind OPENAI_API_KEY
```

### vault bindings

```bash
sudo envpod vault myagent bindings
# KEY                  DOMAIN                HEADER
# OPENAI_API_KEY       api.openai.com        Authorization: Bearer {value}
# ANTHROPIC_API_KEY    api.anthropic.com     Authorization: Bearer {value}
```

---

## queue

View the action staging queue for a pod.

```
envpod queue <name> [--json]
envpod queue <name> add --tier <tier> --description <desc> [--delay <secs>]
```

### queue (list)

```bash
sudo envpod queue myagent
# ID        TIER    STATUS   CREATED    DESCRIPTION
# a1b2c3    staged  queued   14:22:01   send_email to=ops@co.com subject="Done"
# a1b2c4    delayed queued   14:22:05   delete /tmp/old_cache (executes in 28s)

# JSON
sudo envpod queue myagent --json
```

### queue add

Manually add a staged action (for testing or host-initiated governance):

```bash
sudo envpod queue myagent add \
  --tier staged \
  --description "Deploy build artifact to staging server"

sudo envpod queue myagent add \
  --tier delayed \
  --description "Purge old log files" \
  --delay 300
```

---

## approve

Approve a staged action. envpod fetches credentials from the vault and executes it.

```
envpod approve <pod> <id>
```

`id` can be the full UUID or any unique 8-character prefix.

**Use cases:**

```bash
# Approve by prefix
sudo envpod approve myagent a1b2c3

# Approve all pending (shell loop)
sudo envpod queue myagent --json \
  | jq -r '.[] | select(.status == "queued" and .tier == "staged") | .id' \
  | xargs -I{} sudo envpod approve myagent {}

# Review content, then approve
sudo envpod queue myagent --json | jq '.[] | {id: .id[:8], desc: .description}'
sudo envpod approve myagent a1b2c3

# Approve a commit that's gated by queue
sudo envpod approve myagent <commit-action-id>  # triggers the actual commit
```

---

## cancel

Cancel a queued action before it executes.

```
envpod cancel <pod> <id>
```

**Use cases:**

```bash
# Cancel by prefix
sudo envpod cancel myagent a1b2c4

# Cancel all pending delayed actions (e.g. before they auto-execute)
sudo envpod queue myagent --json \
  | jq -r '.[] | select(.tier == "delayed" and .status == "queued") | .id' \
  | xargs -I{} sudo envpod cancel myagent {}
```

---

## undo

Undo an already-executed action.

```
envpod undo <name> [<id>] [--all]
```

| Flag | Description |
|---|---|
| `id` | Action ID to undo. Omit to list undoable actions. |
| `--all` | Undo all undoable actions |

**Use cases:**

```bash
# See what can be undone
sudo envpod undo myagent

# Undo a specific action
sudo envpod undo myagent <id>

# Undo everything
sudo envpod undo myagent --all
```

Not all actions are undoable — external effects (emails sent, HTTP POSTs) cannot be reversed. Filesystem actions and freezes can be.

---

## actions

Manage the action catalog — the host-defined menu of what agents are allowed to do.

```
envpod actions <pod> ls
envpod actions <pod> add --name <n> --description <d> [--tier <t>] [--param name[:required]...]
envpod actions <pod> remove <name>
envpod actions <pod> set-tier <name> <tier>
```

### actions ls

```bash
sudo envpod actions myagent ls
# NAME              TIER       SCOPE      TYPE
# send_alert        immediate  external   slack_message
# commit_work       staged     internal   git_commit
# save_output       immediate  internal   file_write
# log_event         immediate  internal   (custom)
```

### actions add

```bash
# Custom action with params
sudo envpod actions myagent add \
  --name create_jira_ticket \
  --description "Create a Jira ticket from a bug report" \
  --tier staged \
  --param title:required \
  --param description:required \
  --param priority \
  --param labels

# Simple immediate action
sudo envpod actions myagent add \
  --name ping_health \
  --description "Check if the service is alive" \
  --tier immediate

# Then edit actions.json to add action_type and config
```

For built-in types, edit `{pod_dir}/actions.json` directly to set `action_type` and `config`:
```json
{
  "name": "send_alert",
  "action_type": "slack_message",
  "tier": "immediate",
  "config": {"auth_vault_key": "SLACK_WEBHOOK_URL"}
}
```

### actions remove

```bash
sudo envpod actions myagent remove send_alert
```

### actions set-tier

Change tier live — takes effect on the next `list_actions` query from the agent (hot-reload, no restart).

```bash
# Demote to staged (add human checkpoint)
sudo envpod actions myagent set-tier send_alert staged

# Block an action permanently for this run
sudo envpod actions myagent set-tier send_sms blocked

# Promote to immediate (remove checkpoint)
sudo envpod actions myagent set-tier ping_health immediate
```

**Use cases:**

```bash
# Block all external actions during incident response
sudo envpod actions myagent set-tier send_alert blocked
sudo envpod actions myagent set-tier http_post blocked

# Gradually give agent more autonomy as trust builds
# Day 1: everything staged
sudo envpod actions myagent set-tier save_output staged
# Day 5: promote read-only to immediate after observing behavior
sudo envpod actions myagent set-tier save_output immediate
```

---

## ports

View or mutate port forwarding rules on a running pod. No restart required.

```
envpod ports <name>
envpod ports <name> -p host:pod [--publish-all|-P host:pod] [--internal|-i pod_port]
envpod ports <name> --remove host_port
envpod ports <name> --remove-internal pod_port
```

| Flag | Description |
|---|---|
| (no flags) | Show current port forwards |
| `-p host:pod` | Add localhost-only forward |
| `-P host:pod` | Add all-interfaces forward |
| `-i pod_port` | Add pod-to-pod internal port |
| `--remove port` | Remove a forward by host port |
| `--remove-internal port` | Remove an internal port |

**Use cases:**

```bash
# Show current forwards
sudo envpod ports myagent

# Add a port while the pod is running
sudo envpod ports myagent -p 9000:3000

# Remove a port
sudo envpod ports myagent --remove 9000

# Expose a new service without restarting
sudo envpod ports myagent -p 8888:8888

# Add multiple
sudo envpod ports myagent -p 8080:3000 -p 5000:5000

# Open a port for pod-to-pod access
sudo envpod ports myagent -i 3000

# Remove internal port
sudo envpod ports myagent --remove-internal 3000
```

---

## discover

View or mutate pod discovery settings on a running pod. Takes effect immediately via the envpod-dns daemon.

```
envpod discover <name>                         # show status
envpod discover <name> --on                    # register as <name>.pods.local
envpod discover <name> --off                   # unregister
envpod discover <name> --add-pod <other>       # allow querying <other>.pods.local
envpod discover <name> --remove-pod <other>    # remove from allow list
envpod discover <name> --remove-pod '*'        # clear all allowed pods
```

**Use cases:**

```bash
# Check current discovery status
sudo envpod discover myagent

# Enable discovery (other pods with permission can find this pod)
sudo envpod discover myagent --on

# Let this pod query worker-1 and worker-2
sudo envpod discover myagent --add-pod worker-1 --add-pod worker-2

# Open to all discoverable pods
sudo envpod discover myagent --add-pod '*'

# Revoke access to a specific pod
sudo envpod discover myagent --remove-pod worker-1

# Take pod off the network (incident response)
sudo envpod discover myagent --off
sudo envpod discover myagent --remove-pod '*'
```

**Requires:** `envpod dns-daemon` running. Changes also written to pod.yaml for persistence across restarts.

---

## dns

Update DNS policy on a running pod (domain allow/deny lists). Takes effect immediately — no pod restart.

```
envpod dns <name> --allow domain [--allow domain...] [--deny domain...] [--remove-allow domain...] [--remove-deny domain...]
```

| Flag | Description |
|---|---|
| `--allow <domain>` | Add domain to allow list (repeatable) |
| `--deny <domain>` | Add domain to deny list (repeatable) |
| `--remove-allow <domain>` | Remove domain from allow list |
| `--remove-deny <domain>` | Remove domain from deny list |

**Use cases:**

```bash
# Give pod access to a new API
sudo envpod dns myagent --allow api.stripe.com

# Add multiple
sudo envpod dns myagent --allow api.stripe.com --allow cdn.stripe.com

# Block a domain that was previously allowed
sudo envpod dns myagent --deny exfiltration-risk.com

# Remove a previously added allow (tighten policy)
sudo envpod dns myagent --remove-allow api.slack.com

# Incident response: block all outbound
sudo envpod dns myagent --deny '*'
```

---

## dns-daemon

Start the central pod discovery daemon. Required for `allow_discovery` and `allow_pods` features. Bilateral enforcement — both source and target must opt in.

```
envpod dns-daemon [--socket <path>]
```

| Flag | Default | Description |
|---|---|---|
| `--socket <path>` | `/var/lib/envpod/dns.sock` | Unix socket path |

**Use cases:**

```bash
# Start daemon (typically run at system startup)
sudo envpod dns-daemon

# Custom socket path
sudo envpod dns-daemon --socket /run/envpod/discovery.sock

# As a systemd service (recommended for production)
# /etc/systemd/system/envpod-dns.service:
# [Service]
# ExecStart=/usr/local/bin/envpod dns-daemon
# Restart=always

sudo systemctl enable envpod-dns
sudo systemctl start envpod-dns

# Check it's running
ls /var/lib/envpod/dns.sock
```

**Fail-safe:** If the daemon is not running, `*.pods.local` queries return NXDOMAIN. All other DNS and pod features continue normally.

---

## remote

Send a remote control command to a running pod over its Unix socket.

```
envpod remote <name> <cmd> [--payload <json>]
```

| Command | What It Does |
|---|---|
| `freeze` | Pause all processes (SIGSTOP) |
| `resume` | Resume frozen processes (SIGCONT) |
| `kill` | Terminate all processes (SIGKILL) |
| `restrict` | Reduce resource limits live (pass JSON payload) |
| `status` | Get current pod status |
| `alerts` | Get recent monitoring alerts |

**Use cases:**

```bash
# Freeze pod remotely
sudo envpod remote myagent freeze

# Resume after inspection
sudo envpod remote myagent resume

# Get live status
sudo envpod remote myagent status

# Kill runaway process
sudo envpod remote myagent kill

# Restrict CPU and memory live (incident throttling)
sudo envpod remote myagent restrict --payload '{"cpu_cores": 0.5, "memory_mb": 256}'

# Get monitoring alerts
sudo envpod remote myagent alerts
```

---

## monitor

Manage monitoring policy and view alerts.

```
envpod monitor <name> set-policy <path>
envpod monitor <name> alerts [--json]
```

### monitor set-policy

Install a monitoring policy from a YAML file.

```bash
sudo envpod monitor myagent set-policy monitoring-policy.yaml
```

Example `monitoring-policy.yaml`:
```yaml
enabled: true
thresholds:
  cpu_pct: 90
  memory_pct: 85
  dns_queries_per_min: 500
action_on_alert: freeze   # or: log, kill, restrict
```

### monitor alerts

```bash
# Show monitoring alerts from audit log
sudo envpod monitor myagent alerts

# JSON
sudo envpod monitor myagent alerts --json | jq '.[0]'
```

---

## mount

Mount a host path into the pod's overlay (live mount, pod can be running).

```
envpod mount <name> <host_path> [--target <pod_path>] [--readonly]
```

| Flag | Description |
|---|---|
| `host_path` | Path on the host to mount |
| `--target <path>` | Mount point inside pod (defaults to `host_path`) |
| `--readonly` | Read-only bind mount |

**Use cases:**

```bash
# Mount a data directory
sudo envpod mount myagent /data/corpus

# Mount at a different path inside pod
sudo envpod mount myagent /data/corpus --target /workspace/data

# Read-only (agent can read, not write)
sudo envpod mount myagent /home/user/docs --target /workspace/docs --readonly

# Mount a single file
sudo envpod mount myagent /etc/myapp.conf --target /etc/myapp.conf --readonly
```

---

## unmount

Unmount a previously mounted path from the pod.

```
envpod unmount <name> <path>
```

```bash
sudo envpod unmount myagent /workspace/data
```

---

## dashboard

Start the web dashboard. Opens browser automatically at `http://localhost:9090`.

```
envpod dashboard [--port <N>] [--no-open]
```

| Flag | Default | Description |
|---|---|---|
| `--port <N>` | `9090` | Port to listen on |
| `--no-open` | false | Don't open browser |

**Features:** Fleet overview (2s polling), pod detail tabs (Overview, Audit, Diff, Resources, Snapshots, Queue), inline diff viewer (Premium), action buttons (commit, rollback, freeze, resume).

**Use cases:**

```bash
# Start on default port, open browser
sudo envpod dashboard

# Remote machine / headless server
sudo envpod dashboard --no-open

# Custom port
sudo envpod dashboard --port 8080

# Access from another machine (bind to all interfaces — run behind a reverse proxy)
# envpod dashboard listens on 127.0.0.1 by default; use SSH tunneling:
ssh -L 9090:localhost:9090 user@server "sudo envpod dashboard --no-open"
```

---

## gc

Clean up stale iptables rules left by destroyed pods. Batch operation — removes all dangling rules in one pass. Runs fast even with 100+ destroyed pods.

```
envpod gc
```

**Use cases:**

```bash
# After destroying many pods
sudo envpod destroy pod1 pod2 pod3
sudo envpod gc

# Add to cron for scheduled cleanup
# 0 3 * * * /usr/local/bin/envpod gc >> /var/log/envpod-gc.log 2>&1

# Check before gc (see stale rules)
sudo iptables -L | grep envpod
sudo envpod gc
sudo iptables -L | grep envpod  # should be empty
```

**Note:** `envpod destroy --full` does immediate iptables cleanup for that specific pod, so `gc` is not needed for it. `gc` handles the accumulation from many `destroy` calls without `--full`.

---

## completions

Generate shell tab completions.

```
envpod completions <shell>
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`

Completions include:
- **All subcommands** — `envpod <TAB>` lists every command
- **Pod names** — all pod subcommands (`run`, `diff`, `commit`, `destroy`, `fg`, `snapshot`, etc.) dynamically complete pod names from the state directory
- **Base pod names** — `envpod base destroy <TAB>` dynamically completes base pod names
- **Flags and options** — `--<TAB>` lists available flags per subcommand

**Install:**

```bash
# Bash
sudo envpod completions bash > /etc/bash_completion.d/envpod

# Zsh
sudo envpod completions zsh > /usr/local/share/zsh/site-functions/_envpod
# Then add to ~/.zshrc: autoload -Uz compinit && compinit

# Fish
sudo envpod completions fish > ~/.config/fish/completions/envpod.fish

# One-liner (bash): add to ~/.bashrc
eval "$(sudo envpod completions bash)"
```

---

## Patterns and Workflows

### New Agent Workflow

```bash
# 1. Define pod
cat > myagent/pod.yaml <<EOF
name: myagent
network:
  mode: Filtered
  allow:
    - api.openai.com
queue:
  socket: true
EOF

# 2. Init
sudo envpod init myagent -c myagent/pod.yaml

# 3. Set secrets
echo "sk-..." | sudo envpod vault myagent set OPENAI_API_KEY

# 4. Define what agent can do
cat > /var/lib/envpod/pods/myagent/actions.json <<'EOF'
[{"name":"save_output","action_type":"file_write","tier":"immediate"}]
EOF

# 5. Run
sudo envpod run myagent -- python3 agent.py

# 6. Review changes
sudo envpod diff myagent
sudo envpod audit myagent

# 7. Approve or discard
sudo envpod commit myagent   # OR: sudo envpod rollback myagent
```

---

### Desktop Pod (Start/Stop)

```bash
# Create a desktop pod
sudo envpod init my-desktop --preset desktop

# Start it in the background (services auto-start)
sudo envpod start my-desktop

# Open http://localhost:6080 in your browser
# Open shells as needed
sudo envpod run my-desktop -- bash

# Stop when done (preserves everything)
sudo envpod stop my-desktop

# Resume tomorrow — same state, same overlay
sudo envpod start my-desktop
```

---

### Parallel Agent Fleet

```bash
# Build one base, clone many
sudo envpod base create coder -c configs/coder.yaml -v
for i in $(seq 1 20); do
  sudo envpod clone coder worker-$i
done

# Run all workers
for i in $(seq 1 20); do
  sudo envpod run worker-$i -- python3 worker.py --shard $i &
done
wait

# Review and commit selectively
for i in $(seq 1 20); do
  echo "=== worker-$i ==="
  sudo envpod diff worker-$i
done
sudo envpod commit worker-3
sudo envpod commit worker-7
# Others:
for i in $(seq 1 20); do sudo envpod rollback worker-$i 2>/dev/null; done

# Clean up: prune all stopped pods and unused bases
sudo envpod prune --bases
sudo envpod gc
```

---

### Checkpoint + Resume

```bash
sudo envpod run myagent -- python3 long_task.py &

# After 2 hours, checkpoint
sudo envpod lock myagent
sudo envpod snapshot myagent create -n "2h-checkpoint"
sudo envpod remote myagent resume

# Later, if the pod crashes, restore and continue
sudo envpod snapshot myagent restore 2h-checkpoint -y
sudo envpod run myagent -- python3 long_task.py --resume
```

---

### Incident Response

```bash
# Immediate containment
sudo envpod lock --all

# Inspect what happened
sudo envpod audit suspicious-agent --json | jq '.[-20:]'
sudo envpod diff suspicious-agent

# Block all outbound
sudo envpod dns suspicious-agent --deny '*'

# Kill if needed
sudo envpod remote suspicious-agent kill

# Preserve evidence before destroy
sudo envpod snapshot suspicious-agent create -n "incident-$(date +%s)"
sudo envpod diff suspicious-agent --json > /tmp/incident-diff.json
sudo envpod audit suspicious-agent --json > /tmp/incident-audit.json

# Resume other pods
sudo envpod remote pod1 resume
sudo envpod remote pod2 resume
```

---

### Security-First Pod Setup

```bash
# Audit config before creating the pod
sudo envpod audit --security -c pod.yaml

# Check for CRITICAL and HIGH findings only
sudo envpod audit --security -c pod.yaml --json \
  | jq '.findings[] | select(.severity | test("CRITICAL|HIGH"))'

# If clean, proceed
sudo envpod init myagent -c pod.yaml

# Runtime security checks
sudo envpod monitor myagent set-policy monitoring-policy.yaml
sudo envpod monitor myagent alerts --json
```

---

### Rolling Update (Zero-Downtime Agent Replace)

```bash
# Current: agent-v1 running and serving
# New: agent-v2 ready to take over

sudo envpod base create agent-v2 -c v2/pod.yaml

# Start v2 on a different port
sudo envpod clone agent-v2 agent-v2-live
sudo envpod run agent-v2-live -p 8081:3000 -- ./server.sh

# Test v2
curl http://localhost:8081/health

# Switch traffic (update nginx/haproxy to point at 8081)
# Stop v1
sudo envpod lock agent-v1-live
sudo envpod destroy agent-v1-live
```

---

### CI/CD Gate: Block Deploys with Uncommitted Agent Changes

```bash
#!/bin/bash
# ci-gate.sh — fails if any agent has uncommitted changes
PODS=$(sudo envpod ls --json | jq -r '.[].name')
DIRTY=0
for pod in $PODS; do
  COUNT=$(sudo envpod diff $pod --json | jq length)
  if [ "$COUNT" -gt 0 ]; then
    echo "FAIL: $pod has $COUNT uncommitted change(s)"
    DIRTY=1
  fi
done
exit $DIRTY
```

---

*Copyright 2026 Xtellix Inc. All rights reserved.*
