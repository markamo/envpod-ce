// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Host preflight diagnostics for `envpod doctor`.
//!
//! Read-only. Every check returns a [`DoctorCheck`] with a
//! [`DoctorStatus`], a human-readable message, and an optional
//! remediation hint. The report is assembled by [`run_report`] and
//! rendered (or JSON-serialized) by the CLI.
//!
//! **Privilege mode (Q8).** Doctor must run happily without root. Checks
//! that need privileges and cannot be performed as the current user
//! return [`DoctorStatus::Unknown`] — never [`DoctorStatus::Fail`]. The
//! CLI exit code is zero unless at least one `Fail` is present.
//!
//! **Scope.** This module ships the check *library*. The CLI subcommand
//! in `envpod doctor` composes them, groups the output, and exits.
//! The check list here is a superset — the CLI decides which groups
//! to run in which mode.

use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

/// Outcome of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DoctorStatus {
    /// Check passed — the thing envpod depends on is present and healthy.
    Pass,
    /// Check passed "enough" but with a caveat worth surfacing. Never
    /// blocks. Also used for soft prerequisites (e.g. optional binaries).
    Warn,
    /// Check failed — envpod will not work until this is fixed. Exit
    /// non-zero on any `Fail`.
    Fail,
    /// Check could not be performed with the current permissions or on
    /// the current platform. Information, not a judgment. The summary
    /// line recommends sudo when any check downgraded to `Unknown`.
    Unknown,
}

/// Logical group a check belongs to. The CLI renders one heading per
/// group, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorGroup {
    Host,
    Runtime,
    Reachability,
}

/// A single check in the report.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    /// Stable machine-readable name, e.g. `kernel_version`,
    /// `cgroups_v2`, `dns_daemon`. Support tooling filters on this —
    /// treat as part of the public API.
    pub name: &'static str,
    pub group: DoctorGroup,
    pub status: DoctorStatus,
    /// One-line human message describing what was observed.
    pub message: String,
    /// Remediation suggestion shown when status is Fail or Warn. `None`
    /// when the check passed.
    pub hint: Option<String>,
}

impl DoctorCheck {
    fn pass(name: &'static str, group: DoctorGroup, message: impl Into<String>) -> Self {
        Self { name, group, status: DoctorStatus::Pass, message: message.into(), hint: None }
    }
    fn warn(
        name: &'static str, group: DoctorGroup,
        message: impl Into<String>, hint: impl Into<String>,
    ) -> Self {
        Self {
            name, group, status: DoctorStatus::Warn,
            message: message.into(), hint: Some(hint.into()),
        }
    }
    fn fail(
        name: &'static str, group: DoctorGroup,
        message: impl Into<String>, hint: impl Into<String>,
    ) -> Self {
        Self {
            name, group, status: DoctorStatus::Fail,
            message: message.into(), hint: Some(hint.into()),
        }
    }
    fn unknown(
        name: &'static str, group: DoctorGroup,
        message: impl Into<String>, hint: impl Into<String>,
    ) -> Self {
        Self {
            name, group, status: DoctorStatus::Unknown,
            message: message.into(), hint: Some(hint.into()),
        }
    }
}

/// Schema version for `envpod doctor --json`. Bumps whenever the check
/// list, field names, or status semantics change in a non-additive way.
pub const SCHEMA_VERSION: u32 = 1;

/// Full diagnostic report. Serializable to JSON for tooling.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
}

/// Rolled-up counts + the single decision the CLI exit code hinges on.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorSummary {
    pub passed: usize,
    pub warned: usize,
    pub failed: usize,
    pub unknown: usize,
    /// True when the CLI should exit non-zero.
    pub blocking: bool,
    /// True when any check downgraded to `Unknown` because of missing
    /// privileges — the summary line recommends `sudo` in this case.
    pub sudo_would_help: bool,
}

