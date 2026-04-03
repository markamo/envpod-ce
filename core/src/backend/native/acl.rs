// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! POSIX ACL management for COW overlay mounts.
//!
//! When a mount uses COW overlay (`cow: true`), the agent (UID 60000) needs
//! read/write permission on host files so overlayfs copy-up works correctly.
//! We use `setfacl` to grant the agent user access without changing file
//! ownership. ACLs are set at pod start and removed at pod destroy.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// Grant agent user read/write access to all files under `path`.
/// Uses `setfacl -R -m u:{uid}:rwX` — capital X grants execute only on
/// directories and files that already have execute permission.
pub fn set_agent_acl(path: &Path, uid: u32) -> Result<()> {
    let status = Command::new("setfacl")
        .args(["-R", "-m", &format!("u:{uid}:rwX"), &path.to_string_lossy()])
        .status()
        .context("failed to run setfacl")?;
    if !status.success() {
        anyhow::bail!(
            "setfacl failed on {}: exit code {:?}",
            path.display(),
            status.code()
        );
    }
    tracing::info!(path = %path.display(), uid, "set agent ACL for COW mount");
    Ok(())
}

/// Remove agent user ACL entries from all files under `path`.
pub fn remove_agent_acl(path: &Path, uid: u32) -> Result<()> {
    let status = Command::new("setfacl")
        .args(["-R", "-x", &format!("u:{uid}"), &path.to_string_lossy()])
        .status()
        .context("failed to run setfacl")?;
    if !status.success() {
        // Non-fatal: ACL might already be removed or path might not exist
        tracing::warn!(
            path = %path.display(),
            uid,
            "setfacl remove failed (non-fatal)"
        );
    } else {
        tracing::info!(path = %path.display(), uid, "removed agent ACL");
    }
    Ok(())
}
