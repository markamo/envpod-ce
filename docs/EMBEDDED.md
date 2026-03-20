# Running envpod on Embedded / Edge Systems

> **EnvPod v0.1** — Zero-trust governance environments for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Business Source License 1.1

---

envpod ships a static ARM64 binary with no runtime dependencies. It runs on any ARM64 Linux system that supports namespaces, cgroups v2, and OverlayFS — including the NVIDIA Jetson Orin and Raspberry Pi 4/5.

## Supported Platforms

| Platform | Architecture | Kernel | cgroups v2 | GPU |
|----------|-------------|--------|------------|-----|
| **NVIDIA Jetson Orin** (NX, AGX) | aarch64 | 5.15+ (JetPack 6) | Yes (JetPack 6+) | CUDA via device passthrough |
| **Raspberry Pi 4** (4GB / 8GB) | aarch64 | 6.1+ (RPi OS 64-bit) | Needs cmdline flag | No CUDA |
| **Raspberry Pi 5** (4GB / 8GB) | aarch64 | 6.6+ (RPi OS 64-bit) | Yes (default) | No CUDA; Hailo HAT+ via /dev/hailo0 |
| Generic ARM64 Linux | aarch64 | 5.11+ | Yes | Varies |

---

## Installation

### Download the ARM64 binary

```bash
# On your Jetson / Pi
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-arm64.tar.gz | tar xz
cd envpod-*-linux-arm64
sudo bash install.sh
```

The binary is statically linked (`aarch64-unknown-linux-musl`) — no glibc or other runtime needed.

### Verify prerequisites

```bash
sudo envpod setup-check
```

This checks kernel version, cgroups v2, OverlayFS, iptables, and iproute2.

---

## NVIDIA Jetson Orin

### System requirements

- **JetPack 6** (Ubuntu 22.04-based, kernel 5.15) — recommended
- cgroups v2 enabled by default in JetPack 6
- `iptables` and `iproute2` already installed in JetPack

### GPU passthrough

Jetson Orin has an integrated NVIDIA Ampere GPU. envpod exposes it via device passthrough when `devices.gpu: true`:

```yaml
devices:
  gpu: true
```

Device nodes passed through:
```
/dev/nvidia0         GPU compute device
/dev/nvidiactl       CUDA control device
/dev/nvidia-modeset  CUDA/display driver
/dev/nvhost-ctrl     Jetson multimedia engine
/dev/nvhost-gpu      GPU compute engine
/dev/nvhost-nvdla0   Deep Learning Accelerator (if present)
/dev/nvhost-nvdla1   Deep Learning Accelerator (if present)
/dev/dri/card0       DRM render node
/dev/dri/renderD128  Direct render device
```

> **Note:** Jetson's GPU uses NVIDIA's unified memory architecture. The memory limit in `processor.memory` applies to the shared CPU+GPU pool. Reserve 4–6 GB for JetPack runtime overhead.

### Quick start

```bash
# Initialize a Jetson inference pod
sudo envpod init jetson-agent -c examples/jetson-orin.yaml

# Run a CUDA inference script inside the pod
sudo envpod run jetson-agent -- python3 infer.py

# Inspect what changed
sudo envpod diff jetson-agent

# Commit results back to host
sudo envpod commit jetson-agent /workspace/results
```

### JetPack-specific setup

To install PyTorch inside the pod (JetPack 6 wheel):

```yaml
setup:
  - "pip3 install torch torchvision --index-url https://developer.download.nvidia.com/compute/redist/jp/v60"
```

Or ONNX Runtime with TensorRT execution provider:

```yaml
setup:
  - "pip3 install onnxruntime-gpu"
```

### NVIDIA DLA (Deep Learning Accelerator)

Jetson Orin includes 2× DLA cores for INT8/FP16 inference. Exposed via `/dev/nvhost-nvdla0` and `/dev/nvhost-nvdla1`. These are passed through automatically when `devices.gpu: true`.

Use TensorRT to target DLA:
```python
config.default_device_type = trt.DeviceType.DLA
config.DLA_core = 0
```

---

## Raspberry Pi 4 / 5

### System requirements

**Raspberry Pi 5 (recommended):** Raspberry Pi OS 64-bit (Bookworm) — cgroups v2 enabled by default.

**Raspberry Pi 4:** Requires enabling cgroups v2 manually.

### Enable cgroups v2 on Raspberry Pi 4

Edit `/boot/firmware/cmdline.txt` (Bookworm) or `/boot/cmdline.txt` (Bullseye) and add:

```
cgroup_memory=1 cgroup_enable=memory systemd.unified_cgroup_hierarchy=1
```

Then reboot:
```bash
sudo reboot
```

Verify:
```bash
grep -w cgroup2 /proc/mounts
# Should output: cgroup2 /sys/fs/cgroup cgroup2 rw,...
```

### Ubuntu 24.04 on Raspberry Pi

Ubuntu 24.04 LTS for Raspberry Pi enables cgroups v2 by default. No extra configuration needed.

```bash
# Check
cat /sys/fs/cgroup/cgroup.controllers
# Should list: cpuset cpu io memory hugetlb pids rdma misc
```

### Quick start

