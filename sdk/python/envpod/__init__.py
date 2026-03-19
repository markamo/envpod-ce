"""envpod — Python SDK for the zero-trust governance layer for AI agents.

Thin wrapper around the envpod CLI binary. Every method calls the binary
via subprocess — no reimplementation of isolation logic.

Usage:
    from envpod import Pod

    with Pod("my-agent", config="examples/coding-agent.yaml") as pod:
        result = pod.run("python3 agent.py")
        diff = pod.diff()
        pod.commit("src/", rollback_rest=True)

    # Or without context manager:
    pod = Pod("my-agent")
    pod.init(config="examples/coding-agent.yaml")
    pod.run("bash")
    pod.destroy()
"""

from envpod.pod import Pod
from envpod.screen import screen, screen_api, screen_file
from envpod.installer import ensure_installed

__version__ = "0.1.1"
__all__ = ["Pod", "screen", "screen_api", "screen_file", "ensure_installed"]
