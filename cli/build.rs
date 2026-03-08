use std::process::Command;

fn main() {
    // Git short hash (8 chars)
    let hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Build timestamp (UTC, compact)
    let ts = Command::new("date")
        .args(["-u", "+%Y%m%d-%H%M"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Check for dirty working tree
    let dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    let suffix = if dirty { "-dirty" } else { "" };

    println!("cargo:rustc-env=ENVPOD_BUILD_HASH={hash}{suffix}");
    println!("cargo:rustc-env=ENVPOD_BUILD_TIME={ts}");

    // Rebuild when git HEAD changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
}
