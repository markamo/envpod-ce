#!/usr/bin/env bash
#
# build-release.sh — Build envpod and assemble self-contained release folders.
#
# Output:
#   release/envpod-0.1.0-linux-x86_64/    (x86_64 release, default)
#   release/envpod-0.1.0-linux-aarch64/   (ARM64: Raspberry Pi / Jetson Orin)
#
# Usage:
#   ./build-release.sh              # x86_64 only (default)
#   ./build-release.sh --arch arm64 # aarch64 only
#   ./build-release.sh --all        # both architectures
#
# Prerequisites (x86_64):
#   rustup target add x86_64-unknown-linux-musl
#   apt install musl-tools
#
# Prerequisites (arm64) — choose one:
#   Option A (recommended): cargo install cross   [requires Docker]
#   Option B: cargo install cargo-zigbuild && snap install zig --classic --beta
#   Option C: install aarch64-linux-musl-gcc from musl.cc prebuilt toolchain
#
set -euo pipefail

VERSION="0.1.0"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------

BUILD_X86=true
BUILD_ARM64=false

for arg in "$@"; do
    case "$arg" in
        --arch=x86_64|--arch=amd64|--arch\ x86_64|--arch\ amd64) BUILD_X86=true;  BUILD_ARM64=false ;;
        --arch=arm64|--arch=aarch64)                              BUILD_X86=false; BUILD_ARM64=true  ;;
        --arch)  : ;;  # handled in pair below
        arm64|aarch64)  BUILD_X86=false; BUILD_ARM64=true  ;;
        x86_64|amd64)   BUILD_X86=true;  BUILD_ARM64=false ;;
        --all)  BUILD_X86=true; BUILD_ARM64=true ;;
        --help|-h)
            echo "Usage: $0 [--arch x86_64|arm64] [--all]"
            exit 0
            ;;
        *) fail "Unknown argument: $arg" ;;
    esac
done

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │      envpod release builder v${VERSION}     │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

ARCH_LIST=""
${BUILD_X86}   && ARCH_LIST="${ARCH_LIST} x86_64"
${BUILD_ARM64} && ARCH_LIST="${ARCH_LIST} aarch64"
echo "  Architectures:${ARCH_LIST}"
echo ""

# ---------------------------------------------------------------------------
# build_arch <rust_target> <arch_label> <build_tool>
#
#   rust_target  e.g. x86_64-unknown-linux-musl
#   arch_label   e.g. x86_64 or aarch64
#   build_tool   cargo | cross | zigbuild
# ---------------------------------------------------------------------------

build_arch() {
    local RUST_TARGET="$1"
    local ARCH_LABEL="$2"
    local BUILD_TOOL="$3"

    local RELEASE_NAME="envpod-${VERSION}-linux-${ARCH_LABEL}"
    local RELEASE_DIR="${SCRIPT_DIR}/release/${RELEASE_NAME}"

    # -----------------------------------------------------------------------
    # 1. Build static binary
    # -----------------------------------------------------------------------

    step "Building ${ARCH_LABEL} static binary (${RUST_TARGET})"

    if ! rustup target list --installed | grep -q "${RUST_TARGET}"; then
        echo "  Adding rustup target ${RUST_TARGET}..."
        rustup target add "${RUST_TARGET}"
    fi

    case "${BUILD_TOOL}" in
        cross)
            if ! command -v cross &>/dev/null; then
                fail "'cross' not found. Install with: cargo install cross  (requires Docker)"
            fi
            cross build --release --target "${RUST_TARGET}"
            ;;
        zigbuild)
            if ! command -v cargo-zigbuild &>/dev/null; then
                fail "'cargo-zigbuild' not found. Install with: cargo install cargo-zigbuild"
            fi
            # musl targets don't use glibc versioning — no .2.17 suffix
            cargo zigbuild --release --target "${RUST_TARGET}"
            ;;
        cargo)
            cargo build --release --target "${RUST_TARGET}"
            ;;
        *)
            fail "Unknown build tool: ${BUILD_TOOL}"
            ;;
    esac

    local BINARY="${SCRIPT_DIR}/target/${RUST_TARGET}/release/envpod"
    if [[ ! -f "${BINARY}" ]]; then
        fail "Build failed — binary not found at ${BINARY}"
    fi
    info "Binary built: ${BINARY} ($(du -h "${BINARY}" | cut -f1))"

    # -----------------------------------------------------------------------
    # 2. Create release directory
    # -----------------------------------------------------------------------

    step "Assembling release directory for ${ARCH_LABEL}"

    rm -rf "${RELEASE_DIR}"
    mkdir -p "${RELEASE_DIR}/docs" "${RELEASE_DIR}/examples"

    cp "${BINARY}" "${RELEASE_DIR}/envpod"
    chmod 755 "${RELEASE_DIR}/envpod"
    info "Binary copied"

    # -----------------------------------------------------------------------
    # 3. Generate install.sh
    # -----------------------------------------------------------------------

    cat > "${RELEASE_DIR}/install.sh" << 'INSTALL_EOF'
