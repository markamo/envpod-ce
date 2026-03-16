# Setup Patterns & Troubleshooting

> **EnvPod v0.1.1** — Zero-trust governance environments for AI agents
> Author: Mark Amoboateng · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Common patterns, known issues, and fixes for pod setup commands.

## Setup Command Order

Setup commands in pod.yaml run as root inside the pod. Follow this order for reliability:

```yaml
setup:
  # 1. Fix PEP 668 (Ubuntu 24.04 blocks system pip installs)
  - "rm -f /usr/lib/python*/EXTERNALLY-MANAGED"

  # 2. Clean stale apt lists (overlay can have stale package caches)
  - "rm -rf /var/lib/apt/lists/*"

  # 3. Disable 3rd-party apt sources that may not resolve
  - 'cd /etc/apt/sources.list.d && for f in *.list *.sources; do [ -f "$f" ] && sed -i "s/^deb /# deb /" "$f"; done; true'

  # 4. Install system packages
  - "DEBIAN_FRONTEND=noninteractive apt-get update -qq && apt-get install -y --no-install-recommends git curl ca-certificates python3 python3-pip"

  # 5. Install language-specific tools (pip, npm, etc.)
  - "pip3 install my-package"

  # 6. Application-specific setup
  - "git clone https://github.com/org/repo /opt/repo"
```

## Common Patterns

### Python (pip)

**Problem:** Ubuntu 24.04 blocks system-wide pip installs with PEP 668.

**Fix:** Remove the `EXTERNALLY-MANAGED` file before pip install.

```yaml
setup:
  - "rm -f /usr/lib/python*/EXTERNALLY-MANAGED"
  - "pip3 install my-package"
```

**Problem:** `Cannot uninstall urllib3, RECORD file not found` — debian-installed urllib3 conflicts.

**Fix:** Use `--ignore-installed` to bypass the broken package:

```yaml
  - "pip3 install --ignore-installed urllib3 my-package"
```

### Node.js (nvm + npm)

**Pattern:** Install nvm to `/opt/nvm` (world-accessible), symlink binaries to `/usr/local/bin`:

```yaml
setup:
  - "mkdir -p /opt/nvm && export NVM_DIR=/opt/nvm && curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash"
  - "export NVM_DIR=/opt/nvm && . \"$NVM_DIR/nvm.sh\" && nvm install 22"
  - |
    export NVM_DIR=/opt/nvm
    . "$NVM_DIR/nvm.sh"
    ln -sf "$(which node)" /usr/local/bin/node
    ln -sf "$(which npm)" /usr/local/bin/npm
    ln -sf "$(which npx)" /usr/local/bin/npx
  - "npm install -g my-package"
```

**Why `/opt/nvm`?** The default `~/.nvm` is mode 700 (root-only). `/opt/nvm` is accessible to the non-root `agent` user.

**Why symlinks?** `nvm` modifies `$PATH` via `.bashrc` which doesn't run in non-interactive setup steps. Symlinks to `/usr/local/bin` ensure `node`/`npm` are always available.

### APT Packages

**Problem:** Stale package lists in overlay cause `Unable to locate package`.

**Fix:** Always clean apt lists before update:

```yaml
setup:
  - "rm -rf /var/lib/apt/lists/*"
  - "DEBIAN_FRONTEND=noninteractive apt-get update -qq && apt-get install -y --no-install-recommends my-package"
```

**Problem:** 3rd-party apt sources (ESM, PPA, Chrome) can't resolve through DNS whitelist.

**Fix:** Disable non-essential sources before update:

```yaml
  - 'cd /etc/apt/sources.list.d && for f in *.list *.sources; do [ -f "$f" ] && sed -i "s/^deb /# deb /" "$f"; done; true'
```

### Git Clone (idempotent)

**Pattern:** Use `test -d || git clone` so re-running setup doesn't fail:

```yaml
  - "test -d /opt/my-repo || git clone https://github.com/org/repo /opt/my-repo"
```

### Ollama (local LLM)

**Problem:** Ollama's install script uses `tar` with permissions that fail in user namespaces (`Cannot change mode: Operation not permitted`).

**Fix:** Patch tar to skip permissions, tolerate errors if binary installed:

```yaml
  - "curl -fsSL https://ollama.ai/install.sh | sed 's/tar /tar --no-same-permissions --warning=no-file-changed /' | sh || test -x /usr/local/bin/ollama"
```

