# envpod for Docker Users

> "Docker isolates. Envpod governs."

If you know Docker, you can understand envpod immediately — it uses the same Linux primitives. The mental model is familiar: isolated environments, filesystem layers, network namespaces, resource limits. But envpod was built for a fundamentally different purpose: **governing what an AI agent does, not just isolating where it runs.**

This guide is for developers who already know Docker and want to understand envpod quickly.

---

## The Core Difference

Docker answers: **"Where does this process run?"** It draws a box around the process and controls what it can see (filesystem, network, PIDs).

envpod answers: **"What is this process allowed to do?"** It draws the same box, then adds a governance ceiling: every significant action is intercepted, queued, validated, audited, and optionally requires human approval before execution.

![Docker vs envpod — isolation only vs isolation + governance](images/fig-08-docker-vs-envpod.svg)

The isolation layer is roughly equivalent. The governance ceiling is what's new.

---

## Command Mapping

| Docker | envpod | Notes |
|---|---|---|
| `docker build` | `envpod init --setup` | Setup commands run inside the pod instead of a Dockerfile |
| `docker run` | `envpod run` | Starts the pod and runs a command |
| `docker ps` | `envpod ls` | List running/created pods |
| `docker diff` | `envpod diff` | Show filesystem changes vs baseline |
| `docker commit` | `envpod commit` | Persist changes — but with human review first |
| `docker logs` | `envpod audit` | Structured audit log, not raw stdout |
| `docker stop` | `envpod lock` | Freeze pod (pause all processes) |
| `docker kill` | `envpod destroy` | Destroy pod and clean up |
| `docker exec` | `envpod run` | Re-run a command in the same pod namespace |
| `docker cp` | Read from `{pod_dir}/upper/` | Changes live in overlay upper directory |
| `docker inspect` | `envpod ls --json`, `pod.yaml` | Pod config + runtime state |
| `docker secret` | `envpod vault set` | Encrypted vault |
| `docker-compose up` | `envpod run` (multiple pods) | Pod-to-pod discovery via DNS |
| `docker system prune` | `envpod gc` | Clean up stale resources |

---

## Feature Mapping

### Isolation Primitives

Both envpod and Docker use Linux kernel primitives — they're the same box, built differently.

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| PID namespace | ✓ | ✓ | Process isolation |
| Mount namespace | ✓ | ✓ | Filesystem isolation |
| Network namespace | ✓ | ✓ | Network isolation |
| UTS namespace | ✓ | ✓ | Hostname isolation |
| User namespace | ✓ | ✓ | UID/GID remapping |
| IPC namespace | ✓ | — | Not yet implemented |
| OverlayFS / Union FS | ✓ | ✓ | Copy-on-write filesystem |
| Static binary (no daemon) | — | ✓ | envpod has no central daemon; Docker requires dockerd |
| Rootless mode | ✓ | Planned | Docker supports rootless; envpod currently requires root for namespace setup |

---

### Filesystem

This is where envpod and Docker diverge significantly.

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Union filesystem (layers) | ✓ | ✓ | Both use OverlayFS |
| Read-only bind mounts | ✓ | ✓ | System dirs read-only by default |
| Writable volumes | ✓ | ✓ via overlay | envpod writes go to overlay upper, not host directly |
| `docker diff` (changed files) | ✓ | ✓ | Same concept |
| `docker commit` (persist) | ✓ | ✓ | But Docker auto-commits; envpod requires human review |
| **Human review before commit** | — | ✓ | See every change before it reaches the host FS |
| **Selective commit (per-path)** | — | ✓ | `envpod commit --paths src/` |
| **Rollback** | — | ✓ | `envpod rollback` undoes all agent changes |
| **Named snapshots** | — | ✓ | Save/restore filesystem state mid-run |
| **Auto-snapshot before run** | — | ✓ | Always safe to rollback to pre-run state |
| System directory COW | — | ✓ | `system_access: advanced` gives agent per-dir COW overlays for `/usr`, `/bin`, etc. |

**Key difference:** In Docker, when a container writes a file and you commit it, the change is permanent. In envpod, the agent's writes are staged in the overlay — the host reviews them with `envpod diff` and explicitly approves with `envpod commit` (or rejects with `envpod rollback`).

---

