# Benchmarks

> **EnvPod v0.1.3** — Zero-trust governance environments for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

This document covers envpod's performance characteristics in detail — startup times, disk footprint, GPU passthrough overhead, and behavior at scale. All benchmarks are reproducible via scripts in `tests/`.

**Test environment:** Ubuntu 24.04, Docker 29.2.1, Podman 4.9.3, envpod 0.1.1, NVIDIA TITAN RTX x2.

---

## Table of Contents

- [Why Envpod Is Fast](#why-envpod-is-fast)
- [Head-to-Head: Envpod vs Docker vs Podman](#head-to-head-envpod-vs-docker-vs-podman)
- [Core Benchmarks](#core-benchmarks)
- [Clone vs Init](#clone-vs-init)
- [GPU Passthrough](#gpu-passthrough)
- [Disk Footprint](#disk-footprint)
- [Scale Test](#scale-test)
- [Running the Benchmarks](#running-the-benchmarks)
- [Methodology](#methodology)

---

## Why Envpod Is Fast

Envpod is faster than Docker and Podman because it avoids the overhead they carry:

| What Docker/Podman Do | What Envpod Does Instead |
|------------------------|--------------------------|
| Pull and unpack a container image | Symlink the host rootfs (no copy) |
| Create a writable container layer (snapshot) | Create empty OverlayFS upper dir (~1 KB) |
| Start containerd/runc/conmon shim processes | Direct `clone()` syscall into namespaces |
| Allocate a virtual network bridge | Lightweight veth pair into a netns |
| Copy CUDA libraries into the image (338 MB) | Bind-mount `/dev/nvidia*` from host (0 bytes) |

The result: envpod is **1.4–15x faster** depending on the operation, while adding governance features (COW diff/commit, DNS filtering, audit trail, vault, action queue) that neither Docker nor Podman provide.

---

## Head-to-Head: Envpod vs Docker vs Podman

**Script:** `tests/benchmark-podman.sh` (10 iterations)

| Test | Docker | Podman | Envpod | vs Docker | vs Podman |
|------|--------|--------|--------|-----------|-----------|
| fresh: run /bin/true | 552ms | 560ms | **401ms** | **151ms faster** | **159ms faster** |
| warm: run /bin/true | 95ms | 270ms | **32ms** | **63ms faster** | **238ms faster** |
| fresh: file I/O (write+read 1MB) | 604ms | 573ms | **413ms** | **191ms faster** | **160ms faster** |
| fresh: GPU nvidia-smi | 755ms | 745ms | **447ms** | **308ms faster** | **298ms faster** |
| warm: GPU nvidia-smi | 137ms | 244ms | **76ms** | **61ms faster** | **168ms faster** |

**Definitions:**
- **fresh** = create from base + run + destroy. Equivalent to `docker run --rm` / `podman run --rm` / `envpod clone` + `envpod run` + `envpod destroy`.
- **warm** = run in an existing instance. Equivalent to `docker exec` / `podman exec` / `envpod run`.

**Takeaway:** Envpod wins every test. The warm-start advantage (32ms vs 95ms Docker, 270ms Podman) matters most for interactive agent use — sub-50ms command execution feels instant.

---

## Core Benchmarks

**Script:** `tests/benchmark.sh` (50 iterations)

| Command | Median | Min | Max | P95 |
|---------|--------|-----|-----|-----|
| `envpod init` | 1.363s | 1.329s | 1.413s | 1.386s |
| `envpod clone` | ~130ms | — | — | — |
| `envpod run -- /bin/true` | 23ms | 20ms | 1.348s* | 45ms |
| `envpod run --root -- /bin/true` | 21ms | 20ms | 44ms | 41ms |
| `envpod diff` | 7ms | 7ms | 8ms | 7ms |
| `envpod rollback` | 8ms | 7ms | 9ms | 9ms |
| Full lifecycle (init+run+diff+destroy) | 3.348s | 3.286s | 3.405s | 3.400s |

*First run after init is ~1.3s (cold start: DNS resolver startup, cgroup initialization). All subsequent runs are 20–45ms.

**What each operation does:**

| Command | Work Performed |
|---------|---------------|
| `init` | Create rootfs (copy /etc + apt state), set up OverlayFS, create cgroup, create network namespace + veth pair, start DNS resolver. Optionally snapshot base with `--create-base`. |
| `clone` | Symlink rootfs, copy base overlay (~1 KB), create cgroup + netns (skip rootfs rebuild + DNS cold start) |
| `run` | Enter namespaces via `clone()`, set up seccomp filter, exec command |
| `diff` | Walk OverlayFS upper directory, compare against lower |
| `rollback` | Delete OverlayFS upper directory contents |

---

## Clone vs Init

**Script:** `tests/benchmark-clone.sh` (10 iterations)

| Operation | Time | Speedup |
|-----------|------|---------|
| `envpod init` (full pod creation) | ~1.36s | baseline |
| `envpod clone` (from base pod) | ~130ms | **~10x faster** |

Clone skips the expensive parts of init:
- **Rootfs creation** (~1s saved): Clone symlinks the rootfs directory instead of copying `/etc` and apt state from the host.
- **DNS cold start** (~300ms saved): Clone's first run still needs to start the DNS resolver, but the clone operation itself is instant.
- **Setup commands** (variable): Clone inherits the base snapshot taken after setup completed — no need to re-run `pip install` or `npm install`.

This is analogous to Docker's image→container model: `envpod init` is like `docker build` (slow, do once), `envpod clone` is like `docker create` (fast, do many times).

---

## GPU Passthrough

**Script:** `tests/benchmark-gpu.sh` (10 iterations, NVIDIA TITAN RTX x2)

| Command | Host | Pod | Overhead |
|---------|------|-----|----------|
| `nvidia-smi` query | 52ms | 80ms | +28ms (namespace entry) |
| `nvidia-smi --list-gpus` | — | 73ms | — |
| `envpod init` (gpu: true vs false) | — | 1.358s vs 1.350s | ~0ms |
| `envpod run /bin/true` (gpu: true vs false) | — | 20ms vs 25ms | ~0ms |

**How it works:** GPU passthrough is a zero-copy bind-mount of `/dev/nvidia*` and `/dev/dri/*` devices into the pod's mount namespace. There is no virtualization layer, no device emulation, and no driver translation. The pod process talks directly to the same kernel driver as the host.

**The 28ms overhead** is entirely from namespace entry (`clone()` + `setns()` syscalls), not from GPU access. Once inside the pod, GPU performance is identical to the host.

**Enabling GPU:** Set `devices.gpu: true` in pod.yaml. Envpod auto-detects NVIDIA devices and mounts them. No CUDA image required — the host's CUDA libraries are bind-mounted, saving 233 MB compared to Docker's CUDA base image.

---

## Disk Footprint

**Script:** `tests/benchmark-size.sh`

### Base Image / Base Pod (Ubuntu 24.04)

| Runtime | Size |
|---------|------|
| Docker image (`ubuntu:24.04`) | 119 MB |
| Podman image (`ubuntu:24.04`) | 77 MB |
| Envpod base pod | **105 MB** |

Envpod is 12% smaller than Docker. Docker and Podman copy the full distro userland into the image. Envpod copies only `/etc` + apt state — `/usr`, `/bin`, `/lib` are bind-mounted from the host at runtime.

### GPU Base (CUDA 12.0, Ubuntu 22.04)

| Runtime | Size |
|---------|------|
| Docker image (`nvidia/cuda:12.0.0-base-ubuntu22.04`) | 338 MB |
| Podman image | 229 MB |
| Envpod base pod (gpu: true) | **105 MB** |

Envpod GPU base is **69% smaller** than Docker's CUDA image. CUDA libraries live on the host and are bind-mounted into the pod — they're never copied.

### Per-Instance Overhead

| Runtime | Unique Data Per Instance |
|---------|--------------------------|
| Docker container layer | 4 KB |
| Podman container layer | 11 KB |
| Envpod pod (from init) | **1 KB** |
| Envpod clone (from base) | **1 KB** |

Clones share the base rootfs via symlink. The only unique data per clone is the empty OverlayFS upper directory and pod metadata.

### What This Means at Scale

Running 100 agent pods:

| Runtime | Total Disk |
|---------|-----------|
| Docker (100 containers) | 119 MB + 400 KB = ~119 MB |
| Podman (100 containers) | 77 MB + 1.1 MB = ~78 MB |
| Envpod (1 base + 100 clones) | 105 MB + 100 KB = ~105 MB |

All three runtimes are efficient at scale because they use copy-on-write layers. The real disk difference is in the base image, where envpod's bind-mount approach avoids duplicating host binaries.

---

## Scale Test

**Script:** `tests/benchmark-scale.sh` (50 instances)

| Phase | Docker | Podman | Envpod | vs Docker | vs Podman |
|-------|--------|--------|--------|-----------|-----------|
| Create 50 instances | 6.3s | 6.9s | **407ms** | **15x faster** | **17x faster** |
| Run /bin/true in all 50 | 11.4s | 26.8s | **7.5s** | **1.5x faster** | **3.6x faster** |
| Destroy all 50 | 1.2s | 2.4s | **1.6s** | — | — |
| gc (iptables cleanup) | — | — | 9.8s | — | — |
| **Full lifecycle** | **19.0s** | **36.2s** | **9.5s** | **2x faster** | **3.8x faster** |
| **Full lifecycle (with gc)** | — | — | **19.3s** | — | — |

Destroy is fast because envpod defers iptables cleanup. See [Fast Destroy + gc](#fast-destroy--gc) below.

### Why Creation Is 15x Faster

`envpod clone` creates a symlink to the base rootfs + empty overlay directories (~1 KB of unique data, 8ms per clone). Docker's `docker create` allocates a full container layer via the storage driver (overlay2), which involves more filesystem operations even though the layer itself is small (127ms per container).

### Fast Destroy + gc

Each envpod pod has its own network namespace with iptables rules. Cleaning up iptables is serialized by the kernel's `xtables` lock (`/run/xtables.lock`), which makes per-pod teardown expensive at scale.

Envpod solves this with a **two-phase destroy**:

1. **`envpod destroy`** — Fast. Deletes the veth pair and network namespace (2 kernel calls). Skips iptables cleanup entirely. Dead rules reference non-existent interfaces and never match traffic, so they're harmless.

2. **`envpod gc`** — Deferred. Cleans up all orphaned resources in one pass:
   - Stale iptables rules referencing dead veth interfaces
   - Orphaned network namespaces (`envpod-*` with no matching pod)
   - Orphaned cgroups under `/sys/fs/cgroup/envpod/`
   - Orphaned pod directories with no state file
   - Stale state files pointing to non-existent pod directories
   - Stale netns index files for non-existent pods

```bash
# Fast: destroy 50 pods (defers iptables cleanup)
sudo envpod destroy clone-1 clone-2 ... clone-50

# Full cleanup: remove all orphaned resources
sudo envpod gc
```

This is similar to how garbage collectors work in programming languages — defer the expensive cleanup to a batch operation rather than paying the cost on every deallocation.

**When to run gc:**
- After batch-destroying many pods
- Before running benchmarks (clean slate)
- Periodically via cron if you create/destroy pods frequently
- After a system crash or unclean shutdown
- Not urgent — stale resources consume negligible memory and never affect running pods

### Why Envpod Wins the Full Lifecycle

Envpod's full lifecycle (create + run + destroy) is **2x faster than Docker** and **3.8x faster than Podman** at 50 instances:

- **Without gc (9.5s):** The normal workflow. Destroy 50 pods in 1.6s, stale iptables rules are harmless. Run gc whenever convenient.
- **With gc (19.3s):** Even including the deferred iptables cleanup, envpod is still competitive with Docker (19.0s) while providing governance features neither Docker nor Podman offer.

For the common pattern of "spin up N agents, run tasks, tear down" — envpod is the fastest option. Run the benchmark yourself to see numbers on your hardware:

```bash
sudo ./tests/benchmark-scale.sh 50
```

The script measures destroy and gc separately so you can see the breakdown.

---

## Running the Benchmarks

All benchmark scripts are in `tests/` and require root (for namespace operations):

```bash
# Head-to-head comparison (requires Docker + Podman + envpod)
sudo ./tests/benchmark-podman.sh 10

# Docker vs envpod only (no Podman required)
sudo ./tests/benchmark-docker.sh 10

# Core envpod benchmarks (init, run, diff, rollback, lifecycle)
sudo ./tests/benchmark.sh 50

# Clone vs init speed comparison
sudo ./tests/benchmark-clone.sh 10

# GPU passthrough overhead (requires NVIDIA GPU)
sudo ./tests/benchmark-gpu.sh 10

# Disk footprint comparison (requires Docker + Podman)
sudo ./tests/benchmark-size.sh

# Scale test: create + run + destroy N instances
sudo ./tests/benchmark-scale.sh 50
```

The number argument sets iteration count (default varies by script). Higher counts give more stable medians but take longer.

---

## Methodology

### Timing

All benchmarks use wall-clock time via Bash's `date +%s%N` (nanosecond precision). Each test runs the operation N times, records every measurement, then reports median, min, max, and P95.

### Warm-up

Scripts that compare fresh vs warm performance run a warm-up iteration before measurement to eliminate filesystem cache effects.

### Environment

- Host OS: Ubuntu 24.04 (kernel 6.8+)
- Filesystem: ext4 (no reflinks — btrfs/XFS would make clone even faster)
- CPU: Multi-core x86_64
- GPU: NVIDIA TITAN RTX x2 (for GPU benchmarks)
- Docker: 29.2.1 (containerd backend)
- Podman: 4.9.3 (crun runtime)

### Reproducibility

Results will vary by hardware, kernel version, and filesystem. The relative comparisons (envpod vs Docker/Podman) should be consistent across similar Linux systems. Run the scripts on your own hardware for numbers specific to your environment.

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
