"""Screening helpers — check text for injection, credentials, PII, exfiltration."""

import json
import subprocess
import shutil
from typing import Optional


def screen(text: str, json_output: bool = False) -> dict:
    """Screen text for prompt injection, credential exposure, PII, and exfiltration.

    Args:
        text: Text to screen.
        json_output: Always returns dict regardless of this flag.

    Returns:
        Dict with keys: matched (bool), category, pattern, fragment.
    """
    binary = shutil.which("envpod")
    if not binary:
        raise RuntimeError("envpod binary not found")

    result = subprocess.run(
        [binary, "screen", "--json"],
        input=text, capture_output=True, text=True
    )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {"matched": False, "category": None, "pattern": None, "fragment": None}


def screen_api(body: str) -> dict:
    """Screen an API request body (JSON) for injection, credentials, PII.

    Args:
        body: JSON string of the API request body.

    Returns:
        Dict with keys: matched (bool), category, pattern, fragment.
    """
    binary = shutil.which("envpod")
    if not binary:
        raise RuntimeError("envpod binary not found")

    result = subprocess.run(
        [binary, "screen", "--api", "--json"],
        input=body, capture_output=True, text=True
    )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {"matched": False, "category": None, "pattern": None, "fragment": None}


def screen_file(path: str) -> dict:
    """Screen a file's contents.

    Args:
        path: Path to file to screen.

    Returns:
        Dict with keys: matched (bool), category, pattern, fragment.
    """
    binary = shutil.which("envpod")
    if not binary:
        raise RuntimeError("envpod binary not found")

    result = subprocess.run(
        [binary, "screen", "--json", "--file", path],
        capture_output=True, text=True
    )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {"matched": False, "category": None, "pattern": None, "fragment": None}
