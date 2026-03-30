# Health Checks

envpod monitors your pod's health and automatically restarts it when something goes wrong. One health check per pod — either an HTTP endpoint or a shell command.

## Quick Start

```yaml
# pod.yaml — HTTP health check
health:
  endpoint: /health
  port: 8080
  interval: 30
  retries: 3
  action: restart
```

```yaml
# pod.yaml — Process health check
health:
  command: "pgrep python3"
  interval: 15
  retries: 2
  action: restart
```

## How It Works

Health checks run from the **host**, not inside the pod. A crashed pod can't lie about being healthy.

```
Every 30 seconds:
  HTTP check → curl http://<pod-ip>:8080/health
    → 200 OK? → healthy
    → timeout/error? → failure count +1
    → 3 consecutive failures → restart pod

  OR

  Command check → envpod run <pod> -- pgrep python3
    → exit 0? → healthy
    → exit 1? → failure count +1
    → 2 consecutive failures → restart pod
```

## Configuration

| Field | Default | Description |
|---|---|---|
| `endpoint` | — | HTTP GET path (e.g., `/health`). Returns 200 = healthy. |
| `port` | 80 | Port for HTTP check |
| `command` | — | Shell command inside pod. Exit 0 = healthy. |
| `interval` | 30 | Seconds between checks |
| `timeout` | 5 | Seconds before check times out |
| `retries` | 3 | Consecutive failures before action triggers |
| `action` | restart | What to do: `restart`, `freeze`, or `alert` |
| `grace_period` | 10 | Seconds to wait for graceful shutdown before SIGKILL |

Use `endpoint` OR `command`, not both.

## Check Types

### HTTP Health Check

Best for web servers, APIs, and any service with an HTTP endpoint:

```yaml
health:
  endpoint: /health
  port: 9500
  interval: 30
  timeout: 5
  retries: 3
  action: restart
```

**What it catches:** app crashed, app hung, app returning errors, port not listening, deadlocks, OOM killed.

### Command Health Check

Best for background workers, daemons, and services without HTTP:

```yaml
health:
  command: "pgrep -f 'python3 worker.py'"
  interval: 15
  retries: 2
  action: restart
```

**What it catches:** process crashed, process not running, OOM killed.

**Useful commands:**
```yaml
command: "pgrep python3"                    # process running?
command: "ss -tlnp | grep :8080"            # port listening?
command: "redis-cli ping | grep PONG"       # service responding?
command: "curl -sf http://localhost:8080/"   # internal HTTP check
```

## Actions

| Action | What happens |
|---|---|
| `restart` | SIGTERM → wait grace_period → SIGKILL → restart pod |
| `freeze` | Halt pod via cgroup freezer. State preserved for investigation. |
| `alert` | Log to audit trail only. No pod action. |

### Graceful Shutdown

When `restart` triggers:

```
1. SIGTERM sent to pod process
2. Wait grace_period seconds (default: 10)
3. If still running → SIGKILL
4. Pod restarted with same config
5. Health checks resume
```

```yaml
health:
  grace_period: 30    # 30 seconds for services to shut down cleanly
```

## Audit Trail

Every health event is logged:

```bash
envpod audit my-pod
```

| Event | Meaning |
|---|---|
| `health_check_pass` | Check passed (after previous failures) |
| `health_check_fail` | Check failed (includes failure count) |
| `health_restart` | Pod restarted by health check |
| `health_freeze` | Pod frozen by health check |
| `health_alert` | Alert logged |

## Examples

### Static Website

```yaml
name: my-site
health:
  endpoint: /
  port: 8080
  interval: 30
  retries: 3
  action: restart
setup:
  - "apt-get update && apt-get install -y python3"
start_command: ["bash", "-c", "python3 -m http.server 8080"]
```

### Flask API

```yaml
name: my-api
health:
  endpoint: /health
  port: 5000
  interval: 30
  retries: 3
  action: restart
  grace_period: 15
setup:
  - "apt-get update && apt-get install -y python3 python3-pip"
  - "pip install flask"
start_command: ["bash", "-c", "cd /opt/my-api && python3 app.py"]
```

### Background Worker

```yaml
name: worker
health:
  command: "pgrep -f worker.py"
  interval: 15
  retries: 2
  action: restart
start_command: ["bash", "-c", "cd /opt/app && python3 worker.py"]
```

### Ollama Server

```yaml
name: ollama
health:
  endpoint: /
  port: 11434
  interval: 30
  retries: 5
  action: restart
  grace_period: 15
devices:
  gpu: true
start_command: ["bash", "-c", "ollama serve"]
```

## CE vs Premium

| Feature | CE (free) | Premium |
|---|---|---|
| Single health check | Yes | Yes |
| HTTP endpoint check | Yes | Yes |
| Command check | Yes | Yes |
| Auto-restart on failure | Yes | Yes |
| Graceful shutdown | Yes | Yes |
| Audit trail | Yes | Yes |
| Multiple checks per pod | — | Yes |
| Per-service recovery (not whole pod) | — | Yes |
| Recovery action sequences | — | Yes |
| Live add/remove checks at runtime | — | Yes |
| Agent self-registers health checks | — | Yes |
| Pause/resume (maintenance mode) | — | Yes |
| Notifications (Slack, webhook) | — | Yes |
| Scorecard integration | — | Yes |

CE gives you reliable health monitoring for one service per pod. Premium lets you monitor multiple services independently, restart individual services without killing the pod, and get notified when things go wrong.

[Learn more about Premium →](https://envpod.dev/#pricing)
