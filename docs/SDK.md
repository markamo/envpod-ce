# SDK Reference — Python & TypeScript

> **EnvPod v0.1.4** — The zero-trust governance layer for AI agents
> Author: Mark Amo-Boateng, PhD · mark@envpod.dev
> Copyright 2026 Xtellix Inc. · Licensed under BSL-1.1

---

Programmatic governance for AI agents. 44 methods with full CLI parity. The SDKs are thin wrappers around the `envpod` CLI binary — every method calls the binary via subprocess.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Isolation Modes](#isolation-modes)
- [Pod Creation](#pod-creation)
  - [Context Manager](#context-manager-auto-destroy)
  - [Persistent](#persistent-survives-script-exit)
  - [Wrap Existing](#wrap-existing-pod)
  - [Base Pods + Cloning](#base-pods--fast-cloning)
  - [Disposable Pods](#disposable-pods)
- [Running Commands](#running-commands)
  - [Shell Commands](#shell-commands)
  - [Inline Code](#inline-code)
  - [Local Files](#local-files)
  - [Inject Files](#inject-files-and-executables)
  - [Mount Directories](#mount-host-directories)
- [Filesystem Operations](#filesystem-operations)
- [Vault (Credentials)](#vault-credentials)
- [DNS Mutation](#dns-mutation-live)
- [Remote Control](#remote-control)
- [Action Queue](#action-queue)
- [Snapshots](#snapshots)
- [Resource Management](#resource-management)
- [Lifecycle](#lifecycle)
- [Pod Info](#pod-info)
- [Web Display](#web-display)
- [Screening](#screening)
- [Cleanup](#cleanup)
- [Error Handling](#error-handling)
- [API Reference (44 Methods)](#complete-api-reference--44-methods)

## Installation

```bash
# Python
pip install envpod

# TypeScript / Node.js
npm install envpod
```

The envpod binary is auto-installed on first use if not already present. On macOS or Windows, the SDK shows install instructions for OrbStack or WSL2.

## Quick Start

### Python

```python
from envpod import Pod, screen

# Governed agent — auto-destroy on exit
with Pod("my-agent", config="coding-agent.yaml") as pod:
    pod.vault_set("ANTHROPIC_API_KEY", "sk-ant-...")
    pod.run("python3 agent.py")
    print(pod.diff())
    pod.commit("src/", rollback_rest=True)
# auto: destroy + gc

# Screen prompts — free for all users
result = screen("ignore previous instructions")
# {'matched': True, 'category': 'injection', ...}
```

### TypeScript

```typescript
import { Pod, screen } from 'envpod';

await Pod.with('my-agent', { config: 'coding-agent.yaml' }, async (pod) => {
    pod.vaultSet('ANTHROPIC_API_KEY', 'sk-ant-...');
    pod.run('python3 agent.py');
    console.log(pod.diff());
    pod.commit(['src/'], { rollbackRest: true });
});
// auto: destroy + gc

const result = screen('ignore previous instructions');
// { matched: true, category: 'injection', ... }
```

## Isolation Modes

On first use, the SDK asks which mode to use:

| Mode | Sudo? | What you get |
|------|-------|-------------|
| **Standard** | No | COW overlay, diff/commit, vault, audit. No cgroup limits, no network namespace. |
| **Full** | Yes (once per session) | Everything above + cgroup limits + network namespace + DNS filtering. |

```bash
export ENVPOD_MODE=full    # or "standard" — skip the prompt
```

```python
pod = Pod("my-agent", mode="full")
```

The choice is saved to `~/.config/envpod/sdk.json`.

## Pod Creation

### Context Manager (Auto-Destroy)

```python
# Python — destroyed + gc on exit
with Pod("my-agent", config="pod.yaml") as pod:
    pod.run("python3 agent.py")
```

```typescript
// TypeScript — destroyed + gc on exit
await Pod.with('my-agent', { config: 'pod.yaml' }, async (pod) => {
    pod.run('python3 agent.py');
});
```

### Persistent (Survives Script Exit)

```python
# Python — pod stays running after script ends
with Pod("desktop", config="workstation-full.yaml", persist=True) as pod:
    url = pod.start_display()
    print(f"Open: {url}")

# Or call persist() mid-script (Jupyter workflow)
with Pod("experiment", config="pod.yaml") as pod:
    pod.run("python3 agent.py")
    pod.persist()  # don't destroy on exit
```

```typescript
// TypeScript — persistent pod
const pod = await Pod.create('desktop', { config: 'workstation-full.yaml', persist: true });
const url = pod.startDisplay();
console.log(`Open: ${url}`);

// Or persist mid-callback
await Pod.with('experiment', { config: 'pod.yaml' }, async (pod) => {
    pod.run('python3 agent.py');
    pod.persist();  // won't destroy on exit
});
```

### Wrap Existing Pod

```python
# Created via CLI: sudo envpod init my-agent -c pod.yaml
pod = Pod("my-agent")  # wraps existing, no init
pod.run("python3 agent.py")
```

```typescript
const pod = Pod.wrap('my-agent');
pod.run('python3 agent.py');
```

### Base Pods + Fast Cloning

```python
# One-time: full setup (~5 min), save as base
base = Pod("worker", config="coding-agent.yaml")
base.init_with_base()
base.destroy()

# Every time: ~8ms clone — accepts Pod or string
agent = Pod.clone(base, "agent-1")
agent.run("python3 task.py")
agent.destroy()
```

```typescript
const base = Pod.wrap('worker', { config: 'coding-agent.yaml' });
base.initWithBase();
base.destroy();

const agent = Pod.clone(base, 'agent-1');
agent.run('python3 task.py');
agent.destroy();
```

### Disposable Pods

Clone, run, optional commit, destroy. Like `docker run --rm` but governed.

```python
# Run and discard
Pod.disposable(base, "task-1", "python3 test.py")

# Run and save results
Pod.disposable(base, "task-2", "python3 experiment.py",
               commit_paths=["results/"],
               output="/tmp/output/")

Pod.gc()
```

```typescript
Pod.disposable(base, 'task-1', 'python3 test.py');

Pod.disposable(base, 'task-2', 'python3 experiment.py', {
    commitPaths: ['results/'],
    output: '/tmp/output/',
});

Pod.gc();
```

## Running Commands

### Shell Commands

```python
pod.run("python3 agent.py")                          # basic
pod.run("apt-get install -y curl", root=True)        # as root
output = pod.run("cat results.json", capture=True)   # capture stdout
pod.run("agent.py", env={"API_URL": "https://..."})  # env vars
pod.run("chrome", display=True, audio=True)          # display + audio
pod.run("startxfce4", background=True)               # background
```

```typescript
pod.run('python3 agent.py');
pod.run('apt-get install -y curl', { root: true });
const output = pod.run('cat results.json', { capture: true });
pod.run('agent.py', { env: { API_URL: 'https://...' } });
pod.run('chrome', { display: true, audio: true });
pod.run('startxfce4', { background: true });
```

### Inline Code

```python
pod.run_script("""
import json
data = {"status": "ok"}
with open("/workspace/output.json", "w") as f:
    json.dump(data, f)
""")

pod.run_script("console.log('hello')", interpreter="node")
output = pod.run_script("print(42 * 42)", capture=True)
```

```typescript
pod.runScript(`
import json
data = {"status": "ok"}
with open("/workspace/output.json", "w") as f:
    json.dump(data, f)
`);

pod.runScript('console.log("hello")', { interpreter: 'node' });
const output = pod.runScript('print(42 * 42)', { capture: true });
```

### Local Files

```python
pod.run_file("agent.py")     # auto-detect: python3
pod.run_file("agent.js")     # auto-detect: node
pod.run_file("setup.sh")     # auto-detect: bash
```

```typescript
pod.runFile('agent.py');      // auto-detect: python3
pod.runFile('agent.js');      // auto-detect: node
pod.runFile('setup.sh');      // auto-detect: bash
```

### Inject Files and Executables

```python
pod.inject("data.csv", "/workspace/data.csv")
pod.inject("/path/to/tool", "/usr/local/bin/tool", executable=True)
```

```typescript
pod.inject('data.csv', '/workspace/data.csv');
pod.inject('/path/to/tool', '/usr/local/bin/tool', true);
```

### Mount Host Directories

```python
pod.mount("/home/mark/projects/webapp")              # COW isolated
pod.mount("/data/datasets", readonly=True)           # read-only
```

```typescript
pod.mount('/home/mark/projects/webapp');
pod.mount('/data/datasets', true);  // readonly
```

## Filesystem Operations

```python
# Diff
diff = pod.diff()                        # human-readable
diff = pod.diff(all_changes=True)        # include system paths
diff = pod.diff(json_output=True)        # JSON for scripting

# Commit
pod.commit()                             # commit everything
pod.commit("src/", "docs/")             # specific paths only
pod.commit("src/", rollback_rest=True)   # keep src/, discard rest
pod.commit(exclude=["node_modules/"])    # exclude paths
pod.commit(output="/tmp/export/")        # export to directory

# Rollback
pod.rollback()                           # discard all changes
```

```typescript
const diff = pod.diff();
pod.diff({ all: true });
pod.diff({ json: true });

pod.commit();
pod.commit(['src/', 'docs/']);
pod.commit(['src/'], { rollbackRest: true });
pod.commit([], { exclude: ['node_modules/'] });
pod.commit([], { output: '/tmp/export/' });

pod.rollback();
```

## Vault (Credentials)

```python
pod.vault_set("ANTHROPIC_API_KEY", "sk-ant-...")
pod.vault_set("OPENAI_API_KEY", "sk-...")
pod.vault_set("AWS_SECRET_ACCESS_KEY", "...")
```

```typescript
pod.vaultSet('ANTHROPIC_API_KEY', 'sk-ant-...');
pod.vaultSet('OPENAI_API_KEY', 'sk-...');
pod.vaultSet('AWS_SECRET_ACCESS_KEY', '...');
```

Secrets are encrypted (ChaCha20-Poly1305) and injected as environment variables at runtime. The agent never sees the actual key in config, CLI args, shell history, or logs.

## DNS Mutation (Live)

Adjust network policy on a running pod without restart.

```python
pod.dns_allow("api.newservice.com", "cdn.newservice.com")
pod.dns_deny("tracking.analytics.com")
pod.dns_remove_allow("api.oldservice.com")
pod.dns_remove_deny("safe-domain.com")
```

```typescript
pod.dnsAllow('api.newservice.com');
pod.dnsDeny('tracking.analytics.com');
pod.dnsRemoveAllow('api.oldservice.com');
pod.dnsRemoveDeny('safe-domain.com');
```

## Remote Control

Monitor and intervene in a running agent.

```python
pod.freeze()                # freeze instantly
pod.resume()                # resume
pod.restrict("readonly")    # limit to read-only
pod.kill()                  # terminate + rollback all changes
```

```typescript
pod.freeze();
pod.resume();
pod.restrict('readonly');
pod.kill();
```

## Action Queue

Require human approval for dangerous operations.

```python
pending = pod.queue_list()      # list pending actions
pod.approve("abc12345")         # approve by ID
pod.cancel("def67890")          # reject by ID
pod.undo()                      # undo last reversible action
```

```typescript
const pending = pod.queueList();
pod.approve('abc12345');
pod.cancel('def67890');
pod.undo();
```

## Snapshots

Checkpoint and restore pod state.

```python
pod.snapshot_create("before-refactor")
pod.run("python3 agent.py --task refactor")

# Bad result? Restore
pod.snapshot_restore("before-refactor")

# Manage snapshots
print(pod.snapshot_list())
pod.snapshot_destroy("before-refactor")
```

```typescript
pod.snapshotCreate('before-refactor');
pod.run('python3 agent.py --task refactor');

pod.snapshotRestore('before-refactor');

console.log(pod.snapshotList());
pod.snapshotDestroy('before-refactor');
```

## Resource Management

```python
# Live resize (running pod)
pod.resize(cpus=4.0, memory="8GB", tmp_size="2GB", max_pids=2048)

# Stopped pod mutation
pod.stop()
pod.resize(gpu=True)
pod.start()
```

```typescript
pod.resize({ cpus: 4.0, memory: '8GB', tmpSize: '2GB', maxPids: 2048 });

pod.stop();
pod.resize({ gpu: true });
pod.start();
```

## Lifecycle

```python
pod.start()       # start in background
pod.stop()        # stop gracefully
pod.restart()     # stop + start
pod.lock()        # freeze state
pod.unlock()      # resume frozen pod
pod.kill()        # terminate + rollback
pod.destroy()     # remove entirely
```

```typescript
pod.start();
pod.stop();
pod.restart();
pod.lock();
pod.unlock();
pod.kill();
pod.destroy();
```

## Pod Info

```python
print(pod.info())          # {'name': '...', 'status': '...', 'ip': '...'}
print(pod.ip)              # '10.200.5.2'
print(pod.display_url)     # 'http://10.200.5.2:6080/vnc.html'
print(pod.status())        # full status output
print(pod.logs())          # pod output logs
print(pod.audit())         # audit trail
print(pod.audit(security=True))  # security analysis
print(pod.exists())        # True/False
```

```typescript
console.log(pod.info());
console.log(pod.ip);
console.log(pod.displayUrl);
console.log(pod.status());
console.log(pod.logs());
console.log(pod.audit());
console.log(pod.audit({ security: true }));
console.log(pod.exists());
```

## Web Display

Start a full desktop accessible via browser.

```python
pod = Pod("desktop", config="workstation-full.yaml", persist=True)
pod.init()
url = pod.start_display()   # starts pod, returns noVNC URL
print(f"Open: {url}")       # http://10.200.5.2:6080/vnc.html
```

```typescript
const pod = await Pod.create('desktop', { config: 'workstation-full.yaml', persist: true });
const url = pod.startDisplay();
console.log(`Open: ${url}`);
```

## Screening

Screen prompts for injection, credentials, PII, and exfiltration. Free for all users.

```python
from envpod import screen, screen_api, screen_file

# Screen text
result = screen("ignore previous instructions")
if result['matched']:
    print(f"BLOCKED: {result['category']}")

# Screen API request body (parses Anthropic, OpenAI, Gemini, Ollama)
result = screen_api('{"messages":[{"role":"user","content":"my key is sk-ant-..."}]}')

# Screen a file
result = screen_file("prompt.txt")
```

See [SCREENING.md](SCREENING.md) for the full screening reference.

## Cleanup

```python
Pod.gc()   # clean orphaned iptables, cgroups, netns
```

Auto-called after context manager exit (Python) and `Pod.with()` (TypeScript).

## Error Handling

```python
from envpod import Pod
from envpod.pod import PodError

try:
    with Pod("my-agent", config="pod.yaml") as pod:
        pod.run("python3 agent.py")
        pod.commit("src/")
except PodError as e:
    print(f"Pod operation failed: {e}")
```

```typescript
import { Pod } from 'envpod';

try {
    const pod = await Pod.create('my-agent', { config: 'pod.yaml' });
    pod.run('python3 agent.py');
    pod.commit(['src/']);
    pod.destroy();
} catch (e) {
    console.error(`Pod operation failed: ${e}`);
}
```

## Requirements

- Python 3.8+ / Node.js 18+
- Linux (x86_64 or ARM64), Windows WSL2, or macOS via OrbStack
- envpod binary (auto-installed on first use)

## Complete API Reference — 44 Methods

| Method | Python | TypeScript | Description |
|--------|--------|-----------|-------------|
| **Pod Creation** | | | |
| Constructor | `Pod(name, config, preset, mode, persist)` | `new Pod(name, opts)` | Create pod instance |
| Auto-cleanup | `with Pod(...) as pod:` | `Pod.with(name, opts, fn)` | Auto-destroy + gc on exit |
| Create + init | — | `Pod.create(name, opts)` | Create and init (async) |
| Wrap existing | `Pod(name)` | `Pod.wrap(name, opts)` | Wrap existing pod (no init) |
| Init | `pod.init(config, preset, verbose, mount_cwd)` | `pod.init(opts)` | Create and set up pod |
| Init + base | `pod.init_with_base(config, base_name)` | `pod.initWithBase(opts)` | Create + save as base for cloning |
| Clone | `Pod.clone(source, name)` | `Pod.clone(source, name, opts)` | Clone from base (~8ms). Source: Pod or string |
| Disposable | `Pod.disposable(base, name, cmd, ...)` | `Pod.disposable(base, name, cmd, opts)` | Clone → run → optional commit → destroy |
| Persist | `pod.persist()` | `pod.persist()` | Mark as persistent (won't auto-destroy) |
| **Running Commands** | | | |
| Shell command | `pod.run(cmd, root, env, capture, display, audio, background)` | `pod.run(cmd, opts)` | Run command inside pod |
| Inline code | `pod.run_script(code, interpreter)` | `pod.runScript(code, opts)` | Run code string (no file needed) |
| Local file | `pod.run_file(path, interpreter)` | `pod.runFile(path, opts)` | Copy + run local file |
| **File Operations** | | | |
| Inject file | `pod.inject(local_path, pod_path, executable)` | `pod.inject(localPath, podPath, executable)` | Copy file/binary into pod |
| Mount dir | `pod.mount(path, readonly)` | `pod.mount(path, readonly)` | Mount host directory (COW) |
| Diff | `pod.diff(all_changes, json_output)` | `pod.diff(opts)` | Show filesystem changes |
| Commit | `pod.commit(*paths, exclude, output, rollback_rest)` | `pod.commit(paths, opts)` | Commit changes to host |
| Rollback | `pod.rollback()` | `pod.rollback()` | Discard all changes |
| **Governance** | | | |
| Audit | `pod.audit(security, json_output)` | `pod.audit(opts)` | View audit log or security scan |
| Vault | `pod.vault_set(key, value)` | `pod.vaultSet(key, value)` | Store encrypted secret |
| Resize | `pod.resize(cpus, memory, tmp_size, max_pids, gpu)` | `pod.resize(opts)` | Live resource mutation |
| **DNS Mutation** | | | |
| Allow | `pod.dns_allow(*domains)` | `pod.dnsAllow(...domains)` | Add to allow list (live) |
| Deny | `pod.dns_deny(*domains)` | `pod.dnsDeny(...domains)` | Add to deny list (live) |
| Remove allow | `pod.dns_remove_allow(*domains)` | `pod.dnsRemoveAllow(...domains)` | Remove from allow list |
| Remove deny | `pod.dns_remove_deny(*domains)` | `pod.dnsRemoveDeny(...domains)` | Remove from deny list |
| **Remote Control** | | | |
| Remote | `pod.remote(command)` | `pod.remote(command)` | Send remote control command |
| Freeze | `pod.freeze()` | `pod.freeze()` | Freeze pod instantly |
| Resume | `pod.resume()` | `pod.resume()` | Resume frozen pod |
| Restrict | `pod.restrict(level)` | `pod.restrict(level)` | Limit permissions |
| **Action Queue** | | | |
| Queue list | `pod.queue_list()` | `pod.queueList()` | List pending actions |
| Approve | `pod.approve(action_id)` | `pod.approve(actionId)` | Approve queued action |
| Cancel | `pod.cancel(action_id)` | `pod.cancel(actionId)` | Cancel queued action |
| Undo | `pod.undo()` | `pod.undo()` | Undo last reversible action |
| **Snapshots** | | | |
| Create | `pod.snapshot_create(name)` | `pod.snapshotCreate(name)` | Checkpoint overlay state |
| Restore | `pod.snapshot_restore(name)` | `pod.snapshotRestore(name)` | Restore checkpoint |
| List | `pod.snapshot_list()` | `pod.snapshotList()` | List all snapshots |
| Destroy | `pod.snapshot_destroy(name)` | `pod.snapshotDestroy(name)` | Delete snapshot |
| **Lifecycle** | | | |
| Start | `pod.start()` | `pod.start()` | Start in background |
| Start display | `pod.start_display()` | `pod.startDisplay()` | Start + return noVNC URL |
| Stop | `pod.stop()` | `pod.stop()` | Stop gracefully |
| Restart | `pod.restart()` | `pod.restart()` | Stop + start |
| Lock | `pod.lock()` | `pod.lock()` | Freeze pod state |
| Unlock | `pod.unlock()` | `pod.unlock()` | Resume frozen pod |
| Kill | `pod.kill()` | `pod.kill()` | Terminate + rollback |
| Destroy | `pod.destroy()` | `pod.destroy()` | Remove pod entirely |
| **Info** | | | |
| Status | `pod.status()` | `pod.status()` | Pod status and resources |
| Logs | `pod.logs()` | `pod.logs()` | Pod output logs |
| Info | `pod.info()` | `pod.info()` | Pod info as dict |
| Display URL | `pod.display_url` | `pod.displayUrl` | noVNC URL (property) |
| IP address | `pod.ip` | `pod.ip` | Pod IP (property) |
| Exists | `pod.exists()` | `pod.exists()` | Check if pod exists |
| **Cleanup** | | | |
| GC | `Pod.gc()` | `Pod.gc()` | Clean orphaned resources |

### Screening Functions

| Function | Python | TypeScript | Description |
|----------|--------|-----------|-------------|
| Screen text | `screen(text)` | `screen(text)` | Check for injection, credentials, PII, exfiltration |
| Screen API body | `screen_api(body)` | `screenApi(body)` | Parse JSON and screen message content |
| Screen file | `screen_file(path)` | `screenFile(path)` | Screen file contents |

All screening functions return:
```
{ matched: bool, category: str|null, pattern: str|null, fragment: str|null }
```

## More Examples

See [SDK Examples](../sdk/EXAMPLES.md) for 18 comprehensive usage patterns including CI/CD, multi-model comparison, Ollama, batch processing, Jupyter workflows, and full governed pipelines.

---

Copyright 2026 Xtellix Inc. All rights reserved. Licensed under BSL 1.1.
