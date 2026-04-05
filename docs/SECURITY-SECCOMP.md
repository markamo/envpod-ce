# Seccomp-BPF Syscall Filtering

Every pod process runs under a seccomp-BPF filter that restricts which syscalls the agent can invoke. Blocked syscalls return `EPERM` (not SIGKILL) — failures are debuggable without crashing the process.

The filter is applied as the last step in namespace setup, after all mount/pivot_root operations are complete.

## Three Profiles

```yaml
security:
  seccomp: default    # default | browser | none
```

| Profile | Syscalls | Use case |
|---------|----------|----------|
| `default` | ~130 | Most apps: shells, compilers, interpreters, CLI tools, databases |
| `browser` | ~143 | Chrome, Firefox, Electron apps (adds sandbox + timer syscalls) |
| `none` | All | Debugging only. No filter applied. Full host syscall access. |

## Default Profile (~130 syscalls)

Covers shells, compilers, interpreters, package managers, databases, web servers, and most CLI tools.

### File I/O
`read` `write` `open`* `openat` `creat`* `close` `stat`* `fstat` `lstat`* `newfstatat` `lseek` `pread64` `pwrite64` `readv` `writev` `preadv2` `pwritev2` `access`* `faccessat` `faccessat2` `dup` `dup2`* `dup3` `fcntl` `flock`

### File Operations
`fsync` `fdatasync` `ftruncate` `truncate` `getdents64` `getcwd` `chdir` `fchdir` `rename`* `renameat` `renameat2` `mkdir`* `mkdirat` `rmdir`* `link`* `linkat` `unlink`* `unlinkat` `symlink`* `symlinkat` `readlink`* `readlinkat` `chmod`* `fchmod` `fchmodat` `chown`* `fchown` `fchownat` `lchown`* `umask`

### File Metadata
`statfs` `fstatfs` `utimensat` `copy_file_range` `statx`

### Extended Attributes
`getxattr` `lgetxattr` `fgetxattr` `listxattr` `llistxattr` `flistxattr`

### Memory Management
`mmap` `mprotect` `munmap` `brk` `mremap` `madvise` `msync` `mincore` `mlock` `mlock2` `munlock`

### Process Management
`fork`* `vfork`* `clone` `clone3` `execve` `execveat` `exit` `exit_group` `wait4` `waitid` `getpid` `getppid` `gettid` `getuid` `geteuid` `getgid` `getegid` `getgroups` `getresuid` `getresgid` `setpgid` `getpgid` `getpgrp`* `setsid` `getrlimit`* `setrlimit`* `prlimit64` `getrusage` `sched_yield` `sched_getaffinity` `sched_setaffinity` `sched_getparam` `sched_getscheduler` `sched_setscheduler`

### Process Control
`set_tid_address` `set_robust_list` `get_robust_list` `prctl` `arch_prctl`* `rseq` `capget` `capset` `setuid` `setgid` `setgroups` `setresuid` `setresgid`

### Signals
`rt_sigaction` `rt_sigprocmask` `rt_sigreturn` `rt_sigsuspend` `rt_sigpending` `rt_sigtimedwait` `kill` `tgkill` `sigaltstack`

### Networking
`socket` `connect` `accept` `accept4` `bind` `listen` `sendto` `recvfrom` `sendmsg` `recvmsg` `sendmmsg` `recvmmsg` `shutdown` `getsockname` `getpeername` `setsockopt` `getsockopt` `socketpair`

### File Allocation
`fallocate` `sync_file_range`

### System V IPC
`shmget` `shmat` `shmdt` `shmctl` `semget` `semop` `semctl` `msgget` `msgsnd` `msgrcv` `msgctl` `sendfile`*

### Polling & Events
`poll`* `ppoll` `select`* `pselect6` `epoll_create1` `epoll_ctl` `epoll_wait`* `epoll_pwait` `epoll_pwait2` `eventfd2` `signalfd4` `timerfd_create` `timerfd_settime` `timerfd_gettime`

### Pipes
`pipe`* `pipe2` `splice` `tee`

### Time
`clock_gettime` `clock_getres` `clock_nanosleep` `gettimeofday` `nanosleep` `setitimer` `getitimer`

### inotify
`inotify_init1` `inotify_add_watch` `inotify_rm_watch`

### Miscellaneous
`uname` `sysinfo` `getrandom` `futex` `futex_waitv` `futex_wake` `futex_wait` `futex_requeue` `restart_syscall` `ioctl` `memfd_create` `membarrier` `close_range`

*\* x86_64 only — aarch64 uses newer "at" variants*

