# Testing Example Configs

envpod ships a test script that initializes all example pod configs and reports pass/fail.

## Running the Test Suite

```bash
sudo ./tests/test-all-examples.sh               # test all examples
sudo ./tests/test-all-examples.sh --skip-desktop # skip slow desktop configs
sudo ./tests/test-all-examples.sh --cleanup      # destroy all test pods
```

Each example is initialized as `test-<name>` (e.g., `test-claude-code`). The script runs `envpod init` with setup and reports live output.

## Example Config Status

Tested on Ubuntu 24.04 (x86_64), envpod v0.1.1.

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

### Known Issues

| Config | Issue | Status |
|--------|-------|--------|
| `browser.yaml` | `/etc` overlay not writable in `safe` system_access mode — `apt-get update` can't write to `/etc/apt` | Code fix needed |
| `workstation-full.yaml` | `setup_script` path injection not implemented in `cmd_init` | Code fix needed |

### Skipped (platform-specific)

| Config | Reason |
|--------|--------|
| `jetson-orin.yaml` | Requires ARM64 + NVIDIA Jetson hardware |
| `raspberry-pi.yaml` | Requires ARM64 hardware |
| `browser-wayland.yaml` | Requires Wayland compositor |
| `monitoring-policy.yaml` | Not a pod config (monitoring policy definition) |