impl DoctorReport {
    fn build(checks: Vec<DoctorCheck>) -> Self {
        let mut passed = 0;
        let mut warned = 0;
        let mut failed = 0;
        let mut unknown = 0;
        let mut sudo_would_help = false;
        for c in &checks {
            match c.status {
                DoctorStatus::Pass => passed += 1,
                DoctorStatus::Warn => warned += 1,
                DoctorStatus::Fail => failed += 1,
                DoctorStatus::Unknown => {
                    unknown += 1;
                    // A message starting with "requires root" flags the
                    // sudo-would-help case. We only set this when the
                    // real user is not already root.
                    if !is_root() && c.message.contains("requires root") {
                        sudo_would_help = true;
                    }
                }
            }
        }
        DoctorReport {
            schema_version: SCHEMA_VERSION,
            checks,
            summary: DoctorSummary {
                passed,
                warned,
                failed,
                unknown,
                blocking: failed > 0,
                sudo_would_help,
            },
        }
    }
}

// ── entry points ──────────────────────────────────────────────────────────

/// Run the full doctor report against a given envpod base dir. The base
/// dir is usually `/var/lib/envpod` (root) or `~/.local/share/envpod`
/// (user); the caller passes whichever matches the current install.
pub fn run_report(base_dir: &Path) -> DoctorReport {
    let mut checks = Vec::new();
    push_host_checks(&mut checks);
    push_runtime_checks(&mut checks, base_dir);
    push_reachability_checks(&mut checks);
    DoctorReport::build(checks)
}

/// Run only the host-prerequisite group. Useful for quick preflight from
/// other tools (e.g. the installer).
pub fn run_host_only() -> DoctorReport {
    let mut checks = Vec::new();
    push_host_checks(&mut checks);
    DoctorReport::build(checks)
}

// ── host prerequisites ────────────────────────────────────────────────────

fn push_host_checks(out: &mut Vec<DoctorCheck>) {
    out.push(check_kernel_version());
    out.push(check_cgroups_v2());
    out.push(check_overlayfs());
    out.push(check_binary("ip", "iproute2"));
    out.push(check_iptables_or_nft());
    out.push(check_binary("curl", "curl"));
    out.push(check_binary("gzip", "gzip"));
    out.push(check_python3_version());
    out.push(check_binary_optional("cloudflared", "cloudflared"));
    out.push(check_envpod_group());
}

/// Parse `/proc/sys/kernel/osrelease` for a `MAJOR.MINOR.*` version.
/// Returns `(major, minor)` on success.
pub(crate) fn read_kernel_version() -> Option<(u32, u32)> {
    let text = fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    parse_kernel_version(&text)
}

