#!/bin/bash
set -e

# Enable IP forwarding (Docker disables this inside containers)
if [ -f /proc/sys/net/ipv4/ip_forward ]; then
    echo 1 > /proc/sys/net/ipv4/ip_forward
fi

# Welcome message
cat << 'EOF'

  ╔═══════════════════════════════════════════════════════╗
  ║              envpod — test environment                ║
  ║       Zero-trust governance for AI agents             ║
  ╚═══════════════════════════════════════════════════════╝

  Quick start:

    envpod init my-agent -c /opt/envpod/examples/basic-internet.yaml
    envpod run my-agent -- bash

  Governance demo:

    # Inside the pod, make some changes:
    echo "data" > /tmp/output.txt

    # Exit the pod, then review:
    envpod diff my-agent

    # Approve or discard:
    envpod commit my-agent
    envpod rollback my-agent

  Credential vault:

    envpod vault my-agent set API_KEY
    envpod run my-agent -- bash
    # $API_KEY is available but never in config files

  Example configs:  ls /opt/envpod/examples/
  Full docs:        https://github.com/markamo/envpod-ce
  Website:          https://envpod.dev

EOF

# Run whatever command was passed (default: bash)
exec "$@"