#!/usr/bin/env bash
#
# envpod installer — pre-built static binary.
# No Rust, git, or internet access required.
#
# Usage:
#   sudo bash install.sh
#
set -euo pipefail

ENVPOD_VERSION="0.1.0"
INSTALL_DIR="/usr/local/bin"
STATE_DIR="/var/lib/envpod"
EXAMPLES_DIR="/usr/local/share/envpod/examples"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
fail()  { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "\n${BOLD}→ $*${NC}"; }

echo -e "${BOLD}"
echo "  ┌──────────────────────────────────────┐"
echo "  │       envpod installer v${ENVPOD_VERSION}        │"
echo "  │    Zero-trust environments for AI    │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

if [[ $EUID -ne 0 ]]; then
    fail "This installer must be run as root (sudo bash install.sh)"
fi

# ---------------------------------------------------------------------------
# 1. Prerequisites
# ---------------------------------------------------------------------------

step "Checking prerequisites"

KVER=$(uname -r | cut -d. -f1-2)
KMAJOR=$(echo "$KVER" | cut -d. -f1)
KMINOR=$(echo "$KVER" | cut -d. -f2)
if [[ "$KMAJOR" -lt 5 ]] || { [[ "$KMAJOR" -eq 5 ]] && [[ "$KMINOR" -lt 11 ]]; }; then
    fail "Kernel $KVER is too old. envpod requires Linux 5.11+ (found: $(uname -r))"
fi
info "Kernel $(uname -r) (>= 5.11)"

if [[ ! -f /sys/fs/cgroup/cgroup.controllers ]]; then
    echo ""
    warn "cgroups v2 not active."
    echo "  Raspberry Pi OS: add 'systemd.unified_cgroup_hierarchy=1' to /boot/firmware/cmdline.txt"
    echo "  Other distros:   boot with cgroup_enable=memory cgroup_memory=1"
    fail "cgroups v2 required"
fi
info "cgroups v2 available"

if ! modprobe -n overlay 2>/dev/null && ! grep -q overlay /proc/filesystems 2>/dev/null; then
    warn "OverlayFS not loaded — trying modprobe overlay..."
    modprobe overlay 2>/dev/null || fail "OverlayFS not available. Run: modprobe overlay"
fi
info "OverlayFS available"

if ! command -v iptables &>/dev/null; then
    fail "iptables not found. Install: apt install iptables"
fi
info "iptables found"

if ! command -v ip &>/dev/null; then
    fail "iproute2 (ip) not found. Install: apt install iproute2"
fi
info "iproute2 found"

# ---------------------------------------------------------------------------
# 2. Install binary
# ---------------------------------------------------------------------------

step "Installing binary"

if [[ ! -f "$SCRIPT_DIR/envpod" ]]; then
    fail "envpod binary not found in $SCRIPT_DIR. Re-extract the release archive."
fi