pub(crate) fn parse_kernel_version(raw: &str) -> Option<(u32, u32)> {
    let head = raw.trim().split(|c: char| !c.is_ascii_digit() && c != '.').next()?;
    let mut parts = head.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

fn check_kernel_version() -> DoctorCheck {
    match read_kernel_version() {
        Some((major, minor)) if (major, minor) >= (5, 11) => DoctorCheck::pass(
            "kernel_version",
            DoctorGroup::Host,
            format!("kernel {major}.{minor} (>= 5.11)"),
        ),
        Some((major, minor)) => DoctorCheck::fail(
            "kernel_version",
            DoctorGroup::Host,
            format!("kernel {major}.{minor} is too old (envpod requires 5.11+)"),
            "upgrade the host kernel — cgroup v2 freezer + overlayfs features required",
        ),
        None => DoctorCheck::unknown(
            "kernel_version",
            DoctorGroup::Host,
            "could not read /proc/sys/kernel/osrelease (non-Linux host?)",
            "envpod currently supports Linux 5.11+; WSL2 reports the Linux view here",
        ),
    }
}

/// Confirm the unified cgroup v2 hierarchy is mounted by reading
/// `/proc/mounts`. Not root-gated — the file is world-readable.
fn check_cgroups_v2() -> DoctorCheck {
    let mounts = match fs::read_to_string("/proc/mounts") {
        Ok(s) => s,
        Err(_) => return DoctorCheck::unknown(
            "cgroups_v2",
            DoctorGroup::Host,
            "could not read /proc/mounts",
            "verify /proc is mounted (not typical in envpods themselves)",
        ),
    };
    let unified = mounts.lines().any(|l| {
        // `cgroup2 /sys/fs/cgroup cgroup2 rw,...` is the unified hierarchy.
        let mut it = l.split_whitespace();
        let _src = it.next();
        let mount_point = it.next().unwrap_or("");
        let fstype = it.next().unwrap_or("");
        fstype == "cgroup2" && mount_point == "/sys/fs/cgroup"
    });
    if !unified {
        return DoctorCheck::fail(
            "cgroups_v2",
            DoctorGroup::Host,
            "cgroup v2 unified hierarchy is not mounted at /sys/fs/cgroup",
            "boot with `systemd.unified_cgroup_hierarchy=1` or verify your distro uses cgroup v2 by default",
        );
    }
    // Best-effort: confirm the controllers we need are available.
    let needed = ["cpu", "memory", "pids"];
    let controllers = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")
        .unwrap_or_default();
    let available: Vec<&str> = controllers.split_whitespace().collect();
    let missing: Vec<&str> = needed.iter()
        .copied()
        .filter(|c| !available.contains(c))
        .collect();
    if !missing.is_empty() {
        return DoctorCheck::warn(
            "cgroups_v2",
            DoctorGroup::Host,
            format!(
                "cgroup v2 mounted but controllers missing: {}",
                missing.join(", "),
            ),
            "ensure the parent slice delegates controllers \
             (echo '+cpu +memory +pids' > /sys/fs/cgroup/cgroup.subtree_control)",
        );
    }
    DoctorCheck::pass(
        "cgroups_v2",
        DoctorGroup::Host,
        "cgroup v2 mounted with cpu/memory/pids controllers",
    )
}

/// Check overlayfs availability by scanning `/proc/filesystems`. Does
/// NOT attempt to mount — that would require root + a writable test dir
/// and is outside "read-only preflight."
fn check_overlayfs() -> DoctorCheck {
    let fs_list = match fs::read_to_string("/proc/filesystems") {
        Ok(s) => s,
        Err(_) => return DoctorCheck::unknown(
            "overlayfs",
            DoctorGroup::Host,
            "could not read /proc/filesystems",
            "verify /proc is mounted",
        ),
    };
    let has_overlay = fs_list.lines().any(|l| {
        // Format is either `nodev\toverlay` or just `\toverlay`.
        l.split_whitespace().any(|tok| tok == "overlay")
    });
    if has_overlay {
        DoctorCheck::pass(
            "overlayfs", DoctorGroup::Host,
            "overlay filesystem available",
        )
    } else {
        DoctorCheck::fail(
            "overlayfs",
            DoctorGroup::Host,
            "overlay filesystem not registered",
            "load the overlay module: `sudo modprobe overlay` (usually auto-loads on first mount; nested containers without privileged mode may need host changes)",
        )
    }
}

fn check_binary(name: &str, package_hint: &str) -> DoctorCheck {
    let name_static = static_leak(name);
    if which(name).is_some() {
        DoctorCheck::pass(
            name_static, DoctorGroup::Host,
            format!("{name} found on PATH"),
        )
    } else {
        DoctorCheck::fail(
            name_static,
            DoctorGroup::Host,
            format!("{name} not found on PATH"),
            format!("install the `{package_hint}` package for your distro"),
        )
    }
}

fn check_binary_optional(name: &str, package_hint: &str) -> DoctorCheck {
    let name_static = static_leak(name);
    if which(name).is_some() {
        DoctorCheck::pass(
            name_static, DoctorGroup::Host,
            format!("{name} found on PATH"),
        )
    } else {
        DoctorCheck::warn(
            name_static,
            DoctorGroup::Host,
            format!("{name} not found on PATH"),
            format!(
                "optional — envpod can download `{package_hint}` at first publish, \
                 but preinstalling avoids network-dependent publishes"
            ),
        )
    }
}

fn check_iptables_or_nft() -> DoctorCheck {
    if which("iptables").is_some() {
        DoctorCheck::pass(
            "iptables_or_nft", DoctorGroup::Host,
            "iptables found on PATH",
        )
    } else if which("nft").is_some() {
        DoctorCheck::warn(
            "iptables_or_nft", DoctorGroup::Host,
            "iptables not found but nft is available",
            "envpod currently uses iptables — install iptables or iptables-nft shim on this host",
        )
    } else {
        DoctorCheck::fail(
            "iptables_or_nft", DoctorGroup::Host,
            "neither iptables nor nft found on PATH",
            "install iptables (Debian/Ubuntu: iptables; Fedora: iptables-legacy or iptables-nft)",
        )
    }
}

fn check_python3_version() -> DoctorCheck {
    let Some(path) = which("python3") else {
        return DoctorCheck::fail(
            "python3_version",
            DoctorGroup::Host,
            "python3 not found on PATH",
            "install python3 ≥ 3.8 — the envpod auth proxy is generated as a Python script",
        );
    };
    let out = Command::new(&path)
        .arg("--version")
        .output();
    let version_line = match out {
        Ok(o) => String::from_utf8_lossy(
            if !o.stdout.is_empty() { &o.stdout } else { &o.stderr }
        ).trim().to_string(),
        Err(_) => {
            return DoctorCheck::unknown(
                "python3_version",
                DoctorGroup::Host,
                format!("python3 at {} is not executable", path.display()),
                "ensure python3 has execute permission for this user",
            );
        }
    };
    match parse_python_version(&version_line) {
        Some((3, minor)) if minor >= 8 => DoctorCheck::pass(
            "python3_version",
            DoctorGroup::Host,
            format!("python3 {} (>= 3.8)", version_line.trim_start_matches("Python ").trim()),
        ),
        Some((3, minor)) => DoctorCheck::fail(
            "python3_version",
            DoctorGroup::Host,
            format!("python3 3.{minor} is too old"),
            "upgrade python3 to 3.8 or later (auth proxy uses f-strings + walrus)",
        ),
        Some((major, minor)) => DoctorCheck::fail(
            "python3_version",
            DoctorGroup::Host,
            format!("python3 reports version {major}.{minor} (expected 3.x)"),
            "ensure python3 on PATH is a CPython 3 interpreter",
        ),
        None => DoctorCheck::unknown(
            "python3_version",
            DoctorGroup::Host,
            format!("could not parse `python3 --version` output: {version_line:?}"),
            "verify python3 reports a standard `Python X.Y.Z` version string",
        ),
    }
}

pub(crate) fn parse_python_version(raw: &str) -> Option<(u32, u32)> {
    // `python3 --version` prints `Python 3.12.0` (stdout on 3.4+; stderr
    // on very old 3.x but we already joined them).
    let text = raw.trim();
    let after = text.strip_prefix("Python ").unwrap_or(text);
    let mut parts = after.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
    Some((major, minor))
}

fn check_envpod_group() -> DoctorCheck {
    if is_root() {
        return DoctorCheck::pass(
            "envpod_group",
            DoctorGroup::Host,
            "running as root — group membership not required",
        );
    }
    // getgrnam lookup: walk /etc/group for the envpod entry.
    let group_text = fs::read_to_string("/etc/group").unwrap_or_default();
    let has_envpod_group = group_text.lines().any(|l| l.starts_with("envpod:"));
    if !has_envpod_group {
        return DoctorCheck::warn(
            "envpod_group",
            DoctorGroup::Host,
            "envpod group does not exist on this host",
            "the installer creates an `envpod` group so non-root users can run pods; \
             rerun `install.sh` or create it manually",
        );
    }
    // Check whether the current user is a member of the envpod group.
    let uid = unsafe { libc::getuid() };
    let user = user_name_for_uid(uid).unwrap_or_else(|| format!("uid={uid}"));
    let in_group = group_text.lines().find_map(|l| {
        if l.starts_with("envpod:") {
            let members = l.rsplit(':').next().unwrap_or("");
            Some(members.split(',').any(|m| m == user))
        } else {
            None
        }
    }).unwrap_or(false);
    if in_group {
        DoctorCheck::pass(
            "envpod_group", DoctorGroup::Host,
            format!("user {user} is a member of the envpod group"),
        )
    } else {
        DoctorCheck::warn(
            "envpod_group",
            DoctorGroup::Host,
            format!("user {user} is not a member of the envpod group"),
            "add yourself: `sudo usermod -aG envpod $USER` (logout/login to apply)",
        )
    }
}

// ── runtime services ─────────────────────────────────────────────────────

fn push_runtime_checks(out: &mut Vec<DoctorCheck>, base_dir: &Path) {
    out.push(check_base_dir_writable(base_dir));
    out.push(check_dns_daemon(base_dir));
    out.push(check_license_monitor_pid(base_dir));
}

fn check_base_dir_writable(base_dir: &Path) -> DoctorCheck {
    // If the dir exists, probe writability via a temp file name. If it
    // doesn't, just require that the parent is writable — the installer
    // creates it.
    let target = if base_dir.exists() { base_dir.to_path_buf() } else {
        base_dir.parent().unwrap_or(Path::new("/")).to_path_buf()
    };
    let probe = target.join(".envpod-doctor-write-probe");
    match fs::write(&probe, b"") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            DoctorCheck::pass(
                "base_dir_writable",
                DoctorGroup::Runtime,
                format!("{} is writable by this user", target.display()),
            )
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            if is_root() {
                DoctorCheck::fail(
                    "base_dir_writable",
                    DoctorGroup::Runtime,
                    format!("{} not writable even as root: {e}", target.display()),
                    "check filesystem mount options (read-only?) and disk health",
                )
            } else {
                DoctorCheck::unknown(
                    "base_dir_writable",
                    DoctorGroup::Runtime,
                    format!(
                        "requires root or envpod-group membership to check write \
                         access to {}", target.display(),
                    ),
                    "re-run `envpod doctor` under sudo or after adding the user to the envpod group",
                )
            }
        }
        Err(e) => DoctorCheck::fail(
            "base_dir_writable",
            DoctorGroup::Runtime,
            format!("{}: {e}", target.display()),
            "verify the envpod base dir exists and is writable",
        ),
    }
}