### Networking

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Network namespace | ✓ | ✓ | Complete network isolation |
| Bridge network | ✓ | veth pairs | Different implementation, same effect |
| Host networking mode | ✓ | ✓ via `network: {mode: Unsafe}` | |
| Port forwarding (localhost) | `-p` / `--publish` | `ports: ["8080:3000"]` | |
| Port forwarding (all interfaces) | `-P` / `--publish-all` | `public_ports: ["8080:3000"]` | envpod flags this as a security finding |
| **Pod-to-pod networking** | Docker networks | `internal_ports` + `allow_pods` | envpod uses DNS-based discovery, not bridge networks |
| DNS resolution | Container DNS | ✓ per-pod resolver | envpod embeds a full DNS server per pod |
| **DNS allow/deny lists** | — | ✓ | `network.allow: [api.openai.com]` |
| **DNS remap / CNAME override** | — | ✓ | Remap domains to different IPs |
| **Anti-DNS-tunneling** | — | ✓ | Rejects excessively long labels, random-looking subdomains |
| **Bandwidth rate limits** | ✓ (via tc) | ✓ | `network.bandwidth_limit_mbps` |
| **Live DNS mutation** | — | ✓ | Add/remove allow rules while pod is running, no restart |
| **Pod-to-pod discovery** | Docker Compose service names | `*.pods.local` DNS | Bilateral policy enforcement via central dns-daemon |
| Custom DNS servers | `--dns` flag | `network.dns_servers` | |
| Disable networking | `--network none` | `network.mode: Isolated` | |

---

### Resource Limits

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| CPU limits | `--cpus`, `--cpu-shares` | `processor.cpu_cores` | Both use cgroups v2 |
| Memory limits | `--memory` | `processor.memory_limit_mb` | |
| IO limits | `--blkio-weight` | `processor.io_weight` | |
| PID limit | `--pids-limit` | `processor.max_pids` | |
| CPU affinity | — | `processor.cpu_affinity` | Pin to specific CPU cores |
| GPU access | `--gpus` | `devices.gpu: true` | envpod auto-mounts all GPU devices |

---

### Security Hardening

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| seccomp-BPF profiles | ✓ default profile | ✓ | envpod applies seccomp on run |
| AppArmor / SELinux | ✓ | Planned | |
| Drop capabilities (`--cap-drop`) | ✓ | Planned | |
| Read-only root filesystem | `--read-only` | ✓ COW overlay | envpod overlay is inherently COW |
| Non-root user | `USER` in Dockerfile | `agent` user (UID 60000) | envpod default is non-root |
| No-new-privileges | `--security-opt no-new-privileges` | ✓ default | |
| **Static security audit** | Docker Scout (separate tool) | `envpod audit --security` | Built-in, runs on pod.yaml without starting the pod |

---

### Configuration

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Configuration format | Dockerfile + `docker run` flags | `pod.yaml` | Single declarative file for everything |
| Environment variables | `-e KEY=VALUE` | `env:` block in pod.yaml | |
| Secrets (env vars) | `--env-file`, Docker secrets | `envpod vault set` | Vault is encrypted at rest; Docker env secrets are plaintext |
| Volume mounts | `-v host:container` | `filesystem.bind_mounts` | |
| Entrypoint | `ENTRYPOINT` in Dockerfile | `entrypoint:` in pod.yaml | |
| Working directory | `WORKDIR` | `filesystem.workspace` | |
| **Hot-reload config** | No (container restart required) | ✓ (DNS, action catalog, discovery) | Change policy while pod is running |

---

### Pod / Image Management

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Base image (starting rootfs) | Docker Hub images | `envpod init` (host rootfs) | envpod currently uses the host filesystem as the lower layer |
| Layered builds | Multi-stage Dockerfile | Setup commands in pod.yaml | Same idea: run commands, snapshot the result |
| **Base pods** | Docker base images | `envpod base create` | Snapshot after setup for fast reuse |
| **Fast clone** | `docker pull` (registry) | `envpod clone` (~130ms) | Clones from base snapshot, symlinks rootfs |
| Clone from current state | `docker commit` + `docker run` | `envpod clone --current` | |
| Garbage collection | `docker system prune` | `envpod gc` | |
| **Named snapshots** | — | `envpod snapshot create` | Save/restore mid-execution state |

---

### Secrets & Credentials

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Environment variable secrets | ✓ (plaintext in process env) | ✓ | Available in both |
| Docker Swarm secrets (file in container) | ✓ | — | Docker-specific |
| **Encrypted vault** | — | ✓ | ChaCha20-Poly1305 encrypted at rest |
| **Vault live mutation** | — | ✓ | Add/change secrets while pod is running |

In Docker, secrets injected as environment variables can be read by any process in the container: `cat /proc/1/environ`. In envpod, the vault is encrypted at rest and injected only at runtime — the agent gets the value as an env var but it never appears in config files, command lines, or logs.

