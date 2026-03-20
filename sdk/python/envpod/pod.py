"""Pod — the core abstraction for governed AI agent environments."""

import json
import subprocess
import shutil
from typing import List, Optional


class PodError(Exception):
    """Raised when an envpod command fails."""
    pass


class Pod:
    """A governed environment for an AI agent.

    Args:
        name: Pod name (must be unique).
        config: Path to pod.yaml config file (for init).
        preset: Built-in preset name (alternative to config).
        mode: "standard" (no sudo) or "full" (sudo, complete isolation).
              If None, prompts interactively on first use.
    """

    def __init__(self, name: str, config: Optional[str] = None,
                 preset: Optional[str] = None, mode: Optional[str] = None,
                 persist: bool = False):
        self.name = name
        self._config = config
        self._preset = preset
        self._mode = mode or _get_mode()
        self._initialized = False
        self._persist = persist

        # Ensure envpod binary is available
        from envpod.installer import ensure_installed
        ensure_installed()

    def __enter__(self):
        if not self.exists():
            self.init(config=self._config, preset=self._preset)
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if not self._persist:
            self.destroy()
            self.gc()
        return False

    def init(self, config: Optional[str] = None, preset: Optional[str] = None,
             verbose: bool = False, mount_cwd: bool = True) -> None:
        """Create and set up the pod.

        Args:
            config: Path to pod.yaml config file.
            preset: Built-in preset name.
            verbose: Show live setup output.
            mount_cwd: Mount current working directory into the pod (default True).
        """
        args = ["init", self.name]
        cfg = config or self._config
        pre = preset or self._preset
        if cfg:
            args.extend(["-c", cfg])
        elif pre:
            args.extend(["--preset", pre])
        if verbose:
            args.append("--verbose")
        self._run(args)
        self._initialized = True
        self._mount_cwd = mount_cwd

    def run(self, command: str, root: bool = False,
            env: Optional[dict] = None, capture: bool = False,
            display: bool = False, audio: bool = False,
            background: bool = False) -> Optional[str]:
        """Run a command inside the pod.

        Args:
            command: Shell command to execute.
            root: Run as root inside the pod.
            env: Additional environment variables.
            capture: If True, capture and return stdout instead of printing.
            display: Enable display forwarding (Wayland/X11).
            audio: Enable audio forwarding (PipeWire/PulseAudio).
            background: Run in background (detached).

        Returns:
            stdout as string if capture=True, else None.
        """
        args = ["run", self.name]
        if root:
            args.append("--root")
        if getattr(self, '_mount_cwd', True):
            args.append("-w")
        if display:
            args.append("-d")
        if audio:
            args.append("-a")
        if background:
            args.append("-b")
        if env:
            for k, v in env.items():
                args.extend(["--env", f"{k}={v}"])
        args.append("--")
        args.extend(["sh", "-c", command])
        return self._run(args, capture=capture)

    def run_script(self, code: str, interpreter: str = "python3",
                   root: bool = False, env: Optional[dict] = None,
                   capture: bool = False) -> Optional[str]:
        """Run inline code inside the pod.

        Writes code to a temp file in the overlay, executes it, cleans up.

        Args:
            code: Source code string to execute.
            interpreter: Interpreter to use (python3, node, bash, etc.)
            root: Run as root inside the pod.
            env: Additional environment variables.
            capture: If True, capture and return stdout.
        """
        import base64
        encoded = base64.b64encode(code.encode()).decode()
        shell_cmd = (
            f'echo "{encoded}" | base64 -d > /tmp/.envpod-script && '
            f'{interpreter} /tmp/.envpod-script && '
            f'rm -f /tmp/.envpod-script'
        )
        return self.run(shell_cmd, root=root, env=env, capture=capture)

    def run_file(self, path: str, interpreter: Optional[str] = None,
                 root: bool = False, env: Optional[dict] = None,
                 capture: bool = False) -> Optional[str]:
        """Copy a local file into the pod and run it.

        Args:
            path: Path to local file.
            interpreter: Interpreter (auto-detected from extension if not set).
            root: Run as root inside the pod.
            env: Additional environment variables.
            capture: If True, capture and return stdout.
        """
        import os
        if not os.path.exists(path):
            raise PodError(f"file not found: {path}")

        with open(path) as f:
            code = f.read()

        if interpreter is None:
            ext = os.path.splitext(path)[1].lower()
            interpreter = {
                '.py': 'python3',
                '.js': 'node',
                '.ts': 'npx tsx',
                '.sh': 'bash',
                '.rb': 'ruby',
                '.pl': 'perl',
            }.get(ext, 'bash')

        return self.run_script(code, interpreter=interpreter, root=root,
                               env=env, capture=capture)

    def mount(self, path: str, readonly: bool = True) -> None:
        """Mount a host directory into the pod (COW isolated).

        Args:
            path: Host path to mount (also the mount point inside the pod).
            readonly: If True, mount as read-only.
        """
        args = ["mount", self.name, path]
        if readonly:
            args.append("--readonly")
        self._run(args)

    def inject(self, local_path: str, pod_path: str = "/tmp/",
               executable: bool = False) -> None:
        """Copy a local file into the pod's overlay.

        Args:
            local_path: Path to file on host.
            pod_path: Destination path inside pod (directory or full path).
            executable: If True, chmod +x after copying.
        """
        import os
        import base64

        if not os.path.exists(local_path):
            raise PodError(f"file not found: {local_path}")

        with open(local_path, "rb") as f:
            encoded = base64.b64encode(f.read()).decode()

        basename = os.path.basename(local_path)
        if pod_path.endswith("/"):
            dest = f"{pod_path}{basename}"
        else:
            dest = pod_path

        cmd = f'echo "{encoded}" | base64 -d > {dest}'
        if executable:
            cmd += f' && chmod +x {dest}'

        self.run(cmd, root=True)

    def diff(self, all_changes: bool = False, json_output: bool = False) -> str:
        """Show filesystem changes in the pod's overlay.

        Returns:
            Diff output as string.
        """
        args = ["diff", self.name]
        if all_changes:
            args.append("--all")
        if json_output:
            args.append("--json")
        return self._run(args, capture=True)

    def commit(self, *paths: str, exclude: Optional[List[str]] = None,
               output: Optional[str] = None, rollback_rest: bool = False) -> None:
        """Commit overlay changes to the host filesystem.

        Args:
            paths: Specific paths to commit (commits all if empty).
            exclude: Paths to exclude from commit.
            output: Export to this directory instead of host filesystem.
            rollback_rest: Rollback all uncommitted changes after commit.
        """
        args = ["commit", self.name]
        if rollback_rest:
            args.append("--rollback-rest")
        if output:
            args.extend(["--output", output])
        if exclude:
            for e in exclude:
                args.extend(["--exclude", e])
        args.extend(paths)
        self._run(args)

    def rollback(self) -> None:
        """Discard all overlay changes."""
        self._run(["rollback", self.name])

    def audit(self, security: bool = False, json_output: bool = False) -> str:
        """Show audit log or run security analysis.

        Returns:
            Audit output as string.
        """
        args = ["audit", self.name]
        if security:
            args.append("--security")
        if json_output:
            args.append("--json")
        return self._run(args, capture=True)

    def status(self) -> str:
        """Show pod status and resource usage."""
        return self._run(["status", self.name], capture=True)

    def start(self) -> None:
        """Start the pod in background."""
        self._run(["start", self.name])

    def stop(self) -> None:
        """Stop the pod."""
        self._run(["stop", self.name])

    def lock(self) -> None:
        """Freeze the pod."""
        self._run(["lock", self.name])

    def unlock(self) -> None:
        """Resume a frozen pod."""
        self._run(["unlock", self.name])

    def restart(self) -> None:
        """Restart the pod (stop + start)."""
        self._run(["restart", self.name])

    def kill(self) -> None:
        """Terminate pod processes and rollback changes."""
        self._run(["kill", self.name])

    def destroy(self) -> None:
        """Remove the pod entirely."""
        try:
            self._run(["destroy", self.name])
        except PodError:
            pass  # Pod may already be destroyed

    def resize(self, cpus: Optional[float] = None, memory: Optional[str] = None,
               tmp_size: Optional[str] = None, max_pids: Optional[int] = None,
               gpu: Optional[bool] = None) -> None:
        """Resize pod resources (live if running, config if stopped)."""
        args = ["resize", self.name]
        if cpus is not None:
            args.extend(["--cpus", str(cpus)])
        if memory is not None:
            args.extend(["--memory", memory])
        if tmp_size is not None:
            args.extend(["--tmp-size", tmp_size])
        if max_pids is not None:
            args.extend(["--max-pids", str(max_pids)])
        if gpu is not None:
            args.extend(["--gpu", str(gpu).lower()])
        self._run(args)

    def vault_set(self, key: str, value: str) -> None:
        """Store a secret in the pod's vault."""
        self._run(["vault", self.name, "set", key, value])

    def init_with_base(self, config: Optional[str] = None, preset: Optional[str] = None,
                        base_name: Optional[str] = None, verbose: bool = False) -> None:
        """Create pod and save as base for fast cloning.

        Args:
            config: Path to pod.yaml config file.
            preset: Built-in preset name.
            base_name: Name for the base (defaults to pod name).
            verbose: Show live setup output.
        """
        args = ["init", self.name, "--create-base"]
        if base_name:
            args[-1] = f"--create-base={base_name}"
        cfg = config or self._config
        pre = preset or self._preset
        if cfg:
            args.extend(["-c", cfg])
        elif pre:
            args.extend(["--preset", pre])
        if verbose:
            args.append("--verbose")
        self._run(args)
        self._initialized = True

    @staticmethod
    def clone(source, name: str, mode: Optional[str] = None) -> 'Pod':
        """Clone a pod from a base (fast — ~8ms).

        Args:
            source: Source pod/base name (str) or Pod instance.
            name: Name for the new cloned pod.
            mode: Isolation mode (standard/full).

        Returns:
            A new Pod instance for the clone.
        """
        source_name = source.name if isinstance(source, Pod) else source
        pod = Pod(name, mode=mode)
        pod._run(["clone", source_name, name])
        pod._initialized = True
        return pod

    def detach(self) -> 'Pod':
        """Mark pod as persistent — won't auto-destroy on context exit.

        Returns self for chaining.
        """
        self._persist = True
        return self

    def start_display(self) -> Optional[str]:
        """Start the pod with web display (noVNC) and return the URL.

        Starts the pod in background with desktop environment.
        Returns the noVNC URL for browser access.
        """
        self.start()
        url = self.display_url
        if url:
            print(f"  Desktop → {url}")
        return url

    def snapshot_create(self, name: Optional[str] = None) -> None:
        """Create a snapshot of the pod's current overlay state."""
        args = ["snapshot", self.name, "create"]
        if name:
            args.extend(["--name", name])
        self._run(args)

    def snapshot_restore(self, name: str) -> None:
        """Restore a snapshot."""
        self._run(["snapshot", self.name, "restore", name])

    def snapshot_list(self) -> str:
        """List all snapshots."""
        return self._run(["snapshot", self.name, "ls"], capture=True)

    def snapshot_destroy(self, name: str) -> None:
        """Delete a snapshot."""
        self._run(["snapshot", self.name, "destroy", name])

    def logs(self) -> str:
        """Show pod output logs."""
        return self._run(["logs", self.name], capture=True)

    def info(self) -> dict:
        """Get pod info as a dict (name, status, IP, display URL, etc.)."""
        try:
            result = self._run(["ls", "--json"], capture=True)
            pods = json.loads(result)
            for p in pods:
                if p.get("name") == self.name:
                    return p
        except (PodError, json.JSONDecodeError):
            pass
        return {}

    @property
    def display_url(self) -> Optional[str]:
        """Get the noVNC display URL if web display is enabled."""
        info = self.info()
        return info.get("display_url") or info.get("novnc_url")

    @property
    def ip(self) -> Optional[str]:
        """Get the pod's IP address."""
        info = self.info()
        return info.get("ip") or info.get("pod_ip")

    @staticmethod
    def disposable(base, name: str, command: str,
                   commit_paths: Optional[List[str]] = None,
                   output: Optional[str] = None,
                   mode: Optional[str] = None,
                   root: bool = False,
                   env: Optional[dict] = None) -> Optional[str]:
        """Clone from base, run command, optionally save results, destroy.

        The fastest governed execution — ~8ms clone, run, commit, destroy.
        Equivalent to `docker run --rm` but with governance.

        Args:
            base: Base pod name to clone from.
            name: Name for the disposable pod.
            command: Shell command to run.
            commit_paths: Paths to commit (None = discard everything).
            output: Export committed files to this directory instead of host.
            mode: Isolation mode (standard/full).
            root: Run as root.
            env: Environment variables.

        Returns:
            Diff output as string if commit_paths is set, else None.
        """
        base_name = base.name if isinstance(base, Pod) else base
        pod = Pod.clone(base_name, name, mode=mode)
        try:
            pod.run(command, root=root, env=env)
            if commit_paths:
                diff = pod.diff()
                pod.commit(*commit_paths, output=output, rollback_rest=True)
                return diff
            return None
        finally:
            pod.destroy()

    @staticmethod
    def gc() -> None:
        """Clean up orphaned resources (iptables, cgroups, netns)."""
        binary = shutil.which("envpod")
        if binary:
            try:
                cmd = ["sudo", binary, "gc"] if _get_mode() == "full" else [binary, "gc"]
                subprocess.run(cmd, capture_output=True)
            except subprocess.CalledProcessError:
                pass

    def exists(self) -> bool:
        """Check if the pod already exists."""
        try:
            result = self._run(["ls", "--json"], capture=True)
            pods = json.loads(result)
            return any(p.get("name") == self.name for p in pods)
        except (PodError, json.JSONDecodeError):
            return False

    def _run(self, args: List[str], capture: bool = False) -> Optional[str]:
        """Execute an envpod command."""
        cmd = self._build_cmd(args)
        try:
            if capture:
                result = subprocess.run(
                    cmd, capture_output=True, text=True, check=True
                )
                return result.stdout
            else:
                subprocess.run(cmd, check=True)
                return None
        except subprocess.CalledProcessError as e:
            stderr = e.stderr or ""
            raise PodError(f"envpod {' '.join(args)} failed (exit {e.returncode}): {stderr.strip()}")

    def _build_cmd(self, args: List[str]) -> List[str]:
        """Build the full command with sudo if needed."""
        binary = shutil.which("envpod")
        if not binary:
            raise PodError("envpod binary not found. Install: curl -fsSL https://envpod.dev/install.sh | sudo bash")
        if self._mode == "full":
            return ["sudo", binary] + args
        return [binary] + args


