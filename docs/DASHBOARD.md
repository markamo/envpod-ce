# Web Dashboard

`envpod dashboard` starts a browser-based fleet management UI on `localhost:9090`.

## Quick Start

```bash
# Start the dashboard (opens browser automatically)
envpod dashboard

# Custom port
envpod dashboard --port 8080

# Run as a background daemon
envpod dashboard --daemon

# Stop the daemon
envpod dashboard --stop
```

## Authentication

The dashboard generates a random session token on startup and prints it to the terminal:

```
envpod dashboard running at http://127.0.0.1:9090
  session token: a1b2c3d4e5f6...
```

All API requests require this token via `Authorization: Bearer <token>`. The token is automatically injected into the dashboard HTML pages — no manual setup needed.

This blocks:
- **CSRF attacks** — a malicious webpage cannot call `localhost:9090/api/...` without the token
- **Local process attacks** — other scripts/agents on the machine cannot manipulate your pods

The token changes on every restart. In daemon mode, the token is written to `dashboard.token` in the base directory.

## Fleet Overview

The main page shows all pods as cards with:
- Status indicator (running, stopped, frozen, created)
- Memory usage and PID count
- Change count (files modified in overlay)

Click any pod to view its detail page.

### Create Pod

Click **+ Create Pod** on the fleet page. The modal provides:

- **Pod Name** — required, must be unique
- **Preset** — 18 built-in presets grouped by category (Coding Agents, Frameworks, Browser Agents, Environments), or "Custom" for a blank config
- **CPU Cores** — override the preset default
- **Memory** — override the preset default (e.g., "8GB")
- **GPU** — toggle GPU passthrough

## Pod Detail

The detail page has action buttons and tabbed sections.

### Actions

| Button | Description |
|--------|-------------|
| **Commit** | Apply all overlay changes to the host filesystem |
| **Rollback** | Discard all overlay changes |
| **Freeze** | Pause pod execution (cgroup freeze) |
| **Resume** | Resume a frozen pod |
| **Clone** | Clone this pod to a new name (from current state) |
| **Destroy** | Permanently delete the pod and all data (triple confirmation) |

### Tabs

**Overview** — Configuration summary: type, user, network mode, DNS mode, CPU, memory, vault proxy, pod directory.

**Audit** — Append-only action log with timestamps, action types, details, and success/failure status. Most recent entries first.

**Diff** — Filesystem changes in the overlay:
- File list with type indicators: `+` Added, `~` Modified, `−` Deleted
- Expandable inline diff viewer (git-style, color-coded)
- Per-file commit buttons
- Checkbox selection for batch commit
- "Select all" toggle

**Resources** — Live cgroup metrics (auto-refreshes every 2s):
- CPU usage (seconds)
- Memory current and limit
- PID count and limit

**Snapshots** — Overlay checkpoints:
- Create with optional label
- Restore (replaces current overlay)
- Promote to reusable base pod
- Delete

**Queue** — Staged action management:
- Pending actions with approve/cancel buttons
- History of executed/cancelled actions
- Auto-refreshes every 3s

## API Reference

All endpoints require `Authorization: Bearer <token>`.

### Pod Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/pods` | List all pods |
| `POST` | `/api/v1/pods` | Create a pod |
| `GET` | `/api/v1/pods/{id}` | Pod detail |
| `DELETE` | `/api/v1/pods/{id}` | Destroy a pod |
| `POST` | `/api/v1/pods/{id}/clone` | Clone a pod |

### Pod Actions

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/v1/pods/{id}/commit` | Commit all changes |
| `POST` | `/api/v1/pods/{id}/commit-files` | Commit specific files |
| `POST` | `/api/v1/pods/{id}/rollback` | Rollback all changes |
| `POST` | `/api/v1/pods/{id}/freeze` | Freeze pod |
| `POST` | `/api/v1/pods/{id}/resume` | Resume pod |

### Monitoring

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/pods/{id}/audit` | Audit log (paginated) |
| `GET` | `/api/v1/pods/{id}/resources` | Live cgroup stats |
| `GET` | `/api/v1/pods/{id}/diff` | Filesystem diff list |
| `GET` | `/api/v1/pods/{id}/file-diff?path=...` | Inline file diff |

### Snapshots

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/pods/{id}/snapshots` | List snapshots |
| `POST` | `/api/v1/pods/{id}/snapshots` | Create snapshot |
| `POST` | `/api/v1/pods/{id}/snapshots/{snap}/restore` | Restore |
| `POST` | `/api/v1/pods/{id}/snapshots/{snap}/promote` | Promote to base |
| `DELETE` | `/api/v1/pods/{id}/snapshots/{snap}` | Delete |

### Queue

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/pods/{id}/queue` | List actions |
| `POST` | `/api/v1/pods/{id}/queue/{id}/approve` | Approve |
| `POST` | `/api/v1/pods/{id}/queue/{id}/cancel` | Cancel |

### Presets

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/presets` | List presets by category |

### Request/Response Examples

**Create Pod:**
```bash
curl -X POST http://localhost:9090/api/v1/pods \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-agent", "preset": "claude-code", "cores": 4, "memory": "8GB"}'
```

**Clone Pod:**
```bash
curl -X POST http://localhost:9090/api/v1/pods/my-agent/clone \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"new_name": "my-agent-copy"}'
```

**Destroy Pod:**
```bash
curl -X DELETE http://localhost:9090/api/v1/pods/my-agent \
  -H "Authorization: Bearer $TOKEN"
```

## Security Notes

- The dashboard binds to `127.0.0.1` only — not accessible from the network
- Session token prevents unauthorized API access from local processes and CSRF
- All destructive operations (destroy, rollback, freeze) require confirmation in the UI
- Destroy requires triple confirmation: two dialogs + typing the pod name
- The dashboard runs with the same privileges as the user who started it
