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

    // Edition: read from edition.toml (single source of truth for branding)
    let edition_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("edition.toml");
    let content = if edition_file.exists() {
        std::fs::read_to_string(&edition_file).unwrap_or_default()
    } else {
        String::new()
    };

    let read_field = |key: &str, default: &str| -> String {
        content.lines()
            .find(|l| l.starts_with(key) && l.contains('='))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_matches('"').to_string())
            .or_else(|| std::env::var(&format!("ENVPOD_{}", key.to_uppercase())).ok())
            .unwrap_or_else(|| default.to_string())
    };

    let edition = read_field("edition", "CE");
    let pubkey = read_field("public_key", "");
    let product = read_field("product", "envpod");
    let tagline = read_field("tagline", "Zero-trust governance environments for AI agents");
    let author = read_field("author", "");
    let company = read_field("company", "");
    let email = read_field("email", "");
    let website = read_field("website", "https://envpod.dev");
    let lic = read_field("license", "BSL-1.1");
    let patent = read_field("patent", "");
    let motto = read_field("motto", "Docker isolates. Envpod governs.");

    println!("cargo:rustc-env=ENVPOD_EDITION={edition}");
    println!("cargo:rustc-env=ENVPOD_PRODUCT={product}");
    println!("cargo:rustc-env=ENVPOD_TAGLINE={tagline}");
    println!("cargo:rustc-env=ENVPOD_AUTHOR={author}");
    println!("cargo:rustc-env=ENVPOD_COMPANY={company}");
    println!("cargo:rustc-env=ENVPOD_EMAIL={email}");
    println!("cargo:rustc-env=ENVPOD_WEBSITE={website}");
    println!("cargo:rustc-env=ENVPOD_LICENSE_TYPE={lic}");
    println!("cargo:rustc-env=ENVPOD_PATENT={patent}");
    println!("cargo:rustc-env=ENVPOD_MOTTO={motto}");
    if !pubkey.is_empty() {
        println!("cargo:rustc-env=ENVPOD_LICENSE_PUBKEY={pubkey}");
    }
    println!("cargo:rerun-if-changed=edition.toml");

    // Rebuild when git HEAD changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
}