fn check_dns_daemon(base_dir: &Path) -> DoctorCheck {
    // Standard daemon socket lives at `/var/lib/envpod/dns.sock` per
    // core/src/dns_daemon; accept either the standard path OR a
    // base-dir-relative path for user-mode installs.
    let candidates = [
        PathBuf::from("/var/lib/envpod/dns.sock"),
        base_dir.join("dns.sock"),
    ];
    let socket = candidates.iter().find(|p| p.exists());
    match socket {
        None => DoctorCheck::warn(
            "dns_daemon",
            DoctorGroup::Runtime,
            "DNS daemon socket not found",
            "pod discovery (*.pods.local) needs the daemon running; start it with \
             `envpod dns-daemon` (or systemctl start envpod-dns if installed)",
        ),
        Some(path) => DoctorCheck::pass(
            "dns_daemon",
            DoctorGroup::Runtime,
            format!("DNS daemon socket present at {}", path.display()),
        ),
    }
}

fn check_license_monitor_pid(base_dir: &Path) -> DoctorCheck {
    let pid_path = base_dir.join("license-monitor.pid");
    if !pid_path.exists() {
        return DoctorCheck::pass(
            "license_monitor",
            DoctorGroup::Runtime,
            "no license-monitor PID file (CE-only hosts legitimately have none)",
        );
    }
    // flock probe — shared, non-blocking. If we can acquire a shared
    // lock, nobody holds an exclusive lock, which means the monitor is
    // not running.
    if let Some(alive) = probe_flock(&pid_path) {
        if alive {
            let pid = fs::read_to_string(&pid_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            DoctorCheck::pass(
                "license_monitor",
                DoctorGroup::Runtime,
                match pid {
                    Some(p) => format!("license monitor running (pid {p})"),
                    None => "license monitor running".to_string(),
                },
            )
        } else {
            DoctorCheck::warn(
                "license_monitor",
                DoctorGroup::Runtime,
                "license-monitor.pid present but no process holds the flock",
                "stale PID file — runs next `envpod start` / `envpod publish` will \
                 reclaim and re-launch the monitor automatically",
            )
        }
    } else {
        DoctorCheck::unknown(
            "license_monitor",
            DoctorGroup::Runtime,
            "requires root or envpod-group membership to probe license-monitor.pid",
            "re-run under sudo for full coverage",
        )
    }
}

/// Returns `Some(true)` if the file is flocked exclusively by another
/// process, `Some(false)` if not locked, `None` on permission or I/O
/// error. Mirrors the monitor's own probe.
pub(crate) fn probe_flock(path: &Path) -> Option<bool> {
    let file = fs::OpenOptions::new().read(true).open(path).ok()?;
    let fd = file.as_raw_fd();
    unsafe {
        let got = libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB);
        if got == 0 {
            libc::flock(fd, libc::LOCK_UN);
            Some(false)
        } else {
            let errno = std::io::Error::last_os_error().raw_os_error();
            if errno == Some(libc::EWOULDBLOCK) {
                Some(true)
            } else {
                None
            }
        }
    }
}

