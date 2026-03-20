// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! OverlayFS management for copy-on-write filesystem isolation.
//!
//! Overlay layout per pod:
//!   {pod_dir}/rootfs/  — minimal rootfs (overlay lower layer at runtime)
//!   {pod_dir}/upper/   — writable layer (agent writes land here)
//!   {pod_dir}/work/    — overlayfs internal bookkeeping
//!   {pod_dir}/merged/  — union view (lower + upper, what the agent sees)
//!
//! The lower layer is a minimal rootfs with only system essentials
//! (bind-mounted from the host at runtime). Pods never see the full host
//! filesystem. All writes go to upper only — the host FS is never modified.
//!
//! `envpod diff`     → walks upper, reports adds/mods/deletes
//! `envpod commit`   → copies upper changes to real FS (uses "/" as target)
//! `envpod rollback` → deletes upper contents

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::TrackingConfig;
use crate::types::{DiffKind, FileDiff};

// ---------------------------------------------------------------------------
// Directory setup
// ---------------------------------------------------------------------------

/// Create the overlay directory structure for a new pod.
pub fn create_dirs(pod_dir: &Path) -> Result<()> {
    fs::create_dir_all(pod_dir.join("upper"))
        .context("create upper dir")?;
    fs::create_dir_all(pod_dir.join("work"))
        .context("create work dir")?;
    fs::create_dir_all(pod_dir.join("merged"))
        .context("create merged dir")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rootfs — minimal root filesystem for true isolation
// ---------------------------------------------------------------------------

/// System directories that get bind-mounted from the host at runtime.
/// These provide the binaries and libraries the pod needs to function.
/// NOTE: /etc is NOT here — it's copied into rootfs at init time so the
/// overlay can handle it (needed for resolv.conf override via COW upper).
const SYSTEM_BIND_DIRS: &[&str] = &["usr"];

/// Directories that might be symlinks on modern distros (e.g. /bin → /usr/bin).
/// We replicate the host layout: create symlinks if the host uses them,
/// or create empty dirs that get bind-mounted at runtime if they're real dirs.
const MAYBE_SYMLINK_DIRS: &[&str] = &["bin", "sbin", "lib", "lib64"];

/// Empty directories created in the rootfs for structure.
const EMPTY_DIRS: &[&str] = &[
    "proc", "dev", "sys", "tmp", "home", "opt", "root", "run", "srv",
    "mnt", "media", "boot",
    // /var hierarchy — package managers (apt, dpkg) need these
    "var", "var/lib", "var/lib/apt", "var/lib/apt/lists", "var/lib/apt/lists/partial",
    "var/lib/dpkg", "var/cache", "var/cache/apt", "var/cache/apt/archives",
    "var/cache/apt/archives/partial", "var/log", "var/tmp",
];

/// Create a minimal rootfs directory structure for a pod.
///
/// The rootfs serves as the overlay lower layer at runtime, replacing the
/// host's `/`. System essentials (`/usr`, `/etc`, etc.) are bind-mounted
/// from the host into this rootfs during `start()`, so the pod only sees
/// what it needs — not the entire host filesystem.
///
/// Layout:
/// ```text
/// rootfs/
/// ├── usr/          # bind-mounted from host /usr at runtime
/// ├── etc/          # bind-mounted from host /etc at runtime
/// ├── bin → usr/bin # symlink (modern) or empty dir (legacy)
/// ├── sbin → usr/sbin
/// ├── lib → usr/lib
/// ├── lib64 → usr/lib64
/// ├── proc/         # fresh procfs at runtime
/// ├── dev/          # bind-mount from host at runtime
/// ├── sys/          # bind-mount from host (read-only)
/// ├── tmp/          # fresh tmpfs at runtime
/// └── ...           # other empty dirs
/// ```
pub fn create_rootfs(pod_dir: &Path) -> Result<()> {
    let rootfs = pod_dir.join("rootfs");
    fs::create_dir_all(&rootfs).context("create rootfs dir")?;

    // Create directories for system bind-mounts
    for dir in SYSTEM_BIND_DIRS {
        fs::create_dir_all(rootfs.join(dir))
            .with_context(|| format!("create rootfs/{dir}"))?;
    }

    // For /bin, /sbin, /lib, /lib64: replicate host layout
    for dir in MAYBE_SYMLINK_DIRS {
        let host_path = Path::new("/").join(dir);

        if !host_path.exists() && host_path.symlink_metadata().is_err() {
            // Doesn't exist on host at all (e.g. /lib64 on some systems) — skip
            continue;
        }

        if host_path.symlink_metadata().map(|m| m.is_symlink()).unwrap_or(false) {
            // Host has a symlink (e.g. /bin → usr/bin). Read the target and
            // create a matching symlink in rootfs.
            let target = fs::read_link(&host_path)
                .with_context(|| format!("read symlink /{dir}"))?;
            std::os::unix::fs::symlink(&target, rootfs.join(dir))
                .with_context(|| format!("create rootfs/{dir} symlink → {}", target.display()))?;
        } else {
            // Host has a real directory — create empty dir (bind-mounted at runtime)
            fs::create_dir_all(rootfs.join(dir))
                .with_context(|| format!("create rootfs/{dir}"))?;
        }
    }

    // Create empty structural directories
    for dir in EMPTY_DIRS {
        fs::create_dir_all(rootfs.join(dir))
            .with_context(|| format!("create rootfs/{dir}"))?;
    }

    // Copy host /etc into rootfs/etc so the overlay can handle it naturally.
    // This avoids bind-mounting /etc (which breaks resolv.conf override —
    // on Ubuntu, /etc/resolv.conf is a symlink to /run/... which doesn't
    // exist in the minimal rootfs). With /etc in the lower layer, the
    // overlay's upper layer provides the pod's custom resolv.conf via COW.
    let rootfs_etc = rootfs.join("etc");
    copy_host_etc(&rootfs_etc)
        .context("copy /etc to rootfs")?;

    // Sanitize nsswitch.conf for the pod environment.
    // The host may have entries like `mdns4_minimal [NOTFOUND=return]` which
    // depend on avahi-daemon (not available in pods). Without this fix,
    // glibc's resolver aborts before reaching the `dns` source, breaking
    // all hostname resolution for curl/wget/etc.
    sanitize_nsswitch_conf(&rootfs_etc)
        .context("sanitize nsswitch.conf")?;

    // Copy host package manager state so apt/dpkg work inside pods.
    // Without this, apt fails with "List directory ... is missing".
    copy_host_var_state(&rootfs)
        .context("copy /var package state to rootfs")?;

    // Create default non-root 'agent' user (UID 60000) for pod processes.
    // Pods run as this user by default for full pod boundary protection.
    create_default_user(&rootfs)
        .context("create default agent user")?;

    Ok(())
}

/// Copy host /etc contents into rootfs/etc, preserving symlinks and permissions.
///
/// Uses `cp -a` for robustness (handles symlinks, permissions, special files).
/// Non-zero exit codes are tolerated: `cp -a` returns non-zero when it can't
/// read sensitive files (e.g. /etc/shadow, /etc/ssl/private) — the important
/// files (resolv.conf, hosts, ld.so.cache, passwd, group) are world-readable
/// and always copy successfully. When envpod runs as root (production), all
/// files copy fine.
fn copy_host_etc(rootfs_etc: &Path) -> Result<()> {
    // cp --reflink=auto -a /etc/. rootfs/etc/  — copies CONTENTS of /etc into rootfs/etc
    // --reflink=auto: instant CoW on btrfs/xfs, transparent fallback on ext4.
    // Suppress stderr to avoid noisy "Permission denied" messages in tests.
    let status = std::process::Command::new("cp")
        .args(["--reflink=auto", "-a", "--", "/etc/.", &rootfs_etc.to_string_lossy()])
        .stderr(std::process::Stdio::null())
        .status()
        .context("run cp -a /etc")?;

    // Non-zero exit is expected when running as non-root (tests) because
    // some /etc files are root-only. The essential files still get copied.
    if !status.success() {
        tracing::debug!(
            "cp -a /etc exited with {} (expected if not root — sensitive files skipped)",
            status
        );
    }

    Ok(())
}

/// Copy host package manager state into the rootfs so apt/dpkg work inside pods.
///
/// Only copies what's strictly necessary for apt to function:
/// - `/var/lib/dpkg` — package database (required for dpkg/apt to know what's installed)
/// - `/var/lib/apt` — sources and structure, but NOT the lists cache (242MB+ of
///   downloaded package indexes). Agents run `apt-get update` to rebuild lists.
///
/// Skipped (Docker does the same via `apt clean && rm -rf /var/lib/apt/lists/*`):
/// - `/var/cache/apt` — cached .deb archives (246MB+), not needed
/// - `/var/lib/apt/lists` — downloadable package lists (regenerated by `apt-get update`)
fn copy_host_var_state(rootfs: &Path) -> Result<()> {
    // dpkg database — required for apt to know installed packages
    let dpkg_src = Path::new("/var/lib/dpkg");
    if dpkg_src.exists() {
        let dest = rootfs.join("var/lib/dpkg");
        fs::create_dir_all(&dest).context("create rootfs/var/lib/dpkg")?;
        let status = std::process::Command::new("cp")
            .args(["--reflink=auto", "-a", "--", "/var/lib/dpkg/.", &dest.to_string_lossy()])
            .stderr(std::process::Stdio::null())
            .status()
            .context("cp -a /var/lib/dpkg")?;
        if !status.success() {
            tracing::debug!("cp -a /var/lib/dpkg exited with {status} (non-critical files may have been skipped)");
        }
    }

    // apt sources and structure — but skip the large lists/ cache.
    // Create the directory structure apt needs, copy only config/sources.
    let apt_dirs = [
        "var/lib/apt/lists/partial",  // required empty dir for apt-get update
        "var/cache/apt/archives/partial",  // required empty dir for apt-get install
    ];
    for dir in &apt_dirs {
        fs::create_dir_all(rootfs.join(dir))
            .with_context(|| format!("create rootfs/{dir}"))?;
    }

    // Copy apt sources configuration (small files)
    let apt_config_dirs = [
        ("var/lib/apt/extended_states", "/var/lib/apt/extended_states"),
    ];
    for (rel, host_src) in &apt_config_dirs {
        let src = Path::new(host_src);
        if src.exists() {
            let dest = rootfs.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(src, &dest).ok(); // best-effort
        }
    }

    Ok(())
}

/// Create a default non-root `agent` user (UID 60000) in the rootfs.
///
/// Pods run as this user by default, providing full pod boundary protection
/// (17/17 jailbreak tests pass vs 15/17 as root). The user is added to
/// `/etc/passwd` and `/etc/group`, and a home directory is created.
///
/// UID 60000 is chosen to avoid collision with typical host users (UID 1000+).
/// Idempotent: skips if 'agent' already exists in passwd.
fn create_default_user(rootfs: &Path) -> Result<()> {
    let passwd_path = rootfs.join("etc/passwd");
    let group_path = rootfs.join("etc/group");

    // Skip if agent user already exists (idempotent)
    if passwd_path.exists() {
        let passwd = fs::read_to_string(&passwd_path)
            .context("read passwd")?;
        if passwd.lines().any(|l| l.starts_with("agent:")) {
            return Ok(());
        }
    }

    // Append agent user to /etc/passwd
    use std::io::Write;
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&passwd_path)
            .context("open passwd for append")?;
        writeln!(f, "agent:x:60000:60000:Agent:/home/agent:/bin/bash")
            .context("write agent to passwd")?;
    }

    // Append agent group to /etc/group
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&group_path)
            .context("open group for append")?;
        writeln!(f, "agent:x:60000:")
            .context("write agent to group")?;
    }

    // Create home directory with correct ownership.
    // chown requires root — tolerate EPERM for non-root test environments.
    let home = rootfs.join("home/agent");
    fs::create_dir_all(&home)
        .context("create /home/agent")?;
    if let Err(e) = nix::unistd::chown(
        &home,
        Some(nix::unistd::Uid::from_raw(60000)),
        Some(nix::unistd::Gid::from_raw(60000)),
    ) {
        tracing::debug!("chown /home/agent: {e} (expected if not root)");
    }

    Ok(())
}

