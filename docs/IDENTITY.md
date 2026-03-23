# Pod & Agent Identity (Premium)

Every pod gets a cryptographic identity. Agents within a pod get their own UUID, scoped vault access, and JWT tokens. This is the foundation for AuthZ, OPA policy, and multi-agent governance.

## Pod Identity

Generated automatically at `envpod init`. Each pod gets an Ed25519 keypair.

```
{pod_dir}/identity/
  pod.key        # Ed25519 private key (mode 0600)
  pod.pub        # Ed25519 public key
  pod.json       # {pod_id, pod_name, public_key_hex, created_at}
  agents.json    # Agent registry
```

View pod identity:

```bash
envpod agent my-pod identity
# Pod Identity
# pod_id:     a1b2c3d4-...
# public_key: 3f8a9b2c...
```

## Agent Identity

Agents are declared in pod.yaml or registered at runtime.

### Declarative (pod.yaml)

```yaml
identity:
  agents:
    - name: coder
      capabilities: [read, write, execute, network]
      vault_keys: [ANTHROPIC_API_KEY, GITHUB_TOKEN]
    - name: reviewer
      capabilities: [read]
      vault_keys: [OPENAI_API_KEY]
    - name: deployer
      capabilities: [read, execute, network]
      vault_keys: [AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY]
```

Agents declared in pod.yaml are auto-registered at `envpod init`.

### Runtime Registration

```bash
envpod agent my-pod register tester \
  --capabilities read,write \
  --vault-keys TEST_API_KEY

envpod agent my-pod list
envpod agent my-pod remove tester
```

Runtime-registered agents persist across restarts. Config-declared and runtime-registered agents coexist — sync adds missing config agents without removing runtime ones.

## Running as an Agent

```bash
envpod run my-pod --agent coder -- bash
```

This injects the following environment variables into the pod:

| Variable | Value |
|---|---|
| `ENVPOD_POD_ID` | Pod UUID |
| `ENVPOD_POD_NAME` | Pod name |
| `ENVPOD_POD_PUBLIC_KEY` | Pod Ed25519 public key (hex) |
| `ENVPOD_AGENT_ID` | Agent UUID |
| `ENVPOD_AGENT_NAME` | Agent name |
| `ENVPOD_AGENT_TOKEN` | JWT signed by pod key |

Without `--agent`, only `ENVPOD_POD_ID`, `ENVPOD_POD_NAME`, and `ENVPOD_POD_PUBLIC_KEY` are set. All vault keys are injected (backward compatible).

## JWT Token

```bash
envpod agent my-pod token coder
```

Prints a JWT signed by the pod's Ed25519 key. Default TTL: 24 hours.

### Claims

```json
{
  "iss": "envpod",
  "sub": "pod:my-pod/agent:coder",
  "pod_id": "a1b2c3d4-...",
  "pod_name": "my-pod",
  "agent_id": "e5f6g7h8-...",
  "agent_name": "coder",
  "capabilities": ["read", "write", "execute", "network"],
  "vault_keys": ["ANTHROPIC_API_KEY", "GITHUB_TOKEN"],
  "iat": 1711234567,
  "exp": 1711320967
}
```

The `sub` field uses the hierarchical format `pod:<name>/agent:<name>` for use in OPA policies.

## Vault Scoping

When running as an agent with `vault_keys` specified, only those keys are injected:

```bash
# coder gets ANTHROPIC_API_KEY + GITHUB_TOKEN only
envpod run my-pod --agent coder -- env | grep API
# ANTHROPIC_API_KEY=sk-ant-...

# reviewer gets OPENAI_API_KEY only
envpod run my-pod --agent reviewer -- env | grep API
# OPENAI_API_KEY=sk-...
```

If `vault_keys` is empty, all vault secrets are injected (backward compatible).

## Audit Attribution

Every action records which agent performed it:

```bash
envpod audit my-pod --json | jq '.[-1]'
# {
#   "timestamp": "2026-03-22T...",
#   "pod_name": "my-pod",
#   "action": "commit",
#   "detail": "3 file(s)",
#   "success": true,
#   "agent": "coder"
# }
```

Pod-level actions (init, destroy) have `agent: null`.

## Security Audit

`envpod audit --security` reports finding I-07 for agents with unrestricted vault access:

```
I-07 [NOTE] Agent has unrestricted vault access
  Agent 'tester' has no vault_keys restriction — all vault secrets will be injected.
```

## Dashboard

The pod detail overview tab shows:
- Pod public key
- Agent table: name, ID (truncated), capabilities, vault keys

## OPA Integration (Planned)

JWT claims feed directly into OPA policy evaluation:

```rego
package envpod

default allow = false

# Only agents with "write" capability can commit
allow {
    input.agent.capabilities[_] == "write"
    input.action == "commit"
}

# Only deployer can access AWS keys
allow {
    input.agent.name == "deployer"
    input.vault_key == "AWS_ACCESS_KEY_ID"
}
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `envpod agent <pod> register <name> [--capabilities ...] [--vault-keys ...]` | Register agent |
| `envpod agent <pod> list [--json]` | List agents |
| `envpod agent <pod> remove <name>` | Remove agent |
| `envpod agent <pod> token <name> [--ttl 86400]` | Print JWT |
| `envpod agent <pod> identity` | Show pod identity |
| `envpod run <pod> --agent <name> -- <cmd>` | Run as agent |