## Browser Profile (~143 syscalls)

Everything in Default, plus 13 syscalls needed by Chrome, Firefox, and Electron apps:

| Syscall | Why |
|---------|-----|
| `seccomp` | Chromium installs its own BPF filter (zygote sandbox) |
| `personality` | Chromium probes READ_IMPLIES_EXEC |
| `ioprio_get` `ioprio_set` | Disk cache I/O priority |
| `unshare` | Chromium namespace sandbox for renderer/GPU processes |
| `chroot` | Chromium sandbox isolation |
| `ptrace` | Chromium crash reporter and sandbox setup |
| `timer_create` `timer_settime` `timer_delete` `timer_gettime` `timer_getoverrun` | POSIX interval timers |
| `inotify_init`* | Old inotify API (some Chromium code paths) |

*\* x86_64 only*

## None Profile

No filter applied. The process has full syscall access. **Use only for debugging.**

```yaml
security:
  seccomp: none    # WARNING: no syscall filtering
```

When `seccomp: none` is used, `envpod audit --security` flags it as security finding S-04.

## How to Choose

| Application | Profile | Why |
|-------------|---------|-----|
| Python, Node, Rust, Go | `default` | Standard syscalls sufficient |
| Flask, Express, Actix | `default` | Web servers use standard networking |
| Claude Code, Cursor, Codex | `default` | CLI tools, no browser syscalls |
| Chrome, Firefox, Electron | `browser` | Needs sandbox + timer syscalls |
| VS Code (desktop via noVNC) | `browser` | Electron-based |
| PostgreSQL, MySQL, Redis | `default` | IPC + fallocate included since v0.1.7 |
| Docker-in-pod | `none` | Needs all namespace/mount syscalls |
| Debugging syscall issues | `none` | Temporary, find the missing syscall |

## Tracing Missing Syscalls

When an app fails with `EPERM` or "Operation not permitted", trace which syscall is blocked:

```bash
# Run with strace inside the pod (requires seccomp: none temporarily)
envpod run my-pod --root -- strace -f -c your-command 2> /tmp/trace.log

# Or trace specific categories
envpod run my-pod --root -- strace -f -e trace=process your-command
envpod run my-pod --root -- strace -f -e trace=network your-command
envpod run my-pod --root -- strace -f -e trace=ipc your-command
```

Note: `strace` requires `ptrace`, which is only in the `browser` profile or `none`.

## Blocked Syscalls (Notable)

These syscalls are intentionally NOT in any profile:

| Syscall | Why blocked |
|---------|-------------|
| `mount` `umount2` | Filesystem manipulation — handled by envpod namespace setup |
| `reboot` `kexec_load` | System shutdown/restart |
| `init_module` `delete_module` | Kernel module loading |
| `pivot_root` | Root filesystem swap — handled by envpod |
| `setns` | Namespace escape |
| `keyctl` | Kernel key management |
| `bpf` | eBPF program loading (except via `seccomp` in browser profile) |
| `perf_event_open` | Performance monitoring (information leak) |
| `add_key` `request_key` | Kernel keyring access |
| `io_uring_setup` `io_uring_enter` | io_uring (large attack surface, CVE-prone) |
| `ptrace` (default) | Process tracing — only in browser profile |

## Default Action

Blocked syscalls return `EPERM` (errno 1), not `SIGKILL`. This means:
- Applications see "Operation not permitted" instead of crashing
- Error handling in the application can report which operation failed
- `strace` can show exactly which syscall was denied
- Debugging is possible without switching to `none` profile

## Architecture Support

| Arch | Notes |
|------|-------|
| x86_64 | Full support. ~130 default, ~143 browser |
| aarch64 | Full support. Slightly fewer syscalls (no legacy "at" variants) |

Some syscalls exist only on x86_64 (e.g., `open`, `stat`, `fork`). On aarch64, modern equivalents are used (`openat`, `newfstatat`, `clone`). The filter handles this automatically.

## Custom Profiles (Planned)

Future versions will support per-pod custom syscall profiles via pod.yaml:

```yaml
security:
  seccomp: custom
  seccomp_allow:
    - io_uring_setup    # explicitly allow io_uring
    - io_uring_enter
  seccomp_deny:
    - ptrace            # explicitly deny even in browser mode
```

## Related

- [SECURITY-MODEL.md](SECURITY-MODEL.md) — Full 8-layer security architecture
- [SECURITY-POSTURE.md](SECURITY-POSTURE.md) — Audit matrix for all example configs
- [VERIFY.md](VERIFY.md) — Adversarial verification testing