// ── reachability ─────────────────────────────────────────────────────────

fn push_reachability_checks(out: &mut Vec<DoctorCheck>) {
    // Reachability is warn-only by policy (Q2) — offline workflows are
    // legitimate. Failures here never block.
    //
    // CE build: CE has no `license activate` (Premium is a separate
    // binary download, not a CE+key model). So `activate.envpod.dev`
    // and `premium.envpod.dev` are not relevant. Probe `envpod.dev`
    // instead — that's the CE install.sh source and the canonical
    // upgrade path home page.
    out.push(check_tcp_reachable(
        "envpod_dev", "envpod.dev", 443,
    ));
}

fn check_tcp_reachable(name: &'static str, host: &str, port: u16) -> DoctorCheck {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr_iter = match (host, port).to_socket_addrs() {
        Ok(it) => it,
        Err(_) => return DoctorCheck::warn(
            name,
            DoctorGroup::Reachability,
            format!("DNS resolution failed for {host}"),
            "verify host DNS (resolv.conf / systemd-resolved) — offline mode is OK, just flagged",
        ),
    };
    let addrs: Vec<_> = addr_iter.collect();
    if addrs.is_empty() {
        return DoctorCheck::warn(
            name,
            DoctorGroup::Reachability,
            format!("no addresses resolved for {host}"),
            "check DNS — envpod works offline within grace period but won't reach server",
        );
    }
    for addr in &addrs {
        if TcpStream::connect_timeout(addr, Duration::from_millis(1500)).is_ok() {
            return DoctorCheck::pass(
                name,
                DoctorGroup::Reachability,
                format!("{host}:{port} reachable"),
            );
        }
    }
    DoctorCheck::warn(
        name,
        DoctorGroup::Reachability,
        format!("{host}:{port} not reachable within 1.5s"),
        "check internet access — offline grace period keeps envpod working; \
         reactivation on a revoked license does require reach",
    )
}

