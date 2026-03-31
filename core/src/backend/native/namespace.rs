// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Linux namespace creation and isolated process spawning.
//!
//! Spawns the agent process inside fully isolated namespaces:
//! 1. PID namespace  — child becomes PID 1 (cannot see host PIDs)
//! 2. UTS namespace  — pod gets its own hostname
//! 3. Mount namespace + OverlayFS — COW root filesystem
//! 4. Bind-mounted /proc, /dev, /sys for the process to function
//! 5. pivot_root to fully switch the root to the overlay
//! 6. seccomp-BPF syscall filter (allowlist of ~130 safe syscalls)
//!
//! The process cannot see or modify the host filesystem — all writes
//! land in the overlay's upper layer.

use std::io::{IsTerminal, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use nix::mount::MsFlags;
use nix::sched::CloneFlags;

use crate::config::SystemAccess;

/// Spawn a process inside an isolated mount namespace with overlayfs.
///
/// The child process will:
/// 1. Write itself into the pod's cgroup (if cgroup_procs is Some)
/// 2. Join an existing network namespace (if netns_path is Some)
/// 3. Unshare into new Mount + PID + UTS namespaces
/// 4. Set hostname to `pod_name` (UTS isolation)
/// 5. Mount overlayfs (host root as lower, pod upper as writable layer)
/// 6. Bind-mount /proc, /dev, /sys into the overlay
/// 7. pivot_root into the overlay merged view
/// 8. Apply seccomp-BPF syscall filter
///
/// If `log_file` is Some, stdout and stderr are tee'd to the given path
/// (output still appears on the terminal in real time).
///
/// Returns the child PID (in the host PID namespace).
#[allow(clippy::too_many_arguments)]
pub fn spawn_isolated(
    command: &[String],
    lower_dirs: &[PathBuf],
    upper: &Path,
    work: &Path,
    merged: &Path,
    cgroup_procs: Option<&Path>,
    netns_path: Option<&Path>,
    pod_name: &str,
    log_file: Option<&Path>,
    env_vars: Option<&std::collections::HashMap<String, String>>,
    seccomp_profile: super::seccomp::SeccompProfile,
    shm_size: Option<u64>,
    tmp_size: u64,
    rootfs: &Path,
    mount_entries: &[(PathBuf, PathBuf, bool)],
    devices: crate::config::DevicesConfig,
    system_access: SystemAccess,
    quiet_log: Option<PathBuf>,
    run_as: Option<(u32, u32)>,
) -> Result<u32> {
    anyhow::ensure!(!command.is_empty(), "command must not be empty");

    // Clone paths for move into the pre_exec closure
    let lower_dirs = lower_dirs.to_vec();
    let upper = upper.to_path_buf();
    let work = work.to_path_buf();
    let merged = merged.to_path_buf();
    let cgroup_procs = cgroup_procs.map(|p| p.to_path_buf());
    let netns_path = netns_path.map(|p| p.to_path_buf());
    let pod_name = pod_name.to_string();
    let rootfs = rootfs.to_path_buf();
    let mount_entries = mount_entries.to_vec();
    let devices = devices.clone();

    // Resolve HOME directory for non-root users before values move into closures.
    // Read from upper (has useradd changes) or rootfs (base copy) — merged
    // isn't mounted yet at this point.
    let user_home = if let Some((uid, _gid)) = run_as {
        let pod_dir = upper.parent().unwrap_or(Path::new("/"));
        let upper_passwd = pod_dir.join("upper/etc/passwd");
        let rootfs_passwd = pod_dir.join("rootfs/etc/passwd");
        let passwd_path = if upper_passwd.exists() { upper_passwd } else { rootfs_passwd };
        if let Ok(contents) = std::fs::read_to_string(&passwd_path) {
            contents
                .lines()
                .find_map(|line| {
                    let fields: Vec<&str> = line.split(':').collect();
                    if fields.len() >= 6 {
                        if let Ok(puid) = fields[2].parse::<u32>() {
                            if puid == uid {
                                return Some(fields[5].to_string());
                            }
                        }
                    }
                    None
                })
        } else {
            None
        }
    } else {
        None
    };

    // Pre-compute diagnostic path before values move into closures
    let diag_path = upper.parent().map(|p| p.join("pre_exec_error.txt"));

    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]);

    // Inject vault secrets and extra env vars.
    // Remove conflicting display/audio vars from the inherited host environment
    // so the pod only sees the protocol it's configured for.
    if let Some(vars) = env_vars {
        for (key, value) in vars {
            cmd.env(key, value);
        }
        if vars.contains_key("WAYLAND_DISPLAY") {
            cmd.env_remove("DISPLAY");
        }
        if vars.contains_key("PIPEWIRE_RUNTIME_DIR") {
            cmd.env_remove("PULSE_SERVER");
            cmd.env_remove("PULSE_COOKIE");
        }
    }

    // Set HOME for non-root users
    if let Some(ref home) = user_home {
        cmd.env("HOME", home);
    }

    // Quiet mode: redirect output directly to log file (no pipes, no threads).
    // Non-interactive + log_file: tee to both terminal and log file.
    // Interactive: use PTY relay so pod gets its own terminal without stealing ours.
    let interactive = std::io::stdin().is_terminal();

    // PTY relay: create a pseudo-terminal for the pod. The pod gets the slave
    // end and can call setsid+TIOCSCTTY safely (on its own PTY, not ours).
    // We relay I/O between the real terminal and the master end.
    let pty_master = if interactive && quiet_log.is_none() {
        match nix::pty::openpty(None, None) {
            Ok(pty) => {
                use std::os::unix::io::AsRawFd;
                let master_fd = pty.master.as_raw_fd();
                let slave_fd = pty.slave.as_raw_fd();
                // Dup slave for stdin/stdout/stderr (each needs its own fd)
                let slave_in = slave_fd;
                let slave_out = nix::unistd::dup(slave_fd).unwrap_or(slave_fd);
                let slave_err = nix::unistd::dup(slave_fd).unwrap_or(slave_fd);
                cmd.stdin(unsafe { Stdio::from_raw_fd(slave_in) });
                cmd.stdout(unsafe { Stdio::from_raw_fd(slave_out) });
                cmd.stderr(unsafe { Stdio::from_raw_fd(slave_err) });
                // Forward terminal size to the PTY
                let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
                if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut ws) } == 0 {
                    unsafe { libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws) };
                }
                // Dup master so it outlives the OwnedFd
                let master_dup = nix::unistd::dup(master_fd).unwrap_or(master_fd);
                // Keep pty alive until spawn (OwnedFd closes on drop)
                std::mem::forget(pty);
                Some(master_dup)
            }
            Err(_) => None, // fallback to inherited stdio
        }
    } else {
        None
    };

    if let Some(ref quiet_path) = quiet_log {
        let log_out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(quiet_path)
            .with_context(|| format!("open quiet log for stdout: {}", quiet_path.display()))?;
        let log_err = log_out.try_clone().context("clone quiet log for stderr")?;
        cmd.stdout(Stdio::from(log_out));
        cmd.stderr(Stdio::from(log_err));
    } else if log_file.is_some() && !interactive {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    // pre_exec runs in the child process after fork, before exec.
    // This is where we set up the isolated environment.
    //
    // IMPORTANT: Rust's Command error pipe converts Error::other() to EINVAL
    // on the parent side, losing all custom messages. We write diagnostics to
    // a file so the real error is recoverable.
    let diag_path_clone = diag_path.clone();
    unsafe {
        cmd.pre_exec(move || {
            let result = pre_exec_setup(
                &cgroup_procs,
                &netns_path,
                &pod_name,
                &lower_dirs,
                &upper,
                &work,
                &merged,
                &rootfs,
                &mount_entries,
                shm_size,
                tmp_size,
                seccomp_profile,
                &devices,
                system_access,
                run_as,
            );

            if let Err(ref e) = result {
                // Write the real error to a diagnostic file before the error
                // gets mangled by the Command error pipe serialization.
                if let Some(ref path) = diag_path_clone {
                    let _ = std::fs::write(path, format!("{e}"));
                }
                // Also try stderr (may or may not be visible)
                let _ = std::io::Write::write_all(
                    &mut std::io::stderr(),
                    format!("envpod pre_exec error: {e}\n").as_bytes(),
                );
            }

            result
        });
    }

    let child_result = cmd.spawn();

    // If spawn failed, check for diagnostic file with the real error message
    // (Rust's pre_exec error pipe converts Error::other() to EINVAL, losing
    // all custom messages — the diag file preserves the actual error).
    if child_result.is_err() {
        if let Some(ref path) = diag_path {
            if let Ok(msg) = std::fs::read_to_string(path) {
                std::fs::remove_file(path).ok();
                anyhow::bail!("pre_exec failed: {msg}");
            }
        }
    }

    let mut child = child_result.context("failed to spawn isolated process")?;
    let pid = child.id();

    // Quiet mode: stdout/stderr already redirected to log file via Stdio::from().
    // No pipes to drain — the child writes directly to the file.
    // Non-interactive + log_file: tee to both terminal and log file.
    // Interactive mode: stdout/stderr inherited, no pipes to read.
    if quiet_log.is_some() {
        // Nothing to do — child writes directly to the log file
    } else if !interactive {
        if let Some(log_path) = log_file {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
                .with_context(|| format!("open log file: {}", log_path.display()))?;

            if let Some(stdout) = child.stdout.take() {
                let mut log_clone = log.try_clone().context("clone log file for stdout")?;
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stdout);
                    for line in reader.lines().map_while(Result::ok) {
                        println!("{line}");
                        let _ = writeln!(log_clone, "{line}");
                    }
                });
            }

            if let Some(stderr) = child.stderr.take() {
                let mut log_clone = log.try_clone().context("clone log file for stderr")?;
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        eprintln!("{line}");
                        let _ = writeln!(log_clone, "{line}");
                    }
                });
            }
        }
    }

    // PTY relay: forward I/O between real stdin/stdout and the PTY master.
    // Two threads: one reads stdin → writes master, other reads master → writes stdout.
    // Threads exit when the pod exits (master EOF) or stdin closes.
    if let Some(master_fd) = pty_master {
        use std::os::unix::io::FromRawFd;

        // Set real terminal to raw mode so keystrokes pass through immediately
        let saved_raw = nix::sys::termios::tcgetattr(std::io::stdin()).ok();
        if let Some(ref t) = saved_raw {
            let mut raw = t.clone();
            nix::sys::termios::cfmakeraw(&mut raw);
            let _ = nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                &raw,
            );
        }

        let master_read = unsafe { std::fs::File::from_raw_fd(nix::unistd::dup(master_fd).unwrap_or(master_fd)) };
        let mut master_write = unsafe { std::fs::File::from_raw_fd(master_fd) };

        // stdin → master (user typing → pod)
        let stdin_thread = std::thread::spawn(move || {
            use std::io::Read;
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if std::io::Write::write_all(&mut master_write, &buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // master → stdout (pod output → user)
        let stdout_thread = std::thread::spawn(move || {
            use std::io::Read;
            let mut master = master_read;
            let mut stdout = std::io::stdout().lock();
            let mut buf = [0u8; 4096];
            loop {
                match master.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if std::io::Write::write_all(&mut stdout, &buf[..n]).is_err() {
                            break;
                        }
                        let _ = std::io::Write::flush(&mut stdout);
                    }
                }
            }
        });

        // Wait for output thread (pod exit closes master → EOF)
        let _ = stdout_thread.join();
        // stdin thread will exit when we drop master_write or stdin closes
        drop(stdin_thread); // detach — will exit on its own

        // Restore terminal from raw mode
        if let Some(ref t) = saved_raw {
            let _ = nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                t,
            );
        }
    }

    Ok(pid)
}

