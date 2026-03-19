"""Auto-install envpod binary if not found."""

import os
import shutil
import subprocess
import sys


INSTALL_URL = "https://envpod.dev/install.sh"


def ensure_installed() -> str:
    """Ensure envpod binary is available. Auto-install if missing.

    Returns:
        Path to the envpod binary.
    """
    binary = shutil.which("envpod")
    if binary:
        return binary

    print("envpod binary not found. Installing...")
    print()

    try:
        subprocess.run(
            ["bash", "-c", f"curl -fsSL {INSTALL_URL} | sudo bash"],
            check=True
        )
    except subprocess.CalledProcessError:
        print(
            "Auto-install failed. Install manually:",
            f"  curl -fsSL {INSTALL_URL} | sudo bash",
            sep="\n",
            file=sys.stderr
        )
        raise RuntimeError("envpod installation failed")

    binary = shutil.which("envpod")
    if not binary:
        raise RuntimeError("envpod installed but not found in PATH")

    return binary
