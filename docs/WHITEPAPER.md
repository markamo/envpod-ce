# envpod: Zero-Trust Governance for Autonomous AI Agents

**Mark Amo-Boateng, PhD — Xtellix Inc. — March 2026**

---

## The Problem

AI agents are autonomous systems with real-world side effects. They write files, call APIs, push code, and send messages — often without human review. Every major agent framework (Claude Code, Codex, Aider, SWE-agent, LangGraph) runs with full access to the host environment. A compromised or misconfigured agent can exfiltrate credentials, corrupt filesystems, or take irreversible actions before anyone notices.

Containers solve **isolation** — drawing a box around a process. But they do not solve **governance** — controlling what happens inside the box. Docker, Podman, and cloud sandboxes prevent escape but provide no mechanism for reviewing file changes, approving actions, protecting secrets at the transport layer, or auditing every decision. The agent writes directly to the filesystem, and those writes are permanent.

## The Solution

envpod is a governance runtime for AI agents. Every agent runs inside a **pod** — an environment with a foundation, four isolation walls, and a governance ceiling.

**Foundation — Copy-on-Write Filesystem:**
OverlayFS captures every write in a private overlay. The host is untouched until a human runs `envpod commit`. `envpod diff` shows exactly what changed. `envpod rollback` discards everything. Named snapshots allow restoring any checkpoint. The foundation is what makes everything else reversible — the pod sits on top of it.

**Four Walls:**
- **Processor** — cgroups v2 enforce CPU, memory, and PID limits. Seccomp-BPF filters block dangerous syscalls. Budget enforcement auto-kills pods after a configurable duration.
- **Network** — Per-pod network namespace with an embedded DNS resolver. Domain-level allow/deny lists, anti-DNS-tunneling, bandwidth caps. Live mutation without restart.
- **Memory** — PID, mount, UTS, and user namespace separation. Process isolation prevents visibility into host or other pods.
- **Devices** — Selective GPU, display, and audio passthrough. The agent gets hardware access without escaping the governance layer.

**Governance Ceiling:**
- **Action Queue** — 20 built-in action types (HTTP, filesystem, git, custom) with four approval tiers: immediate, delayed, staged (human approval), and blocked. Every action tracks an undo mechanism.
- **Credential Vault** — ChaCha20-Poly1305 encrypted at rest. Secrets are injected as environment variables at runtime — never in config files, CLI arguments, or logs. Vault proxy injection (Pro) intercepts HTTPS at the transport layer so the agent never sees real API keys.
- **Audit Log** — Append-only JSONL. Every action, approval, vault access, DNS query, and lifecycle event is timestamped. Static security analysis (`envpod audit --security`) checks pod configuration before deployment. A built-in jailbreak test probes all isolation boundaries from inside the pod.

## Architecture

envpod is a single 12 MB static Linux binary with zero runtime dependencies. No daemon, no container engine, no base images. The agent runs on the host's real filesystem layout — reads from the real directory tree, writes captured in the overlay — eliminating the path remapping and environment mismatch that plague container-based approaches.

The governance layer is **backend-agnostic**, communicating through an `IsolationBackend` trait. The current implementation uses native Linux primitives (namespaces, cgroups v2, OverlayFS, seccomp-BPF). Docker and microVM backends are planned.

**Web Display** enables browser-based desktop access via noVNC — a virtual display (Xvfb), VNC server (x11vnc), and WebSocket bridge run inside the pod with audio streaming (PulseAudio + Opus/WebM) and file upload. Guardian cgroups keep display services alive during pod freeze/thaw. All processes auto-restart on crash.

**Pod cloning** creates governed copies in ~130ms (vs ~1.3s for full init). At fleet scale, 100 pods spin up in under 1 second — 55x faster than Docker — because clones share the base rootfs via OverlayFS and only per-pod writes use additional disk.

## Key Differentiator

No other tool combines OS-level isolation with a governance layer. Docker isolates but doesn't govern. E2B and Daytona govern but run on third-party infrastructure. Firejail sandboxes but offers no action queue, vault, or overlay-as-review-gate.

envpod operates at the intersection: the agent thinks it's on your real system — but every write is captured, every action is queued, every secret is protected, and every decision is audited. Nothing persists without human approval.

## Availability

envpod CE is free, self-hosted, and licensed under BSL 1.1 (converts to AGPL-3.0 in 2030). It ships as a single static binary for Linux x86_64 and ARM64 with 42 example configurations and 18 built-in presets.

GitHub: [github.com/markamo/envpod-ce](https://github.com/markamo/envpod-ce) | Website: [envpod.dev](https://envpod.dev)

---

*Copyright 2026 Xtellix Inc. All rights reserved.*
