# Testing Example Configs

envpod ships a test script that initializes all example pod configs and reports pass/fail.

## Running the Test Suite

```bash
sudo ./tests/test-all-examples.sh               # test all examples
sudo ./tests/test-all-examples.sh --skip-desktop # skip slow desktop configs
sudo ./tests/test-all-examples.sh --cleanup      # destroy all test pods
sudo ./tests/test-all-examples.sh browser workstation-full  # test specific examples
```

Each example is initialized as `test-<name>` (e.g., `test-claude-code`). The script runs `envpod init` with setup and reports live output.

## Example Config Status

Tested on Ubuntu 24.04 (x86_64), envpod v0.1.3.

### Passing (init + setup complete)

| Config | Category |
|--------|----------|
| `aider.yaml` | Coding agent |
| `basic-cli.yaml` | Environment |
| `basic-internet.yaml` | Environment |
| `claude-code.yaml` | Coding agent |
| `clone-user.yaml` | Environment |
| `codex.yaml` | Coding agent |
| `coding-agent.yaml` | Coding agent |
| `demo-pod.yaml` | Environment |
| `devbox.yaml` | Environment |
| `discovery-client.yaml` | Networking |
| `discovery-service.yaml` | Networking |
| `fuse-agent.yaml` | Framework |
| `gemini-cli.yaml` | Coding agent |
| `google-adk.yaml` | Framework |
| `hardened-sandbox.yaml` | Security |
| `host-apps.yaml` | Environment |
| `langgraph.yaml` | Framework |
| `ml-training.yaml` | Environment |
| `nodejs.yaml` | Environment |
| `openclaw.yaml` | Framework |
| `opencode.yaml` | Coding agent |
| `playwright.yaml` | Browser agent |
| `python-env.yaml` | Environment |
| `swe-agent.yaml` | Coding agent |
| `browser-use.yaml` | Browser agent |
| `desktop.yaml` | Desktop |
| `desktop-openbox.yaml` | Desktop |
| `desktop-sway.yaml` | Desktop |
| `desktop-web.yaml` | Desktop |
| `desktop-user.yaml` | Desktop |
| `gimp.yaml` | Desktop |
| `vscode.yaml` | Desktop |
| `web-display-novnc.yaml` | Desktop |
| `workstation.yaml` | Desktop |
| `workstation-gpu.yaml` | Desktop |
| `browser.yaml` | Browser agent |
| `browser-wayland.yaml` | Browser agent |
| `workstation-full.yaml` | Desktop |

### Skipped (platform-specific)

| Config | Reason |
|--------|--------|
| `jetson-orin.yaml` | Requires ARM64 + NVIDIA Jetson hardware |
| `raspberry-pi.yaml` | Requires ARM64 hardware |
| `monitoring-policy.yaml` | Not a pod config (monitoring policy definition) |

### Fixed Issues (v0.1.3)

| Config | Issue | Fix |
|--------|-------|-----|
| `browser.yaml` | ReadOnly mount on `/etc/alternatives` blocked `update-alternatives` during openbox post-install | Removed the unnecessary mount entry |
| `workstation-full.yaml` | `inject_setup_script` wrote to `upper/usr/local/bin/` but with `system_access: advanced`, `/usr` uses `sys_upper/` overlay — script was invisible (exit 127) | Write to `sys_upper_dir()` when system_access is advanced/dangerous |
| `workstation-full.yaml` | LibreOffice postinst calls `install(1)` on `/etc/apparmor.d/local/` which fails with EPERM in user namespaces (no `CAP_MAC_ADMIN`) | Patch postinst to replace `install --mode 644` with `touch`, then `dpkg --configure -a` completes properly — all components (Writer, Calc, Impress, Draw, Math, Base) fully functional |
| `browser-wayland.yaml` | Had same `/etc/alternatives` ReadOnly mount as browser.yaml | Removed the mount entry |