```bash
# Initialize a Pi edge agent pod
sudo envpod init pi-agent -c examples/raspberry-pi.yaml

# Run your agent
sudo envpod run pi-agent -- python3 agent.py

# Inspect changes
sudo envpod diff pi-agent
```

### On-device LLM inference (llama.cpp)

For CPU inference with quantized models (e.g., Llama 3 Q4_K_M at ~4GB):

```yaml
processor:
  cores: 3.0
  memory: "6GB"    # Requires Pi 5 8GB model
  max_pids: 256

setup:
  - "pip3 install llama-cpp-python"
```

```bash
sudo envpod run pi-agent -- python3 -c "
from llama_cpp import Llama
llm = Llama(model_path='/models/llama-3-8b-q4.gguf', n_ctx=4096)
output = llm('What is 2+2?', max_tokens=64)
print(output['choices'][0]['text'])
"
```

### Hailo AI HAT+ (Raspberry Pi 5)

The Hailo 8L NPU HAT for Raspberry Pi 5 provides 13 TOPS at 2.5W. Pass through the Hailo device:

```yaml
# In your pod.yaml, under filesystem.system_access: advanced
# The /dev/hailo0 device will be available inside the pod
```

> Hailo device passthrough support: envpod detects `/dev/hailo0` and passes it through when `devices.gpu: true` on Pi 5.

---

## Resource Limits for Embedded Systems

Embedded systems have constrained resources. Recommended limits:

### Jetson Orin NX 16GB

```yaml
processor:
  cores: 6.0       # Leave 2 for JetPack runtime
  memory: "10GB"   # Leave 6GB for OS + CUDA runtime
  max_pids: 1024
```

### Raspberry Pi 5 8GB

```yaml
processor:
  cores: 3.0       # Leave 1 core for host OS
  memory: "6GB"    # Leave 2GB for host OS
  max_pids: 512
```

### Raspberry Pi 4 4GB

```yaml
processor:
  cores: 3.0
  memory: "2GB"    # Leave 2GB for host OS + overlay caching
  max_pids: 256
```

---

## DNS Configuration for Edge

Edge deployments often operate with limited or restricted internet access. Recommended:

```yaml
network:
  mode: Isolated
  dns:
    mode: Allowlist
    allow:
      - api.anthropic.com    # Or your model API
      # Add only what's needed
```

For **air-gapped** edge deployments:

```yaml
network:
  mode: None   # No network at all
```

For **local network only** (talk to a local inference server):

```yaml
network:
  mode: Isolated
  dns:
    mode: Allowlist
    allow: []              # No internet
  # Use vault to inject local API endpoint URL
```

---

## Cross-Compiling envpod for ARM64

If you're building from source, two options:

### Option 1: `cross` (recommended, Docker-based)

```bash
# Install cross
cargo install cross

# Build ARM64 static binary
cross build --release --target aarch64-unknown-linux-musl

# Binary at:
# target/aarch64-unknown-linux-musl/release/envpod
```

### Option 2: `cargo-zigbuild` (no Docker needed)

```bash
# Install zig and cargo-zigbuild
snap install zig --classic --beta
cargo install cargo-zigbuild

# Build
cargo zigbuild --release --target aarch64-unknown-linux-musl.2.17
```

### Using build-release.sh

```bash
# Build x86_64 only (default)
./build-release.sh

# Build ARM64 only
./build-release.sh --arch arm64

# Build both architectures
./build-release.sh --all
```

Output:
```
release/envpod-0.1.1-linux-x86_64/     x86_64 release
release/envpod-0.1.1-linux-arm64/      ARM64 release
envpod-linux-x86_64.tar.gz
envpod-linux-arm64.tar.gz
```

---

## Troubleshooting

### `FUSE: device not found` on Raspberry Pi OS

The `fuse` kernel module may not be loaded. The fuse-agent example config requires FUSE:

```bash
sudo modprobe fuse
echo fuse | sudo tee -a /etc/modules   # persist across reboots
```

### `overlay: unknown filesystem type` on Pi 4

OverlayFS may not be loaded:

```bash
sudo modprobe overlay
echo overlay | sudo tee -a /etc/modules
```

### cgroups v2 not detected

```bash
# Check
cat /sys/fs/cgroup/cgroup.controllers

# If empty, cgroups v2 is not active:
# - Raspberry Pi OS: add systemd.unified_cgroup_hierarchy=1 to cmdline.txt
# - Ubuntu: verify systemd is using unified hierarchy
```

### `iptables: No chain/target/match by that name`

Install iptables:

```bash
# Debian/Ubuntu (Pi, Jetson)
sudo apt install iptables

# Or use nftables-compat if nftables is default:
sudo apt install iptables-nft
sudo update-alternatives --set iptables /usr/sbin/iptables-nft
```

### GPU not visible inside pod (`nvidia-smi: command not found`)

On Jetson, `nvidia-smi` may not be installed in the pod's rootfs. Install it:

```yaml
setup:
  - "apt-get install -y nvidia-utils-525"   # Adjust version for your JetPack
```

Or use `tegrastats` (Jetson-native):

```bash
sudo envpod run jetson-agent -- tegrastats
```

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under the Business Source License 1.1.
