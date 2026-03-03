# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in envpod, please report it responsibly.

**Email:** security@envpod.dev

- **Do not** open a public GitHub issue for security vulnerabilities
- Include a clear description and steps to reproduce
- We will acknowledge receipt within **48 hours**
- We target a patch within **7 days** for confirmed vulnerabilities

## Scope

The following are in scope for security reports:

- Namespace escape (PID, mount, network, user, UTS)
- OverlayFS sandbox breakout (writes reaching host filesystem)
- Vault decryption or key extraction (ChaCha20-Poly1305 bypass)
- Vault proxy credential leakage (agent accessing real API keys)
- Audit log tampering or bypass
- Seccomp-BPF filter bypass
- cgroups v2 resource limit escape
- DNS filtering bypass or data exfiltration via DNS tunneling
- Privilege escalation within a pod
- Pod-to-pod isolation breach
- Remote control command injection

## Out of Scope

- Vulnerabilities requiring root access on the host (envpod requires root to operate)
- Denial of service via resource exhaustion within a pod's configured limits
- Attacks requiring physical access to the machine
- Issues in upstream dependencies (report these to the upstream project)

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |

## Disclosure Policy

We follow coordinated disclosure. Once a fix is released, we will:

1. Credit the reporter (unless anonymity is requested)
2. Publish a security advisory on GitHub
3. Release a patched version

## Contact

- **Security:** security@envpod.dev
- **General:** mark@envpod.dev
