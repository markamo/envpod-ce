# Running envpod inside Docker

This guide lets you test envpod without giving it root access on your host machine. All envpod features work inside Docker with the configuration below.

> **Note:** On bare metal, none of these workarounds are needed. Docker testing is a convenience path for evaluation. The production experience is bare metal Linux where envpod manages the kernel directly.

## Quick Start (Dockerfile)

The fastest way — everything pre-installed:

```bash
# Build the image (from the repo root)
docker build -t envpod-demo -f docker/Dockerfile docker/

# Run it
docker run -it --privileged --cgroupns=host \
  -v /tmp/envpod-test:/var/lib/envpod \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  envpod-demo

# You're in. Create a pod and go:
envpod init test -c /opt/envpod/examples/basic-internet.yaml
envpod run test -- bash
```

## Quick Start (manual)

If you prefer to set up step by step:

```bash
# Start container with required privileges
docker run -it --privileged --cgroupns=host \
  -v /tmp/envpod-test:/var/lib/envpod \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  ubuntu:24.04

# Install system dependencies
apt-get update
apt-get install -y curl iptables iproute2 net-tools dnsutils

# Enable IP forwarding (Docker disables this inside containers)
echo 1 > /proc/sys/net/ipv4/ip_forward

# Install envpod
cd /home
curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz
cd envpod-*-linux-x86_64
sudo bash install.sh

# Create a pod
envpod init test -c examples/basic-internet.yaml

# Enter the pod
envpod run test -- bash
```

## Why these flags?

| Flag | Why it's needed |
|------|----------------|
| `--privileged` | envpod creates kernel namespaces, mounts overlayfs, and writes cgroup controllers. Docker's default seccomp and capability restrictions block these operations. |
| `--cgroupns=host` | Without this, Docker gives the container its own cgroup namespace. Writes to cgroup controller files return `ENOTSUP` (error 95) because the kernel won't let a cgroup-namespaced process manage cgroups it doesn't own. |
| `-v /tmp/envpod-test:/var/lib/envpod` | Docker's own filesystem is overlayfs. Mounting overlayfs on top of overlayfs fails with `EINVAL`. This volume gives envpod a real filesystem (ext4/xfs) to place its overlays on. |
| `-v /sys/fs/cgroup:rw` | Ensures the cgroup v2 filesystem is writable inside the container. |
| `echo 1 > ip_forward` | Docker disables IP forwarding inside containers. envpod's pod network namespace routes traffic through the container, which requires forwarding to be enabled. |

## What envpod sets up automatically

When you run `envpod init`, it configures:

- **veth pair** between the pod network namespace and the container (e.g., `10.200.1.2/30` inside pod, `10.200.1.1/30` on host side)
- **iptables FORWARD rules** allowing pod traffic to reach eth0
- **iptables MASQUERADE** rule NATing pod traffic through the container's interface
- **Per-pod DNS resolver** listening on `10.200.1.1:53`, forwarding to upstream DNS
- **resolv.conf** inside the pod pointing to `10.200.1.1`

## Testing the governance workflow

This is what Docker cannot do:

```bash
# Enter the pod
envpod run test -- bash

# Agent makes changes
echo "hello" > /tmp/test.txt
mkdir /tmp/agent-work
echo "results" > /tmp/agent-work/output.csv
exit

# Review what the agent changed
envpod diff test

# Approve the changes
envpod commit test

# Or discard everything
envpod rollback test
```

## Testing credential vault

```bash
# Store a secret
envpod vault test set MY_API_KEY
# Enter value at prompt

# Run — secret is injected as env var
envpod run test -- bash
echo $MY_API_KEY    # available at runtime
cat pod.yaml        # not stored here

# Exit and check audit trail
exit
envpod audit test
```

## Testing DNS filtering

Use a pod config with DNS whitelist:

```bash
envpod init restricted -c examples/claude-code.yaml
envpod run restricted -- bash

# Allowed domain (if in whitelist)
curl https://api.anthropic.com

# Blocked domain
curl https://evil.com
# Connection refused — DNS resolver blocks it
```

## Troubleshooting

**`cgroup write: Not supported (os error 95)`**
You're missing `--cgroupns=host`. Restart the container with that flag.

**`mount overlayfs failed: EINVAL`**
Nested overlayfs. Add `-v /tmp/envpod-test:/var/lib/envpod` to give envpod a real filesystem.

**`curl` hangs inside the pod**
Check IP forwarding: `cat /proc/sys/net/ipv4/ip_forward`. If `0`, run `echo 1 > /proc/sys/net/ipv4/ip_forward` from the Docker container (outside the pod).

**DNS fails but IP works inside the pod**
The DNS resolver may need a moment to start after `envpod init`. Re-enter the pod and try again. Verify with `dig @10.200.1.1 google.com` from inside the pod.

## Limitations vs bare metal

- Pod networking is double-NATed (pod → Docker → host). Port forwarding behavior may differ.
- GPU passthrough requires `--gpus all` on the Docker command in addition to `--privileged`.
- Performance benchmarks inside Docker will not match bare metal numbers.
- Docker's own iptables rules coexist with envpod's. On bare metal, envpod has full control.

This is a testing environment. For production use, run envpod directly on Linux.

## Repo structure

Place these files in the repo:

```
envpod-ce/
├── docker/
│   ├── Dockerfile
│   └── entrypoint.sh
└── docs/
    └── DOCKER-TESTING.md
```

## Publishing a pre-built image (optional)

For launch, you can push to GitHub Container Registry so people don't need to build:

```bash
docker build -t ghcr.io/markamo/envpod-ce:demo docker/
docker push ghcr.io/markamo/envpod-ce:demo
```

Then the try-it command becomes:

```bash
docker run -it --privileged --cgroupns=host \
  -v /tmp/envpod-test:/var/lib/envpod \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  ghcr.io/markamo/envpod-ce:demo
```

One command. No install. Full governance demo.