/// Sanitize nsswitch.conf for the pod environment.
///
/// Replaces the `hosts:` line to remove entries that depend on services not
/// available inside the pod (e.g. `mdns4_minimal [NOTFOUND=return]` requires
/// avahi-daemon). Without this, glibc's NSS resolution chain aborts before
/// reaching the `dns` source, and programs like curl/wget can't resolve any
/// hostnames.
fn sanitize_nsswitch_conf(rootfs_etc: &Path) -> Result<()> {
    let nsswitch_path = rootfs_etc.join("nsswitch.conf");
    if !nsswitch_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&nsswitch_path)
        .context("read nsswitch.conf")?;

    let mut modified = false;
    let new_content: String = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("hosts:") {
                modified = true;
                // Simple and reliable: files (for /etc/hosts) then dns
                "hosts:          files dns".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if modified {
        // Preserve trailing newline
        let new_content = if content.ends_with('\n') && !new_content.ends_with('\n') {
            new_content + "\n"
        } else {
            new_content
        };
        fs::write(&nsswitch_path, new_content)
            .context("write sanitized nsswitch.conf")?;
    }

    Ok(())
}

/// Return which of the MAYBE_SYMLINK_DIRS are real directories on the host
/// (not symlinks). These need bind-mounting at runtime.
pub fn real_system_dirs() -> Vec<&'static str> {
    MAYBE_SYMLINK_DIRS
        .iter()
        .copied()
        .filter(|dir| {
            let host_path = Path::new("/").join(dir);
            // Exists and is NOT a symlink → real dir that needs bind-mount
            match host_path.symlink_metadata() {
                Ok(m) => !m.is_symlink() && m.is_dir(),
                Err(_) => false,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Mount / unmount
// ---------------------------------------------------------------------------

/// Mount overlayfs. Called inside the child's mount namespace (pre_exec)
/// so the mount doesn't leak to the host.
///
/// Requires CAP_SYS_ADMIN (typically root).
pub fn mount_overlay(
    lower_dirs: &[PathBuf],
    upper: &Path,
    work: &Path,
    merged: &Path,
) -> std::io::Result<()> {
    use nix::mount::{mount, MsFlags};

    let lower = lower_dirs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");

    // index=off: required when lower and upper are on the same filesystem
    // (which happens when rootfs is under the same pod directory as upper/work).
    // metacopy=off: avoids metadata-only copy-up issues with bind-mounted content.
    let opts = format!(
        "lowerdir={lower},upperdir={},workdir={},index=off,metacopy=off",
        upper.display(),
        work.display(),
    );

    mount(
        Some("overlay"),
        merged,
        Some("overlay"),
        MsFlags::empty(),
        Some(opts.as_str()),
    )
    .map_err(|e| std::io::Error::other(format!(
        "mount overlayfs failed: {e}. Ensure envpod is running as root and the overlay kernel module is loaded"
    )))?;

    Ok(())
}

/// Unmount the overlay (lazy detach so open file handles drain).
pub fn unmount_overlay(merged: &Path) -> Result<()> {
    nix::mount::umount2(merged, nix::mount::MntFlags::MNT_DETACH)
        .context("unmount overlayfs")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Infrastructure file exclusions — files that should never appear in diff/commit
// ---------------------------------------------------------------------------

/// Paths (relative to root) that are managed by envpod infrastructure and must
/// never be committed to the host filesystem.
///
/// These files are generated by envpod's network/DNS setup and placed in the
/// overlay. Committing them would corrupt the host (e.g. overwriting the host's
/// /etc/resolv.conf symlink → systemd-resolved).
const EXCLUDED_PATHS: &[&str] = &[
    "etc/resolv.conf",
    "opt/.envpod-setup.sh",
];

/// Check if a relative path should be excluded from diff/commit.
fn is_excluded(rel_path: &Path) -> bool {
    let s = rel_path.to_string_lossy();
    EXCLUDED_PATHS.iter().any(|excl| s == *excl)
}

// ---------------------------------------------------------------------------
// Diff — walk the upper layer to find changes
// ---------------------------------------------------------------------------

/// Produce a list of file-level changes by walking the overlay's upper directory.
///
/// OverlayFS stores changes in the upper layer:
/// - Regular files/dirs    → added (new) or modified (exists in lower)
/// - Whiteout char dev 0/0 → file was deleted from the lower layer
pub fn diff(upper: &Path, lower_dirs: &[PathBuf]) -> Result<Vec<FileDiff>> {
    let mut diffs = Vec::new();
    if upper.exists() {
        let primary_lower = lower_dirs.first().map(|p| p.as_path()).unwrap_or(Path::new("/"));
        walk_upper_for_diff(upper, upper, primary_lower, &mut diffs)?;
    }
    Ok(diffs)
}

fn walk_upper_for_diff(
    upper_root: &Path,
    current: &Path,
    lower_root: &Path,
    diffs: &mut Vec<FileDiff>,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue, // skip unreadable entries
        };

        let rel_path = path
            .strip_prefix(upper_root)
            .context("path not under upper root")?;

        // Skip envpod infrastructure files (e.g. etc/resolv.conf)
        if is_excluded(rel_path) {
            continue;
        }

        // Path as seen from inside the pod (absolute)
        let pod_path = PathBuf::from("/").join(rel_path);

        // Corresponding path in the host filesystem
        let lower_path = lower_root.join(rel_path);

        if is_whiteout(&metadata) {
            diffs.push(FileDiff {
                path: pod_path,
                kind: DiffKind::Deleted,
                size: 0,
            });
        } else if metadata.is_dir() {
            // Recurse into subdirectories
            walk_upper_for_diff(upper_root, &path, lower_root, diffs)?;
        } else {
            let kind = if lower_path.exists() {
                DiffKind::Modified
            } else {
                DiffKind::Added
            };
            diffs.push(FileDiff {
                path: pod_path,
                kind,
                size: metadata.len(),
            });
        }
    }

    Ok(())
}

/// Diff for per-system-dir COW overlays (advanced/dangerous mode).
///
/// `sys_upper` is the base dir (e.g. `pod_dir/sys_upper/`) containing
/// subdirectories like `usr/`, `bin/`, etc. Each subdirectory is the upper
/// layer for that system dir's overlay. Files found here represent changes
/// the agent made to system directories.
pub fn diff_sys_upper(sys_upper: &Path) -> Result<Vec<FileDiff>> {
    let mut diffs = Vec::new();
    let entries = match fs::read_dir(sys_upper) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(diffs),
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        if !entry.metadata()?.is_dir() { continue; }
        let dir_name = entry.file_name();
        let sub_upper = entry.path();
        let host_lower = Path::new("/").join(&dir_name);
        walk_sys_upper_for_diff(&sub_upper, &sub_upper, &host_lower, &dir_name, &mut diffs)?;
    }

    Ok(diffs)
}

fn walk_sys_upper_for_diff(
    upper_root: &Path,
    current: &Path,
    lower_root: &Path,
    sys_dir_name: &std::ffi::OsStr,
    diffs: &mut Vec<FileDiff>,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let rel_path = path
            .strip_prefix(upper_root)
            .context("path not under sys upper root")?;

        // Absolute path as seen in the pod: /{sys_dir}/{rel_path}
        let pod_path = PathBuf::from("/").join(sys_dir_name).join(rel_path);

        if is_excluded(rel_path) { continue; }

        if is_whiteout(&metadata) {
            diffs.push(FileDiff {
                path: pod_path,
                kind: DiffKind::Deleted,
                size: 0,
            });
        } else if metadata.is_dir() {
            walk_sys_upper_for_diff(upper_root, &path, lower_root, sys_dir_name, diffs)?;
        } else {
            let lower_path = lower_root.join(rel_path);
            let kind = if lower_path.exists() {
                DiffKind::Modified
            } else {
                DiffKind::Added
            };
            diffs.push(FileDiff {
                path: pod_path,
                kind,
                size: metadata.len(),
            });
        }
    }

    Ok(())
}

