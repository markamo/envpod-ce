# envpod CE vs Premium

envpod CE is free forever. It gives you kernel-level isolation and governance that no other free tool provides — including 10/10 OWASP Agentic Security coverage.

Premium adds identity, policy intelligence, fleet orchestration, and advanced monitoring for production agent systems.

## Quick Comparison

| | CE (free) | Premium ($399/seat/mo) |
|---|---|---|
| **Isolation** | Full kernel-level (namespaces, cgroups, seccomp) | Same |
| **Policy engine** | Action tiers (immediate/delayed/staged/blocked) | + OPA/Rego with 7 decision points |
| **Identity** | Pod namespace isolation | + Ed25519/JWT per pod/agent + OIDC/SSO |
| **Health checks** | Single check, auto-restart | + Multi-check, per-service recovery, notifications |
| **Monitoring** | Audit trail + security scan | + Governance scorecard, OTLP export, Grafana |
| **Networking** | DNS filtering, bilateral discovery | + L7 OPA, Tailscale, DoH blocking, sealed mode |
| **Vault** | Encrypted secrets, env injection | + Vault proxy (agent never sees keys) |
| **Fleet** | Manual pod management | + IaC, parallel clone, scale, batch executor |
| **Security testing** | jailbreak-test.sh (8 categories) | + `envpod verify` (automated, 15+ tests) |
| **Compliance** | OWASP 10/10 at kernel level | + OWASP attestation, NIST AI RMF mapping |

## Feature-by-Feature

### Isolation & Security

| Feature | CE | Premium |
|---|---|---|
| PID namespace | Yes | Yes |
| Network namespace | Yes | Yes |
| Mount namespace (COW) | Yes | Yes |
| cgroups v2 (CPU/memory/PID limits) | Yes | Yes |
| seccomp-BPF syscall filtering | Yes | Yes |
| NO_NEW_PRIVS | Yes | Yes |
| /proc masking | Yes | Yes |
| Minimal /dev (default-deny) | Yes | Yes |
| GPU info masking | Yes | Yes |
| Sealed mode (zero host visibility) | — | Yes |
| DoH blocking | — | Yes |

### Filesystem

| Feature | CE | Premium |
|---|---|---|
| OverlayFS copy-on-write | Yes | Yes |
| diff / commit / rollback | Yes | Yes |
| Named snapshots | Yes | Yes |
| Base pods + fast cloning (8ms) | Yes | Yes |
| Garbage collection | Yes | Yes |
| OPA commit policy | — | Yes |

### Network

| Feature | CE | Premium |
|---|---|---|
| Per-pod DNS resolver | Yes | Yes |
| Allowlist / denylist / monitor modes | Yes | Yes |
| Anti-DNS tunneling | Yes | Yes |
| Live DNS mutation | Yes | Yes |
| Pod-to-pod discovery (bilateral) | Yes | Yes |
| Port forwarding (localhost/public/internal) | Yes | Yes |
| L7 HTTP policy (OPA) | — | Yes |
| L7 pod-to-pod governance | — | Yes |
| Tailscale per-pod identity | — | Yes |
| DoH blocking | — | Yes |
| Services config | — | Yes |

### Credential Vault

| Feature | CE | Premium |
|---|---|---|
| Encrypted at rest (ChaCha20-Poly1305) | Yes | Yes |
| Env var injection at runtime | Yes | Yes |
| Bulk import (.env files) | Yes | Yes |
| Vault proxy (HTTPS MITM, agent never sees keys) | — | Yes |
| Per-agent vault scoping | — | Yes |

### Action Queue

| Feature | CE | Premium |
|---|---|---|
| Four approval tiers | Yes | Yes |
| 20 built-in action types | Yes | Yes |
| Queue socket (agent submits from inside pod) | Yes | Yes |
| Undo registry | Yes | Yes |
| Hot-reload catalog | Yes | Yes |
| Privilege escalation requests | — | Yes |
| OPA queue policy | — | Yes |

### Health Checks

| Feature | CE | Premium |
|---|---|---|
| Single health check (HTTP or command) | Yes | Yes |
| Auto-restart on failure | Yes | Yes |
| Graceful shutdown (SIGTERM → grace → SIGKILL) | Yes | Yes |
| Audit trail (health events) | Yes | Yes |
| Multiple checks per pod | — | Yes |
| Per-service recovery (not whole pod) | — | Yes |
| Recovery action sequences (run/wait/check/notify) | — | Yes |
| Live add/remove checks at runtime | — | Yes |
| Agent self-registers health checks via socket | — | Yes |
| Pause/resume (maintenance mode) | — | Yes |
| Notifications (Slack, webhook, email) | — | Yes |

### Identity & Policy

| Feature | CE | Premium |
|---|---|---|
| Pod namespace isolation | Yes | Yes |
| OPA/Rego policy engine (7 decision points) | — | Yes |
| Pod identity (Ed25519 keypair) | — | Yes |
| Agent identity (JWT per agent) | — | Yes |
| OIDC/SSO (Okta, Azure AD, Google, Keycloak) | — | Yes |
| Three identity layers (human/pod/agent) | — | Yes |
| Privilege escalation with scoped grants | — | Yes |
| MCP tool call governance | — | Yes |