---

### Monitoring & Logging

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| stdout/stderr logs | `docker logs` | Stdout is visible as usual | |
| Structured audit log | — | ✓ | `audit.jsonl` with every action, timestamp, and outcome |
| Live resource stats | `docker stats` | Dashboard resources tab | |
| **Action-level audit** | — | ✓ | Every queue call, approval, and execution recorded |
| **Security audit (static)** | Docker Scout (separate, paid) | `envpod audit --security` | Free, built-in, runs on pod.yaml |
| **Web dashboard** | Portainer (3rd party) | `envpod dashboard` | Built-in, shows fleet + diff + audit + resources |

---

### Fleet Management

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Multiple containers | ✓ | ✓ (multiple pods) | |
| Service discovery | Docker Compose service names | `*.pods.local` DNS | Bilateral policy enforcement |
| Docker Compose (declarative multi-service) | ✓ | `envpod run` (per pod) | No Compose equivalent yet |
| Swarm / orchestration | Docker Swarm | Planned | |
| Kubernetes compatible | ✓ (via CRI) | — | envpod is not a CRI runtime |

---

### Platform Support

| Feature | Docker | envpod | Notes |
|---|---|---|---|
| Linux | ✓ | ✓ | Both native |
| macOS | ✓ (via VM) | — | envpod is Linux-only (uses Linux namespaces directly) |
| Windows | ✓ (WSL2 / Hyper-V) | — | Linux-only |
| ARM64 (Raspberry Pi, Jetson) | ✓ | ✓ | envpod ships static ARM64 binary |
| x86_64 | ✓ | ✓ | |

---

## What envpod Adds: The Governance Ceiling

These features have no Docker equivalent. They exist specifically to govern AI agent behavior.

### 1. Copy-on-Write Filesystem with Human Review

Docker lets containers write files and `docker commit` makes them permanent immediately. In envpod:

- Agent writes go to the overlay — the host filesystem is unchanged
- `envpod diff` shows exactly what the agent wrote
- `envpod commit` applies changes after human review
- `envpod rollback` discards all agent changes

The agent runs in a COW sandbox. Nothing it does is permanent until a human approves it.

### 2. Action Queue with Approval Tiers

The agent declares its intent — `git_push`, `http_post`, `file_write` — and envpod queues the action. The human sees it, can inspect the params, and either approves or cancels.

```
Tiers:
  immediate  → executes now (COW-protected for filesystem ops)
  delayed    → executes after 30s grace period unless cancelled
  staged     → waits for: envpod approve <id>
  blocked    → permanently rejected, cannot be approved
```

Docker has no equivalent. A container can make HTTP requests, push to git, or delete files without any checkpoint.

### 3. Action Catalog (MCP-style Tool Discovery)

The host defines a menu of allowed actions in `actions.json`. The agent queries this menu at runtime and can only call defined actions. envpod validates every call against the schema, executes it, and audits the result.

20 built-in action types: HTTP (6), Filesystem (7), Git (6), Custom (1).

```bash
envpod actions my-agent ls           # agent discovers available tools
envpod actions my-agent add          # host adds a new action
envpod actions my-agent set-tier git_push staged   # set approval tier
```

### 4. Encrypted Credential Vault

Docker injects secrets as environment variables — readable by any process, shell, or debug tool in the container. envpod provides encrypted vault storage (ChaCha20-Poly1305) — secrets never appear in config files, command lines, or logs.

```bash
echo "sk-..." | envpod vault my-agent set ANTHROPIC_API_KEY
envpod vault my-agent list      # shows key names, never values
envpod vault my-agent import .env
```

### 5. Per-Pod DNS Resolver with Policy

Every pod gets its own embedded DNS server. The host controls what the agent can resolve:

```yaml
network:
  mode: Filtered
  allow:
    - api.anthropic.com
    - api.github.com
  deny:
    - "*.s3.amazonaws.com"   # block S3 exfiltration
```

Changes take effect while the pod is running — no restart needed. Docker has no per-container DNS policy engine.

### 6. Structured Audit Log

Every significant event is recorded in `{pod_dir}/audit.jsonl`:

```json
{"ts":"2026-03-03T14:22:01Z","action":"file_write","status":"executed","path":"/workspace/output.json"}
{"ts":"2026-03-03T14:22:05Z","action":"git_push","status":"queued","tier":"staged","remote":"origin"}
{"ts":"2026-03-03T14:25:11Z","action":"git_push","status":"executed","approved_by":"host"}
```

