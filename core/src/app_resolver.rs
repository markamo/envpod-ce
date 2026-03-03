use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub struct ResolvedApp {
    pub name: String,
    pub binary: PathBuf,
    pub paths: Vec<PathBuf>,
}

/// Resolve a binary name to all paths needed to run it.
pub fn resolve_app(name: &str) -> Result<ResolvedApp> {
    let binary = resolve_binary(name)?;

    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();

    // Add the binary's parent directory
    if let Some(parent) = binary.parent() {
        dirs.insert(parent.to_path_buf());
    }

    // Follow symlinks to the real binary location
    if let Ok(real) = std::fs::canonicalize(&binary) {
        if let Some(parent) = real.parent() {
            dirs.insert(parent.to_path_buf());
        }
    }

    // Resolve shared library dependencies via ldd
    if let Ok(ldd_output) = run_ldd(&binary) {
        for dir in parse_ldd_dirs(&ldd_output) {
            dirs.insert(dir);
        }
    }

    // Add known data directories for specific apps
    for dir in known_data_dirs(name) {
        if dir.exists() {
            dirs.insert(dir);
        }
    }

    let paths: Vec<PathBuf> = dirs.into_iter().filter(|p| p.exists()).collect();

    Ok(ResolvedApp {
        name: name.to_string(),
        binary,
        paths,
    })
}

fn resolve_binary(name: &str) -> Result<PathBuf> {
    let output = Command::new("which")
        .arg(name)
        .output()
        .context("failed to run which")?;

    if !output.status.success() {
        bail!("binary '{}' not found on host", name);
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(path))
}

fn run_ldd(binary: &PathBuf) -> Result<String> {
    let output = Command::new("ldd")
        .arg(binary)
        .output()
        .context("failed to run ldd")?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse ldd output and return deduplicated parent directories of resolved libraries.
pub fn parse_ldd_dirs(ldd_output: &str) -> Vec<PathBuf> {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();

    for line in ldd_output.lines() {
        let line = line.trim();

        // Skip virtual/kernel libraries
        if line.starts_with("linux-vdso")
            || line.starts_with("linux-gate")
            || line.contains("ld-linux")
            || line.is_empty()
        {
            continue;
        }

        // Format: "libfoo.so => /path/to/libfoo.so (0x...)"
        if let Some(path_str) = line.split("=>").nth(1) {
            let path_str = path_str.trim();
            // Strip the address suffix "(0x...)"
            if let Some(path_str) = path_str.split_whitespace().next() {
                let path = PathBuf::from(path_str);
                if path.is_absolute() {
                    if let Some(parent) = path.parent() {
                        dirs.insert(parent.to_path_buf());
                    }
                }
            }
        } else if line.starts_with('/') {
            // Format: "/lib64/ld-linux-x86-64.so.2 (0x...)" — already filtered above
            // but handle direct paths like "/lib/x86_64-linux-gnu/libpthread.so.0"
            if let Some(path_str) = line.split_whitespace().next() {
                let path = PathBuf::from(path_str);
                if let Some(parent) = path.parent() {
                    dirs.insert(parent.to_path_buf());
                }
            }
        }
    }

    dirs.into_iter().collect()
}

/// Return additional data directories for known applications.
pub fn known_data_dirs(name: &str) -> Vec<PathBuf> {
    match name {
        "google-chrome" | "google-chrome-stable" | "chromium" | "chromium-browser" => vec![
            PathBuf::from("/opt/google"),
            PathBuf::from("/usr/share/chromium"),
            PathBuf::from("/usr/share/google-chrome"),
        ],
        "python3" | "python" => {
            let mut dirs = vec![PathBuf::from("/usr/share/python3")];
            // Glob /usr/lib/python3*
            if let Ok(entries) = std::fs::read_dir("/usr/lib") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with("python3") {
                        dirs.push(entry.path());
                    }
                }
            }
            dirs
        }
        "node" | "npm" | "npx" | "nodejs" => vec![
            PathBuf::from("/usr/lib/node_modules"),
            PathBuf::from("/usr/share/nodejs"),
        ],
        "code" => vec![PathBuf::from("/usr/share/code")],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ldd_output() {
        let ldd_output = "\tlinux-vdso.so.1 (0x00007ffd3abfe000)
\tlibdl.so.2 => /lib/x86_64-linux-gnu/libdl.so.2 (0x00007f1234560000)
\tlibpthread.so.0 => /lib/x86_64-linux-gnu/libpthread.so.0 (0x00007f1234540000)
\tlibm.so.6 => /usr/lib/x86_64-linux-gnu/libm.so.6 (0x00007f1234500000)
\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x00007f1234300000)
\t/lib64/ld-linux-x86-64.so.2 (0x00007f1234580000)
\tlibfoo.so.1 => /opt/custom/lib/libfoo.so.1 (0x00007f12342e0000)";

        let dirs = parse_ldd_dirs(ldd_output);

        assert!(dirs.contains(&PathBuf::from("/lib/x86_64-linux-gnu")));
        assert!(dirs.contains(&PathBuf::from("/usr/lib/x86_64-linux-gnu")));
        assert!(dirs.contains(&PathBuf::from("/opt/custom/lib")));
        // linux-vdso and ld-linux should be skipped
        assert!(!dirs.contains(&PathBuf::from("/lib64")));
    }

    #[test]
    fn test_parse_ldd_deduplicates() {
        let ldd_output = "\tliba.so => /usr/lib/liba.so (0x1)
\tlibb.so => /usr/lib/libb.so (0x2)
\tlibc.so => /usr/lib/libc.so (0x3)";

        let dirs = parse_ldd_dirs(ldd_output);

        // All three libs are in /usr/lib — should appear only once
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], PathBuf::from("/usr/lib"));
    }

    #[test]
    fn test_parse_ldd_empty_and_garbage() {
        assert!(parse_ldd_dirs("").is_empty());
        assert!(parse_ldd_dirs("not a real ldd output").is_empty());
        assert!(parse_ldd_dirs("\tlinux-vdso.so.1 (0x00007fff)").is_empty());
    }

    #[test]
    fn test_known_data_dirs_chrome() {
        let dirs = known_data_dirs("google-chrome");
        assert!(dirs.contains(&PathBuf::from("/opt/google")));
        assert!(dirs.contains(&PathBuf::from("/usr/share/google-chrome")));
    }

    #[test]
    fn test_known_data_dirs_node() {
        let dirs = known_data_dirs("node");
        assert!(dirs.contains(&PathBuf::from("/usr/lib/node_modules")));
        assert!(dirs.contains(&PathBuf::from("/usr/share/nodejs")));
    }

    #[test]
    fn test_known_data_dirs_python() {
        let dirs = known_data_dirs("python3");
        assert!(dirs.contains(&PathBuf::from("/usr/share/python3")));
    }

    #[test]
    fn test_known_data_dirs_unknown() {
        let dirs = known_data_dirs("some-random-app");
        assert!(dirs.is_empty());
    }
}