def _get_mode() -> str:
    """Get the isolation mode from config or prompt interactively."""
    import os
    config_dir = os.path.expanduser("~/.config/envpod")
    config_file = os.path.join(config_dir, "sdk.json")

    # Check saved preference
    if os.path.exists(config_file):
        try:
            with open(config_file) as f:
                config = json.load(f)
                return config.get("mode", "full")
        except (json.JSONDecodeError, IOError):
            pass

    # Check environment variable
    env_mode = os.environ.get("ENVPOD_MODE")
    if env_mode in ("standard", "full"):
        return env_mode

    # Interactive prompt
    print()
    print("  envpod — choose isolation mode:")
    print("    [1] Standard — full governance, no sudo needed")
    print("        (no cgroup limits, no network namespace)")
    print("    [2] Full — complete isolation + governance (requires sudo)")
    print("        (cgroup limits, network namespace, DNS filtering)")
    print()

    try:
        choice = input("  Choice [1/2]: ").strip()
    except (EOFError, KeyboardInterrupt):
        choice = "2"

    mode = "standard" if choice == "1" else "full"

    # Save preference
    try:
        os.makedirs(config_dir, exist_ok=True)
        with open(config_file, "w") as f:
            json.dump({"mode": mode}, f)
        print(f"  Saved to {config_file}")
    except IOError:
        pass

    return mode
