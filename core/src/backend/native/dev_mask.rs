// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Device masking for pod isolation.
//!
//! Replaces the blanket host `/dev` bind-mount with a minimal `/dev` tree
//! containing only essential pseudo-devices. GPU devices (NVIDIA, DRI) are
//! only exposed when explicitly opted in via `DevicesConfig::gpu`.
//!
//! Also masks GPU-related info in `/proc` and `/sys` when GPU access is denied,
//! preventing agents from fingerprinting host GPU hardware.

use std::io;
use std::path::Path;

use nix::mount::MsFlags;

use crate::config::DevicesConfig;

/// Essential device nodes that every process needs.
const ESSENTIAL_DEVICES: &[&str] = &[
    "null", "zero", "full", "random", "urandom", "tty",
];

/// Optional device that may not exist on all systems.
const OPTIONAL_DEVICES: &[&str] = &["console"];

/// NVIDIA device nodes for GPU passthrough.
const NVIDIA_DEVICES: &[&str] = &[
    "nvidia0",
    "nvidia1",
    "nvidia2",
    "nvidia3",
    "nvidiactl",
    "nvidia-modeset",
    "nvidia-uvm",
    "nvidia-uvm-tools",
];

/// DRI device nodes for GPU passthrough.
const DRI_DEVICES: &[&str] = &[
    "dri/card0",
    "dri/card1",
    "dri/renderD128",
    "dri/renderD129",
];

/// ALSA/sound device nodes for audio passthrough.
const SND_DEVICES: &[&str] = &[
    "snd/controlC0",
    "snd/controlC1",
    "snd/pcmC0D0c",
    "snd/pcmC0D0p",
    "snd/pcmC0D1c",
    "snd/pcmC0D1p",
    "snd/pcmC1D0c",
    "snd/pcmC1D0p",
    "snd/seq",
    "snd/timer",
];

/// GPU-related paths in /proc and /sys to mask when GPU is denied.
const GPU_INFO_PATHS: &[&str] = &[
    "proc/driver/nvidia",
    "sys/module/nvidia",
    "sys/class/drm",
    "sys/bus/pci/drivers/nvidia",
];

/// Set up a minimal `/dev` tree in the overlay merged directory.
///
/// Replaces the old blanket bind-mount of host `/dev`. Steps:
/// 1. Mount tmpfs on `{merged}/dev`
/// 2. Bind-mount essential devices from host
/// 3. Set up devpts for PTY support
/// 4. Create standard symlinks (stdin, stdout, stderr, fd)
/// 5. Optionally bind-mount GPU devices
/// 6. Bind-mount any extra device paths
/// 7. Mount pod-private `/dev/shm` tmpfs
pub fn setup_minimal_dev(
    merged: &Path,
    devices: &DevicesConfig,
    shm_bytes: u64,
) -> io::Result<()> {
    let dev_path = merged.join("dev");
    std::fs::create_dir_all(&dev_path)?;

    // 1. Mount tmpfs on /dev (5MB, mode 0755)
    nix::mount::mount(
        Some("tmpfs"),
        &dev_path,
        Some("tmpfs"),
        MsFlags::MS_NOSUID,
        Some("size=5m,mode=0755"),
    )
    .map_err(nix_to_io)?;

    // 2. Bind-mount essential device nodes
    for name in ESSENTIAL_DEVICES {
        bind_device_node(&dev_path, name)?;
    }
    for name in OPTIONAL_DEVICES {
        bind_device_node_optional(&dev_path, name);
    }

    // 3. Set up devpts
    setup_devpts(&dev_path)?;

    // 4. Create standard symlinks
    std::os::unix::fs::symlink("/proc/self/fd/0", dev_path.join("stdin"))?;
    std::os::unix::fs::symlink("/proc/self/fd/1", dev_path.join("stdout"))?;
    std::os::unix::fs::symlink("/proc/self/fd/2", dev_path.join("stderr"))?;
    std::os::unix::fs::symlink("/proc/self/fd", dev_path.join("fd"))?;

    // 5. GPU devices (opt-in)
    if devices.gpu {
        setup_gpu_devices(&dev_path);
    }

    // 5.5. Audio devices (opt-in)
    if devices.audio {
        setup_audio_devices(&dev_path);
    }

    // 6. Extra device paths
    for extra in &devices.extra {
        let name = extra.strip_prefix("/dev/").unwrap_or(extra);
        bind_device_node_optional(&dev_path, name);
    }

    // 7. Pod-private /dev/shm
    let shm_path = dev_path.join("shm");
    std::fs::create_dir_all(&shm_path)?;
    let shm_opts = format!("size={shm_bytes},mode=1777");
    nix::mount::mount(
        Some("tmpfs"),
        &shm_path,
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        Some(shm_opts.as_str()),
    )
    .map_err(nix_to_io)?;

    Ok(())
}

/// Bind-mount a single device node from host `/dev` into the pod's `/dev`.
///
/// Creates parent directories and the mount point file as needed.
fn bind_device_node(dev_path: &Path, name: &str) -> io::Result<()> {
    let source = Path::new("/dev").join(name);
    let target = dev_path.join(name);

    if !source.exists() {
        return Err(io::Error::other(format!(
            "essential device /dev/{name} not found on host"
        )));
    }

    // Create parent directories if needed (e.g., for "dri/card0")
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create mount point (empty file for device nodes)
    if source.is_dir() {
        std::fs::create_dir_all(&target)?;
    } else {
        std::fs::File::create(&target)?;
    }

    nix::mount::mount(
        Some(source.as_path()),
        &target,
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )
    .map_err(|e| io::Error::other(format!("bind mount /dev/{name}: {e}")))?;

    Ok(())
}

