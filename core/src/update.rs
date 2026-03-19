// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Update checker — version check + screening rules update.
//!
//! On `envpod init` and `envpod clone`, fetches a single JSON from envpod.dev
//! to check for new versions and updated screening rules. Non-blocking, fails
//! silently, 2-second timeout. Results cached for 24 hours.
//!
//! The update.json endpoint is a static file on Cloudflare — every request
//! is counted for install telemetry.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

const UPDATE_URL: &str = "https://envpod.dev/update.json";
const CHECK_INTERVAL: Duration = Duration::from_secs(86400); // 24 hours
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
pub struct UpdateInfo {
    pub envpod: EnvpodVersion,
    pub screening: ScreeningVersion,
}

#[derive(Debug, Deserialize)]
pub struct EnvpodVersion {
    pub latest: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct ScreeningVersion {
    pub latest: String,
    pub url: String,
}

/// Result of an update check.
pub struct UpdateCheckResult {
    /// If set, a newer envpod version is available.
    pub new_version: Option<String>,
    /// If true, screening rules were updated.
    pub rules_updated: bool,
}

/// Path to the last-check timestamp file.
fn last_check_path(base_dir: &Path) -> PathBuf {
    base_dir.join("screening").join(".last-update-check")
}

/// Check if enough time has passed since the last update check.
fn should_check(base_dir: &Path) -> bool {
    let path = last_check_path(base_dir);
    match fs::metadata(&path) {
        Ok(meta) => {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                    return elapsed >= CHECK_INTERVAL;
                }
            }
            true // can't determine age, check anyway
        }
        Err(_) => true, // file doesn't exist, first run
    }
}

/// Record that we just checked.
fn mark_checked(base_dir: &Path) {
    let path = last_check_path(base_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&path, "").ok();
}

/// Perform the update check. Non-blocking, 2-second timeout, fails silently.
///
/// Returns None if the check was skipped (too recent) or failed.
pub fn check_for_updates(
    base_dir: &Path,
    current_version: &str,
) -> Option<UpdateCheckResult> {
    if !should_check(base_dir) {
        return None;
    }

    // Spawn a blocking HTTP request with timeout
    let response = fetch_update_json()?;
    mark_checked(base_dir);

    let mut result = UpdateCheckResult {
        new_version: None,
        rules_updated: false,
    };

    // Check envpod version
    if version_newer(&response.envpod.latest, current_version) {
        result.new_version = Some(response.envpod.latest);
    }

    // Check screening rules version
    let rules_path = base_dir.join("screening").join("rules.json");
    let current_rules_version = read_rules_version(&rules_path);
    if current_rules_version.as_deref() != Some(&response.screening.latest) {
        // Download new rules
        if let Some(new_rules) = fetch_url(&response.screening.url) {
            // Verify it's valid JSON before writing
            if serde_json::from_str::<serde_json::Value>(&new_rules).is_ok() {
                fs::create_dir_all(base_dir.join("screening")).ok();
                if fs::write(&rules_path, &new_rules).is_ok() {
                    result.rules_updated = true;
                }
            }
        }
    }

    Some(result)
}

/// Fetch the update.json from envpod.dev.
fn fetch_update_json() -> Option<UpdateInfo> {
    let body = fetch_url(UPDATE_URL)?;
    serde_json::from_str(&body).ok()
}

/// Simple blocking HTTP GET with timeout.
fn fetch_url(url: &str) -> Option<String> {
    // Use a simple subprocess curl — avoids adding an HTTP client dependency
    // for a non-critical feature. curl is available on all supported platforms.
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time", "2",
            "--connect-timeout", "2",
            url,
        ])
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

/// Compare two semver strings. Returns true if `latest` is newer than `current`.
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    l > c
}

/// Read the version field from an existing rules.json file.
fn read_rules_version(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(version_newer("0.1.2", "0.1.1"));
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(!version_newer("0.1.1", "0.1.1"));
        assert!(!version_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn version_with_prefix() {
        assert!(version_newer("v0.1.2", "0.1.1"));
        assert!(version_newer("0.1.2", "v0.1.1"));
    }
}