// ── small helpers ────────────────────────────────────────────────────────

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn user_name_for_uid(uid: u32) -> Option<String> {
    // Read /etc/passwd — no external deps. This misses NSS-only users
    // but those are rare on envpod hosts.
    let text = fs::read_to_string("/etc/passwd").ok()?;
    for line in text.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _x = fields.next()?;
        let u: u32 = fields.next()?.parse().ok()?;
        if u == uid {
            return Some(name.to_string());
        }
    }
    None
}

/// Leak a `&str` to `&'static str`. Used only inside `check_binary` +
/// `check_binary_optional` so the `name` field of `DoctorCheck` can be
/// a static string matching the binary name on PATH. Each binary name
/// is leaked exactly once per doctor invocation, so the memory cost is
/// bounded and trivial.
fn static_leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kernel_version_various() {
        assert_eq!(parse_kernel_version("6.5.0-15-generic"), Some((6, 5)));
        assert_eq!(parse_kernel_version("5.11.0-1021-azure"), Some((5, 11)));
        assert_eq!(parse_kernel_version("4.19.0"), Some((4, 19)));
        assert_eq!(parse_kernel_version(""), None);
        assert_eq!(parse_kernel_version("garbage"), None);
        assert_eq!(parse_kernel_version("5"), None); // missing minor
    }

    #[test]
    fn parse_python_version_various() {
        assert_eq!(parse_python_version("Python 3.12.0"), Some((3, 12)));
        assert_eq!(parse_python_version("Python 3.8.10"), Some((3, 8)));
        assert_eq!(parse_python_version("Python 3.11.0rc1"), Some((3, 11)));
        assert_eq!(parse_python_version("Python 2.7.18"), Some((2, 7)));
        assert_eq!(parse_python_version("3.9.7"), Some((3, 9)));
        assert_eq!(parse_python_version(""), None);
        assert_eq!(parse_python_version("Python"), None);
    }

    #[test]
    fn doctor_status_serializes_lowercase() {
        // Stable machine name — tooling parses this, treat as API.
        assert_eq!(
            serde_json::to_string(&DoctorStatus::Pass).unwrap(),
            "\"pass\"",
        );
        assert_eq!(
            serde_json::to_string(&DoctorStatus::Fail).unwrap(),
            "\"fail\"",
        );
        assert_eq!(
            serde_json::to_string(&DoctorStatus::Unknown).unwrap(),
            "\"unknown\"",
        );
    }

    #[test]
    fn doctor_report_summary_counts() {
        let checks = vec![
            DoctorCheck::pass("a", DoctorGroup::Host, "ok"),
            DoctorCheck::warn("b", DoctorGroup::Host, "eh", "do X"),
            DoctorCheck::warn("c", DoctorGroup::Runtime, "eh", "do Y"),
            DoctorCheck::fail("d", DoctorGroup::Runtime, "bad", "fix it"),
            DoctorCheck::unknown("e", DoctorGroup::Host, "unknown state", "re-run"),
        ];
        let report = DoctorReport::build(checks);
        assert_eq!(report.summary.passed, 1);
        assert_eq!(report.summary.warned, 2);
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.summary.unknown, 1);
        assert!(report.summary.blocking);
    }

    #[test]
    fn doctor_report_no_fail_is_non_blocking() {
        let checks = vec![
            DoctorCheck::pass("a", DoctorGroup::Host, "ok"),
            DoctorCheck::warn("b", DoctorGroup::Host, "eh", "do X"),
            DoctorCheck::unknown("c", DoctorGroup::Host, "unknown", "re-run"),
        ];
        let report = DoctorReport::build(checks);
        assert!(!report.summary.blocking);
    }

    #[test]
    fn sudo_would_help_only_set_for_requires_root_messages() {
        // An `Unknown` whose message starts with "requires root" — and
        // only when the real user is not root — flips the hint.
        let checks = vec![
            DoctorCheck::unknown(
                "base_dir_writable", DoctorGroup::Runtime,
                "requires root or envpod-group membership to probe",
                "re-run under sudo",
            ),
        ];
        let report = DoctorReport::build(checks);
        // We can't assert True unconditionally — the test may run as
        // root. Assert the invariant: sudo_would_help implies not root.
        if report.summary.sudo_would_help {
            assert!(!is_root());
        } else {
            assert!(is_root());
        }
    }

    #[test]
    fn schema_version_is_serializable() {
        let report = DoctorReport::build(vec![]);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"].as_u64(), Some(SCHEMA_VERSION as u64));
        assert!(json["checks"].is_array());
        assert!(json["summary"].is_object());
    }

    #[test]
    fn which_finds_sh() {
        // /bin/sh is on every Linux CI runner.
        let sh = which("sh");
        assert!(sh.is_some(), "expected /bin/sh on PATH");
    }

    #[test]
    fn which_returns_none_for_nonexistent() {
        assert!(which("this-binary-does-not-exist-on-any-reasonable-system").is_none());
    }

    #[test]
    fn overlayfs_check_uses_proc_filesystems() {
        // Just verify the check runs and returns a reasonable shape —
        // we can't assert pass/fail because CI runners vary.
        let c = check_overlayfs();
        assert_eq!(c.name, "overlayfs");
        assert_eq!(c.group, DoctorGroup::Host);
        assert!(matches!(
            c.status,
            DoctorStatus::Pass | DoctorStatus::Fail | DoctorStatus::Unknown,
        ));
    }

    #[test]
    fn probe_flock_returns_none_for_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no-such-file.pid");
        assert_eq!(probe_flock(&missing), None);
    }

    #[test]
    fn probe_flock_returns_some_false_on_unlocked_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("unlocked.pid");
        fs::write(&path, "12345").unwrap();
        assert_eq!(probe_flock(&path), Some(false));
    }

    #[test]
    fn probe_flock_detects_held_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("held.pid");
        let file = fs::OpenOptions::new()
            .create(true).read(true).write(true).open(&path).unwrap();
        let fd = file.as_raw_fd();
        unsafe {
            let got = libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB);
            assert_eq!(got, 0, "expected LOCK_EX to succeed on fresh file");
        }
        // Now probe from this same test — LOCK_SH on a file we already
        // hold LOCK_EX on DOES succeed (flock allows re-locking from the
        // same FD), but the probe opens a NEW FD, which should see it
        // held.
        assert_eq!(probe_flock(&path), Some(true));
        // Clean up.
        unsafe { libc::flock(fd, libc::LOCK_UN); }
    }

    #[test]
    fn check_dns_daemon_missing_returns_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let c = check_dns_daemon(tmp.path());
        // /var/lib/envpod/dns.sock may or may not exist on the test
        // host. If it does, status is Pass; if not, status is Warn.
        // Either way the name is stable and no Fail state is reached.
        assert_eq!(c.name, "dns_daemon");
        assert!(matches!(c.status, DoctorStatus::Pass | DoctorStatus::Warn));
    }

    #[test]
    fn run_host_only_returns_host_checks() {
        let report = run_host_only();
        assert!(!report.checks.is_empty());
        assert!(report.checks.iter().all(|c| c.group == DoctorGroup::Host));
    }

    #[test]
    fn iptables_or_nft_check_produces_stable_name() {
        let c = check_iptables_or_nft();
        assert_eq!(c.name, "iptables_or_nft");
    }

    /// Drift guard (session 25, commit 5). Every check name the library
    /// ships MUST appear verbatim in `docs/TROUBLESHOOTING.md` so the
    /// operator-facing remediation table stays in sync. A rename or a
    /// new check that skips the doc update trips this test immediately.
    ///
    /// The doc file is read relative to `CARGO_MANIFEST_DIR/../docs` so
    /// this test works from both `cargo test -p envpod-core` and a
    /// workspace-root run.
    #[test]
    fn every_check_name_appears_in_troubleshooting_doc() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let doc_path = manifest.join("..").join("docs").join("TROUBLESHOOTING.md");
        let doc = match std::fs::read_to_string(&doc_path) {
            Ok(s) => s,
            Err(e) => panic!(
                "could not read {} — drift guard needs it present: {e}",
                doc_path.display()
            ),
        };

        // Assemble the union of check names from a full report (uses a
        // scratch base_dir so reachability still runs but its names are
        // included).
        let tmp = tempfile::tempdir().unwrap();
        let report = run_report(tmp.path());
        let mut missing: Vec<&str> = Vec::new();
        for c in &report.checks {
            if !doc.contains(c.name) {
                missing.push(c.name);
            }
        }
        assert!(
            missing.is_empty(),
            "docs/TROUBLESHOOTING.md is missing these shipped check names: {missing:?}. \
             Add a row to the `envpod doctor` table when you add/rename a check.",
        );
    }
}