cp "$SCRIPT_DIR/envpod" "$INSTALL_DIR/envpod"
chmod 755 "$INSTALL_DIR/envpod"
info "Installed to $INSTALL_DIR/envpod"

# ---------------------------------------------------------------------------
# 3. Create state directories
# ---------------------------------------------------------------------------

step "Creating state directories"
mkdir -p "$STATE_DIR/state" "$STATE_DIR/pods"
info "$STATE_DIR/{state,pods} created"

# ---------------------------------------------------------------------------
# 4. Shell completions
# ---------------------------------------------------------------------------

step "Installing shell completions"

REAL_USER="${SUDO_USER:-root}"
REAL_HOME=$(eval echo "~$REAL_USER")

install_bash_completions() {
    local comp_dir="/etc/bash_completion.d"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions bash > "$comp_dir/envpod"
    info "Bash completions installed to $comp_dir/envpod"
}

install_zsh_completions() {
    local comp_dir="$REAL_HOME/.zfunc"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions zsh > "$comp_dir/_envpod"
    local zshrc="$REAL_HOME/.zshrc"
    if [[ -f "$zshrc" ]] && ! grep -q '.zfunc' "$zshrc" 2>/dev/null; then
        echo 'fpath=(~/.zfunc $fpath)' >> "$zshrc"
    fi
    chown -R "$REAL_USER":"$REAL_USER" "$comp_dir" 2>/dev/null || true
    info "Zsh completions installed to $comp_dir/_envpod"
}

install_fish_completions() {
    local comp_dir="$REAL_HOME/.config/fish/completions"
    mkdir -p "$comp_dir"
    "$INSTALL_DIR/envpod" completions fish > "$comp_dir/envpod.fish"
    chown -R "$REAL_USER":"$REAL_USER" "$comp_dir" 2>/dev/null || true
    info "Fish completions installed to $comp_dir/envpod.fish"
}

REAL_SHELL=$(getent passwd "$REAL_USER" 2>/dev/null | cut -d: -f7 || echo "/bin/bash")
case "$REAL_SHELL" in
    */zsh)  install_zsh_completions; install_bash_completions ;;
    */fish) install_fish_completions ;;
    *)      install_bash_completions ;;
esac

# ---------------------------------------------------------------------------
# 5. Enable IP forwarding
# ---------------------------------------------------------------------------

step "Enabling IP forwarding"

CURRENT_FWD=$(sysctl -n net.ipv4.ip_forward 2>/dev/null || echo "0")
if [[ "$CURRENT_FWD" == "1" ]]; then
    info "IP forwarding already enabled"
else
    sysctl -w net.ipv4.ip_forward=1 >/dev/null
    info "IP forwarding enabled (runtime)"
fi

SYSCTL_CONF="/etc/sysctl.d/99-envpod.conf"
if [[ ! -f "$SYSCTL_CONF" ]]; then
    echo "net.ipv4.ip_forward = 1" > "$SYSCTL_CONF"
    info "Persisted to $SYSCTL_CONF"
else
    info "$SYSCTL_CONF already exists"
fi

# ---------------------------------------------------------------------------
# 6. Install examples
# ---------------------------------------------------------------------------

step "Installing examples"

if [[ -d "$SCRIPT_DIR/examples" ]]; then
    mkdir -p "$EXAMPLES_DIR"
    cp "$SCRIPT_DIR/examples/"*.yaml "$EXAMPLES_DIR/"
    cp "$SCRIPT_DIR/examples/"*.sh "$EXAMPLES_DIR/" 2>/dev/null || true
    info "Examples installed to $EXAMPLES_DIR/"
else
    warn "No examples directory found — skipping"
fi

# ---------------------------------------------------------------------------
# 7. Verify
# ---------------------------------------------------------------------------

step "Verifying installation"

INSTALLED_VERSION=$("$INSTALL_DIR/envpod" --version 2>&1 || true)
if [[ -z "$INSTALLED_VERSION" ]]; then
    fail "envpod binary not working"
fi
info "$INSTALLED_VERSION"