**Mounting models:** `~/.ollama/models` now works in pod.yaml mount paths (tilde auto-expands to the real user's home). For system-installed Ollama, use `/usr/share/ollama/.ollama/models` instead.

### LibreOffice

**Problem:** `libreoffice-common` postinst calls `install(1)` on AppArmor files, which fails with EPERM in user namespaces.

**Fix:** Patch postinst to use `touch` instead, then force-configure:

```yaml
  # In desktop-app-setup.sh:
  set +e
  apt-get install -y --no-install-recommends libreoffice
  if [ $? -ne 0 ]; then
      sed -i 's|install --mode 644 /dev/null|touch|g' \
          /var/lib/dpkg/info/libreoffice-common.postinst
      dpkg --configure -a
  fi
  set -e
```

### Chrome / VS Code (desktop apps)

**Pattern:** Use the `desktop-app-setup.sh` helper script:

```yaml
setup_script: examples/desktop-app-setup.sh
setup:
  - "bash /usr/local/bin/desktop-app-setup.sh chrome vscode"
```

Supported apps: `chrome`, `firefox`, `vscode`, `gimp`, `libreoffice`, `slack`, `brave`, `blender`, `inkscape`, `obs`, `vlc`, `cursor`, `obsidian`.

## Known Issues & Fixes

| Issue | Cause | Fix |
|-------|-------|-----|
| `EXTERNALLY-MANAGED` pip error | Ubuntu 24.04 PEP 668 | `rm -f /usr/lib/python*/EXTERNALLY-MANAGED` |
| `Cannot uninstall urllib3` | Debian-installed package has no RECORD | `pip3 install --ignore-installed urllib3` |
| `Unable to locate package` | Stale apt lists in overlay | `rm -rf /var/lib/apt/lists/*` before update |
| `Could not resolve` apt source | 3rd-party source not in DNS whitelist | Disable source or add domain to whitelist |
| `Cannot change mode` tar error | `fchmod` blocked in user namespace | `tar --no-same-permissions` or `\|\| true` |
| `install: Operation not permitted` | AppArmor `install(1)` needs CAP_MAC_ADMIN | Patch postinst: `sed -i 's/install --mode 644/touch/'` |
| `setup_script` exit 127 | Script injected to wrong overlay layer | Fixed in v0.1.1 — `inject_setup_script` uses `sys_upper` for advanced mode |
| `/etc/alternatives` read-only | ReadOnly mount blocks `update-alternatives` | Remove the mount from pod.yaml |
| `No space left on device` (pip) | `/tmp` tmpfs too small for downloads | Increase `processor.tmp_size` (e.g., `4GB`) |
| Setup fails on re-run | `git clone` fails if dir exists | Use `test -d \|\| git clone` pattern |
| OAuth login fails in pod | Token written but agent can't read (root ownership) | Fixed in v0.1.1 — `fix_upper_ownership` chowns agent home after setup |

## DNS Whitelist Domains

Common domains needed by setup commands:

| Package Manager | Domains |
|----------------|---------|
| **apt (Ubuntu)** | `*.ubuntu.com`, `*.archive.ubuntu.com` |
| **pip (PyPI)** | `pypi.org`, `*.pypi.org`, `files.pythonhosted.org` |
| **npm** | `registry.npmjs.org`, `*.npmjs.org` |
| **nvm** | `*.githubusercontent.com`, `nodejs.org`, `*.nodejs.org` |
| **Ollama** | `ollama.ai`, `*.ollama.ai`, `*.ollama.com` |
| **Chrome** | `dl.google.com`, `*.google.com` |
| **VS Code** | `packages.microsoft.com`, `*.microsoft.com`, `*.vscode-cdn.net` |
| **GitHub** | `github.com`, `*.github.com`, `*.githubusercontent.com` |
| **Anthropic** | `api.anthropic.com`, `*.anthropic.com` |
| **Hugging Face** | `huggingface.co`, `*.huggingface.co` |

## Tips

1. **Use `system_access: advanced`** if your setup installs packages to `/usr/local/bin` or `/usr/lib`. Default `safe` mode makes these read-only.

2. **Increase `tmp_size`** for heavy installs. Default 100MB is too small for PyTorch (8GB), Ollama (4GB), or aider (1GB).

3. **Test with `--verbose`**: `sudo envpod setup my-pod --verbose` shows full output instead of logs.

4. **Resume failed setup**: `sudo envpod setup my-pod` re-runs all steps. Steps that check for existing files (idempotent patterns) skip quickly.

5. **Discover DNS domains**: Start with `dns.mode: Blacklist`, run setup, then check `envpod audit my-pod` for queried domains. Convert to whitelist.

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
