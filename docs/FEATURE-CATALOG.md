# envpod Feature Catalog

The single source of truth for every envpod capability. For evaluators, enterprise buyers, and teams deciding between CE and Premium.

One YAML. One binary (9 MB CE / 22 MB Premium). One command. No daemon, no runtime dependencies.

```bash
# CE (free forever)
curl -fsSL https://envpod.dev/install.sh | sh

# Premium ($399/seat/mo)
curl -fsSL https://premium.envpod.dev/install.sh | sh
envpod license activate <KEY>
```

---

## Table of Contents

1. [Filesystem Governance](#1-filesystem-governance)
2. [Network Isolation](#2-network-isolation)
3. [Process Isolation](#3-process-isolation)
4. [Credential Vault](#4-credential-vault)
5. [Action Queue & Governance](#5-action-queue--governance)
6. [Monitoring & Audit](#6-monitoring--audit)
7. [Prompt Screening](#7-prompt-screening)
8. [Devices & Display](#8-devices--display)
9. [Identity & Authentication](#9-identity--authentication)
10. [Fleet Management](#10-fleet-management)
11. [Remote Management](#11-remote-management)
12. [Service Proxy](#12-service-proxy)
13. [Web Dashboard](#13-web-dashboard)
14. [Developer Experience](#14-developer-experience)
15. [Licensing & Distribution](#15-licensing--distribution)
16. [Compliance & Standards](#16-compliance--standards)
17. [Platform Support](#17-platform-support)
18. [Performance](#18-performance)

---

## Feature Summary

| # | Category | CE Features | Premium Features |
|---|----------|------------|-----------------|
| 1 | **Filesystem** | COW overlay, diff/commit/rollback, selective commit, mount_cwd, custom mounts (bind + COW), system access modes, snapshots, cloning (8ms), base pods, base resize, garbage collection, prune, disk/tmp limits, tracking config | + Sealed mode, OPA commit policy |
| 2 | **Network** | DNS filtering (allow/deny/monitor), anti-tunneling, port forwarding (local/public/internal), firewall, pod-to-pod discovery (bilateral), live DNS/port/discovery mutation, named services | + DoH blocking, L7 HTTP policy, L7 pod-to-pod governance, Tailscale mesh, Headscale |
| 3 | **Process** | PID namespace, cgroups v2, seccomp-BPF (3 profiles), NO_NEW_PRIVS, /proc masking, minimal /dev, GPU info masking, pod resize | — |
| 4 | **Vault** | ChaCha20-Poly1305 encrypted, env injection, bulk import, live mutation | + Vault proxy (MITM), per-agent scoping, OPA vault policy |
| 5 | **Action Queue** | 4 tiers, 20 action types, queue socket, undo registry, hot-reload catalog, approve/cancel/undo | + Privilege escalation (12 types, 3 scopes), OPA queue policy, MCP tool governance |
| 6 | **Monitoring** | Audit log (39 types), monitoring agent, budget (time-based), health checks (single), security scan | + Multi-dimension budget, multi-check health, recovery sequences, notifications, scorecard, OWASP attestation, adversarial verify, OpenTelemetry, Grafana+Loki |
| 7 | **Screening** | Layer 1 regex (~1ms, 53 patterns), jailbreak test, auto-update rules | + Layer 2 local AI (planned), Layer 3 cloud AI (planned) |
| 8 | **Devices** | GPU passthrough, display forwarding (Wayland/X11), audio (PipeWire/PulseAudio), noVNC desktop, audio streaming, file upload, clipboard, desktop envs (XFCE/Openbox/Sway), resize | — |
| 9 | **Identity** | — | Pod identity (Ed25519), agent registry, JWT tokens, per-agent vault scoping, audit attribution, OIDC/SSO (Okta/Azure/Google/Keycloak) |
| 10 | **Fleet** | init/run/start/stop/restart/kill/lock/unlock/fg/destroy/ls/status/logs, service register, clone, base pods | + envpod up/., IaC (apply), parallel clone, batch executor, scale, IDE integration, ssh-proxy |
| 11 | **Remote** | Local Unix socket control | + HTTP API (9 endpoints), WebSocket relay, RemotePod SDK (Python+TS), node daemon, SSH proxy |
| 12 | **Service Proxy** | — | expose_service (*.envpod.cloud), two-token auth, subdomain locking, token rotation, public/private, relay+Tailscale routing |
| 13 | **Dashboard** | Fleet view, pod detail (audit/diff/resources/snapshots/queue), freeze/resume, daemon mode | + Create/destroy/clone pods, agent table, pod public key |
| 14 | **Developer UX** | Interactive init, 18+ presets, 68+ examples, setup scripts, pre-setup, start_command, envpod group, rootless, completions, tilde expansion, update checker | + envpod up, toolchain (18 entries), devcontainer/Dockerfile compat, providers (SSH), framework integrations (15+) |
| 15 | **Licensing** | BSL 1.1, free forever, 9 MB binary | Proprietary, $399/seat, 22 MB binary, heartbeat, key file, offline grace, 27 gates |
| 16 | **Compliance** | OWASP 10/10 at kernel level | + OWASP attestation, NIST AI RMF, EU AI Act mapping |
| 17 | **Platforms** | 11 platforms (Ubuntu, Debian, Fedora, Rocky, Arch, openSUSE, WSL2, macOS beta, RPi, Jetson, Docker nested) | Same |
| 18 | **Performance** | Init 1.3s, clone 8ms, DNS 2.6x faster, 4MB/pod, 9MB binary | Same benchmarks, 22 MB binary |

---

## 1. Filesystem Governance

The foundation. Every agent write goes to a copy-on-write overlay. The host is never modified until a human reviews and commits.

### Copy-on-Write Filesystem (CE)

```yaml
name: my-agent
filesystem:
  system_access: safe
```

```bash
envpod init my-agent
envpod run my-agent -- bash -c "echo 'hello' > /opt/output.txt"
envpod diff my-agent          # + /opt/output.txt (6 bytes)
envpod commit my-agent        # applies to host
envpod rollback my-agent      # or discard everything
```

### System Access Modes (CE)

| Mode | System dirs | Agent can modify system? |
|------|------------|------------------------|
| `safe` (default) | Read-only bind mounts | No |
| `advanced` | Per-dir COW overlays | Yes, to overlay only |
| `dangerous` | Per-dir COW overlays | Yes, to overlay only |

### Mount CWD — COW Overlay (CE)

```yaml
filesystem:
  mount_cwd: true
```

```bash
cd /home/user/project
envpod init my-agent -c pod.yaml
envpod run my-agent -- bash          # starts in /home/user/project
envpod diff my-agent                 # shows what agent changed
envpod commit my-agent --all         # push to host, ownership preserved
```

POSIX ACLs enable overlay copy-up. Pre-copy ensures agent can modify existing files. Host never touched until commit.

### Custom Mounts (CE)

```yaml
filesystem:
  mounts:
    - path: /data/models                    # read-write bind mount (default)
    - path: /etc/configs
      permissions: readonly                 # read-only bind mount
    - path: /home/user/project
      cow: true                             # COW overlay (host protected)
    - path: /host/data
      target: /data                         # mount at different path in pod
```

### Selective Commit (CE)

```bash
envpod commit my-agent /opt/output.txt       # commit one file
envpod commit my-agent --rollback-rest        # commit tracked, discard rest
envpod commit my-agent --output /tmp/export   # export to custom dir
```

### Tracking Config — Watch/Ignore (CE)

```yaml
filesystem:
  tracking:
    watch: [/home, /opt, /workspace]     # only show these in diff
    ignore: [/var/lib/apt, /var/cache]   # always exclude
```

`--all` bypasses tracking. Controls what `envpod diff` and `envpod commit` show.

### Snapshots & Cloning (CE)

```bash
envpod snapshot my-agent create "before-refactor"
envpod snapshot my-agent restore before-refactor
envpod snapshot my-agent list

envpod base my-agent create python-base
envpod base resize python-base --memory 8GB    # resize base config
envpod clone python-base experiment            # 8ms clone
```

### Garbage Collection (CE)

```bash
envpod gc              # clean stale iptables rules
envpod prune           # remove all stopped/created pods
envpod prune --bases   # also remove unused bases
```

### Disk & Tmp Size Limits (CE)

```yaml
processor:
  disk_size: 10GB     # max overlay storage (loopback ext4)
  tmp_size: 2GB       # pod /tmp tmpfs size
```

### Sealed Mode (Premium)

Zero host visibility. Agent cannot see the host filesystem.

```yaml
filesystem:
  sealed: true
```

---

## 2. Network Isolation

Every pod gets its own network namespace with DNS filtering.

### DNS Filtering (CE)

```yaml
network:
  mode: Filtered
  dns:
    mode: Allowlist          # Allowlist | Denylist | Monitor
    allow:
      - api.anthropic.com
      - api.openai.com
```

```bash
envpod dns my-agent --allow pypi.org          # live mutation, no restart
envpod dns my-agent --deny evil.com
```

### Anti-DNS Tunneling (CE)

Built into every pod DNS resolver. Detects encoded payloads in DNS queries.

### Port Forwarding (CE)

```yaml
network:
  ports: ["8080:8080"]              # localhost only
  public_ports: ["443:443"]         # all interfaces
  internal_ports: ["5432:5432"]     # pod-to-pod only
```

```bash
envpod ports my-agent -p 9090:9090    # live, no restart
```

### Port Exposure Firewall (CE)

```yaml
network:
  expose: [8080, 443]
```

```bash
envpod expose my-agent --add 8080     # live mutation
envpod expose my-agent --list
```

Only declared ports reachable on pod IP. All others dropped.

### Pod-to-Pod Discovery (CE)

Bilateral consent required.

```yaml
# Pod A
network:
  allow_discovery: true
  allow_pods: [pod-b]
```

```bash
envpod dns-daemon                                 # start central daemon
envpod discover my-agent --add-pod other-agent    # live mutation
# Inside pod: ping pod-b.pods.local
```

### Named Services (CE)

```yaml
network:
  services:
    - name: api
      port: 8080
    - name: metrics
      port: 9090
```

### DoH Blocking (Premium)

```yaml
network:
  block_doh: true    # blocks 14 known DoH resolver IPs
```

### L7 Network Policy (Premium)

HTTP method + path filtering per agent via OPA.

### L7 Pod-to-Pod Governance (Premium)

OPA policy on inter-pod traffic with identity verification. Method, path, capabilities, content_type checked.

### Tailscale Mesh (Premium)

```yaml
network:
  tailscale:
    enabled: true
    auth_key_vault: TAILSCALE_KEY
```

Per-pod tailnet identity, WireGuard tunnels, remote access from anywhere.

### Headscale (Premium)

Self-hosted Tailscale coordination at `mesh.envpod.dev`.

---

## 3. Process Isolation

### PID Namespace (CE)

Agent is PID 1. Cannot see or signal host processes.

### cgroups v2 Resource Limits (CE)

```yaml
processor:
  cores: 2.0
  memory: 4GB
  max_pids: 256
  cpu_affinity: "0-3"
```

### Pod Resize (CE)

```bash
envpod resize my-agent --cpus 4.0 --memory 8GB
envpod resize my-agent --gpu true --display true --audio true
envpod resize my-agent --desktop xfce --web-display true
```

Live config mutation. Some changes require restart.

### seccomp-BPF Syscall Filtering (CE)

```yaml
security:
  seccomp: default      # default (~145) | browser (+10) | none
```

### NO_NEW_PRIVS (CE)

Always enabled. Prevents privilege escalation via setuid binaries.

### /proc Masking (CE)

Sensitive entries bind-mounted to `/dev/null`. `/proc/cpuinfo` sanitized (model name hidden, CPU count matches cgroup). `/proc/1/` entries masked.

### Minimal /dev (CE)

Default-deny device policy. Only essential devices: null, urandom, tty, zero, full, random.

### GPU Info Masking (CE)

When GPU disabled, `/dev/nvidia*` info paths masked with empty tmpfs.

---

## 4. Credential Vault

### Encrypted Storage (CE)

ChaCha20-Poly1305 at rest.

```bash
envpod vault my-agent set API_KEY sk-ant-abc123...
envpod vault my-agent list
envpod vault my-agent import .env
envpod vault my-agent delete API_KEY
```

Agent sees secrets as environment variables + live file at `/run/envpod/secrets.env`.

### Vault Proxy — Agent Never Sees Keys (Premium)

Transparent HTTPS MITM. Agent sends dummy credentials, proxy injects real ones.

```yaml
vault:
  proxy: true
  bindings:
    - domain: api.anthropic.com
      header: x-api-key
      vault_key: ANTHROPIC_KEY
```

### Per-Agent Vault Scoping (Premium)

```yaml
identity:
  agents:
    - name: coder
      vault_keys: [GITHUB_TOKEN]
    - name: deployer
      vault_keys: [AWS_KEY, SSH_KEY]
```

### OPA Vault Access Policy (Premium)

Rego rules control which agents can read which keys.

---

## 5. Action Queue & Governance

### Four Approval Tiers (CE)

| Tier | Behavior | Example |
|------|----------|---------|
| `ImmediateProtected` | Executes now, reversible via COW | File write |
| `Delayed` | Auto-executes after timeout | File delete (30s) |
| `Staged` | Requires human approval | Network request |
| `Blocked` | Always denied | System modification |

### 20 Built-in Action Types (CE)

HTTP (GET/POST/PUT/DELETE/PATCH/HEAD), filesystem (read/write/delete/mkdir/chmod/chown/exec), git (clone/commit/push/pull/checkout/branch), custom.

### Queue Management (CE)

```bash
envpod queue my-agent                        # view pending
envpod approve my-agent <id>                 # approve staged
envpod cancel my-agent <id>                  # deny
envpod undo my-agent <id>                    # undo executed
envpod actions my-agent list                 # view action catalog
envpod actions my-agent reload               # hot-reload catalog
```

### Queue Socket (CE)

Agent submits actions from inside the pod via Unix socket. Undo registry tracks every executed action for rollback.

### Privilege Escalation (Premium)

12 types: network, pod_access, vault_secret, file_write, gpu, tool, skill, mcp_server, capability, custom, and more. 3 scopes: one-time, session, permanent.

### OPA Policy Engine — 7 Decision Points (Premium)

```bash
envpod policy my-agent init
envpod policy my-agent edit
envpod policy my-agent check
```

| Decision Point | Controls |
|---------------|---------|
| Queue tier | Which actions need approval |
| Vault access | Which keys each agent can read |
| Commit auth | What can be committed |
| DNS override | Dynamic DNS per request |
| L7 network | HTTP method/path filtering |
| MCP tools | Per-agent tool call governance |
| Pod-to-pod | Inter-pod traffic authorization |

---

## 6. Monitoring & Audit

### Append-Only Audit Log (CE)

39 action types. Agent cannot access or modify.

```bash
envpod audit my-agent
envpod audit my-agent --json
envpod audit --security                      # config scan
envpod audit --security --config pod.yaml    # scan before deploy
```

### Monitoring Agent (CE)

```bash
envpod monitor my-agent start     # auto-freeze on violations
envpod monitor my-agent status
envpod monitor my-agent stop
```

### Budget Enforcement (CE + Premium)

```yaml
budget:
  max_duration: 8h
  warning: 30m
  grace_period: 30s        # SIGTERM → wait → SIGKILL
```

Premium adds multi-dimension:
```yaml
budget:
  max_requests: 1000       # Premium
  max_bandwidth: 1GB       # Premium
  max_storage: 5GB         # Premium
  action: freeze           # Premium: freeze instead of kill
```

```bash
envpod budget my-agent status     # Premium
envpod budget my-agent extend 2h  # Premium
envpod budget my-agent reset      # Premium
```

### Health Checks (CE + Premium)

```yaml
health:
  checks:
    - name: api
      http: http://localhost:8080/health
      interval: 30s
```

CE: single check, auto-restart. Premium: multiple checks, per-service recovery sequences, notifications (Slack/webhook/email), live add/remove, pause/resume.

### Governance Scorecard (Premium)

7 dimensions: network, filesystem, vault, tools, pod_comms, policy, queue. GPA/CWA grading. Auto-governance rules (auto-freeze on low score).

### OWASP ASI Attestation (Premium)

```bash
envpod audit my-agent --owasp    # 10/10 signed compliance report
```

### Adversarial Verification (Premium)

```bash
envpod verify my-agent    # 15 real attack tests, 4 categories
```

Boundary (7), network (3), process (2), privilege (3). Runs from host.

### OpenTelemetry Export (Premium)

```yaml
monitoring:
  otlp:
    endpoint: http://localhost:4318
    service_name: my-agent
```

Exports to Grafana, Datadog, New Relic, Splunk, Honeycomb. Logs + metrics + traces.

### Grafana + Loki Dashboards (Premium)

Pre-built dashboards: fleet overview, pod detail, security. Loki for log aggregation. `docker compose up` in `monitoring/`.

---

## 7. Prompt Screening

### Layer 1 — Regex (CE)

~1ms. 53 patterns across 4 categories.

```bash
envpod screen "ignore previous instructions"
envpod screen --file prompt.txt
envpod screen --api '{"messages":[...]}'
envpod screen --json "text"
```

Detects: injection (27), credentials (13), exfiltration (13), PII (3). Auto-updates rules via `envpod update`.

### Layer 2 — Local AI (Premium, Planned)

~200ms. Ollama classifier in governed screening pod.

### Layer 3 — Cloud AI (Premium, Planned)

~500ms. Claude/GPT classifier with separate API key.

Each layer runs only if the previous passed.

### Jailbreak Test (CE)

```bash
envpod run my-agent -- /opt/jailbreak-test.sh    # 8 security categories
```

Built-in boundary probe covering: path traversal, proc escape, network, syscalls, mount escape, privilege escalation, device access, information leaks.

---

## 8. Devices & Display

### GPU Passthrough (CE)

```yaml
devices:
  gpu: true
  extra: ["/dev/dri"]
```

NVIDIA and AMD. GPU info masked when disabled.

### Web Display — noVNC Desktop (CE)

```yaml
devices:
  display: true
  audio: true
web_display:
  enabled: true
  desktop_env: xfce       # xfce | openbox | sway
  resolution: 1920x1080
```

Features: full desktop in browser, audio streaming (PipeWire/PulseAudio via Opus/WebM), file upload, clipboard sync, resize.

### Display Protocol Support (CE)

Wayland, X11, auto-detect. Display forwarding via socket bind mounts.

### Pod Resize (CE)

```bash
envpod resize my-agent --gpu true --display true --audio true
envpod resize my-agent --desktop xfce --web-display true
```

---

## 9. Identity & Authentication

### Pod Identity (Premium)

Ed25519 keypair generated at `envpod init`.

```bash
envpod token my-agent              # show pod ID + auth token
envpod token my-agent --regenerate # new keypair
```

### Agent Registry (Premium)

```yaml
identity:
  agents:
    - name: coder
      capabilities: [code, test]
      vault_keys: [GITHUB_TOKEN]
```

```bash
envpod run my-agent --agent coder -- bash
envpod agent my-agent list
envpod agent my-agent register reviewer --capabilities read
envpod agent my-agent revoke reviewer
```

### OIDC / SSO (Premium)

```yaml
identity:
  oidc:
    issuer: https://accounts.google.com
    client_id: YOUR_CLIENT_ID
    allowed_groups: [engineering]
```

Supports Okta, Azure AD, Google, Keycloak, Auth0. Three identity layers: human → pod → agent.

---

## 10. Fleet Management

### Pod Lifecycle (CE)

```bash
envpod init my-agent -c pod.yaml
envpod run my-agent -- bash
envpod start my-agent -b          # background
envpod stop my-agent              # graceful SIGTERM
envpod restart my-agent
envpod restart --all
envpod kill my-agent              # SIGKILL + rollback
envpod lock my-agent              # freeze (cgroup freezer)
envpod unlock my-agent            # resume
envpod fg my-agent                # reattach detached pod
envpod destroy my-agent
envpod ls                         # fleet overview
envpod status my-agent            # resource usage
envpod logs my-agent              # stdout/stderr
envpod about                      # version, edition, license
```

### Service Registration (CE)

```bash
envpod service register my-agent    # auto-start on boot
envpod service list
envpod service restart my-agent
envpod service unregister my-agent
```

### envpod up — One Command (Premium)

```bash
cd /home/user/project
envpod up                          # reads envpod.yaml, init+run
envpod .                           # alias
envpod . --background
envpod . claude                    # run specific command
envpod . --reinit                  # destroy + re-create
```

### Infrastructure as Code (Premium)

```yaml
# fleet.yaml
namespace: production
pods:
  - name: api-server
    config: api.yaml
    depends_on: [database]
  - name: database
    config: db.yaml
  - name: worker
    config: worker.yaml
    replicas: 3
```

```bash
envpod apply fleet.yaml
envpod apply fleet.yaml --dry-run
envpod apply fleet.yaml --destroy
envpod apply fleet.yaml --namespace staging
```

### Parallel Clone & Scale (Premium)

```bash
envpod clone base experiment --parallel 10 --affinity spread -- python3 run.py
envpod batch base job --jobs 50 --cpus 2 --output results/ -- ./evaluate.sh
envpod scale worker --replicas 5
```

Affinity modes: spread, isolate, shared, none.

### IDE Integration (Premium)

```bash
envpod ide my-agent --editor vscode
envpod ide my-agent --editor cursor
envpod ssh-proxy my-agent
```

SSH ProxyCommand — VS Code, Cursor, JetBrains Gateway.

---

## 11. Remote Management

### Local Remote Control (CE)

```bash
envpod remote my-agent freeze
envpod remote my-agent resume
envpod remote my-agent kill
envpod remote my-agent status
```

Via Unix socket on host.

### Remote HTTP API (Premium)

```yaml
remote:
  enabled: true
  port: 9800
```

9 endpoints: status, freeze, resume, kill, diff, audit, budget, identity, run. Token auth on every request.

### WebSocket Relay (Premium)

Pod connects outbound to `relay.envpod.dev`. NAT-friendly.

```python
from envpod import RemotePod
pod = RemotePod("my-agent", pod_id="...", token="...", relay="relay.envpod.dev")
pod.status()
pod.run(["python3", "train.py"])
pod.freeze()
```

### Node Daemon (Premium)

Turn any machine into managed infrastructure.

```bash
envpod node install              # generate keypair, register systemd
envpod node status               # show node info
envpod node token                # show node auth token
envpod node run my-agent         # start pod on remote node
envpod node uninstall
```

Connects to relay, accepts: ls, init, start, stop, destroy.

### SDKs (CE)

```python
from envpod import Pod
with Pod("my-agent", config="pod.yaml") as pod:
    pod.run(["python3", "train.py"])
    pod.vault_set("API_KEY", "sk-...")
    pod.diff()
    pod.commit()
```

```typescript
import { Pod } from 'envpod';
const pod = await Pod.create("my-agent", { config: "pod.yaml" });
await pod.run(["node", "server.js"]);
await pod.destroy();
```

44 methods: lifecycle, vault, DNS, snapshots, resources, display, screening, remote, clone, disposable.

---

## 12. Service Proxy

Expose pod services at `*.envpod.cloud`. See [SERVICE-PROXY.md](SERVICE-PROXY.md) for full docs.

### Configuration (Premium)

```yaml
remote:
  enabled: true
  expose_service:
    port: 8080
    subdomain: my-api       # → https://my-api.envpod.cloud
    public: false           # true = no auth required
```

### Two-Token Security

- **pod_token** — authenticates forwarded requests to the pod. Never exposed externally.
- **service_token** — authenticates external callers. Generated by proxy, shared by pod owner.

### Token Rotation

```bash
envpod expose my-api --rotate-token    # old token instantly invalid
```

### Subdomain Locking

Locked to pod_id on first registration. Same pod can re-register (reconnect). Different pod gets 409 with suggested alternative.

### Routing

Relay tunnel (default, ~200ms) or direct Tailscale IP (~5ms) if enabled.

---

## 13. Web Dashboard

`envpod dashboard` → `http://localhost:9090`

### Fleet View (CE)

Pod list with status, IP, freeze/resume buttons. Refresh on demand (no polling).

### Pod Detail (CE)

Tabs: Overview, Audit, Diff (inline viewer), Resources, Snapshots, Queue.

### Dashboard Daemon (CE)

```bash
envpod dashboard --daemon          # background mode
envpod dashboard --stop            # stop daemon
```

### Premium Dashboard Extras

Create, destroy, clone pods from browser. Agent identity table. Pod public key display.

---

## 14. Developer Experience

### Interactive Init (CE)

```bash
envpod init my-agent              # interactive wizard
envpod init my-agent --preset claude-code
envpod presets                    # list 18+ presets
```

### Setup Scripts (CE)

```yaml
setup:
  - apt-get install -y python3 python3-pip
  - pip install flask
setup_script: setup.sh            # or external script
start_command: python3 app.py
```

### Auto Pre-Setup (CE)

Handles before user setup runs: PEP 668 fix, stale apt lists, 3rd-party source fixes, nvm/Node.js auto-install when npm/npx used.

### Toolchain Declaration (Premium)

```yaml
toolchain: [python3, node22, rust, go]
```

18 entries: Python3, Node (18/20/22), Rust, Go, Java (17/21), Ruby, Bun, Deno, uv, build-essential, git, jq, ripgrep, sqlite, curl, wget, docker-cli.

### Devcontainer / Dockerfile Compat (Premium)

```bash
envpod up --config .devcontainer/devcontainer.json
envpod up --config Dockerfile
```

### Provider Support (Premium)

```bash
envpod up --provider ssh://user@host
```

Deploy pods on remote machines via SSH.

### 68+ Example Configs (CE)

Coding agents, web servers, browsers, desktops, ML, databases, messaging, security testing.

### envpod group — No Sudo (CE)

```bash
# Installer creates envpod group with setuid binary
# After logout/login:
envpod init my-agent    # no sudo needed
```

### Rootless Mode (CE)

No root required. Uses pasta for networking.

### Shell Completions (CE)

```bash
envpod completions bash > /etc/bash_completion.d/envpod
envpod completions zsh > ~/.zfunc/_envpod
envpod completions fish > ~/.config/fish/completions/envpod.fish
```

### Update Checker (CE)

```bash
envpod update    # check version + download latest screening rules
```

### Tilde Expansion (CE)

Mount paths support `~` expansion: `path: ~/project` → `/home/user/project`.

### Framework Integrations (Premium)

Documented governance for 15+ AI frameworks: LangChain, CrewAI, AutoGen, OpenAI Agents SDK, Claude Code, Google ADK, Semantic Kernel, LlamaIndex, Dify, Browser-use, Cursor, Codex, Ollama, SWE-agent.

---

## 15. Licensing & Distribution

### CE License (CE)

BSL 1.1. Converts to AGPL-3.0 on 2030-03-07. Free forever for any use.

### Premium License (Premium)

```bash
envpod license activate <KEY>
envpod license status
envpod license deactivate
```

Ed25519-signed JWT. 24h heartbeat (phone-home to `activate.envpod.dev`). 7-day offline grace. Key file survives binary upgrades.

When license expires, Premium commands print "requires Premium license." All 47 CE commands continue working. 27 Premium gates total.

### Binary

| | CE | Premium |
|---|---|---|
| Size | 9 MB | 22 MB |
| Static | Yes (musl) | Yes (musl) |
| Architectures | x86_64 + aarch64 | x86_64 + aarch64 |
| Dependencies | None | None |
| Strip + LTO | Yes | Yes |

---

## 16. Compliance & Standards

### OWASP Agentic Security (CE + Premium)

10/10 risks covered at kernel level:

| Risk | CE | Premium adds |
|------|----|----|
| ASI-01 Goal Hijacking | Prompt screening, action queue | OPA policy |
| ASI-02 Excessive Capabilities | Action catalog, tiers | OPA, privilege escalation |
| ASI-03 Identity Abuse | Namespace isolation | Ed25519/JWT, OIDC/SSO |
| ASI-04 Code Execution | seccomp, PID ns | OPA tools, sealed mode |
| ASI-05 Output Handling | COW filesystem | OPA commit policy |
| ASI-06 Memory Poisoning | Memory ns, /proc masking | Sealed mode |
| ASI-07 Inter-Agent Comms | Discovery, DNS | L7 OPA, identity |
| ASI-08 Cascading Failures | cgroups, budget | Scorecard auto-governance |
| ASI-09 Trust Deficit | Audit, approval gates | Scorecard, OTLP, Grafana |
| ASI-10 Rogue Agents | Kill, freeze, monitoring | Escalation, auto-freeze |

### NIST AI Risk Management Framework (Premium)

Full mapping of all NIST subcategories. 4 functions: GOVERN, MAP, MEASURE, MANAGE.

### EU AI Act (Premium)

Risk categorization, transparency, human oversight, documentation mapped.

---

## 17. Platform Support

| Platform | Status |
|----------|--------|
| Ubuntu 22.04+ | Full |
| Debian 12+ | Full |
| Fedora 39+ | Full |
| Rocky/Alma 9+ | Full |
| Arch Linux | Full |
| openSUSE Tumbleweed | Full |
| WSL2 (Windows) | Full (GPU with NVIDIA) |
| macOS (OrbStack) | Beta |
| Raspberry Pi 4/5 | Full (ARM64) |
| Jetson Orin | Full (GPU + DLA) |
| Nested in Docker | Works (`--privileged`) |

**Requirements:** Linux kernel 5.11+, cgroups v2, OverlayFS, 9 MB disk.

---

## 18. Performance

| Metric | envpod | Docker |
|--------|--------|--------|
| Init | 1.3s | 1.5s |
| Clone | 8ms | 800ms |
| Warm run | 23ms | 150ms |
| 50-pod fleet | 9.5s | 75s |
| DNS resolution | 2.6x faster | baseline |
| Binary | 9 MB (single) | 199 MB (4 binaries) |
| Memory/pod | ~4 MB | ~12 MB |
| Disk/clone | ~1 MB (COW) | ~240 MB (layer copy) |

---

## Pricing

| Tier | Price | Commands | Key features |
|------|-------|----------|-------------|
| **CE** | $0 forever | 47 | Kernel isolation, COW, vault, audit, dashboard, SDK |
| **Premium** | $399/seat/mo | 60 | + OPA, identity, fleet, remote, OTLP, service proxy |
| **Enterprise** | Custom | 60 | + SLA, dedicated support, compliance signing |

[envpod.dev](https://envpod.dev) · [GitHub](https://github.com/markamo/envpod-ce) · mark@envpod.dev