### Monitoring & Compliance

| Feature | CE | Premium |
|---|---|---|
| Append-only audit log | Yes | Yes |
| Static security scan | Yes | Yes |
| Prompt screening | Yes | Yes |
| Monitoring agent (auto-freeze) | Yes | Yes |
| Budget enforcement (auto-kill) | Yes | Yes |
| Governance scorecard (7 dimensions, CWA grading) | — | Yes |
| Automated security testing (`envpod verify`) | — | Yes |
| OWASP ASI attestation (`envpod audit --owasp`) | — | Yes |
| OpenTelemetry export (Grafana/Datadog/Splunk) | — | Yes |
| Grafana dashboards | — | Yes |
| Audit attribution (per-agent) | — | Yes |

### Fleet Management

| Feature | CE | Premium |
|---|---|---|
| Pod init / run / destroy | Yes | Yes |
| Clone (8ms) | Yes | Yes |
| Start / stop / restart | Yes | Yes |
| Systemd service registration | — | Yes |
| Infrastructure as Code (`envpod apply`) | — | Yes |
| Namespace isolation | — | Yes |
| Parallel clone with CPU affinity | — | Yes |
| Horizontal scaling (`envpod scale`) | — | Yes |
| Wave-based batch executor | — | Yes |
| Dashboard create/destroy/clone | — | Yes |

### Developer Experience

| Feature | CE | Premium |
|---|---|---|
| 24+ CLI subcommands | Yes | Yes |
| Interactive init wizard | Yes | Yes |
| 18+ built-in presets | Yes | Yes |
| 55+ example configs | Yes | Yes |
| Web dashboard (fleet overview, diff viewer) | Yes | Yes |
| Python SDK | Yes | Yes |
| Node.js SDK | Yes | Yes |
| GPU / display / audio passthrough | Yes | Yes |
| Web display (noVNC) | Yes | Yes |

## OWASP Agentic Security

Both CE and Premium cover all 10 OWASP ASI risks at the kernel level:

| OWASP ASI Risk | CE | Premium adds |
|---|---|---|
| ASI-01 Goal Hijacking | Prompt screening, action queue | + OPA policy, policy change governance |
| ASI-02 Excessive Capabilities | Action catalog, tier system | + OPA capability checks, privilege escalation |
| ASI-03 Identity Abuse | Namespace isolation | + Ed25519/JWT, OIDC/SSO |
| ASI-04 Code Execution | seccomp-BPF, PID ns, NO_NEW_PRIVS | + OPA tool governance, sealed mode |
| ASI-05 Output Handling | COW filesystem, diff/commit | + OPA commit policy |
| ASI-06 Memory Poisoning | Memory ns, /proc masking | + Sealed mode |
| ASI-07 Inter-Agent Comms | Bilateral discovery, DNS filtering | + L7 OPA, identity verification |
| ASI-08 Cascading Failures | cgroups limits, budget enforcement | + Scorecard auto-governance |
| ASI-09 Trust Deficit | Audit trail, human approval gates | + Scorecard, OTLP, Grafana |
| ASI-10 Rogue Agents | Kill switch, freeze, monitoring | + Privilege escalation, scorecard auto-freeze |

CE gives you 10/10 coverage — stronger than any application-level governance toolkit. Premium adds depth with per-agent identity, policy intelligence, and fleet-scale monitoring.

See [OWASP-AGENTIC.md](OWASP-AGENTIC.md) for the full mapping.

## When to Upgrade

**Stay on CE if you:**
- Run 1-5 pods on your own machine
- Need basic isolation and governance
- Are a solo developer or small team
- Don't need per-agent identity or OPA policies

**Upgrade to Premium if you:**
- Run multiple services per pod (need per-service health recovery)
- Need OPA/Rego policy engine for fine-grained control
- Need agent identity and OIDC/SSO for your team
- Need fleet management (IaC, scale, batch)
- Need compliance reporting (OWASP attestation, NIST mapping)
- Need observability (scorecard, OTLP export, Grafana)
- Need notifications on health failures
- Run agents that request dynamic privileges

## Pricing

| Tier | Price | What you get |
|---|---|---|
| CE | **$0 forever** | 67 features, OWASP 10/10, kernel-level isolation |
| Premium | **$399/seat/mo** | 110+ features, OPA, identity, scorecard, fleet, OTLP |
| Enterprise | **Custom** | + SLA, dedicated support, compliance signing |

```bash
# Install CE (free)
curl -fsSL https://envpod.dev/install.sh | sh

# Upgrade to Premium
curl -fsSL https://premium.envpod.dev/install.sh | sh
envpod license activate <YOUR_KEY>
```

[Get Premium →](https://envpod.dev/#pricing)