/// Commit per-system-dir overlay changes to the host filesystem.
///
/// `sys_upper` is the base dir (e.g. `pod_dir/sys_upper/`) and `target`
/// is the host root (typically `/`) or an output directory.
pub fn commit_sys_upper(sys_upper: &Path, target: &Path) -> Result<()> {
    let entries = match fs::read_dir(sys_upper) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        if !entry.metadata()?.is_dir() { continue; }
        let dir_name = entry.file_name();
        let sub_upper = entry.path();
        let sub_target = target.join(&dir_name);
        commit_walk(&sub_upper, &sub_upper, &sub_target)?;

        // Clear sub-upper after commit
        fs::remove_dir_all(&sub_upper).ok();
        fs::create_dir_all(&sub_upper).ok();
    }

    Ok(())
}

/// Check if a file is an overlayfs whiteout (character device major=0 minor=0).
/// Whiteouts represent files deleted from the lower layer.
fn is_whiteout(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    if !metadata.file_type().is_char_device() {
        return false;
    }
    // Whiteout: rdev == makedev(0, 0) == 0
    metadata.rdev() == 0
}

// ---------------------------------------------------------------------------
// Commit — merge upper layer changes into the real filesystem
// ---------------------------------------------------------------------------

/// Copy all changes from the upper layer to the real filesystem.
///
/// **DANGER**: This modifies the host filesystem. The governance layer must
/// obtain explicit human approval before calling this.
pub fn commit(upper: &Path, lower_root: &Path) -> Result<()> {
    if upper.exists() {
        commit_walk(upper, upper, lower_root)?;

        // Clear the upper layer so diff shows no remaining changes.
        fs::remove_dir_all(upper).context("clear upper after commit")?;
        fs::create_dir_all(upper).context("recreate upper after commit")?;
    }
    Ok(())
}

