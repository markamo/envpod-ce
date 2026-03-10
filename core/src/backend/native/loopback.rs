// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Loopback disk image management for capped overlay storage.
//!
//! When `disk_size` is set in pod.yaml, the overlay upper and work dirs
//! live on a fixed-size ext4 filesystem backed by a sparse loopback file.
//! This prevents a pod from filling the host disk.

use std::path::Path;

use anyhow::{Context, Result};

/// Create a sparse disk image, format with ext4, mount it, and create
/// the overlay `upper/` and `work/` dirs inside.
pub fn setup_disk_image(pod_dir: &Path, size_bytes: u64) -> Result<()> {
    let img_path = pod_dir.join("disk.img");
    let mount_point = pod_dir.join("disk_mount");

    // Create sparse file (doesn't allocate blocks until written)
    let status = std::process::Command::new("truncate")
        .args(["-s", &size_bytes.to_string(), &img_path.to_string_lossy().to_string()])
        .status()
        .context("run truncate")?;
    anyhow::ensure!(status.success(), "truncate failed (exit {})", status);

    // Format with ext4 (quiet, no journaling for speed)
    let status = std::process::Command::new("mkfs.ext4")
        .args(["-F", "-q", "-O", "^has_journal", &img_path.to_string_lossy().to_string()])
        .status()
        .context("run mkfs.ext4")?;
    anyhow::ensure!(status.success(), "mkfs.ext4 failed (exit {})", status);

    // Create mount point and mount
    std::fs::create_dir_all(&mount_point)
        .context("create disk_mount directory")?;

    let status = std::process::Command::new("mount")
        .args(["-o", "loop", &img_path.to_string_lossy().to_string(), &mount_point.to_string_lossy().to_string()])
        .status()
        .context("mount loopback")?;
    anyhow::ensure!(status.success(), "mount loopback failed (exit {})", status);

    // Create upper and work dirs on the mounted filesystem
    // (overlayfs requires both on the same filesystem)
    std::fs::create_dir_all(mount_point.join("upper"))
        .context("create upper dir on disk image")?;
    std::fs::create_dir_all(mount_point.join("work"))
        .context("create work dir on disk image")?;

    Ok(())
}

/// Remount the disk image (e.g. after pod stop + start).
pub fn remount_disk_image(pod_dir: &Path) -> Result<()> {
    let img_path = pod_dir.join("disk.img");
    let mount_point = pod_dir.join("disk_mount");

    if !img_path.exists() {
        anyhow::bail!("disk.img not found at {}", img_path.display());
    }

    std::fs::create_dir_all(&mount_point)?;

    let status = std::process::Command::new("mount")
        .args(["-o", "loop", &img_path.to_string_lossy().to_string(), &mount_point.to_string_lossy().to_string()])
        .status()
        .context("remount loopback")?;
    anyhow::ensure!(status.success(), "remount loopback failed (exit {})", status);

    Ok(())
}

/// Check if the disk_mount is currently mounted.
pub fn is_mounted(pod_dir: &Path) -> bool {
    let mount_point = pod_dir.join("disk_mount");
    // Check /proc/mounts for the mount point
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        let mp_str = mount_point.to_string_lossy();
        mounts.lines().any(|line| line.contains(mp_str.as_ref()))
    } else {
        false
    }
}

/// Unmount the loopback device (preserves the disk.img for restart).
pub fn unmount_disk_image(pod_dir: &Path) -> Result<()> {
    let mount_point = pod_dir.join("disk_mount");
    if !mount_point.exists() {
        return Ok(());
    }

    let status = std::process::Command::new("umount")
        .arg(&mount_point)
        .status()
        .context("umount loopback")?;

    if !status.success() {
        // Lazy unmount as fallback
        std::process::Command::new("umount")
            .args(["-l", &mount_point.to_string_lossy().to_string()])
            .status()
            .ok();
    }

    Ok(())
}

/// Unmount and delete the disk image (for pod destroy).
pub fn destroy_disk_image(pod_dir: &Path) -> Result<()> {
    unmount_disk_image(pod_dir)?;

    let img_path = pod_dir.join("disk.img");
    if img_path.exists() {
        std::fs::remove_file(&img_path)
            .context("remove disk.img")?;
    }

    let mount_point = pod_dir.join("disk_mount");
    if mount_point.exists() {
        std::fs::remove_dir_all(&mount_point).ok();
    }

    Ok(())
}

