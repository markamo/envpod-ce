# Defense in Depth — 8 Isolation Layers

Every pod is protected by 8 independent layers. Disabling one does not compromise the others. An attacker must defeat all 8 to escape.

## The 8 Layers

| # | Layer | What it blocks | Kernel mechanism |
|---|-------|---------------|-----------------|
| 1 | **seccomp-BPF** | Dangerous syscalls (mount, ptrace, bpf, io_uring) | BPF filter on syscall entry |
| 2 | **Capabilities** | Privilege escalation (NO_NEW_PRIVS + dropped caps) | Capability bounding set |
| 3 | **PID namespace** | Host process visibility and signaling | CLONE_NEWPID |
| 4 | **Mount namespace** | Host filesystem access | CLONE_NEWNS + pivot_root |
| 5 | **Network namespace** | Unrestricted network access | CLONE_NEWNET + veth + iptables |
| 6 | **cgroups v2** | Resource exhaustion (CPU, memory, PIDs) | cgroup controllers |
| 7 | **COW filesystem** | Direct host modification | OverlayFS (upper/lower/merged) |
| 8 | **Audit** | Untracked actions | Append-only JSONL outside overlay |

## What Each Layer Blocks Independently

### Layer 1 — seccomp-BPF

Restricts which syscalls the process can invoke. Default profile allows ~130 safe syscalls. Everything else returns `EPERM`.

**Blocks:** `mount`, `umount`, `pivot_root`, `setns` (namespace escape), `ptrace` (process tracing), `bpf` (eBPF loading), `io_uring` (large attack surface), `reboot`, `kexec_load`, `init_module` (kernel modules), `keyctl` (kernel keyring), `perf_event_open` (info leak).

**Survives without it:** All other 7 layers still enforce. Agent gains syscall access but remains in its namespace, cgroup, and overlay.

### Layer 2 — Capabilities + NO_NEW_PRIVS

Agent runs as UID 60000 with zero capabilities. `NO_NEW_PRIVS` prevents gaining privileges via setuid binaries.

**Blocks:** `sudo`, `su`, setuid escalation, capability acquisition, raw socket creation (without `CAP_NET_RAW`), mounting filesystems (without `CAP_SYS_ADMIN`).

**Survives without it:** seccomp still blocks dangerous syscalls. Namespace isolation still contains the process. Even with capabilities, `pivot_root` and `setns` are seccomp-blocked.

### Layer 3 — PID Namespace

Agent is PID 1 in its own process tree. Cannot see or signal host processes.

**Blocks:** `kill` to host PIDs, `/proc` enumeration of host processes, process injection, debugging host services.

**Survives without it:** Agent sees host PIDs but can't escape mount namespace, can't modify host filesystem (COW), can't access network beyond its namespace. seccomp blocks `ptrace`.

### Layer 4 — Mount Namespace + pivot_root

Agent's root is the overlay merged directory. Host filesystem structure is invisible after `pivot_root`.

**Blocks:** Reading arbitrary host files, writing to host filesystem, accessing host `/etc`, `/var`, home directories (unless explicitly mounted).

**Survives without it:** COW overlay still captures writes. seccomp blocks `mount`/`pivot_root` syscalls. Network namespace still isolates. Agent sees host filesystem but writes go to overlay upper.

### Layer 5 — Network Namespace

Agent gets its own network stack. DNS resolver enforces allowlist/denylist. All traffic goes through veth pair with iptables NAT.

**Blocks:** Connecting to arbitrary hosts, DNS exfiltration, accessing host-only services (localhost), port scanning, ARP spoofing, raw packet injection.

**Survives without it:** Agent reaches the network but DNS filtering still applies (runs on host side). seccomp still active. Filesystem still isolated. Vault proxy still intercepts HTTPS (Premium).

### Layer 6 — cgroups v2

Limits CPU, memory, PIDs, and I/O. Prevents resource exhaustion that could affect the host or other pods.

**Blocks:** Fork bombs (`max_pids`), memory exhaustion (`memory.max`), CPU starvation (`cpu.max`), disk I/O flooding (`io.max`).

**Survives without it:** Agent can consume resources but remains in its namespace. Cannot escape filesystem or network isolation. Other pods are unaffected if they have their own cgroup limits.

### Layer 7 — COW Filesystem (OverlayFS)

Every write goes to the overlay upper layer. Host filesystem is never modified. Changes are reviewable via `envpod diff` and reversible via `envpod rollback`.

**Blocks:** Permanent host modification, supply chain attacks (modifying host binaries), configuration tampering, persistent malware installation.

**Survives without it:** Mount namespace still contains the agent. seccomp blocks filesystem-related syscalls. Audit log still records actions. But writes would go directly to the host — this is the most critical layer for governance.