fn commit_walk(upper_root: &Path, current: &Path, lower_root: &Path) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let rel_path = path
            .strip_prefix(upper_root)
            .context("path not under upper root")?;

        // Skip envpod infrastructure files — never commit them to the host.
        // e.g. etc/resolv.conf would corrupt the host's DNS via symlink.
        if is_excluded(rel_path) {
            continue;
        }

        let target = lower_root.join(rel_path);

        if is_whiteout(&metadata) {
            // Delete from real filesystem
            if target.is_dir() {
                fs::remove_dir_all(&target).ok();
            } else if target.exists() || target.symlink_metadata().is_ok() {
                fs::remove_file(&target).ok();
            }
        } else if metadata.is_dir() {
            fs::create_dir_all(&target)?;
            commit_walk(upper_root, &path, lower_root)?;
        } else {
            // Copy file (preserves content, not full permissions for now)
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target).with_context(|| {
                format!("commit: {} → {}", path.display(), target.display())
            })?;
        }
    }

    Ok(())
}

/// Commit only specific files from the upper layer to the real filesystem.
///
/// Unlike `commit()` which copies everything and clears the upper layer,
/// this moves only the specified paths and leaves everything else untouched.
/// After each file is removed from upper, empty parent directories are
/// cleaned up (up to the upper root).
pub fn commit_selective(upper: &Path, lower_root: &Path, paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        // Strip leading "/" to get relative path
        let rel_path = path.strip_prefix("/").unwrap_or(path);

        // Skip infrastructure files
        if is_excluded(rel_path) {
            anyhow::bail!(
                "cannot commit infrastructure file: {}",
                path.display()
            );
        }

        let upper_file = upper.join(rel_path);
        if !upper_file.exists() && upper_file.symlink_metadata().is_err() {
            anyhow::bail!(
                "path not found in overlay: {}",
                path.display()
            );
        }

        let target = lower_root.join(rel_path);
        let metadata = upper_file
            .symlink_metadata()
            .with_context(|| format!("stat upper file: {}", upper_file.display()))?;

        if is_whiteout(&metadata) {
            // Delete from real filesystem
            if target.is_dir() {
                fs::remove_dir_all(&target).ok();
            } else if target.exists() || target.symlink_metadata().is_ok() {
                fs::remove_file(&target).ok();
            }
            // Remove whiteout from upper
            fs::remove_file(&upper_file)
                .with_context(|| format!("remove whiteout: {}", upper_file.display()))?;
        } else if metadata.is_dir() {
            // For directories, copy contents recursively then remove from upper
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            commit_walk(upper, &upper_file, lower_root)?;
            fs::remove_dir_all(&upper_file)
                .with_context(|| format!("remove dir from upper: {}", upper_file.display()))?;
        } else {
            // Copy file to lower
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&upper_file, &target).with_context(|| {
                format!("commit: {} → {}", upper_file.display(), target.display())
            })?;
            // Remove from upper
            fs::remove_file(&upper_file)
                .with_context(|| format!("remove from upper: {}", upper_file.display()))?;
        }

        // Clean up empty parent directories in upper
        remove_empty_parents(&upper_file, upper);
    }

    Ok(())
}

/// Walk from `path.parent()` upward, removing empty directories.
/// Stops at `stop_at` (exclusive) or at the first non-empty directory.
fn remove_empty_parents(path: &Path, stop_at: &Path) {
    let mut current = match path.parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };

    while current != stop_at && current.starts_with(stop_at) {
        match fs::read_dir(&current) {
            Ok(mut entries) => {
                if entries.next().is_none() {
                    // Directory is empty — remove it
                    if fs::remove_dir(&current).is_err() {
                        break;
                    }
                } else {
                    break; // Non-empty, stop
                }
            }
            Err(_) => break,
        }
        current = match current.parent() {
            Some(p) => p.to_path_buf(),
            None => break,
        };
    }
}

// ---------------------------------------------------------------------------
// Rollback — discard all changes
// ---------------------------------------------------------------------------