Docker logs stdout/stderr. envpod logs what the agent *did*.

### 7. Static Security Audit

```bash
sudo envpod audit --security -c pod.yaml
```

Runs without starting the pod. Checks for common misconfigurations:

- N-03: DNS bypass (Unsafe network mode)
- N-04: Public ports exposed to all interfaces
- I-04: X11 display forwarding (host X11 socket accessible)
- C-01/C-02/C-03: Missing resource limits

### 8. Remote Control and Live Mutation

While a pod is running:

```bash
sudo envpod lock my-agent            # freeze all processes immediately
sudo envpod remote my-agent resume   # unfreeze
sudo envpod discover my-agent --add-pod service  # enable pod-to-pod discovery
```

No restart required. Docker cannot mutate a running container's network policy, secrets, or process state without a full restart.

---

## What Docker Has That envpod Doesn't (Yet)

envpod is purpose-built for AI agent governance on a single Linux machine. It is not trying to replace Docker for all use cases.

| Feature | Status in envpod |
|---|---|
| Pre-built image library (Docker Hub) | Not applicable — envpod uses host rootfs |
| macOS and Windows support | Planned when a VM backend ships |
| Kubernetes CRI compatibility | Not planned — envpod is not a container runtime |
| Docker Compose (multi-service declarative) | Planned |
| Docker Swarm orchestration | Not planned |
| Rootless mode | Planned |
| Production battle-hardening at massive scale | Docker has years of production use; envpod is new |

---

## When to Use envpod

### Use Docker when:

- You are running **any workload** that does not involve an AI agent making autonomous decisions
- You need macOS or Windows support
- You need to pull from Docker Hub and use existing images
- You are deploying microservices, APIs, or databases
- You need Kubernetes integration

### Use envpod when:

- You are running **AI agents** that make decisions and take actions
- You need to review filesystem changes before they reach the host
- You need human approval before the agent makes API calls, pushes to git, or writes files
- You need an audit trail of what the agent did, not just what it printed
- You need DNS-level network policy (block all domains except a whitelist)
- You need to run agents on ARM64 embedded hardware (Raspberry Pi, Jetson)

---

## Migration Cheatsheet

### Dockerfile → pod.yaml

```dockerfile
# Dockerfile
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y python3 pip
RUN pip install anthropic requests
WORKDIR /workspace
ENV PYTHONPATH=/workspace
CMD ["python3", "agent.py"]
```

```yaml
# pod.yaml
name: my-agent
filesystem:
  workspace: /workspace
network:
  mode: Filtered
  allow:
    - api.anthropic.com
    - pypi.org
setup:
  - apt-get update -qq
  - apt-get install -y python3 python3-pip
  - pip3 install anthropic requests
```

```bash
sudo envpod init my-agent -c pod.yaml
sudo envpod run my-agent -- python3 agent.py
```

### docker-compose.yml → multiple pods

```yaml
# docker-compose.yml
services:
  api:
    image: python:3.12
    ports: ["8080:8080"]
  worker:
    image: python:3.12
    environment:
      - API_URL=http://api:8080
```

```yaml
# api/pod.yaml
name: api
ports: ["8080:8080"]
network:
  allow_discovery: true

# worker/pod.yaml
name: worker
network:
  allow_pods: ["api"]
  allow:
    - api.pods.local
```

```bash
sudo envpod dns-daemon &          # start discovery daemon
sudo envpod run api -- python3 api.py
sudo envpod run worker -- python3 worker.py
```

### docker run flags → pod.yaml

| `docker run` flag | pod.yaml equivalent |
|---|---|
| `-e KEY=VALUE` | `env: {KEY: VALUE}` |
| `-p 8080:3000` | `ports: ["8080:3000"]` |
| `-P 8080:3000` | `public_ports: ["8080:3000"]` |
| `--memory 512m` | `processor: {memory_limit_mb: 512}` |
| `--cpus 2` | `processor: {cpu_cores: 2}` |
| `--read-only` | COW overlay is inherently copy-on-write |
| `--network none` | `network: {mode: Isolated}` |
| `--security-opt seccomp=profile.json` | `security: {seccomp_profile: "..."}` |
| `--user 1000:1000` | `security: {user: "agent"}` |
| `--gpus all` | `devices: {gpu: true}` |
| `--device /dev/snd` | `devices: {audio: true}` |
| `-v /host/path:/container/path` | `filesystem: {bind_mounts: [{host: "/host/path", container: "/container/path"}]}` |

---

*Copyright 2026 Xtellix Inc. All rights reserved. Licensed under the Business Source License 1.1.*