### Layer 8 — Audit Trail

Append-only JSONL log outside the pod overlay. 39 action types. Agent cannot read, modify, or delete audit entries.

**Blocks:** Untracked actions, evidence tampering, deniable operations.

**Survives without it:** All isolation layers still function. Agent is contained. But there's no record of what happened — forensics and compliance are impossible.

## Attack Vector vs Defense Layer Matrix

| Attack | L1 seccomp | L2 Caps | L3 PID | L4 Mount | L5 Network | L6 cgroups | L7 COW | L8 Audit |
|--------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Escape to host filesystem | | | | **X** | | | **X** | |
| Modify host binaries | **X** | | | **X** | | | **X** | |
| Kill host processes | | | **X** | | | | | |
| Exfiltrate data via network | | | | | **X** | | | **X** |
| Fork bomb | **X** | | | | | **X** | | |
| Privilege escalation (sudo) | | **X** | | | | | | |
| Load kernel module | **X** | **X** | | | | | | |
| Mount host filesystem | **X** | **X** | | **X** | | | | |
| Ptrace host process | **X** | | **X** | | | | | |
| Exhaust host memory | | | | | | **X** | | |
| DNS tunneling exfiltration | | | | | **X** | | | **X** |
| Read host /etc/shadow | | | | **X** | | | | |
| Persistent malware | | | | **X** | | | **X** | |
| Tamper with audit log | | | | **X** | | | | **X** |
| Raw socket / ARP spoof | **X** | **X** | | | **X** | | | |
| io_uring exploit | **X** | | | | | | | |
| eBPF program loading | **X** | **X** | | | | | | |

**X** = this layer blocks this attack vector.

Most attacks are blocked by 2-3 layers independently. An attacker must defeat ALL blocking layers for a given vector.

## What Survives When a Layer is Relaxed

### `seccomp: none`

**Lost:** Syscall filtering. Agent can call any syscall.
**Still enforced:** PID ns, mount ns, network ns, cgroups, COW, capabilities, audit.
**Risk:** Agent can call `ptrace`, `mount` (but lacks `CAP_SYS_ADMIN`), `bpf`. Most dangerous syscalls still fail without capabilities.

### `user: root`

**Lost:** UID 60000 protection. Agent runs as UID 0 inside the namespace.
**Still enforced:** seccomp, PID ns, mount ns (root inside ns ≠ root on host), network ns, cgroups, COW, audit.
**Risk:** Agent can `chown`, `chmod` within overlay. Can bind to low ports. But `NO_NEW_PRIVS` still prevents capability gain.

### `system_access: dangerous`

**Lost:** Read-only system directory mounts. Agent can modify system dirs via COW overlay.
**Still enforced:** seccomp, capabilities, PID ns, network ns, cgroups, COW (writes to overlay, not host), audit.
**Risk:** Agent can `apt install` malware, modify `/usr/bin/`. But changes are in overlay — `envpod rollback` reverts everything.

### `seccomp: none` + `user: root`

**Lost:** Syscall filtering AND non-root protection. Weakest configuration.
**Still enforced:** PID ns, mount ns, network ns, cgroups, COW, audit.
**Risk:** Significantly elevated. Agent has root + all syscalls inside the namespace. Still can't escape the mount/PID/network namespaces without a kernel vulnerability.

## Comparison with Docker Defaults

| Layer | envpod | Docker (default) |
|-------|--------|-----------------|
| seccomp | ~130 syscalls, `EPERM` on block | ~370 syscalls, wider allowlist |
| Capabilities | Zero caps, `NO_NEW_PRIVS` | 14 default capabilities |
| PID namespace | Always | Optional (`--pid=host` disables) |
| Mount namespace | Always + pivot_root | Always |
| Network namespace | Always + DNS filtering | Always (no DNS filtering) |
| cgroups | Always enforced | Optional (no defaults) |
| COW filesystem | Always (diff/commit/rollback) | Always (no governance) |
| Audit | Always (39 action types) | None built-in |

envpod is stricter than Docker on every layer:
- **seccomp:** 130 vs 370 allowed syscalls (65% more restricted)
- **Capabilities:** 0 vs 14 default caps
- **NO_NEW_PRIVS:** Always on vs opt-in
- **DNS filtering:** Built-in vs none
- **Audit:** Built-in vs none

## Related

- [SECURITY-MODEL.md](SECURITY-MODEL.md) — Full security architecture
- [SECURITY-SECCOMP.md](SECURITY-SECCOMP.md) — Complete syscall lists for all 3 profiles
- [SECURITY-POSTURE.md](SECURITY-POSTURE.md) — Audit matrix for all example configs
- [VERIFY.md](VERIFY.md) — Adversarial verification testing