"$INSTALL_DIR/envpod" ls >/dev/null 2>&1 && info "envpod ls — OK" || warn "envpod ls failed (state dir may need sudo)"

echo ""
echo -e "${GREEN}${BOLD}Installation complete!${NC}"
echo ""
echo "  Quick start:"
echo "    sudo envpod init my-agent -c pod.yaml"
echo "    sudo envpod run my-agent -- bash"
echo "    sudo envpod diff my-agent"
echo ""
echo "  Examples installed to: $EXAMPLES_DIR/"
echo "  Documentation: $SCRIPT_DIR/docs/"
echo ""
INSTALL_EOF
    chmod 755 "${RELEASE_DIR}/install.sh"
    info "install.sh generated"

    # -----------------------------------------------------------------------
    # 4. Generate README.md
    # -----------------------------------------------------------------------

    cat > "${RELEASE_DIR}/README.md" << README_EOF
# envpod v${VERSION}

> **EnvPod v${VERSION}** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under Apache-2.0

**Docker isolates. Envpod governs.**

Every AI agent runs inside a **pod** — an isolated environment with four walls (memory, filesystem, network, processor) and a governance ceiling (credential vault, action queue, monitoring, remote control, audit).

## What's in This Release

\`\`\`
${RELEASE_NAME}/
├── envpod          Static binary for ${ARCH_LABEL} Linux (no dependencies)
├── install.sh      Installer (copy binary, create dirs, completions)
├── README.md       This file
├── LICENSE         Apache-2.0
├── docs/           Documentation
│   ├── INSTALL.md
│   ├── QUICKSTART.md
│   ├── USER-GUIDE.md
│   ├── FAQ.md
│   ├── BENCHMARKS.md
│   ├── SECURITY.md
│   ├── TUTORIALS.md
│   ├── POD-CONFIG.md
│   ├── CAPABILITIES.md
│   ├── ROADMAP.md
│   └── EMBEDDED.md     (Raspberry Pi / Jetson Orin guide)
└── examples/       Pod configs (24 YAML) + jailbreak-test.sh
\`\`\`

## Quick Start

\`\`\`bash
# Install
sudo bash install.sh

# Create a pod from an example config
sudo envpod init my-agent -c examples/coding-agent.yaml

# Run a command inside the pod (fully isolated)
sudo envpod run my-agent -- /bin/bash

# See what the agent changed
sudo envpod diff my-agent

# Accept or reject changes
sudo envpod commit my-agent              # apply all changes to host
sudo envpod commit my-agent /opt/a       # commit specific paths only
sudo envpod rollback my-agent            # discard everything

# View audit trail
sudo envpod audit my-agent

# Security analysis
sudo envpod audit my-agent --security
\`\`\`

See [docs/INSTALL.md](docs/INSTALL.md), [docs/QUICKSTART.md](docs/QUICKSTART.md),
[docs/POD-CONFIG.md](docs/POD-CONFIG.md), [docs/TUTORIALS.md](docs/TUTORIALS.md),
[docs/CAPABILITIES.md](docs/CAPABILITIES.md), [docs/ROADMAP.md](docs/ROADMAP.md),
[docs/BENCHMARKS.md](docs/BENCHMARKS.md), [docs/SECURITY.md](docs/SECURITY.md),
[docs/FAQ.md](docs/FAQ.md), and [docs/EMBEDDED.md](docs/EMBEDDED.md).

## Features

**Filesystem Isolation** — OverlayFS copy-on-write. Agent writes go to an overlay, never the host. Review with diff, accept with commit, discard with rollback.

**Network Isolation** — Each pod gets its own network namespace. Embedded DNS resolver per pod with whitelist, blacklist, or monitor modes. Every DNS query is logged.

**Process Isolation** — PID namespace, cgroups v2 (CPU, memory, PID limits), seccomp-BPF syscall filtering.

**Credential Vault** — Secrets stored encrypted (ChaCha20-Poly1305). Vault proxy injection available: agent never sees real API keys.

**Pod-to-Pod Discovery** — Pods can discover each other by name (\`<name>.pods.local\`) via the central envpod-dns daemon. Policy-controlled, bilateral access.

**Action Queue** — Actions classified by reversibility: immediate, delayed, staged (human approval), blocked.

**Audit Trail** — Append-only JSONL logs. Static security analysis via \`envpod audit --security\`.

**Monitoring Agent** — Background policy engine can autonomously freeze or restrict a pod.

**Remote Control** — Freeze, resume, kill, or restrict a running pod via \`envpod remote\`.

**Display + Audio** — GPU passthrough, Wayland/X11, PipeWire/PulseAudio forwarding for GUI agents.

**Web Dashboard** — \`envpod dashboard\` on localhost:9090 — fleet overview, live resource usage, audit timeline, diff/commit from browser.

**Embedded Systems** — Runs on Raspberry Pi 4/5 and NVIDIA Jetson Orin (ARM64 static binary). See [docs/EMBEDDED.md](docs/EMBEDDED.md).

## CLI Commands

| Command | Description |
|---------|-------------|
| \`envpod init <name> [-c config.yaml]\` | Create a new pod |
| \`envpod setup <name>\` | Re-run setup commands |
| \`envpod run <name> [--root] [-d] [-a] -- <cmd>\` | Run a command inside a pod |
| \`envpod diff <name>\` | Show filesystem changes |
| \`envpod commit <name> [paths...] [--exclude ...]\` | Apply changes to host |
| \`envpod rollback <name>\` | Discard all overlay changes |
| \`envpod audit <name> [--security] [--json]\` | Audit log or security analysis |
| \`envpod status <name>\` | Pod status and resource usage |
| \`envpod lock <name>\` | Freeze pod state |
| \`envpod kill <name>\` | Stop and rollback |
| \`envpod destroy <names...> [--base]\` | Remove pod(s) |
| \`envpod clone <source> <name> [--current]\` | Clone a pod (fast) |
| \`envpod base create/ls/destroy\` | Manage base pods |
| \`envpod ls [--json]\` | List all pods |
| \`envpod vault <name> set/get/remove/bind/unbind\` | Manage credentials + proxy |
| \`envpod ports <name> -p/-P/-i/--remove\` | Live port forwarding mutations |
| \`envpod discover <name> --on/--off/--add-pod\` | Live discovery mutations |
| \`envpod dns-daemon [--socket]\` | Start central DNS daemon |
| \`envpod queue/approve/cancel <name>\` | Action staging queue |
| \`envpod undo <name>\` | Undo last reversible action |
| \`envpod dns <name>\` | Update DNS policy live |
| \`envpod remote <name> <cmd>\` | Remote control |
| \`envpod monitor <name>\` | Monitoring policy |
| \`envpod dashboard [--port 9090]\` | Web dashboard |
| \`envpod gc\` | Clean up orphaned resources |

## System Requirements

- Linux ${ARCH_LABEL}, kernel 5.11+
- cgroups v2 (see [docs/EMBEDDED.md](docs/EMBEDDED.md) for Pi-specific setup)
- OverlayFS (\`modprobe overlay\`)
- iptables, iproute2

## License

Copyright 2026 Xtellix Inc. All rights reserved.

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for the full text.

**Author:** Mark Amoboateng, Xtellix Inc. (mark@envpod.dev)
**Patent:** Provisional patent filed February 22, 2026.
README_EOF
    info "README.md generated"

    # -----------------------------------------------------------------------
    # 5. Generate LICENSE
    # -----------------------------------------------------------------------

    cat > "${RELEASE_DIR}/LICENSE" << 'LICENSE_EOF'

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to the Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by the Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding any notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or exemplary, or consequential damages of any character
      arising as a result of this License or out of the use or inability
      to use the Work (including but not limited to damages for loss of
      goodwill, work stoppage, computer failure or malfunction, or any and
      all other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   Copyright 2026 Xtellix Inc.

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
LICENSE_EOF
    info "LICENSE generated"

    # -----------------------------------------------------------------------
    # 6. Copy docs and examples from repo
    # -----------------------------------------------------------------------

    for doc in INSTALL.md QUICKSTART.md USER-GUIDE.md FAQ.md BENCHMARKS.md \
               SECURITY.md TUTORIALS.md POD-CONFIG.md CAPABILITIES.md \
               ROADMAP.md EMBEDDED.md; do
        if [[ -f "${SCRIPT_DIR}/docs/${doc}" ]]; then
            cp "${SCRIPT_DIR}/docs/${doc}" "${RELEASE_DIR}/docs/${doc}"
        else
            echo "  Warning: docs/${doc} not found — skipping"
        fi
    done
    info "Documentation copied"

    cp "${SCRIPT_DIR}/examples/"*.yaml "${RELEASE_DIR}/examples/"
    cp "${SCRIPT_DIR}/examples/"*.sh "${RELEASE_DIR}/examples/" 2>/dev/null || true
    local EXAMPLE_COUNT
    EXAMPLE_COUNT=$(ls -1 "${RELEASE_DIR}/examples/"*.yaml 2>/dev/null | wc -l)
    local SCRIPT_COUNT
    SCRIPT_COUNT=$(ls -1 "${RELEASE_DIR}/examples/"*.sh 2>/dev/null | wc -l)
    info "Examples copied (${EXAMPLE_COUNT} YAML configs, ${SCRIPT_COUNT} scripts)"

    # -----------------------------------------------------------------------
    # 7. Create tarball
    # -----------------------------------------------------------------------

    step "Creating tarball for ${ARCH_LABEL}"

    local TARBALL="${SCRIPT_DIR}/${RELEASE_NAME}.tar.gz"
    tar czf "${TARBALL}" -C "${SCRIPT_DIR}/release" "${RELEASE_NAME}"
    info "Created ${TARBALL}"

    # -----------------------------------------------------------------------
    # 8. Summary for this arch
    # -----------------------------------------------------------------------

    step "Release summary — ${ARCH_LABEL}"

    echo ""
    echo "  Release directory: ${RELEASE_DIR}/"
    echo ""
    ls -lh "${RELEASE_DIR}/"
    echo ""

    local TARBALL_SIZE
    TARBALL_SIZE=$(du -h "${TARBALL}" | cut -f1)
    local TARBALL_SHA
    TARBALL_SHA=$(sha256sum "${TARBALL}")

    echo -e "  ${BOLD}Tarball:${NC}  ${RELEASE_NAME}.tar.gz (${TARBALL_SIZE})"
    echo -e "  ${BOLD}SHA-256:${NC} ${TARBALL_SHA}"
    echo ""
    info "Done! Distribute ${RELEASE_NAME}.tar.gz to any ${ARCH_LABEL} Linux system."
}

# ---------------------------------------------------------------------------
# Main: build requested architectures
# ---------------------------------------------------------------------------

# Detect ARM64 build tool preference (zigbuild preferred — cross has GLIBC issues)
ARM64_TOOL="cargo"
if command -v cross &>/dev/null; then
    ARM64_TOOL="cross"
fi
if command -v cargo-zigbuild &>/dev/null; then
    ARM64_TOOL="zigbuild"
fi

if ${BUILD_X86}; then
    build_arch "x86_64-unknown-linux-musl" "x86_64" "cargo"
fi

if ${BUILD_ARM64}; then
    echo ""
    echo -e "${BOLD}ARM64 build tool: ${ARM64_TOOL}${NC}"
    echo "  (override: ARM64_TOOL=cargo|cross|zigbuild ./build-release.sh --arch arm64)"
    echo ""
    # Allow override via environment
    ARM64_TOOL="${ARM64_TOOL_OVERRIDE:-${ARM64_TOOL}}"
    build_arch "aarch64-unknown-linux-musl" "aarch64" "${ARM64_TOOL}"
fi

echo ""
echo -e "${GREEN}${BOLD}All builds complete!${NC}"