/// All pre_exec setup extracted into a function so we can capture errors
/// and write diagnostics before the error gets mangled by the pipe.
#[allow(clippy::too_many_arguments)]
fn pre_exec_setup(
    cgroup_procs: &Option<PathBuf>,
    netns_path: &Option<PathBuf>,
    pod_name: &str,
    lower_dirs: &[PathBuf],
    upper: &Path,
    work: &Path,
    merged: &Path,
    rootfs: &Path,
    mount_entries: &[(PathBuf, PathBuf, bool)],
    shm_size: Option<u64>,
    tmp_size: u64,
    seccomp_profile: super::seccomp::SeccompProfile,
    devices: &crate::config::DevicesConfig,
    system_access: SystemAccess,
    run_as: Option<(u32, u32)>,
) -> std::io::Result<()> {
    // 0. Add ourselves to the pod's cgroup (before namespace changes)
    if let Some(ref cg_procs) = cgroup_procs {
        std::fs::write(cg_procs, format!("{}", std::process::id()))
            .map_err(|e| std::io::Error::other(format!("cgroup write: {e}")))?;
    }

    // 0.5. Join existing network namespace (before mount ns change)
    if let Some(ref netns_path) = netns_path {
        let fd = nix::fcntl::open(
            netns_path.as_path(),
            nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .map_err(|e| std::io::Error::other(format!("open netns fd: {e}")))?;

        unsafe {
            nix::sched::setns(
                std::os::unix::io::OwnedFd::from_raw_fd(fd),
                CloneFlags::CLONE_NEWNET,
            )
            .map_err(|e| std::io::Error::other(format!("setns NEWNET: {e}")))?;
        }
    }

    // 1. New PID + Mount + UTS namespaces
    nix::sched::unshare(
        CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWUTS,
    )
    .map_err(|e| std::io::Error::other(format!(
        "unshare(NEWNS|NEWPID|NEWUTS) failed: {e}. Ensure envpod is running as root"
    )))?;

    // 2. Set hostname for UTS isolation
    nix::unistd::sethostname(pod_name)
        .map_err(|e| std::io::Error::other(format!("sethostname: {e}")))?;

    // ── PID namespace fork ──────────────────────────────────────
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if pid > 0 {
            // Child A: reaper process — only job is waitpid + _exit.
            // Close ALL file descriptors (including stdin/stdout/stderr and
            // Rust's Command error pipe). The error pipe write-end MUST be
            // closed here, otherwise cmd.spawn() in the parent blocks until
            // this reaper exits (which is when the actual command exits),
            // preventing post-spawn tasks (DNS server, control server, etc.)
            // from starting. Closing stdin/stdout/stderr also ensures this
            // process doesn't compete for the terminal with Child B.
            for fd in 0..1024 {
                libc::close(fd);
            }
            let mut status: libc::c_int = 0;
            loop {
                let r = libc::waitpid(pid, &mut status, 0);
                if r < 0 {
                    libc::_exit(0);
                }
                if libc::WIFEXITED(status) {
                    libc::_exit(libc::WEXITSTATUS(status));
                } else if libc::WIFSIGNALED(status) {
                    libc::_exit(128 + libc::WTERMSIG(status));
                } else if libc::WIFSTOPPED(status) {
                    continue;
                } else {
                    libc::_exit(0);
                }
            }
        }
    }
    // ── Child: PID 1 in the new namespace ──────────────────────

    // Create a new session — disconnects from the host's controlling terminal.
    // This prevents SIGHUP from the host's session leader (e.g., sudo's PTY relay)
    // from reaching the pod. stdin/stdout/stderr remain connected (inherited FDs).
    nix::unistd::setsid()
        .map_err(|e| std::io::Error::other(format!("setsid: {e}")))?;

    // Re-establish stdin as the controlling terminal of our new session.
    // setsid() disconnected us, but TUI apps (Node.js/Ink, vim, etc.) need
    // /dev/tty and raw mode for keyboard input. TIOCSCTTY makes stdin's
    // terminal the controlling terminal — only works if stdin is a TTY.
    // arg=1 "steals" the terminal from the parent session (requires
    // CAP_SYS_ADMIN, which we have since envpod runs as root).
    // Re-establish stdin as the controlling terminal of our new session.
    // setsid() disconnected us, but TUI apps (Node.js/Ink, vim, etc.) need
    // /dev/tty and raw mode for keyboard input.
    if unsafe { libc::isatty(0) } == 1 {
        let rc = unsafe { libc::ioctl(0, libc::TIOCSCTTY, 1) };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            eprintln!("envpod: TIOCSCTTY failed: {e}");
        }
    }

    // Re-register in cgroup (new PID after fork)
    if let Some(ref cg_procs) = cgroup_procs {
        std::fs::write(cg_procs, format!("{}", std::process::id()))
            .map_err(|e| std::io::Error::other(format!("cgroup re-write: {e}")))?;
    }

    // 3. Make all existing mounts private to prevent propagation
    nix::mount::mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )
    .map_err(|e| std::io::Error::other(format!("make mounts private: {e}")))?;

    // 4. Mount overlay — rootfs (empty skeleton) as lower, or "/" for legacy
    if rootfs.is_dir() {
        match system_access {
            SystemAccess::Safe => {
                // Safe: overlay first, then read-only bind mounts on merged.
                // System dirs are immutable — agents can't write to them.
                let rootfs_lower = vec![rootfs.to_path_buf()];
                super::overlay::mount_overlay(&rootfs_lower, upper, work, merged)?;
                bind_system_essentials(merged)?;
            }
            SystemAccess::Advanced | SystemAccess::Dangerous => {
                // Advanced/Dangerous: overlay first, then per-system-dir
                // COW overlays. Each system dir gets its own overlayfs
                // with host dir as lower and pod-specific upper, so writes
                // go to the pod's sys_upper — never to the host.
                let rootfs_lower = vec![rootfs.to_path_buf()];
                super::overlay::mount_overlay(&rootfs_lower, upper, work, merged)?;
                mount_system_cow_overlays(merged, upper)?;
            }
        }
    } else {
        super::overlay::mount_overlay(lower_dirs, upper, work, merged)?;
    }

    // 4.5. Write pod hostname to overlay's /etc/hostname for consistency
    // (UTS namespace sets hostname, but /etc/hostname still has host value)
    let etc_hostname = merged.join("etc/hostname");
    if etc_hostname.exists() || merged.join("etc").is_dir() {
        std::fs::write(&etc_hostname, format!("{pod_name}\n")).ok();
    }

    // 5. Bind-mount virtual filesystems the process needs
    bind_virtual_filesystems(merged, shm_size, tmp_size, devices)?;

    // 5.1. Mask /proc to reflect pod's cgroup limits (non-fatal)
    // Always mask sensitive /proc entries to prevent host fingerprinting,
    // even without cgroup limits (standard mode).
    if let Err(e) = super::proc_mask::mask_proc_files(merged, cgroup_procs.as_deref()) {
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            format!("envpod: /proc masking failed (non-fatal): {e}\n").as_bytes(),
        );
    }

    // 5.2. Mask GPU info in /proc and /sys when GPU is not allowed (non-fatal)
    if !devices.gpu {
        if let Err(e) = super::dev_mask::mask_gpu_info(merged) {
            let _ = std::io::Write::write_all(
                &mut std::io::stderr(),
                format!("envpod: GPU info masking failed (non-fatal): {e}\n").as_bytes(),
            );
        }
    }

    // 5.5. Bind-mount configured paths from pod.yaml into merged
    for (host_path, pod_path, readonly) in mount_entries {
        if !host_path.exists() {
            eprintln!(
                "warning: mount path {} does not exist on host — skipping",
                host_path.display()
            );
            continue;
        }
        let rel = pod_path.strip_prefix("/").unwrap_or(pod_path);
        let target = merged.join(rel);
        if host_path.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !target.exists() {
                std::fs::File::create(&target)?;
            }
        }
        nix::mount::mount(
            Some(host_path.as_path()),
            &target,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )
        .map_err(|e| std::io::Error::other(format!(
            "bind mount {} → {}: {e}",
            host_path.display(),
            target.display()
        )))?;
        if *readonly {
            nix::mount::mount(
                None::<&str>,
                &target,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
                None::<&str>,
            )
            .map_err(|e| std::io::Error::other(format!(
                "readonly remount {}: {e}",
                target.display()
            )))?;
        }
    }

    // 5.7. Bind-mount vault_env → /run/envpod/secrets.env (live secret file)
    // Gives the agent read access to secrets without a pod restart.
    // Non-fatal: falls back to env var injection if this mount fails.
    if let Some(pod_dir) = upper.parent() {
        let vault_env = pod_dir.join("vault_env");
        if vault_env.exists() {
            let target_dir = merged.join("run/envpod");
            let target = target_dir.join("secrets.env");
            let _ = std::fs::create_dir_all(&target_dir);
            if !target.exists() {
                let _ = std::fs::File::create(&target);
            }
            match nix::mount::mount(
                Some(vault_env.as_path()),
                &target,
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            ) {
                Ok(()) => {
                    // Remount read-only so agent cannot modify it
                    let _ = nix::mount::mount(
                        None::<&str>,
                        &target,
                        None::<&str>,
                        MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
                        None::<&str>,
                    );
                }
                Err(e) => {
                    let _ = std::io::stderr().write_all(
                        format!("envpod: vault_env bind mount failed (non-fatal): {e}\n").as_bytes()
                    );
                }
            }
        }
    }

    // 5.8. Bind-mount queue.sock → /run/envpod/queue.sock (agent action queue)
    // Allows agents to submit and poll actions without env vars.
    // Non-fatal: queue API is unavailable if mount fails.
    if let Some(pod_dir) = upper.parent() {
        let queue_sock = pod_dir.join("queue.sock");
        if queue_sock.exists() {
            let target_dir = merged.join("run/envpod");
            let target = target_dir.join("queue.sock");
            let _ = std::fs::create_dir_all(&target_dir);
            if !target.exists() {
                let _ = std::fs::File::create(&target);
            }
            match nix::mount::mount(
                Some(queue_sock.as_path()),
                &target,
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            ) {
                Ok(()) => {}
                Err(e) => {
                    let _ = std::io::stderr().write_all(
                        format!("envpod: queue.sock bind mount failed (non-fatal): {e}\n")
                            .as_bytes(),
                    );
                }
            }
        }
    }

    // 6. Prepare pivot_root
    let old_root = merged.join("old_root");
    std::fs::create_dir_all(&old_root)
        .map_err(|e| std::io::Error::other(format!("create old_root: {e}")))?;

    // 7. pivot_root — swap the root to our overlay
    nix::unistd::pivot_root(merged, &old_root)
        .map_err(|e| std::io::Error::other(format!("pivot_root: {e}")))?;

    // 8. chdir to /
    std::env::set_current_dir("/")
        .map_err(|e| std::io::Error::other(format!("chdir /: {e}")))?;

    // 9. Detach the old root
    nix::mount::umount2("/old_root", nix::mount::MntFlags::MNT_DETACH)
        .map_err(|e| std::io::Error::other(format!("umount old_root: {e}")))?;

    // 10. Remove old_root mountpoint (best effort)
    std::fs::remove_dir("/old_root").ok();

    // 10.5. Coredump prevention
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0) != 0 {
            return Err(std::io::Error::other("prctl PR_SET_DUMPABLE failed"));
        }
        let zero_limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::setrlimit(libc::RLIMIT_CORE, &zero_limit) != 0 {
            return Err(std::io::Error::other("setrlimit RLIMIT_CORE failed"));
        }
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(std::io::Error::other("prctl PR_SET_NO_NEW_PRIVS failed"));
        }
    }

    // 10.6. Drop to non-root user if requested (after all privileged ops, before seccomp)
    if let Some((uid, gid)) = run_as {
        // Collect supplementary groups: agent's own group + device/socket owner groups.
        // Display/audio sockets are owned by the host user; GPU devices by video/render.
        let mut groups: Vec<libc::gid_t> = vec![gid];
        let device_paths: &[&str] = &[
            // Display/audio sockets
            "/tmp/wayland-0", "/tmp/pipewire-0", "/tmp/pulse-native",
            // GPU devices
            "/dev/dri/card0", "/dev/dri/card1", "/dev/dri/card2",
            "/dev/dri/renderD128", "/dev/dri/renderD129",
        ];
        for path in device_paths {
            if let Ok(meta) = std::fs::metadata(path) {
                use std::os::unix::fs::MetadataExt;
                let device_gid = meta.gid();
                if device_gid != gid && !groups.contains(&device_gid) {
                    groups.push(device_gid);
                }
            }
        }

        unsafe {
            if libc::setgroups(groups.len(), groups.as_ptr()) != 0 {
                return Err(std::io::Error::other(format!(
                    "setgroups failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if libc::setgid(gid) != 0 {
                return Err(std::io::Error::other(format!(
                    "setgid({gid}) failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if libc::setuid(uid) != 0 {
                return Err(std::io::Error::other(format!(
                    "setuid({uid}) failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }
    }

    // 11. Apply seccomp filter — LAST step
    super::seccomp::install_filter(seccomp_profile)
        .map_err(|e| std::io::Error::other(format!("seccomp: {e}")))?;

    Ok(())
}

/// Mount per-system-dir COW overlays for advanced/dangerous mode.
///
/// Each system directory (/usr, /bin, /sbin, /lib, /lib64) gets its own
/// overlayfs with the host dir as lower and a pod-specific upper dir.
/// This allows tools like tar/nvm to chmod system directories while
/// keeping all writes in the pod's sys_upper — never touching the host.
///
/// `upper` is the main overlay upper dir; `upper.parent()` is the pod dir.
fn mount_system_cow_overlays(merged: &Path, upper: &Path) -> std::io::Result<()> {
    let pod_dir = upper.parent().ok_or_else(||
        std::io::Error::other("cannot determine pod dir from upper")
    )?;
    let sys_upper_base = pod_dir.join("sys_upper");
    let sys_work_base = pod_dir.join("sys_work");

    let dirs_to_mount: Vec<&str> = {
        let mut v: Vec<&str> = vec!["usr"];
        v.extend(super::overlay::real_system_dirs());
        v
    };

    for dir in &dirs_to_mount {
        let host_dir = Path::new("/").join(dir);
        let target = merged.join(dir);
        if !host_dir.is_dir() || !target.exists() {
            continue;
        }

        let sys_upper = sys_upper_base.join(dir);
        let sys_work = sys_work_base.join(dir);
        std::fs::create_dir_all(&sys_upper)?;
        std::fs::create_dir_all(&sys_work)?;

        let opts = format!(
            "lowerdir={},upperdir={},workdir={},index=off,metacopy=off",
            host_dir.display(),
            sys_upper.display(),
            sys_work.display(),
        );

        nix::mount::mount(
            Some("overlay"),
            &target,
            Some("overlay"),
            MsFlags::empty(),
            Some(opts.as_str()),
        )
        .map_err(|e| std::io::Error::other(format!(
            "system overlay {} → {}: {e}",
            host_dir.display(),
            target.display()
        )))?;
    }

    Ok(())
}

/// Bind-mount system essentials from the host into the overlay merged view.
///
/// Mounts `/usr` (read-only), plus any of `/bin`, `/sbin`, `/lib`, `/lib64`
/// that are real directories (not symlinks — symlinks were already created
/// in `create_rootfs()` and resolve through the overlay).
///
/// `/etc` is NOT bind-mounted — it's copied into the rootfs at init time,
/// so the overlay handles it naturally (lower=rootfs/etc copy, upper=pod writes).
/// This avoids issues with symlinks like `/etc/resolv.conf → ../run/...`.
///
/// Called AFTER the overlay mount so these bind mounts sit on top of the
/// merged view. The overlay lower layer (rootfs) only provides the directory
/// structure; the actual content comes from these bind mounts.
fn bind_system_essentials(merged: &Path) -> std::io::Result<()> {
    let dirs_to_mount: Vec<&str> = {
        let mut v: Vec<&str> = vec!["usr"];
        // Add /bin, /sbin, /lib, /lib64 only if they're real dirs (not symlinks)
        v.extend(super::overlay::real_system_dirs());
        v
    };

    for dir in &dirs_to_mount {
        let source = Path::new("/").join(dir);
        let target = merged.join(dir);
        if source.is_dir() && target.exists() {
            nix::mount::mount(
                Some(source.as_path()),
                &target,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            )
            .map_err(|e| std::io::Error::other(format!(
                "bind mount {} → {}: {e}",
                source.display(),
                target.display()
            )))?;

            // Remount read-only — system dirs should not be writable
            nix::mount::mount(
                None::<&str>,
                &target,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC,
                None::<&str>,
            )
            .ok(); // best effort — some kernels restrict remount
        }
    }

    Ok(())
}

/// Mount essential virtual filesystems into the overlay merged directory.
///
/// Without these, most programs won't function:
/// - /proc    — fresh procfs (only shows processes in this mount namespace)
/// - /dev     — device nodes (/dev/null, /dev/urandom, /dev/tty, etc.)
/// - /dev/shm — pod-private tmpfs (isolates from host, required by Chromium)
/// - /sys     — kernel/hardware info (mounted read-only for safety)
/// - /tmp     — fresh tmpfs for temporary files
///
/// `shm_size` controls the /dev/shm tmpfs size in bytes. If None, defaults to 64MB.
fn bind_virtual_filesystems(
    merged: &Path,
    shm_size: Option<u64>,
    tmp_size: u64,
    devices: &crate::config::DevicesConfig,
) -> std::io::Result<()> {
    // /proc — mount a fresh procfs instead of bind-mounting host /proc.
    // Combined with CLONE_NEWPID, this ensures the agent only sees
    // processes in its own PID namespace.
    let proc_target = merged.join("proc");
    if proc_target.exists() {
        nix::mount::mount(
            Some("proc"),
            &proc_target,
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
            None::<&str>,
        )
        .map_err(nix_to_io)?;

        // Mask sensitive procfs paths that leak host kernel state.
        // Standard container-masked paths matching OCI/runc defaults.
        // Note: /proc/[pid]/root is a kernel symlink — can't be masked
        // with bind-mount (mount follows the symlink). Instead, we mask
        // /proc/1/ entirely with an empty tmpfs.
        let masked_proc_files = [
            "proc/acpi",
            "proc/kcore",
            "proc/keys",
            "proc/latency_stats",
            "proc/sched_debug",
            "proc/scsi",
            "proc/timer_list",
            "proc/sysrq-trigger",
        ];
        let null_path = merged.join("dev/null");
        for rel in &masked_proc_files {
            let target = merged.join(rel);
            if target.exists() {
                nix::mount::mount(
                    Some(null_path.as_path()),
                    &target,
                    None::<&str>,
                    MsFlags::MS_BIND,
                    None::<&str>,
                )
                .ok(); // best-effort — don't fail pod startup
            }
        }

        // Mask dangerous /proc/1/ entries that could leak host paths.
        // We mask individual entries instead of the whole directory because
        // the pod's main process IS PID 1 (PID namespace), so /proc/self
        // resolves to /proc/1 — masking the whole dir breaks /proc/self/fd
        // which is needed for /dev/fd (bash process substitution, etc.).
        let proc_1 = merged.join("proc/1");
        if proc_1.is_dir() {
            let null_path = merged.join("dev/null");
            for entry in &["root", "cwd", "environ"] {
                let target = proc_1.join(entry);
                if target.exists() {
                    nix::mount::mount(
                        Some(null_path.as_path()),
                        &target,
                        None::<&str>,
                        MsFlags::MS_BIND | MsFlags::MS_RDONLY,
                        None::<&str>,
                    )
                    .ok(); // best-effort
                }
            }
        }
    }

    // /dev — minimal device tree (replaces blanket host /dev bind-mount)
    let shm_bytes = shm_size.unwrap_or(67_108_864); // 64MB default
    super::dev_mask::setup_minimal_dev(merged, devices, shm_bytes)?;

    // /sys — bind from host, read-only
    let sys_target = merged.join("sys");
    if Path::new("/sys").exists() && sys_target.exists() {
        nix::mount::mount(
            Some("/sys"),
            &sys_target,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )
        .map_err(nix_to_io)?;

        // Remount read-only (best effort — some systems restrict this)
        nix::mount::mount(
            None::<&str>,
            &sys_target,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC,
            None::<&str>,
        )
        .ok();
    }

    // /tmp — fresh tmpfs (not shared with host), size from config
    let tmp_target = merged.join("tmp");
    if tmp_target.exists() {
        let tmp_opts = format!("size={tmp_size},mode=1777");
        nix::mount::mount(
            Some("tmpfs"),
            &tmp_target,
            Some("tmpfs"),
            MsFlags::empty(),
            Some(tmp_opts.as_str()),
        )
        .map_err(nix_to_io)?;
    }

    Ok(())
}

/// Convert nix::Error to std::io::Error for use in pre_exec closures.
fn nix_to_io(e: nix::Error) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_rejects_empty_command() {
        let result = spawn_isolated(
            &[],
            &[PathBuf::from("/")],
            Path::new("/tmp/upper"),
            Path::new("/tmp/work"),
            Path::new("/tmp/merged"),
            None,
            None,
            "test-pod",
            None,
            None,
            crate::backend::native::seccomp::SeccompProfile::Default,
            None,
            100 * 1024 * 1024,
            Path::new("/tmp/rootfs"),
            &[],
            crate::config::DevicesConfig::default(),
            crate::config::SystemAccess::Safe,
            None,
            None,
        );
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("empty"),
            "should reject empty command"
        );
    }

    // Full isolation tests require root and are run separately.

    #[test]
    #[ignore = "requires root"]
    fn spawn_echo_in_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("pod");
        super::super::overlay::create_dirs(&pod_dir).unwrap();
        super::super::overlay::create_rootfs(&pod_dir).unwrap();

        let pid = spawn_isolated(
            &["/bin/echo".into(), "hello from pod".into()],
            &[PathBuf::from("/")],
            &pod_dir.join("upper"),
            &pod_dir.join("work"),
            &pod_dir.join("merged"),
            None,
            None,
            "test-echo",
            None,
            None,
            crate::backend::native::seccomp::SeccompProfile::Default,
            None,
            100 * 1024 * 1024,
            &pod_dir.join("rootfs"),
            &[],
            crate::config::DevicesConfig::default(),
            crate::config::SystemAccess::Safe,
            None,
            None,
        )
        .unwrap();

        assert!(pid > 0);

        // Wait for child to exit
        let status = nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid as i32),
            None,
        )
        .unwrap();

        assert!(
            matches!(status, nix::sys::wait::WaitStatus::Exited(_, 0)),
            "child should exit successfully"
        );
    }

    #[test]
    #[ignore = "requires root"]
    fn writes_go_to_upper_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("pod");
        super::super::overlay::create_dirs(&pod_dir).unwrap();
        super::super::overlay::create_rootfs(&pod_dir).unwrap();

        let test_file = format!("/opt/envpod_test_{}", uuid::Uuid::new_v4());
        let pid = spawn_isolated(
            &[
                "/bin/sh".into(),
                "-c".into(),
                format!("echo 'written by agent' > {test_file}"),
            ],
            &[PathBuf::from("/")],
            &pod_dir.join("upper"),
            &pod_dir.join("work"),
            &pod_dir.join("merged"),
            None,
            None,
            "test-write",
            None,
            None,
            crate::backend::native::seccomp::SeccompProfile::Default,
            None,
            100 * 1024 * 1024,
            &pod_dir.join("rootfs"),
            &[],
            crate::config::DevicesConfig::default(),
            crate::config::SystemAccess::Safe,
            None,
            None,
        )
        .unwrap();

        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None).unwrap();

        // The file should NOT exist on the host
        assert!(
            !Path::new(&test_file).exists(),
            "file should not exist on host filesystem"
        );

        // The file SHOULD exist in the overlay's upper layer
        let upper_path = pod_dir
            .join("upper")
            .join(test_file.trim_start_matches('/'));
        assert!(
            upper_path.exists(),
            "file should exist in overlay upper layer: {}",
            upper_path.display()
        );
    }

    #[test]
    #[ignore = "requires root"]
    fn uts_namespace_sets_hostname() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("pod");
        super::super::overlay::create_dirs(&pod_dir).unwrap();
        super::super::overlay::create_rootfs(&pod_dir).unwrap();

        // Write to /opt (not /tmp — /tmp is a fresh tmpfs that bypasses the overlay)
        let output_file = format!("/opt/envpod_hostname_{}", uuid::Uuid::new_v4());
        let pid = spawn_isolated(
            &[
                "/bin/sh".into(),
                "-c".into(),
                format!("mkdir -p /opt && hostname > {output_file}"),
            ],
            &[PathBuf::from("/")],
            &pod_dir.join("upper"),
            &pod_dir.join("work"),
            &pod_dir.join("merged"),
            None,
            None,
            "my-test-pod",
            None,
            None,
            crate::backend::native::seccomp::SeccompProfile::Default,
            None,
            100 * 1024 * 1024,
            &pod_dir.join("rootfs"),
            &[],
            crate::config::DevicesConfig::default(),
            crate::config::SystemAccess::Safe,
            None,
            None,
        )
        .unwrap();

        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None).unwrap();

        // Read the captured hostname from the overlay upper layer
        let upper_path = pod_dir
            .join("upper")
            .join(output_file.trim_start_matches('/'));
        let hostname = std::fs::read_to_string(&upper_path)
            .unwrap_or_else(|_| panic!("should find hostname at {}", upper_path.display()));

        assert_eq!(
            hostname.trim(),
            "my-test-pod",
            "pod should see its own hostname"
        );
    }
}