/// Clear the upper and work directories, discarding all agent changes.
pub fn rollback(upper: &Path, work: &Path) -> Result<()> {
    if upper.exists() {
        fs::remove_dir_all(upper).context("clear upper dir")?;
        fs::create_dir_all(upper).context("recreate upper dir")?;
    }
    if work.exists() {
        fs::remove_dir_all(work).context("clear work dir")?;
        fs::create_dir_all(work).context("recreate work dir")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Destroy — remove everything
// ---------------------------------------------------------------------------

/// Remove all overlay directories for a pod.
pub fn destroy(pod_dir: &Path) -> Result<()> {
    if pod_dir.exists() {
        match fs::remove_dir_all(pod_dir) {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Processes may still be releasing file handles after SIGKILL.
                // Brief wait and retry once.
                std::thread::sleep(std::time::Duration::from_millis(500));
                fs::remove_dir_all(pod_dir).context("remove pod directory")?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// System path protection — prevent accidental commit of system dir changes
// ---------------------------------------------------------------------------

/// Top-level system directory prefixes that are protected during commit.
/// Changes under these paths require `--include-system` (advanced) or
/// produce a warning (dangerous).
pub const PROTECTED_SYSTEM_PATHS: &[&str] = &["usr", "bin", "sbin", "lib", "lib64"];

/// Check whether a relative path (no leading `/`) falls under a protected
/// system directory.
pub fn is_protected(rel_path: &Path) -> bool {
    // Get the first component of the path
    if let Some(first) = rel_path.components().next() {
        let s = first.as_os_str().to_string_lossy();
        PROTECTED_SYSTEM_PATHS.iter().any(|&p| s == p)
    } else {
        false
    }
}

/// Partition a list of diffs into `(safe, protected)` based on whether
/// they fall under a protected system directory.
pub fn partition_protected(diffs: Vec<FileDiff>) -> (Vec<FileDiff>, Vec<FileDiff>) {
    let mut safe = Vec::new();
    let mut protected = Vec::new();
    for d in diffs {
        let rel = d.path.strip_prefix("/").unwrap_or(&d.path);
        if is_protected(rel) {
            protected.push(d);
        } else {
            safe.push(d);
        }
    }
    (safe, protected)
}

// ---------------------------------------------------------------------------
// Tracking filter — scope diff/commit to watched paths
// ---------------------------------------------------------------------------

/// Filter a diff result to only include changes matching the tracking config.
///
/// - If `tracking.watch` is non-empty, keeps only paths that start with a watch prefix.
/// - Then removes any paths that start with an ignore prefix.
/// - If `tracking.watch` is empty, all paths pass the watch filter (only ignore applies).
pub fn filter_diff(diffs: Vec<FileDiff>, tracking: &TrackingConfig) -> Vec<FileDiff> {
    diffs
        .into_iter()
        .filter(|d| {
            let path = d.path.to_string_lossy();

            // Watch filter: if watch list is non-empty, path must match a prefix
            if !tracking.watch.is_empty() {
                let watched = tracking.watch.iter().any(|w| {
                    path == *w || path.starts_with(&format!("{w}/"))
                });
                if !watched {
                    return false;
                }
            }

            // Ignore filter: reject paths matching any ignore prefix
            let ignored = tracking.ignore.iter().any(|ig| {
                path == *ig || path.starts_with(&format!("{ig}/"))
            });
            !ignored
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Clone — snapshot base state and clone pod filesystems
// ---------------------------------------------------------------------------

/// Save the pod's rootfs and upper layer as a base pod for fast cloning.
///
/// Moves rootfs to `{bases_dir}/{base_name}/rootfs/` (shared by all
/// instances) and snapshots upper → `base_upper/` in the base dir.
/// The pod's rootfs is replaced with a symlink to the base.
///
/// After this call, the pod is an "instance" of the base — same as clones.
/// The base lives independently and can outlive any individual pod.
pub fn snapshot_base(pod_dir: &Path, bases_dir: &Path, base_name: &str) -> Result<()> {
    let base_pod_dir = bases_dir.join(base_name);
    if base_pod_dir.exists() {
        tracing::warn!("overwriting existing base '{base_name}' (clones from the old base still work)");
    }
    fs::create_dir_all(&base_pod_dir)
        .with_context(|| format!("create base dir: {}", base_pod_dir.display()))?;

    // Move rootfs to base dir (unless it's already a symlink from a previous snapshot)
    let pod_rootfs = pod_dir.join("rootfs");
    let base_rootfs = base_pod_dir.join("rootfs");
    if !pod_rootfs.symlink_metadata().map(|m| m.is_symlink()).unwrap_or(false) {
        // Real directory — move it to the base dir
        if base_rootfs.exists() {
            fs::remove_dir_all(&base_rootfs).context("remove old base rootfs")?;
        }
        fs::rename(&pod_rootfs, &base_rootfs)
            .context("move rootfs to base dir")?;
        // Replace with symlink
        std::os::unix::fs::symlink(&base_rootfs, &pod_rootfs)
            .context("symlink pod rootfs to base")?;
    }

    // Snapshot upper → base_pod_dir/base_upper
    let upper = pod_dir.join("upper");
    let base_upper = base_pod_dir.join("base_upper");
    if base_upper.exists() {
        fs::remove_dir_all(&base_upper).context("remove old base_upper")?;
    }
    let status = std::process::Command::new("cp")
        .args(["--reflink=auto", "-a", "--",
               &upper.to_string_lossy(),
               &base_upper.to_string_lossy()])
        .status()
        .context("cp base_upper to base dir")?;
    if !status.success() {
        anyhow::bail!("cp upper → base_upper failed (exit {})", status);
    }

    // Snapshot sys_upper if it exists (advanced/dangerous mode)
    let sys_upper = pod_dir.join("sys_upper");
    if sys_upper.exists() {
        let base_sys_upper = base_pod_dir.join("base_sys_upper");
        if base_sys_upper.exists() {
            fs::remove_dir_all(&base_sys_upper).context("remove old base_sys_upper")?;
        }
        let status = std::process::Command::new("cp")
            .args(["--reflink=auto", "-a", "--",
                   &sys_upper.to_string_lossy(),
                   &base_sys_upper.to_string_lossy()])
            .status()
            .context("cp base_sys_upper to base dir")?;
        if !status.success() {
            anyhow::bail!("cp sys_upper → base_sys_upper failed (exit {})", status);
        }
    }

    // Copy pod.yaml so clones inherit the configuration
    let pod_yaml = pod_dir.join("pod.yaml");
    if pod_yaml.exists() {
        fs::copy(&pod_yaml, base_pod_dir.join("pod.yaml"))
            .context("copy pod.yaml to base")?;
    }

    Ok(())
}

/// Check whether a base pod exists for the given name.
pub fn has_base(bases_dir: &Path, base_name: &str) -> bool {
    bases_dir.join(base_name).join("rootfs").exists()
}

/// Resolve the base pod name for a pod by following its rootfs symlink.
///
/// Returns `Some(base_name)` if the pod's rootfs is a symlink into a
/// bases directory, or `None` if it has a real (non-symlinked) rootfs.
pub fn resolve_base_name(pod_dir: &Path) -> Option<String> {
    let rootfs = pod_dir.join("rootfs");
    let target = std::fs::read_link(&rootfs).ok()?;
    // Expected: .../bases/<base_name>/rootfs
    let parent = target.parent()?; // .../bases/<base_name>
    parent.file_name()?.to_str().map(|s| s.to_string())
}

/// Destroy a base pod directory. Returns Ok even if the base doesn't exist.
pub fn destroy_base(bases_dir: &Path, base_name: &str) -> Result<()> {
    let base_pod_dir = bases_dir.join(base_name);
    if base_pod_dir.exists() {
        fs::remove_dir_all(&base_pod_dir)
            .with_context(|| format!("remove base dir: {}", base_pod_dir.display()))?;
    }
    Ok(())
}

/// Clone a pod's filesystem from a source pod to a new destination pod.
///
/// Symlinks rootfs to the shared base pod (resolved from the source pod's
/// symlink). Copies upper layer from the base's base_upper or the source's
/// current upper. Creates fresh work/merged/sys_work dirs and copies pod.yaml.
pub fn clone_filesystem(
    source_dir: &Path,
    dest_dir: &Path,
    use_current: bool,
) -> Result<()> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("create clone dir: {}", dest_dir.display()))?;

    // 1. Symlink rootfs to the shared base pod.
    //    Resolve the source's symlink to get the actual base path, so
    //    cloning a clone still points directly to the base (no chain).
    let src_rootfs = source_dir.join("rootfs");
    let base_rootfs = std::fs::canonicalize(&src_rootfs)
        .unwrap_or(src_rootfs.clone());
    let dst_rootfs = dest_dir.join("rootfs");
    std::os::unix::fs::symlink(&base_rootfs, &dst_rootfs)
        .with_context(|| format!("symlink rootfs: {} → {}", dst_rootfs.display(), base_rootfs.display()))?;

    // 2. Copy upper layer.
    //    --current: copy from source pod's live upper/
    //    default:   copy from the image's base_upper/
    let src_upper = if use_current {
        source_dir.join("upper")
    } else {
        // Find the base dir from the rootfs symlink
        let base_dir = base_rootfs.parent().unwrap_or(source_dir);
        let base = base_dir.join("base_upper");
        if !base.exists() {
            anyhow::bail!(
                "no base pod found — run setup first, or use --current to clone current state"
            );
        }
        base
    };
    let dst_upper = dest_dir.join("upper");
    let status = std::process::Command::new("cp")
        .args(["--reflink=auto", "-a", "--",
               &src_upper.to_string_lossy(),
               &dst_upper.to_string_lossy()])
        .status()
        .context("cp upper for clone")?;
    if !status.success() {
        anyhow::bail!("cp upper failed (exit {})", status);
    }

    // 3. Copy sys_upper if present (advanced/dangerous mode)
    let src_sys_upper = if use_current {
        source_dir.join("sys_upper")
    } else {
        let base_dir = base_rootfs.parent().unwrap_or(source_dir);
        base_dir.join("base_sys_upper")
    };
    if src_sys_upper.exists() {
        let dst_sys_upper = dest_dir.join("sys_upper");
        let status = std::process::Command::new("cp")
            .args(["--reflink=auto", "-a", "--",
                   &src_sys_upper.to_string_lossy(),
                   &dst_sys_upper.to_string_lossy()])
            .status()
            .context("cp sys_upper for clone")?;
        if !status.success() {
            anyhow::bail!("cp sys_upper failed (exit {})", status);
        }
    }

    // 4. Create fresh work/merged dirs (OverlayFS requires these to be empty)
    fs::create_dir_all(dest_dir.join("work")).context("create work dir")?;
    fs::create_dir_all(dest_dir.join("merged")).context("create merged dir")?;

    // 5. Create fresh sys_work dirs if sys_upper was copied
    if dest_dir.join("sys_upper").exists() {
        let sys_upper_dir = dest_dir.join("sys_upper");
        if let Ok(entries) = fs::read_dir(&sys_upper_dir) {
            for entry in entries.flatten() {
                if entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
                    let dir_name = entry.file_name();
                    fs::create_dir_all(dest_dir.join("sys_work").join(&dir_name))
                        .with_context(|| format!("create sys_work/{}", dir_name.to_string_lossy()))?;
                }
            }
        }
    }

    // 6. Copy pod.yaml
    let src_config = source_dir.join("pod.yaml");
    if src_config.exists() {
        fs::copy(&src_config, dest_dir.join("pod.yaml"))
            .context("copy pod.yaml")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_dirs_makes_all_three() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("test-pod");
        create_dirs(&pod_dir).unwrap();

        assert!(pod_dir.join("upper").is_dir());
        assert!(pod_dir.join("work").is_dir());
        assert!(pod_dir.join("merged").is_dir());
    }

    #[test]
    fn create_rootfs_makes_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("test-pod");
        fs::create_dir_all(&pod_dir).unwrap();
        create_rootfs(&pod_dir).unwrap();

        let rootfs = pod_dir.join("rootfs");
        assert!(rootfs.is_dir());

        // System dirs that get bind-mounted should exist as dirs
        assert!(rootfs.join("usr").is_dir());
        assert!(rootfs.join("etc").is_dir());

        // Empty structural dirs
        assert!(rootfs.join("proc").is_dir());
        assert!(rootfs.join("dev").is_dir());
        assert!(rootfs.join("sys").is_dir());
        assert!(rootfs.join("tmp").is_dir());
        assert!(rootfs.join("home").is_dir());
        assert!(rootfs.join("opt").is_dir());
        assert!(rootfs.join("var").is_dir());

        // /bin should exist as either a symlink or dir (depends on host)
        let bin = rootfs.join("bin");
        assert!(bin.symlink_metadata().is_ok(), "/bin should exist in rootfs");
    }

    #[test]
    fn diff_empty_upper_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        fs::create_dir_all(&upper).unwrap();

        let result = diff(&upper, &[PathBuf::from("/")]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn diff_detects_added_file() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        fs::create_dir_all(&upper).unwrap();

        // A file that definitely doesn't exist in lower (/)
        let unique = format!("envpod_test_added_{}", uuid::Uuid::new_v4());
        fs::write(upper.join(&unique), "new content").unwrap();

        let result = diff(&upper, &[PathBuf::from("/")]).unwrap();
        let found = result.iter().find(|d| d.path.to_string_lossy().contains(&unique));
        assert!(found.is_some(), "should detect added file");
        assert_eq!(found.unwrap().kind, DiffKind::Added);
        assert!(found.unwrap().size > 0);
    }

    #[test]
    fn diff_detects_modified_file() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("etc")).unwrap();

        // /etc/passwd exists on all Linux systems
        fs::write(upper.join("etc/passwd"), "modified content").unwrap();

        let result = diff(&upper, &[PathBuf::from("/")]).unwrap();
        let found = result
            .iter()
            .find(|d| d.path == PathBuf::from("/etc/passwd"));
        assert!(found.is_some(), "should detect modified /etc/passwd");
        assert_eq!(found.unwrap().kind, DiffKind::Modified);
    }

    #[test]
    fn diff_excludes_infrastructure_files() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("etc")).unwrap();

        // etc/resolv.conf is an infrastructure file — should be excluded
        fs::write(upper.join("etc/resolv.conf"), "nameserver 10.200.1.1").unwrap();
        // etc/passwd is a normal file — should be included
        fs::write(upper.join("etc/passwd"), "modified").unwrap();

        let result = diff(&upper, &[PathBuf::from("/")]).unwrap();

        let resolv = result.iter().find(|d| d.path == PathBuf::from("/etc/resolv.conf"));
        assert!(resolv.is_none(), "diff should exclude etc/resolv.conf");

        let passwd = result.iter().find(|d| d.path == PathBuf::from("/etc/passwd"));
        assert!(passwd.is_some(), "diff should include etc/passwd");
    }

    #[test]
    fn commit_excludes_infrastructure_files() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("etc")).unwrap();

        // etc/resolv.conf should NOT be committed
        fs::write(upper.join("etc/resolv.conf"), "nameserver 10.200.1.1").unwrap();
        // Regular files should be committed
        fs::write(upper.join("etc/hostname"), "my-pod").unwrap();

        commit(&upper, &fake_lower).unwrap();

        assert!(
            !fake_lower.join("etc/resolv.conf").exists(),
            "commit should NOT copy etc/resolv.conf to host"
        );
        assert!(
            fake_lower.join("etc/hostname").exists(),
            "commit should copy etc/hostname to host"
        );
    }

    #[test]
    fn diff_recurses_into_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        let unique = format!("envpod_test_nested_{}", uuid::Uuid::new_v4());
        fs::create_dir_all(upper.join("a/b/c")).unwrap();
        fs::write(upper.join(format!("a/b/c/{unique}")), "deep").unwrap();

        let result = diff(&upper, &[PathBuf::from("/")]).unwrap();
        let found = result.iter().find(|d| d.path.to_string_lossy().contains(&unique));
        assert!(found.is_some(), "should find file in nested directory");
        assert_eq!(found.unwrap().kind, DiffKind::Added);
    }

    #[test]
    fn commit_copies_files_to_lower() {
        let tmp = tempfile::tempdir().unwrap();

        // Fake lower (not the real /)
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("subdir")).unwrap();
        fs::write(upper.join("subdir/file.txt"), "committed").unwrap();

        commit(&upper, &fake_lower).unwrap();

        let target = fake_lower.join("subdir/file.txt");
        assert!(target.exists());
        assert_eq!(fs::read_to_string(target).unwrap(), "committed");
    }

    #[test]
    fn commit_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("a/b/c")).unwrap();
        fs::write(upper.join("a/b/c/deep.txt"), "deep content").unwrap();

        commit(&upper, &fake_lower).unwrap();

        let target = fake_lower.join("a/b/c/deep.txt");
        assert!(target.exists());
        assert_eq!(fs::read_to_string(target).unwrap(), "deep content");
    }

    #[test]
    fn rollback_clears_upper_and_work() {
        let tmp = tempfile::tempdir().unwrap();
        let upper = tmp.path().join("upper");
        let work = tmp.path().join("work");
        fs::create_dir_all(&upper).unwrap();
        fs::create_dir_all(&work).unwrap();

        fs::write(upper.join("file.txt"), "data").unwrap();
        fs::write(work.join("state"), "overlay state").unwrap();

        rollback(&upper, &work).unwrap();

        assert!(upper.is_dir(), "upper dir should still exist");
        assert!(work.is_dir(), "work dir should still exist");
        assert!(fs::read_dir(&upper).unwrap().count() == 0, "upper should be empty");
        assert!(fs::read_dir(&work).unwrap().count() == 0, "work should be empty");
    }

    #[test]
    fn destroy_removes_pod_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_dir = tmp.path().join("pod");
        create_dirs(&pod_dir).unwrap();
        fs::write(pod_dir.join("upper/file.txt"), "data").unwrap();

        destroy(&pod_dir).unwrap();
        assert!(!pod_dir.exists());
    }

    // -----------------------------------------------------------------------
    // Selective commit tests
    // -----------------------------------------------------------------------

    #[test]
    fn commit_selective_copies_only_specified() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("opt")).unwrap();
        fs::write(upper.join("opt/a.txt"), "aaa").unwrap();
        fs::write(upper.join("opt/b.txt"), "bbb").unwrap();
        fs::write(upper.join("opt/c.txt"), "ccc").unwrap();

        // Commit only /opt/a.txt
        commit_selective(
            &upper,
            &fake_lower,
            &[PathBuf::from("/opt/a.txt")],
        )
        .unwrap();

        // a.txt should be in lower and gone from upper
        assert!(fake_lower.join("opt/a.txt").exists());
        assert_eq!(fs::read_to_string(fake_lower.join("opt/a.txt")).unwrap(), "aaa");
        assert!(!upper.join("opt/a.txt").exists());

        // b.txt and c.txt should still be in upper, not in lower
        assert!(upper.join("opt/b.txt").exists());
        assert!(upper.join("opt/c.txt").exists());
        assert!(!fake_lower.join("opt/b.txt").exists());
        assert!(!fake_lower.join("opt/c.txt").exists());
    }

    #[test]
    fn commit_selective_handles_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(fake_lower.join("opt")).unwrap();
        fs::write(fake_lower.join("opt/existing.txt"), "original").unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("opt")).unwrap();

        // Create a whiteout (char device major=0 minor=0)
        // We can't easily create whiteouts in tests without root,
        // so test with a regular file deletion scenario instead.
        // The whiteout path is tested in the root-only integration tests.

        // Instead, verify that committing a modified file works
        fs::write(upper.join("opt/existing.txt"), "modified").unwrap();

        commit_selective(
            &upper,
            &fake_lower,
            &[PathBuf::from("/opt/existing.txt")],
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(fake_lower.join("opt/existing.txt")).unwrap(),
            "modified"
        );
        assert!(!upper.join("opt/existing.txt").exists());
    }

    #[test]
    fn commit_selective_cleans_empty_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join("a/b/c")).unwrap();
        fs::write(upper.join("a/b/c/deep.txt"), "deep").unwrap();

        commit_selective(
            &upper,
            &fake_lower,
            &[PathBuf::from("/a/b/c/deep.txt")],
        )
        .unwrap();

        // File should be committed
        assert!(fake_lower.join("a/b/c/deep.txt").exists());

        // Empty parent dirs a/b/c, a/b, a should all be cleaned from upper
        assert!(!upper.join("a/b/c").exists());
        assert!(!upper.join("a/b").exists());
        assert!(!upper.join("a").exists());
    }

    #[test]
    fn commit_selective_rejects_unknown_path() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_lower = tmp.path().join("lower");
        fs::create_dir_all(&fake_lower).unwrap();

        let upper = tmp.path().join("upper");
        fs::create_dir_all(&upper).unwrap();

        let result = commit_selective(
            &upper,
            &fake_lower,
            &[PathBuf::from("/nonexistent/file.txt")],
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not found in overlay"),
            "error should mention 'not found in overlay': {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // filter_diff tests
    // -----------------------------------------------------------------------

    fn make_diff(path: &str, kind: DiffKind) -> FileDiff {
        FileDiff {
            path: PathBuf::from(path),
            kind,
            size: 100,
        }
    }

    #[test]
    fn filter_diff_watch_only() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/var/lib/dpkg/status", DiffKind::Modified),
            make_diff("/opt/app/main.py", DiffKind::Added),
        ];
        let tracking = TrackingConfig {
            watch: vec!["/home".into(), "/opt".into()],
            ignore: vec![],
        };
        let filtered = filter_diff(diffs, &tracking);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].path, PathBuf::from("/home/user/file.txt"));
        assert_eq!(filtered[1].path, PathBuf::from("/opt/app/main.py"));
    }

    #[test]
    fn filter_diff_ignore_only() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/var/cache/apt/pkgcache.bin", DiffKind::Modified),
            make_diff("/tmp/scratch", DiffKind::Added),
        ];
        let tracking = TrackingConfig {
            watch: vec![],
            ignore: vec!["/var/cache".into(), "/tmp".into()],
        };
        let filtered = filter_diff(diffs, &tracking);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, PathBuf::from("/home/user/file.txt"));
    }

    #[test]
    fn filter_diff_watch_and_ignore() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/home/user/.cache/junk", DiffKind::Added),
            make_diff("/var/lib/dpkg/status", DiffKind::Modified),
            make_diff("/opt/app/main.py", DiffKind::Modified),
        ];
        let tracking = TrackingConfig {
            watch: vec!["/home".into(), "/opt".into()],
            ignore: vec!["/home/user/.cache".into()],
        };
        let filtered = filter_diff(diffs, &tracking);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].path, PathBuf::from("/home/user/file.txt"));
        assert_eq!(filtered[1].path, PathBuf::from("/opt/app/main.py"));
    }

    #[test]
    fn filter_diff_empty_config_returns_all() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/var/lib/dpkg/status", DiffKind::Modified),
            make_diff("/etc/hostname", DiffKind::Modified),
        ];
        let tracking = TrackingConfig {
            watch: vec![],
            ignore: vec![],
        };
        let filtered = filter_diff(diffs, &tracking);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn filter_diff_exact_path_match() {
        // Ensure /home matches /home/... but not /homepage
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/homepage/index.html", DiffKind::Added),
        ];
        let tracking = TrackingConfig {
            watch: vec!["/home".into()],
            ignore: vec![],
        };
        let filtered = filter_diff(diffs, &tracking);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, PathBuf::from("/home/user/file.txt"));
    }

    // -----------------------------------------------------------------------
    // System path protection tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_protected_detects_system_dirs() {
        assert!(is_protected(Path::new("usr/bin/curl")));
        assert!(is_protected(Path::new("usr/lib/libz.so")));
        assert!(is_protected(Path::new("bin/bash")));
        assert!(is_protected(Path::new("sbin/init")));
        assert!(is_protected(Path::new("lib/x86_64-linux-gnu/libc.so")));
        assert!(is_protected(Path::new("lib64/ld-linux.so")));
    }

    #[test]
    fn is_protected_allows_non_system_dirs() {
        assert!(!is_protected(Path::new("home/user/file.txt")));
        assert!(!is_protected(Path::new("opt/app/main.py")));
        assert!(!is_protected(Path::new("etc/hostname")));
        assert!(!is_protected(Path::new("var/log/syslog")));
        assert!(!is_protected(Path::new("workspace/project/src")));
    }

    #[test]
    fn partition_protected_splits_correctly() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/usr/bin/curl", DiffKind::Modified),
            make_diff("/opt/app/main.py", DiffKind::Added),
            make_diff("/lib/x86_64-linux-gnu/libc.so", DiffKind::Modified),
            make_diff("/bin/custom-tool", DiffKind::Added),
        ];
        let (safe, protected) = partition_protected(diffs);
        assert_eq!(safe.len(), 2);
        assert_eq!(safe[0].path, PathBuf::from("/home/user/file.txt"));
        assert_eq!(safe[1].path, PathBuf::from("/opt/app/main.py"));
        assert_eq!(protected.len(), 3);
        assert_eq!(protected[0].path, PathBuf::from("/usr/bin/curl"));
        assert_eq!(protected[1].path, PathBuf::from("/lib/x86_64-linux-gnu/libc.so"));
        assert_eq!(protected[2].path, PathBuf::from("/bin/custom-tool"));
    }

    #[test]
    fn partition_protected_all_safe() {
        let diffs = vec![
            make_diff("/home/user/file.txt", DiffKind::Added),
            make_diff("/opt/app/main.py", DiffKind::Modified),
        ];
        let (safe, protected) = partition_protected(diffs);
        assert_eq!(safe.len(), 2);
        assert!(protected.is_empty());
    }

    #[test]
    fn partition_protected_all_protected() {
        let diffs = vec![
            make_diff("/usr/bin/curl", DiffKind::Added),
            make_diff("/lib/libz.so", DiffKind::Modified),
        ];
        let (safe, protected) = partition_protected(diffs);
        assert!(safe.is_empty());
        assert_eq!(protected.len(), 2);
    }
}
