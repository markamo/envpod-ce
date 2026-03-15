// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! /proc masking for resource-aware pods.
//!
//! After mounting a fresh procfs in the overlay, this module generates masked
//! versions of `/proc/cpuinfo`, `/proc/meminfo`, and `/proc/stat` that reflect
//! the pod's cgroup limits, then bind-mounts them read-only over the real proc
//! files. If no limits are set, masking is skipped (show everything).
//!
//! This prevents information disclosure — without masking, an agent inside a
//! pod can fingerprint host hardware, see total RAM, and count all CPU cores.

use std::fs;
use std::io;
use std::path::Path;

use nix::mount::MsFlags;

// ---------------------------------------------------------------------------
// Cgroup limits
// ---------------------------------------------------------------------------

/// Parsed cgroup resource limits relevant to /proc masking.
#[derive(Debug, Default)]
pub struct CgroupLimits {
    /// Specific CPU indices allowed (from cpuset.cpus).
    pub allowed_cpus: Option<Vec<usize>>,
    /// Number of CPU cores from cpu.max bandwidth limiting.
    pub cpu_cores: Option<f64>,
    /// Memory limit in bytes (from memory.max).
    pub memory_bytes: Option<u64>,
    /// Current memory usage in bytes (from memory.current).
    pub memory_current: Option<u64>,
    /// Page cache bytes used by this cgroup (from memory.stat "file" field).
    pub memory_cached: Option<u64>,
}

impl CgroupLimits {
    /// Returns true if no limits are set (nothing to mask).
    pub fn is_empty(&self) -> bool {
        self.allowed_cpus.is_none() && self.cpu_cores.is_none() && self.memory_bytes.is_none()
    }
}

/// Read cgroup limits from the cgroup directory.
pub fn read_cgroup_limits(cg_dir: &Path) -> CgroupLimits {
    let mut limits = CgroupLimits::default();

    // cpuset.cpus — e.g. "0-3,5,7-8"
    if let Ok(val) = fs::read_to_string(cg_dir.join("cpuset.cpus")) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            if let Some(cpus) = parse_cpuset(trimmed) {
                if !cpus.is_empty() {
                    limits.allowed_cpus = Some(cpus);
                }
            }
        }
    }

    // cpu.max — e.g. "200000 100000" (quota period)
    if let Ok(val) = fs::read_to_string(cg_dir.join("cpu.max")) {
        limits.cpu_cores = parse_cpu_max(val.trim());
    }

    // memory.max — e.g. "1073741824" or "max"
    if let Ok(val) = fs::read_to_string(cg_dir.join("memory.max")) {
        let trimmed = val.trim();
        if trimmed != "max" {
            if let Ok(bytes) = trimmed.parse::<u64>() {
                limits.memory_bytes = Some(bytes);
            }
        }
    }

    // memory.current — actual cgroup memory usage
    if let Ok(val) = fs::read_to_string(cg_dir.join("memory.current")) {
        if let Ok(bytes) = val.trim().parse::<u64>() {
            limits.memory_current = Some(bytes);
        }
    }

    // memory.stat — extract "file" field (page cache)
    if let Ok(val) = fs::read_to_string(cg_dir.join("memory.stat")) {
        for line in val.lines() {
            if let Some(rest) = line.strip_prefix("file ") {
                if let Ok(bytes) = rest.trim().parse::<u64>() {
                    limits.memory_cached = Some(bytes);
                }
                break;
            }
        }
    }

    limits
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse a cpuset string like "0-3,5,7-8" into a sorted vec of CPU indices.
pub fn parse_cpuset(s: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: usize = start.trim().parse().ok()?;
            let end: usize = end.trim().parse().ok()?;
            for i in start..=end {
                cpus.push(i);
            }
        } else {
            cpus.push(part.parse().ok()?);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Some(cpus)
}

/// Parse cpu.max "quota period" into a core count.
/// "200000 100000" → Some(2.0), "max 100000" → None
pub fn parse_cpu_max(s: &str) -> Option<f64> {
    let mut parts = s.split_whitespace();
    let quota_str = parts.next()?;
    let period_str = parts.next()?;

    if quota_str == "max" {
        return None;
    }

    let quota: f64 = quota_str.parse().ok()?;
    let period: f64 = period_str.parse().ok()?;
    if period <= 0.0 {
        return None;
    }

    Some(quota / period)
}

// ---------------------------------------------------------------------------
// /proc/cpuinfo masking
// ---------------------------------------------------------------------------