/// Bind-mount a device node if it exists on the host. Skip silently if not.
fn bind_device_node_optional(dev_path: &Path, name: &str) {
    let source = Path::new("/dev").join(name);
    if source.exists() {
        let _ = bind_device_node(dev_path, name);
    }
}

/// Set up devpts for PTY support.
///
/// Bind-mounts the host's `/dev/pts` so inherited PTY devices (stdin) are
/// visible inside the pod. This is required for `ttyname()` to resolve the
/// PTY path — Node.js/libuv calls `ttyname_r()` + `open()` to re-open the
/// TTY during `uv_tty_init()`. Without the host PTY visible, TUI apps
/// (Claude Code, vim, etc.) can't initialize their terminal input.
///
/// Trade-off: the pod can see all host PTY devices. This is acceptable
/// because pods are single-agent environments with strong network/filesystem
/// isolation, and PTY visibility is a low-risk attack vector.
fn setup_devpts(dev_path: &Path) -> io::Result<()> {
    let pts_path = dev_path.join("pts");
    std::fs::create_dir_all(&pts_path)?;

    nix::mount::mount(
        Some("/dev/pts"),
        &pts_path,
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )
    .map_err(|e| io::Error::other(format!("bind mount /dev/pts: {e}")))?;

    // Symlink /dev/ptmx → pts/ptmx
    std::os::unix::fs::symlink("pts/ptmx", dev_path.join("ptmx"))?;

    Ok(())
}

/// Bind-mount NVIDIA and DRI device nodes for GPU passthrough.
fn setup_gpu_devices(dev_path: &Path) {
    for name in NVIDIA_DEVICES {
        bind_device_node_optional(dev_path, name);
    }
    for name in DRI_DEVICES {
        bind_device_node_optional(dev_path, name);
    }
}

/// Bind-mount ALSA/sound device nodes for audio passthrough.
fn setup_audio_devices(dev_path: &Path) {
    for name in SND_DEVICES {
        bind_device_node_optional(dev_path, name);
    }
}

/// Mask GPU-related info in /proc and /sys when GPU access is denied.
///
/// Mounts empty read-only tmpfs over GPU info paths to prevent agents from
/// fingerprinting host GPU hardware. Only masks paths that exist on the host.
/// Non-fatal — same pattern as proc_mask.rs.
pub fn mask_gpu_info(merged: &Path) -> io::Result<()> {
    for rel_path in GPU_INFO_PATHS {
        let target = merged.join(rel_path);
        if target.exists() && target.is_dir() {
            nix::mount::mount(
                Some("tmpfs"),
                &target,
                Some("tmpfs"),
                MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
                Some("size=0,mode=0555"),
            )
            .map_err(|e| io::Error::other(format!("mask GPU info {rel_path}: {e}")))?;
        }
    }
    Ok(())
}

/// Convert nix::Error to std::io::Error.
fn nix_to_io(e: nix::Error) -> io::Error {
    io::Error::other(e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn essential_devices_list_is_complete() {
        // Verify all essential device names are well-known pseudo-devices
        for name in ESSENTIAL_DEVICES {
            let path = Path::new("/dev").join(name);
            assert!(
                path.exists(),
                "essential device /dev/{name} should exist on host"
            );
        }
    }

    #[test]
    fn gpu_devices_are_known_paths() {
        // Verify NVIDIA device naming convention
        for name in NVIDIA_DEVICES {
            assert!(
                name.starts_with("nvidia"),
                "NVIDIA device should start with 'nvidia': {name}"
            );
        }
        // Verify DRI device naming convention
        for name in DRI_DEVICES {
            assert!(
                name.starts_with("dri/"),
                "DRI device should start with 'dri/': {name}"
            );
        }
    }

    #[test]
    fn default_devices_config_denies_gpu() {
        let config = DevicesConfig::default();
        assert!(!config.gpu, "default DevicesConfig should deny GPU access");
        assert!(config.extra.is_empty());
    }

    #[test]
    fn gpu_info_paths_are_relative() {
        for path in GPU_INFO_PATHS {
            assert!(
                !path.starts_with('/'),
                "GPU info path should be relative: {path}"
            );
        }
    }

    #[test]
    #[ignore = "requires root — run with: sudo cargo test -- --ignored"]
    fn setup_minimal_dev_creates_essential_nodes() {
        let tmp = tempfile::tempdir().unwrap();
        let merged = tmp.path().join("merged");
        std::fs::create_dir_all(&merged).unwrap();

        let devices = DevicesConfig::default();
        setup_minimal_dev(&merged, &devices, 64 * 1024 * 1024).unwrap();

        // Essential devices should exist
        for name in ESSENTIAL_DEVICES {
            let path = merged.join("dev").join(name);
            assert!(path.exists(), "/dev/{name} should exist in minimal dev");
        }

        // GPU devices should NOT exist (default config)
        for name in NVIDIA_DEVICES {
            let path = merged.join("dev").join(name);
            assert!(!path.exists(), "/dev/{name} should NOT exist without GPU opt-in");
        }

        // Symlinks should exist
        let stdin = merged.join("dev/stdin");
        assert!(stdin.is_symlink(), "/dev/stdin should be a symlink");

        // devpts should be mounted
        let pts = merged.join("dev/pts");
        assert!(pts.is_dir(), "/dev/pts should exist");

        // /dev/shm should be mounted
        let shm = merged.join("dev/shm");
        assert!(shm.is_dir(), "/dev/shm should exist");
    }
}