/// Generate masked cpuinfo content showing only the allowed CPUs.
///
/// Filters the host cpuinfo to only include entries for allowed CPU indices,
/// then renumbers `processor` fields 0, 1, 2... and adjusts `siblings`,
/// `cpu cores`, and `core id`.
pub fn mask_cpuinfo(host_cpuinfo: &str, allowed_cpus: &[usize]) -> String {
    if allowed_cpus.is_empty() {
        return host_cpuinfo.to_string();
    }

    // Split into per-processor blocks (separated by blank lines)
    let blocks: Vec<&str> = host_cpuinfo.split("\n\n").collect();

    let mut result = Vec::new();
    let num_cpus = allowed_cpus.len();

    for block in &blocks {
        // Extract the processor number from this block
        let proc_num = block
            .lines()
            .find(|l| l.starts_with("processor"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse::<usize>().ok());

        let proc_num = match proc_num {
            Some(n) => n,
            None => continue, // skip non-processor blocks
        };

        // Only include if this CPU is in the allowed set
        if !allowed_cpus.contains(&proc_num) {
            continue;
        }

        let new_idx = result.len();

        // Rewrite fields in this block
        let rewritten: Vec<String> = block
            .lines()
            .map(|line| {
                if let Some((key, _val)) = line.split_once(':') {
                    let key_trimmed = key.trim();
                    match key_trimmed {
                        "processor" => format!("processor\t: {new_idx}"),
                        "core id" => format!("core id\t\t: {new_idx}"),
                        "cpu cores" => format!("cpu cores\t: {num_cpus}"),
                        "siblings" => format!("siblings\t: {num_cpus}"),
                        _ => line.to_string(),
                    }
                } else {
                    line.to_string()
                }
            })
            .collect();

        result.push(rewritten.join("\n"));
    }

    if result.is_empty() {
        return host_cpuinfo.to_string();
    }

    let mut out = result.join("\n\n");
    // Ensure trailing newline like real /proc/cpuinfo
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Sanitize cpuinfo without filtering CPUs — strips model name to prevent
/// host CPU fingerprinting. Used when no cgroup CPU limits are set.
pub fn sanitize_cpuinfo(host_cpuinfo: &str) -> String {
    host_cpuinfo
        .lines()
        .map(|line| {
            if line.starts_with("model name") {
                "model name\t: CPU"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// /proc/meminfo masking
// ---------------------------------------------------------------------------

/// Generate masked meminfo content using cgroup memory stats.
///
/// Uses actual cgroup values (memory.max, memory.current, memory.stat)
/// instead of proportional scaling from host values.
pub fn mask_meminfo(host_meminfo: &str, limits: &MemoryMaskInfo) -> String {
    let limit_kb = limits.limit_bytes / 1024;

    // Find the host's MemTotal
    let host_total_kb = host_meminfo
        .lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(extract_meminfo_kb)
        .unwrap_or(1);

    if host_total_kb == 0 {
        return host_meminfo.to_string();
    }

    // If the limit is >= host total, no point masking
    if limit_kb >= host_total_kb {
        return host_meminfo.to_string();
    }

    // Compute values from cgroup stats
    let used_kb = limits.current_bytes.unwrap_or(0) / 1024;
    let cached_kb = limits.cached_bytes.unwrap_or(0) / 1024;
    let free_kb = limit_kb.saturating_sub(used_kb);
    let available_kb = free_kb + cached_kb; // reclaimable cache counts as available
    let available_kb = available_kb.min(limit_kb);

    host_meminfo
        .lines()
        .map(|line| {
            if let Some((key, _)) = line.split_once(':') {
                let key_trimmed = key.trim();
                match key_trimmed {
                    "MemTotal" => format_meminfo_line("MemTotal", limit_kb),
                    "MemFree" => format_meminfo_line("MemFree", free_kb),
                    "MemAvailable" => format_meminfo_line("MemAvailable", available_kb),
                    "Buffers" => format_meminfo_line("Buffers", 0),
                    "Cached" => format_meminfo_line("Cached", cached_kb),
                    "SwapTotal" | "SwapFree" => format_meminfo_line(key_trimmed, 0),
                    // Shmem, Active, Inactive, etc: scale proportionally
                    "Shmem" | "Active" | "Inactive" | "Active(anon)"
                    | "Inactive(anon)" | "Active(file)" | "Inactive(file)"
                    | "SReclaimable" | "SUnreclaim" => {
                        if let Some(val_kb) = extract_meminfo_kb(line) {
                            let ratio = limit_kb as f64 / host_total_kb as f64;
                            let scaled = ((val_kb as f64 * ratio) as u64).min(limit_kb);
                            format_meminfo_line(key_trimmed, scaled)
                        } else {
                            line.to_string()
                        }
                    }
                    _ => line.to_string(),
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Memory information needed for meminfo masking.
#[derive(Debug)]
pub struct MemoryMaskInfo {
    /// memory.max — the pod's memory limit in bytes.
    pub limit_bytes: u64,
    /// memory.current — actual usage in bytes (if available).
    pub current_bytes: Option<u64>,
    /// Page cache bytes (from memory.stat "file" field).
    pub cached_bytes: Option<u64>,
}

/// Extract the kB value from a meminfo line like "MemTotal:       65536 kB"
fn extract_meminfo_kb(line: &str) -> Option<u64> {
    let val_part = line.split(':').nth(1)?;
    let num_str = val_part.split_whitespace().next()?;
    num_str.parse().ok()
}

/// Format a meminfo line with proper alignment.
fn format_meminfo_line(key: &str, value_kb: u64) -> String {
    // /proc/meminfo uses right-aligned values with kB suffix
    format!("{key}:{value_kb:>16} kB")
}

// ---------------------------------------------------------------------------
// /proc/stat masking
// ---------------------------------------------------------------------------

/// Generate masked /proc/stat content showing only allowed CPUs.
///
/// Filters `cpuN` lines to allowed CPUs, renumbers consecutively,
/// and recalculates the aggregate `cpu ` line by summing included per-CPU lines.
#[allow(dead_code)] // kept for future use if dynamic stat filtering is added
pub fn mask_stat(host_stat: &str, allowed_cpus: &[usize]) -> String {
    if allowed_cpus.is_empty() {
        return host_stat.to_string();
    }

    // Collect per-CPU lines that match our allowed set
    let mut included_cpu_lines: Vec<(usize, String)> = Vec::new(); // (new_idx, original_line)

    for line in host_stat.lines() {
        // Match lines like "cpu0 ...", "cpu12 ..."
        if line.starts_with("cpu") && !line.starts_with("cpu ") {
            let cpu_id_str: String = line
                .chars()
                .skip(3) // skip "cpu"
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(cpu_id) = cpu_id_str.parse::<usize>() {
                if allowed_cpus.contains(&cpu_id) {
                    included_cpu_lines.push((included_cpu_lines.len(), line.to_string()));
                }
            }
        }
    }

    // Compute aggregate cpu line by summing all included per-CPU lines
    let aggregate = compute_aggregate_cpu_line(&included_cpu_lines);

    // Build output
    let mut output = Vec::new();

    for line in host_stat.lines() {
        if line.starts_with("cpu ") {
            // Replace aggregate line
            output.push(aggregate.clone());
        } else if line.starts_with("cpu") {
            // This is a cpuN line — skip, we'll add our filtered ones after aggregate
        } else {
            output.push(line.to_string());
        }
    }

    // Insert renumbered per-CPU lines right after the aggregate "cpu " line
    let insert_pos = output
        .iter()
        .position(|l| l.starts_with("cpu "))
        .map(|p| p + 1)
        .unwrap_or(1);

    let renumbered: Vec<String> = included_cpu_lines
        .iter()
        .enumerate()
        .map(|(new_idx, (_orig_idx, line))| {
            // Replace "cpuN " with "cpu{new_idx} "
            let rest = line
                .chars()
                .skip(3) // skip "cpu"
                .skip_while(|c| c.is_ascii_digit())
                .collect::<String>();
            format!("cpu{new_idx}{rest}")
        })
        .collect();

    for (i, line) in renumbered.into_iter().enumerate() {
        output.insert(insert_pos + i, line);
    }

    let mut result = output.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Compute the aggregate "cpu " line by summing per-CPU counter columns.
#[allow(dead_code)]
fn compute_aggregate_cpu_line(cpu_lines: &[(usize, String)]) -> String {
    if cpu_lines.is_empty() {
        return "cpu  0 0 0 0 0 0 0 0 0 0".to_string();
    }

    // Each cpuN line has: cpuN user nice system idle iowait irq softirq steal guest guest_nice
    let mut sums: Vec<u64> = Vec::new();

    for (_idx, line) in cpu_lines {
        let values: Vec<u64> = line
            .split_whitespace()
            .skip(1) // skip "cpuN"
            .filter_map(|v| v.parse().ok())
            .collect();

        if sums.is_empty() {
            sums = values;
        } else {
            for (i, val) in values.iter().enumerate() {
                if i < sums.len() {
                    sums[i] += val;
                }
            }
        }
    }

    let values_str: Vec<String> = sums.iter().map(|v| v.to_string()).collect();
    format!("cpu  {}", values_str.join(" "))
}

// ---------------------------------------------------------------------------
// Entry point: mask /proc files
// ---------------------------------------------------------------------------

/// Mask /proc files in the overlay to reflect the pod's cgroup limits.
///
/// `merged` is the overlay merged directory (before pivot_root).
/// `cgroup_procs` is the path to the cgroup's `cgroup.procs` file.
///
/// Also sets CPU affinity via `sched_setaffinity` so that `nproc` reports
/// the correct count, and masks sysfs CPU topology files for `lscpu`.
///
/// This function is non-fatal — callers should log and continue on error.
///
/// When `cgroup_procs` is None (standard mode without cgroups), we still
/// sanitize /proc/cpuinfo to prevent host CPU fingerprinting.
pub fn mask_proc_files(merged: &Path, cgroup_procs: Option<&Path>) -> io::Result<()> {
    let limits = if let Some(cg_procs) = cgroup_procs {
        let cg_dir = cg_procs
            .parent()
            .ok_or_else(|| io::Error::other("cgroup_procs has no parent directory"))?;
        read_cgroup_limits(cg_dir)
    } else {
        CgroupLimits::default()
    };

    // Determine which CPUs to show (only when cgroup limits restrict them)
    let effective_cpus = effective_cpu_list(&limits);

    // Create staging directory for masked files
    let staging = merged.join(".proc_mask");
    fs::create_dir_all(&staging)?;

    let proc_dir = merged.join("proc");

    // Always mask /proc/cpuinfo — either filter to allowed CPUs (cgroup mode)
    // or sanitize model name to prevent host fingerprinting (all modes).
    if let Some(ref cpus) = effective_cpus {
        if let Err(e) = mask_and_bind(&staging, &proc_dir, "cpuinfo", |content| {
            mask_cpuinfo(content, cpus)
        }) {
            let _ = io::Write::write_all(
                &mut io::stderr(),
                format!("envpod: cpuinfo masking failed: {e}\n").as_bytes(),
            );
        }
    } else {
        // No CPU limit — show all CPUs but strip model name
        if let Err(e) = mask_and_bind(&staging, &proc_dir, "cpuinfo", |content| {
            sanitize_cpuinfo(content)
        }) {
            let _ = io::Write::write_all(
                &mut io::stderr(),
                format!("envpod: cpuinfo sanitize failed: {e}\n").as_bytes(),
            );
        }
    }

    // Mask /proc/meminfo when cgroup memory limit is set
    if let Some(mem_bytes) = limits.memory_bytes {
        let mem_info = MemoryMaskInfo {
            limit_bytes: mem_bytes,
            current_bytes: limits.memory_current,
            cached_bytes: limits.memory_cached,
        };
        if let Err(e) = mask_and_bind(&staging, &proc_dir, "meminfo", |content| {
            mask_meminfo(content, &mem_info)
        }) {
            let _ = io::Write::write_all(
                &mut io::stderr(),
                format!("envpod: meminfo masking failed: {e}\n").as_bytes(),
            );
        }
    }

    // NOTE: /proc/stat is NOT masked. It contains live-updating CPU counters
    // that tools like htop use to compute CPU %. A static bind-mount would
    // freeze the values, making usage always show 0%. The sysfs directory
    // masking below ensures htop shows the correct number of CPU bars, and
    // the real /proc/stat provides live data for the visible CPUs.

    // Set CPU affinity so nproc reports the correct count.
    // When cpuset.cpus is explicitly restrictive, the kernel already
    // handles this. But when cpu.max is the limiting factor, we need
    // to call sched_setaffinity ourselves.
    if let Some(ref cpus) = effective_cpus {
        if let Err(e) = set_cpu_affinity(cpus) {
            let _ = io::Write::write_all(
                &mut io::stderr(),
                format!("envpod: sched_setaffinity failed (non-fatal): {e}\n").as_bytes(),
            );
        }
    }

    // Mask sysfs CPU topology files (for lscpu, nproc --all)
    if let Some(ref cpus) = effective_cpus {
        if let Err(e) = mask_sysfs_cpu_files(merged, &staging, cpus.len()) {
            let _ = io::Write::write_all(
                &mut io::stderr(),
                format!("envpod: sysfs CPU masking failed (non-fatal): {e}\n").as_bytes(),
            );
        }
    }

    Ok(())
}

/// Determine which CPUs to show based on cgroup limits.
///
/// When both cpuset.cpus and cpu.max are set, use whichever is more
/// restrictive. This handles the common case where cpuset.cpus is
/// inherited from the parent cgroup (all host CPUs) but cpu.max
/// actually limits the pod to fewer cores.
fn effective_cpu_list(limits: &CgroupLimits) -> Option<Vec<usize>> {
    let cpuset = limits.allowed_cpus.as_ref();
    let cpu_max_n = limits
        .cpu_cores
        .map(|c| c.ceil() as usize)
        .filter(|&n| n > 0);

    match (cpuset, cpu_max_n) {
        (Some(cpus), Some(n)) => {
            if cpus.len() <= n {
                // cpuset is more restrictive — use the explicit CPU identities
                Some(cpus.clone())
            } else {
                // cpu.max limits further — take first N from the cpuset
                Some(cpus.iter().take(n).copied().collect())
            }
        }
        (Some(cpus), None) => Some(cpus.clone()),
        (None, Some(n)) => Some((0..n).collect()),
        (None, None) => None,
    }
}

/// Set CPU affinity for the current process so `nproc` reports the correct count.
///
/// Uses `sched_setaffinity(0, ...)` — PID 0 means the calling process.
fn set_cpu_affinity(cpus: &[usize]) -> io::Result<()> {
    let mut cpu_set = nix::sched::CpuSet::new();
    for &cpu in cpus {
        cpu_set
            .set(cpu)
            .map_err(|e| io::Error::other(format!("CpuSet::set({cpu}): {e}")))?;
    }
    nix::sched::sched_setaffinity(nix::unistd::Pid::from_raw(0), &cpu_set)
        .map_err(|e| io::Error::other(format!("sched_setaffinity: {e}")))?;
    Ok(())
}

/// Format a CPU count as a cpuset range string: "0-1" for 2 CPUs, "0" for 1.
fn format_cpuset_range(n: usize) -> String {
    if n <= 1 {
        "0\n".to_string()
    } else {
        format!("0-{}\n", n - 1)
    }
}

/// Replace `/sys/devices/system/cpu/` with a minimal version containing only
/// the allowed CPU directories.
///
/// glibc's `sysconf(_SC_NPROCESSORS_CONF)` counts `cpu[0-9]*` directories in
/// sysfs, so tools like `htop` that use this syscall will show the wrong count
/// unless we replace the entire directory. We mount a tmpfs over it and bind-
/// mount only the real cpuN directories that should be visible.
fn mask_sysfs_cpu_files(merged: &Path, staging: &Path, num_cpus: usize) -> io::Result<()> {
    let sysfs_cpu = merged.join("sys/devices/system/cpu");
    if !sysfs_cpu.exists() {
        return Ok(());
    }

    // Create a tmpfs-backed staging area for the replacement sysfs cpu dir
    let sysfs_staging = staging.join("sysfs_cpu");
    fs::create_dir_all(&sysfs_staging)?;
    nix::mount::mount(
        Some("tmpfs"),
        &sysfs_staging,
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        Some("size=1m,mode=0555"),
    )
    .map_err(|e| io::Error::other(format!("tmpfs for sysfs staging: {e}")))?;

    // Create cpuN directories and bind-mount real sysfs content into them
    for i in 0..num_cpus {
        let cpu_name = format!("cpu{i}");
        let real_cpu = sysfs_cpu.join(&cpu_name);
        let staged_cpu = sysfs_staging.join(&cpu_name);
        fs::create_dir_all(&staged_cpu)?;
        if real_cpu.exists() {
            nix::mount::mount(
                Some(real_cpu.as_path()),
                &staged_cpu,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            )
            .map_err(|e| io::Error::other(format!("bind mount {cpu_name}: {e}")))?;
        }
    }

    // Write online/possible/present files
    let range = format_cpuset_range(num_cpus);
    for filename in &["online", "possible", "present"] {
        fs::write(sysfs_staging.join(filename), &range)?;
    }

    // Copy non-cpu files that tools expect (kernel_max, isolated, etc.)
    for filename in &["kernel_max", "isolated", "modalias", "nohz_full"] {
        if let Ok(val) = fs::read_to_string(sysfs_cpu.join(filename)) {
            let _ = fs::write(sysfs_staging.join(filename), val);
        }
    }

    // Copy non-cpu subdirectories that tools may need
    for dirname in &["cpufreq", "cpuidle", "vulnerabilities"] {
        let real_dir = sysfs_cpu.join(dirname);
        let staged_dir = sysfs_staging.join(dirname);
        if real_dir.is_dir() {
            fs::create_dir_all(&staged_dir)?;
            nix::mount::mount(
                Some(real_dir.as_path()),
                &staged_dir,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            )
            .ok(); // best effort — not critical
        }
    }

    // Bind-mount the crafted dir over the real sysfs cpu dir
    nix::mount::mount(
        Some(sysfs_staging.as_path()),
        &sysfs_cpu,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .map_err(|e| io::Error::other(format!("bind sysfs cpu overlay: {e}")))?;

    // Remount read-only (best effort)
    nix::mount::mount(
        None::<&str>,
        &sysfs_cpu,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC,
        None::<&str>,
    )
    .ok();

    Ok(())
}

/// Read a /proc file from the overlay, apply the masking function,
/// write to staging, and bind-mount read-only over the original.
fn mask_and_bind<F>(staging: &Path, proc_dir: &Path, filename: &str, mask_fn: F) -> io::Result<()>
where
    F: FnOnce(&str) -> String,
{
    let source = proc_dir.join(filename);
    let staged = staging.join(filename);

    // Read the current /proc file (already mounted as fresh procfs)
    let content = fs::read_to_string(&source)?;

    // Generate masked content
    let masked = mask_fn(&content);

    // Write to staging
    fs::write(&staged, &masked)?;

    // Bind-mount read-only over the proc file
    nix::mount::mount(
        Some(staged.as_path()),
        &source,
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )
    .map_err(|e| io::Error::other(format!("bind mount {filename}: {e}")))?;

    // Remount read-only
    nix::mount::mount(
        None::<&str>,
        &source,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
        None::<&str>,
    )
    .map_err(|e| io::Error::other(format!("readonly remount {filename}: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- cpuset parser --

    #[test]
    fn parse_cpuset_single() {
        assert_eq!(parse_cpuset("0"), Some(vec![0]));
    }

    #[test]
    fn parse_cpuset_range() {
        assert_eq!(parse_cpuset("0-3"), Some(vec![0, 1, 2, 3]));
    }

    #[test]
    fn parse_cpuset_mixed() {
        assert_eq!(parse_cpuset("0-3,5,7-8"), Some(vec![0, 1, 2, 3, 5, 7, 8]));
    }

    #[test]
    fn parse_cpuset_single_values() {
        assert_eq!(parse_cpuset("1,3,5"), Some(vec![1, 3, 5]));
    }

    #[test]
    fn parse_cpuset_empty() {
        assert_eq!(parse_cpuset(""), Some(vec![]));
    }

    #[test]
    fn parse_cpuset_deduplicates() {
        assert_eq!(parse_cpuset("1,1,2"), Some(vec![1, 2]));
    }

    #[test]
    fn parse_cpuset_invalid() {
        assert_eq!(parse_cpuset("abc"), None);
    }

    // -- cpu.max parser --

    #[test]
    fn parse_cpu_max_two_cores() {
        assert_eq!(parse_cpu_max("200000 100000"), Some(2.0));
    }

    #[test]
    fn parse_cpu_max_half_core() {
        assert_eq!(parse_cpu_max("50000 100000"), Some(0.5));
    }

    #[test]
    fn parse_cpu_max_unlimited() {
        assert_eq!(parse_cpu_max("max 100000"), None);
    }

    #[test]
    fn parse_cpu_max_empty() {
        assert_eq!(parse_cpu_max(""), None);
    }

    // -- CgroupLimits --

    #[test]
    fn cgroup_limits_is_empty() {
        let limits = CgroupLimits::default();
        assert!(limits.is_empty());
    }

    #[test]
    fn cgroup_limits_not_empty_with_memory() {
        let limits = CgroupLimits {
            memory_bytes: Some(1024 * 1024 * 1024),
            ..Default::default()
        };
        assert!(!limits.is_empty());
    }

    // -- effective_cpu_list --

    #[test]
    fn effective_cpus_cpuset_more_restrictive() {
        let limits = CgroupLimits {
            allowed_cpus: Some(vec![0, 2]),
            cpu_cores: Some(4.0),
            ..Default::default()
        };
        // cpuset (2 CPUs) is more restrictive than cpu.max (4 cores)
        assert_eq!(effective_cpu_list(&limits), Some(vec![0, 2]));
    }

    #[test]
    fn effective_cpus_cpu_max_more_restrictive() {
        // Simulates inherited cpuset (all 24 host CPUs) with cpu.max = 2 cores
        let limits = CgroupLimits {
            allowed_cpus: Some((0..24).collect()),
            cpu_cores: Some(2.0),
            ..Default::default()
        };
        // cpu.max (2 cores) is more restrictive — take first 2 from cpuset
        assert_eq!(effective_cpu_list(&limits), Some(vec![0, 1]));
    }

    #[test]
    fn effective_cpus_falls_back_to_cpu_max() {
        let limits = CgroupLimits {
            cpu_cores: Some(2.0),
            ..Default::default()
        };
        assert_eq!(effective_cpu_list(&limits), Some(vec![0, 1]));
    }

    #[test]
    fn effective_cpus_fractional_rounds_up() {
        let limits = CgroupLimits {
            cpu_cores: Some(1.5),
            ..Default::default()
        };
        assert_eq!(effective_cpu_list(&limits), Some(vec![0, 1]));
    }

    #[test]
    fn effective_cpus_none_when_no_limits() {
        let limits = CgroupLimits::default();
        assert_eq!(effective_cpu_list(&limits), None);
    }

    // -- format_cpuset_range --

    #[test]
    fn format_cpuset_range_single() {
        assert_eq!(format_cpuset_range(1), "0\n");
    }

    #[test]
    fn format_cpuset_range_multiple() {
        assert_eq!(format_cpuset_range(4), "0-3\n");
    }

    // -- cpuinfo masking --

    #[test]
    fn mask_cpuinfo_filters_to_allowed() {
        let host = "\
processor\t: 0
model name\t: Intel Core i9
core id\t\t: 0
cpu cores\t: 4
siblings\t: 4

processor\t: 1
model name\t: Intel Core i9
core id\t\t: 1
cpu cores\t: 4
siblings\t: 4

processor\t: 2
model name\t: Intel Core i9
core id\t\t: 2
cpu cores\t: 4
siblings\t: 4

processor\t: 3
model name\t: Intel Core i9
core id\t\t: 3
cpu cores\t: 4
siblings\t: 4
";

        let masked = mask_cpuinfo(host, &[0, 2]);

        // Should have exactly 2 processor blocks
        let proc_count = masked.matches("processor\t:").count();
        assert_eq!(proc_count, 2, "should have 2 processors");

        // Renumbered 0 and 1
        assert!(masked.contains("processor\t: 0"));
        assert!(masked.contains("processor\t: 1"));
        assert!(!masked.contains("processor\t: 2"));
        assert!(!masked.contains("processor\t: 3"));

        // cpu cores and siblings updated to 2
        assert!(masked.contains("cpu cores\t: 2"));
        assert!(masked.contains("siblings\t: 2"));
    }

    #[test]
    fn mask_cpuinfo_empty_allowed_returns_original() {
        let host = "processor\t: 0\nmodel name\t: Test\n";
        let masked = mask_cpuinfo(host, &[]);
        assert_eq!(masked, host);
    }

    // -- meminfo masking --

    #[test]
    fn mask_meminfo_uses_cgroup_stats() {
        let host = "\
MemTotal:       65536000 kB
MemFree:        32768000 kB
MemAvailable:   40960000 kB
Buffers:         1024000 kB
Cached:          8192000 kB
SwapTotal:       2097152 kB
SwapFree:        2097152 kB
Active:          8000000 kB
";

        let one_gb = 1024 * 1024 * 1024u64;
        let info = MemoryMaskInfo {
            limit_bytes: one_gb,
            current_bytes: Some(500 * 1024 * 1024), // 500 MiB used
            cached_bytes: Some(100 * 1024 * 1024),  // 100 MiB page cache
        };
        let masked = mask_meminfo(host, &info);

        // MemTotal = limit
        let mem_total_line = masked.lines().find(|l| l.starts_with("MemTotal:")).unwrap();
        let mem_total_kb = extract_meminfo_kb(mem_total_line).unwrap();
        assert_eq!(mem_total_kb, 1048576, "MemTotal should be 1GiB in kB");

        // MemFree = limit - current = 1GiB - 500MiB = 524MiB = 536576 kB
        let mem_free_line = masked.lines().find(|l| l.starts_with("MemFree:")).unwrap();
        let mem_free_kb = extract_meminfo_kb(mem_free_line).unwrap();
        assert_eq!(mem_free_kb, 536576, "MemFree should be 524MiB in kB");

        // MemAvailable = free + cached = 536576 + 102400
        let mem_avail_line = masked.lines().find(|l| l.starts_with("MemAvailable:")).unwrap();
        let mem_avail_kb = extract_meminfo_kb(mem_avail_line).unwrap();
        assert_eq!(mem_avail_kb, 638976, "MemAvailable should be free + cached");

        // Cached = cgroup page cache
        let cached_line = masked.lines().find(|l| l.starts_with("Cached:")).unwrap();
        let cached_kb = extract_meminfo_kb(cached_line).unwrap();
        assert_eq!(cached_kb, 102400, "Cached should be 100MiB in kB");

        // Buffers = 0 for pod
        let buffers_line = masked.lines().find(|l| l.starts_with("Buffers:")).unwrap();
        assert_eq!(extract_meminfo_kb(buffers_line).unwrap(), 0);

        // Swap = 0
        let swap_total_line = masked.lines().find(|l| l.starts_with("SwapTotal:")).unwrap();
        assert_eq!(extract_meminfo_kb(swap_total_line).unwrap(), 0);

        // Active should be proportionally scaled (not unchanged)
        let active_line = masked.lines().find(|l| l.starts_with("Active:")).unwrap();
        let active_kb = extract_meminfo_kb(active_line).unwrap();
        assert!(active_kb < 8000000, "Active should be scaled down");
    }

    #[test]
    fn mask_meminfo_no_current_bytes_shows_all_free() {
        let host = "MemTotal:       65536000 kB\nMemFree:        32768000 kB\n";
        let info = MemoryMaskInfo {
            limit_bytes: 1024 * 1024 * 1024, // 1 GiB
            current_bytes: None,
            cached_bytes: None,
        };
        let masked = mask_meminfo(host, &info);

        // MemFree should equal MemTotal when no usage info available
        let total = extract_meminfo_kb(
            masked.lines().find(|l| l.starts_with("MemTotal:")).unwrap(),
        ).unwrap();
        let free = extract_meminfo_kb(
            masked.lines().find(|l| l.starts_with("MemFree:")).unwrap(),
        ).unwrap();
        assert_eq!(total, free);
    }

    #[test]
    fn mask_meminfo_limit_exceeds_host_returns_original() {
        let host = "MemTotal:       65536 kB\nMemFree:        32768 kB\n";
        let info = MemoryMaskInfo {
            limit_bytes: 100 * 1024 * 1024 * 1024, // 100 GiB
            current_bytes: None,
            cached_bytes: None,
        };
        let masked = mask_meminfo(host, &info);
        assert_eq!(masked, host);
    }

    // -- stat masking --

    #[test]
    fn mask_stat_filters_cpus() {
        let host = "\
cpu  1000 200 300 4000 50 60 70 0 0 0
cpu0 250 50 75 1000 12 15 17 0 0 0
cpu1 250 50 75 1000 13 15 18 0 0 0
cpu2 250 50 75 1000 12 15 17 0 0 0
cpu3 250 50 75 1000 13 15 18 0 0 0
intr 123456789 0 0 0
ctxt 987654321
btime 1700000000
processes 12345
procs_running 2
procs_blocked 0
";

        let masked = mask_stat(host, &[0, 2]);

        // Should have aggregate + 2 per-CPU lines
        let cpu_lines: Vec<&str> = masked.lines().filter(|l| l.starts_with("cpu")).collect();
        assert_eq!(cpu_lines.len(), 3, "aggregate + 2 per-cpu lines");

        // Renumbered to cpu0, cpu1
        assert!(masked.contains("cpu0 "), "should have cpu0");
        assert!(masked.contains("cpu1 "), "should have cpu1");
        assert!(!masked.contains("cpu2 "), "should not have cpu2");
        assert!(!masked.contains("cpu3 "), "should not have cpu3");

        // Aggregate should be sum of included cpus
        let agg_line = masked.lines().find(|l| l.starts_with("cpu ")).unwrap();
        let agg_values: Vec<u64> = agg_line
            .split_whitespace()
            .skip(1)
            .filter_map(|v| v.parse().ok())
            .collect();
        // cpu0 + cpu2: user=250+250=500, nice=50+50=100, etc.
        assert_eq!(agg_values[0], 500, "user should be 500");
        assert_eq!(agg_values[1], 100, "nice should be 100");

        // Non-CPU lines preserved
        assert!(masked.contains("intr 123456789"));
        assert!(masked.contains("ctxt 987654321"));
        assert!(masked.contains("btime 1700000000"));
    }

    #[test]
    fn mask_stat_empty_allowed_returns_original() {
        let host = "cpu  100 0 0 0 0 0 0 0 0 0\ncpu0 100 0 0 0 0 0 0 0 0 0\n";
        let masked = mask_stat(host, &[]);
        assert_eq!(masked, host);
    }

    // -- format_meminfo_line --

    #[test]
    fn format_meminfo_line_alignment() {
        let line = format_meminfo_line("MemTotal", 1048576);
        assert!(line.starts_with("MemTotal:"));
        assert!(line.ends_with("kB"));
        assert!(line.contains("1048576"));
    }
}
