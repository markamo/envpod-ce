// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Network namespace creation, veth setup, NAT, and iptables operations.
//!
//! Uses `ip` and `iptables` commands for MVP reliability and debuggability.
//! A pure-Rust rtnetlink implementation is planned for v0.2.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::state::NetworkState;

/// Default subnet base: 10.200.{idx}.0/30 for each pod.
/// Host gets .1, pod gets .2.
pub const DEFAULT_SUBNET_BASE: &str = "10.200";

// ---------------------------------------------------------------------------
// Staleness detection
// ---------------------------------------------------------------------------

/// Check if a named network namespace exists.
pub fn netns_exists(netns_name: &str) -> bool {
    Path::new(&format!("/run/netns/{netns_name}")).exists()
}

/// Check if a host-side veth interface exists.
pub fn veth_exists(host_veth: &str) -> bool {
    Command::new("ip")
        .args(["link", "show", host_veth])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Ensure the pod index file exists (idempotent).
///
/// After a reboot the index directory may still exist (on-disk state),
/// but we re-create the file to be safe.
pub fn ensure_pod_index(base_dir: &Path, idx: u8) -> Result<()> {
    let index_dir = base_dir.join("netns_index");
    std::fs::create_dir_all(&index_dir).context("create netns_index dir")?;
    let path = index_dir.join(idx.to_string());
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("ensure pod index file: {}", path.display()))?;
    Ok(())
}

/// Recreate network namespace, veth pair, NAT, and iptables from persisted state.
///
/// Uses the SAME names/IPs from the `NetworkState` so the `PodHandle` stays valid
/// without modification. `isolated_mode` controls whether pod-internal iptables
/// rules (DNS restriction) are applied.
pub fn restore_network(
    state: &NetworkState,
    base_dir: &Path,
    isolated_mode: bool,
) -> Result<()> {
    // 1. Recreate the network namespace (idempotent: skip if still alive from a crash)
    if netns_exists(&state.netns_name) {
        tracing::debug!("netns {} already exists, reusing (pod survived crash)", state.netns_name);
    } else {
        ip_cmd(&["netns", "add", &state.netns_name])
            .with_context(|| format!("recreate netns {}", state.netns_name))?;
    }

    // 2. Ensure pod index file exists
    ensure_pod_index(base_dir, state.pod_index)?;

    // 3. Build VethConfig from persisted values (direct, not from_index)
    let veth_config = VethConfig {
        netns_name: state.netns_name.clone(),
        host_veth: state.host_veth.clone(),
        pod_veth: state.pod_veth.clone(),
        host_ip: state.host_ip.clone(),
        pod_ip: state.pod_ip.clone(),
        subnet: format!("{}.{}.0/30", state.subnet_base, state.pod_index),
    };

    // 4. Set up veth pair (idempotent: skip if already alive from a crash)
    if veth_exists(&veth_config.host_veth) {
        tracing::debug!("veth {} already exists, reusing (pod survived crash)", veth_config.host_veth);
    } else {
        setup_veth(&veth_config).context("restore veth pair")?;
    }

    // 5. Re-detect host interface (may change after reboot), fallback to persisted
    let host_iface = detect_host_interface_cached(Some(base_dir))
        .unwrap_or_else(|_| state.host_interface.clone());

    // 6. Set up NAT (already idempotent via -C checks)
    setup_host_nat(&host_iface, &veth_config.subnet, &veth_config.host_veth)
        .context("setup host NAT and DNS INPUT rules")?;

    // 7. Pod-internal iptables rules (DNS restriction, already idempotent)
    if isolated_mode {
        setup_pod_iptables(&state.netns_name, &state.host_ip)
            .context("restore pod iptables")?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Network namespace lifecycle
// ---------------------------------------------------------------------------

/// Create a persistent named network namespace.
pub fn create_netns(short_id: &str) -> Result<String> {
    let name = format!("envpod-{short_id}");
    ip_cmd(&["netns", "add", &name])
        .with_context(|| format!("create netns {name}"))?;
    Ok(name)
}

/// Remove a network namespace and all associated resources.
///
/// If `full` is true, also removes iptables rules for this pod's veth
/// interface immediately. Otherwise, dead rules are left for `envpod gc`.
pub fn destroy_netns(state: &NetworkState, full: bool) -> Result<()> {
    // Full cleanup: remove iptables rules BEFORE deleting the veth,
    // while the interface name still exists for matching.
    if full {
        cleanup_pod_iptables(&state.host_veth);
    }

    ip_cmd(&["link", "del", &state.host_veth]).ok();
    ip_cmd(&["netns", "del", &state.netns_name])
        .with_context(|| format!("delete netns {}", state.netns_name))?;
    Ok(())
}

/// Remove all iptables rules referencing a specific veth interface.
fn cleanup_pod_iptables(veth: &str) {
    let output = match Command::new("iptables-save").output() {
        Ok(o) => o,
        Err(_) => return,
    };
    let rules = String::from_utf8_lossy(&output.stdout);

    let mut current_table = String::new();
    for line in rules.lines() {
        if line.starts_with('*') {
            current_table = line[1..].trim().to_string();
            continue;
        }
        if !line.starts_with("-A ") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if (*part == "-i" || *part == "-o") && i + 1 < parts.len() && parts[i + 1] == veth {
                let delete_rule = line.replacen("-A ", "-D ", 1);
                let args: Vec<&str> = if current_table != "filter" {
                    let mut a = vec!["-t", &current_table];
                    a.extend(delete_rule.split_whitespace());
                    a
                } else {
                    delete_rule.split_whitespace().collect()
                };
                Command::new("iptables")
                    .args(&args)
                    .stderr(std::process::Stdio::null())
                    .status()
                    .ok();
                break;
            }
        }
    }
}

/// Garbage-collect stale iptables rules left by destroyed pods.
///
/// After `destroy_netns`, iptables rules referencing deleted veth interfaces
/// remain in the chains but never match traffic. This function removes them.
/// Returns the number of rules removed.
pub fn gc_iptables() -> Result<usize> {
    // 1. Get all current veth-*-h interfaces
    let active_veths: std::collections::HashSet<String> = std::fs::read_dir("/sys/class/net/")
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| name.starts_with("veth-"))
                .collect()
        })
        .unwrap_or_default();

    // 2. Dump current iptables rules
    let output = Command::new("iptables-save")
        .output()
        .context("iptables-save")?;
    let rules = String::from_utf8_lossy(&output.stdout);

    // 3. Find rules referencing dead veth interfaces
    let mut dead_rules: Vec<String> = Vec::new();
    let mut current_table = String::new();

    for line in rules.lines() {
        if line.starts_with('*') {
            current_table = line[1..].trim().to_string();
            continue;
        }
        if !line.starts_with("-A ") {
            continue;
        }
        // Check if this rule references a veth-*-h interface that no longer exists
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if (*part == "-i" || *part == "-o") && i + 1 < parts.len() {
                let iface = parts[i + 1];
                if iface.starts_with("veth-") && !active_veths.contains(iface) {
                    // Convert -A to -D for deletion
                    let delete_rule = line.replacen("-A ", "-D ", 1);
                    dead_rules.push(format!("{}\t{}", current_table, delete_rule));
                    break;
                }
            }
        }
    }

    if dead_rules.is_empty() {
        return Ok(0);
    }

    // 4. Remove dead rules — batch by table
    let count = dead_rules.len();
    for entry in &dead_rules {
        let (table, rule) = entry.split_once('\t').unwrap();
        let args: Vec<&str> = if table != "filter" {
            let mut a = vec!["-t", table];
            a.extend(rule.split_whitespace());
            a
        } else {
            rule.split_whitespace().collect()
        };
        Command::new("iptables")
            .args(&args)
            .stderr(std::process::Stdio::null())
            .status()
            .ok();
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Pod index allocation
// ---------------------------------------------------------------------------

/// Allocate a unique pod index (1..254) for subnet assignment.
///
/// Uses a simple file-based approach: creates `{base_dir}/netns_index/{idx}`
/// files to track which indices are in use.
///
/// Also checks the kernel routing table: if a stale veth from a crashed pod
/// still holds the subnet (e.g. after `envpod run` was killed without cleanup),
/// the index file may be gone but the route is still live. Skipping live subnets
/// prevents two pods sharing the same subnet, which causes DNS response misrouting.
pub fn allocate_pod_index(base_dir: &Path, subnet_base: &str) -> Result<u8> {
    let index_dir = base_dir.join("netns_index");
    std::fs::create_dir_all(&index_dir)
        .context("create netns_index dir")?;

    for idx in 1..=254u8 {
        let path = index_dir.join(idx.to_string());
        // Try to claim the index file exclusively
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {
                // Index file claimed — verify the subnet isn't already live in the
                // kernel routing table (stale veth from a previously leaked pod).
                if subnet_route_exists(idx, subnet_base) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                return Ok(idx);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("allocate pod index"),
        }
    }

    anyhow::bail!("no available pod indices (max 254 concurrent network pods)")
}

/// Return true if the kernel routing table already has a route for the subnet
/// that would be assigned to `idx` (e.g. `10.200.1.0/30` for idx=1).
/// This catches stale vetches left behind by crashed or improperly cleaned up pods.
fn subnet_route_exists(idx: u8, subnet_base: &str) -> bool {
    let subnet = format!("{subnet_base}.{idx}.0/30");
    Command::new("ip")
        .args(["route", "show", &subnet])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Release a pod index.
pub fn release_pod_index(base_dir: &Path, idx: u8) {
    let path = base_dir.join("netns_index").join(idx.to_string());
    std::fs::remove_file(path).ok();
}

// ---------------------------------------------------------------------------
// Veth pair setup
// ---------------------------------------------------------------------------

/// Configuration for a veth pair.
pub struct VethConfig {
    pub netns_name: String,
    pub host_veth: String,
    pub pod_veth: String,
    pub host_ip: String,
    pub pod_ip: String,
    pub subnet: String,
}

impl VethConfig {
    /// Create a VethConfig from a pod index, short ID, and subnet base.
    ///
    /// `subnet_base` is e.g. `"10.200"` — pods get `{base}.{idx}.0/30`.
    pub fn from_index(idx: u8, short_id: &str, netns_name: &str, subnet_base: &str) -> Self {
        Self {
            netns_name: netns_name.to_string(),
            host_veth: format!("veth-{short_id}-h"),
            pod_veth: format!("veth-{short_id}-p"),
            host_ip: format!("{subnet_base}.{idx}.1"),
            pod_ip: format!("{subnet_base}.{idx}.2"),
            subnet: format!("{subnet_base}.{idx}.0/30"),
        }
    }
}

/// Create veth pair, move pod-end into netns, assign IPs, set routes.
pub fn setup_veth(config: &VethConfig) -> Result<()> {
    // Create veth pair
    ip_cmd(&[
        "link", "add", &config.host_veth, "type", "veth",
        "peer", "name", &config.pod_veth,
    ])
    .context("create veth pair")?;

    // Move pod-side veth into the network namespace
    ip_cmd(&[
        "link", "set", &config.pod_veth, "netns", &config.netns_name,
    ])
    .context("move veth to netns")?;

    // Assign IP to host-side veth
    ip_cmd(&[
        "addr", "add", &format!("{}/30", config.host_ip), "dev", &config.host_veth,
    ])
    .context("assign host veth IP")?;

    // Bring up host-side veth
    ip_cmd(&["link", "set", &config.host_veth, "up"])
        .context("bring up host veth")?;

    // Configure pod-side veth inside the netns
    ip_netns_exec(&config.netns_name, &[
        "ip", "addr", "add", &format!("{}/30", config.pod_ip), "dev", &config.pod_veth,
    ])
    .context("assign pod veth IP")?;

    ip_netns_exec(&config.netns_name, &[
        "ip", "link", "set", &config.pod_veth, "up",
    ])
    .context("bring up pod veth")?;

    // Bring up loopback inside netns
    ip_netns_exec(&config.netns_name, &[
        "ip", "link", "set", "lo", "up",
    ])
    .context("bring up pod loopback")?;

    // Set default route inside netns (via host veth IP)
    ip_netns_exec(&config.netns_name, &[
        "ip", "route", "add", "default", "via", &config.host_ip,
    ])
    .context("set pod default route")?;

    // Allow unprivileged ICMP ping inside the netns (all GIDs)
    ip_netns_exec(&config.netns_name, &[
        "sysctl", "-w", "net.ipv4.ping_group_range=0 2147483647",
    ])
    .context("set ping_group_range in pod netns")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// NAT / iptables
// ---------------------------------------------------------------------------

/// Enable IP forwarding and set up MASQUERADE + FORWARD + INPUT rules for a pod's subnet.
///
/// Three layers of iptables rules are needed to work with UFW/firewall defaults:
/// 1. INPUT: Accept DNS (port 53) from the pod to the host veth (DNS server)
/// 2. FORWARD: Accept pod traffic forwarding through the host to the internet
/// 3. NAT/POSTROUTING: MASQUERADE pod traffic behind the host's IP
///
/// INPUT rules use direct `iptables -I INPUT 1` to guarantee insertion before
/// UFW reject chains. `iptables-restore --noflush` with `-I` is unreliable on
/// iptables-nft (Ubuntu 22+) — positional inserts may be silently ignored,
/// leaving DNS packets dropped by UFW's default INPUT policy.
pub fn setup_host_nat(host_iface: &str, subnet: &str, host_veth: &str) -> Result<()> {
    // Enable IP forwarding
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1")
        .context("enable ip_forward")?;

    // INPUT rules: use direct iptables -I INPUT 1 (idempotent via -C check).
    // Must be at position 1 so they fire before UFW/nftables DROP chains.
    // iptables-restore --noflush with -I is unreliable on iptables-nft.
    for proto in &["udp", "tcp"] {
        let check = Command::new("iptables")
            .args(["-C", "INPUT", "-i", host_veth, "-p", proto,
                   "--dport", "53", "-s", subnet, "-j", "ACCEPT"])
            .output()
            .context("check iptables INPUT rule")?;
        if !check.status.success() {
            // Rule not present — insert at position 1 (before UFW chains)
            Command::new("iptables")
                .args(["-I", "INPUT", "1", "-i", host_veth, "-p", proto,
                       "--dport", "53", "-s", subnet, "-j", "ACCEPT"])
                .output()
                .context("insert iptables INPUT DNS rule")?;
        }
    }

    // FORWARD and NAT rules: iptables-restore --noflush with -A (append) works
    // reliably on both iptables-legacy and iptables-nft.
    let rules = format!(
        "*filter\n\
         -A FORWARD -i {veth} -o {host} -s {subnet} -j ACCEPT\n\
         -A FORWARD -i {host} -o {veth} -m state --state RELATED,ESTABLISHED -j ACCEPT\n\
         COMMIT\n\
         *nat\n\
         -A POSTROUTING -s {subnet} -o {host} -j MASQUERADE\n\
         COMMIT\n",
        veth = host_veth,
        host = host_iface,
        subnet = subnet,
    );

    iptables_restore(&rules).context("setup host FORWARD+NAT rules")?;

    Ok(())
}

/// Set up iptables inside the pod's netns to restrict DNS to only the designated server.
///
/// This ensures the pod can't bypass our DNS filtering by using a different resolver.
///
/// **IPv6 strategy:** Disable IPv6 entirely via sysctl (pods only use IPv4 veths),
/// then add ip6tables DROP rules as defense-in-depth. This prevents agents from
/// bypassing DNS filtering by querying external IPv6 resolvers (e.g., `2001:4860:...`).
/// IPv4-mapped addresses (`::ffff:X.X.X.X`) still work because the kernel sends
/// them as IPv4 packets at the network layer.
///
/// Uses `iptables-restore --noflush` and `ip6tables-restore --noflush` inside
/// the netns to load all rules in 2 calls instead of 10 sequential invocations.
pub fn setup_pod_iptables(netns_name: &str, dns_ip: &str) -> Result<()> {
    // --- Disable IPv6 in the pod's network namespace ---
    // Pods only use IPv4 veth pairs. IPv6 is unnecessary attack surface.
    // Two sysctl calls are cheap (no subprocess — they run inside the existing netns exec).
    ip_netns_exec(netns_name, &[
        "sysctl", "-q", "-w", "net.ipv6.conf.all.disable_ipv6=1",
    ])
    .context("sysctl: disable IPv6 (all)")?;

    ip_netns_exec(netns_name, &[
        "sysctl", "-q", "-w", "net.ipv6.conf.default.disable_ipv6=1",
    ])
    .context("sysctl: disable IPv6 (default)")?;

    // --- IPv4 rules: ACCEPT our DNS server, DROP all other DNS ---
    let ipv4_rules = format!(
        "*filter\n\
         -A OUTPUT -p udp --dport 53 -d {dns} -j ACCEPT\n\
         -A OUTPUT -p tcp --dport 53 -d {dns} -j ACCEPT\n\
         -A OUTPUT -p udp --dport 53 -j DROP\n\
         -A OUTPUT -p tcp --dport 53 -j DROP\n\
         COMMIT\n",
        dns = dns_ip,
    );

    ip_netns_exec_stdin(netns_name, "iptables-restore", &["--noflush"], &ipv4_rules)
        .context("iptables-restore: pod DNS rules")?;

    // --- IPv6 rules: ACCEPT mapped DNS server, DROP all other DNS ---
    let dns_ip_v6_mapped = format!("::ffff:{}", dns_ip);

    let ipv6_rules = format!(
        "*filter\n\
         -A OUTPUT -p udp --dport 53 -d {dns6} -j ACCEPT\n\
         -A OUTPUT -p tcp --dport 53 -d {dns6} -j ACCEPT\n\
         -A OUTPUT -p udp --dport 53 -j DROP\n\
         -A OUTPUT -p tcp --dport 53 -j DROP\n\
         COMMIT\n",
        dns6 = dns_ip_v6_mapped,
    );

    ip_netns_exec_stdin(netns_name, "ip6tables-restore", &["--noflush"], &ipv6_rules)
        .context("ip6tables-restore: pod IPv6 DNS rules")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Host interface detection
// ---------------------------------------------------------------------------

/// Detect the host's default outbound network interface.
///
/// Uses a file cache at `{base_dir}/.host_interface` to avoid spawning
/// `ip route get 8.8.8.8` on every init/clone/restore. The cache is
/// invalidated if the interface no longer exists in `/sys/class/net/`.
pub fn detect_host_interface_cached(base_dir: Option<&Path>) -> Result<String> {
    // Try reading cached value
    if let Some(dir) = base_dir {
        let cache_path = dir.join(".host_interface");
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let iface = cached.trim().to_string();
            // Validate: does this interface still exist?
            if !iface.is_empty() && Path::new(&format!("/sys/class/net/{iface}")).exists() {
                return Ok(iface);
            }
        }
    }

    // Cache miss or stale — detect via subprocess
    let output = Command::new("ip")
        .args(["route", "get", "8.8.8.8"])
        .output()
        .context("run ip route get")?;

    if !output.status.success() {
        anyhow::bail!(
            "ip route get failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let iface = parse_route_interface(&stdout)?;

    // Write to cache (best-effort)
    if let Some(dir) = base_dir {
        std::fs::write(dir.join(".host_interface"), &iface).ok();
    }

    Ok(iface)
}

/// Parse the interface name from `ip route get` output.
///
/// Example output: `8.8.8.8 via 192.168.1.1 dev eth0 src 192.168.1.100 uid 0`
pub fn parse_route_interface(output: &str) -> Result<String> {
    let parts: Vec<&str> = output.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "dev" {
            if let Some(iface) = parts.get(i + 1) {
                return Ok(iface.to_string());
            }
        }
    }
    anyhow::bail!("could not detect host interface from: {output}")
}

// ---------------------------------------------------------------------------
// resolv.conf
// ---------------------------------------------------------------------------

/// Write a resolv.conf pointing to the pod's DNS server into the rootfs.
///
/// Written to the overlay **upper** layer (`upper/etc/resolv.conf`) so that
/// the rootfs stays immutable and can be shared across cloned pods. The
/// `etc/resolv.conf` path is in EXCLUDED_PATHS, so it never appears in
/// `envpod diff` and is never committed to the host.
pub fn write_pod_resolv_conf(upper_dir: &Path, dns_ip: &str) -> Result<()> {
    let etc_dir = upper_dir.join("etc");
    std::fs::create_dir_all(&etc_dir)
        .context("create etc dir in upper layer")?;

    let resolv_path = etc_dir.join("resolv.conf");

    let content = format!(
        "# Generated by envpod — pod DNS resolver\nnameserver {dns_ip}\n"
    );
    std::fs::write(&resolv_path, content)
        .context("write resolv.conf to upper layer")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Command helpers
// ---------------------------------------------------------------------------

/// Run `ip` with the given arguments.
fn ip_cmd(args: &[&str]) -> Result<()> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .with_context(|| format!("run: ip {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ip {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

/// Run a command inside a network namespace via `ip netns exec`.
fn ip_netns_exec(netns_name: &str, cmd: &[&str]) -> Result<()> {
    let output = Command::new("ip")
        .args(["netns", "exec", netns_name])
        .args(cmd)
        .output()
        .with_context(|| format!("run in netns {netns_name}: {}", cmd.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "ip netns exec {} {} failed: {}",
            netns_name,
            cmd.join(" "),
            stderr.trim()
        );
    }
    Ok(())
}

/// Run `iptables` with the given arguments.
/// Load iptables rules via `iptables-restore --noflush`.
///
/// Loads all rules in a single subprocess call instead of one per rule.
fn iptables_restore(rules: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("iptables-restore")
        .arg("--noflush")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn iptables-restore")?;

    child.stdin.take().unwrap().write_all(rules.as_bytes())
        .context("write rules to iptables-restore")?;

    let output = child.wait_with_output().context("wait iptables-restore")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("iptables-restore failed: {}", stderr.trim());
    }
    Ok(())
}

/// Run a command inside a network namespace with data piped to stdin.
fn ip_netns_exec_stdin(netns_name: &str, cmd: &str, args: &[&str], stdin_data: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("ip")
        .args(["netns", "exec", netns_name, cmd])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn in netns {netns_name}: {cmd}"))?;

    child.stdin.take().unwrap().write_all(stdin_data.as_bytes())
        .with_context(|| format!("write stdin to {cmd}"))?;

    let output = child.wait_with_output()
        .with_context(|| format!("wait {cmd} in netns {netns_name}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{cmd} in netns {netns_name} failed: {}", stderr.trim());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Port forwarding
// ---------------------------------------------------------------------------

/// Persisted state for a single active port forward rule.
#[derive(Serialize, Deserialize)]
struct PortForwardRecord {
    proto: String,
    host_port: u16,
    container_port: u16,
    pod_ip: String,
    /// If true, only OUTPUT DNAT was added (no PREROUTING/FORWARD) — localhost-only access.
    #[serde(default)]
    host_only: bool,
}

/// Parse a port spec into (host_port, container_port, proto, host_only).
///
/// Formats:
/// - `"8080:3000"`              → all interfaces, TCP
/// - `"8080:3000/udp"`          → all interfaces, UDP
/// - `"3000"`                   → same port both sides, TCP, all interfaces
/// - `"127.0.0.1:8080:3000"`    → localhost only, TCP
/// - `"127.0.0.1:8080:3000/tcp"` → localhost only, explicit TCP
fn parse_port_spec(spec: &str) -> Result<(u16, u16, &str, bool)> {
    // Detect localhost-only scope prefix
    let (host_only, rest) = if let Some(r) = spec.strip_prefix("127.0.0.1:") {
        (true, r)
    } else {
        (false, spec)
    };

    let (ports_part, proto) = match rest.rsplit_once('/') {
        Some((p, proto)) => (p, proto),
        None => (rest, "tcp"),
    };
    let (host_str, container_str) = match ports_part.split_once(':') {
        Some((h, c)) => (h, c),
        None => (ports_part, ports_part),
    };
    let host_port: u16 = host_str.parse()
        .with_context(|| format!("invalid host port in port spec '{spec}'"))?;
    let container_port: u16 = container_str.parse()
        .with_context(|| format!("invalid container port in port spec '{spec}'"))?;
    Ok((host_port, container_port, proto, host_only))
}

/// Set up iptables DNAT rules to forward host ports to the pod.
///
/// Saves the active rules to `{pod_dir}/port_forwards_active.json` for cleanup.
///
/// Port spec formats:
/// - `"8080:3000"`               — all host interfaces (network-wide)
/// - `"127.0.0.1:8080:3000"`     — localhost only (OUTPUT DNAT, no PREROUTING)
/// - `"8080:3000/udp"`           — UDP, all interfaces
/// - `"127.0.0.1:8080:3000/tcp"` — localhost only, explicit TCP
pub fn setup_port_forwards(pod_dir: &Path, host_veth: &str, pod_ip: &str, ports: &[String]) -> Result<()> {
    if ports.is_empty() {
        return Ok(());
    }

    // Enable route_localnet on host veth AND lo so that localhost OUTPUT DNAT works.
    // lo: packets originate with src 127.0.0.1 — kernel needs route_localnet to
    //     allow routing a 127.x source to a non-local destination after DNAT.
    // host veth: packets exit through the veth toward the pod — kernel needs
    //     route_localnet to accept forwarding packets with 127.x addresses.
    for iface in &["lo", host_veth] {
        let rl_path = format!("/proc/sys/net/ipv4/conf/{iface}/route_localnet");
        if let Err(e) = std::fs::write(&rl_path, "1") {
            tracing::warn!("route_localnet write failed ({rl_path}): {e}");
        }
    }

    let mut records = Vec::new();
    for spec in ports {
        let (host_port, container_port, proto, host_only) = parse_port_spec(spec)?;
        let host_port_s = host_port.to_string();
        let container_port_s = container_port.to_string();
        let dest = format!("{pod_ip}:{container_port}");

        if !host_only {
            // PREROUTING: external traffic on any host interface → pod.
            // Skipped for localhost-only ports — those must originate from the host.
            iptables_cmd(&[
                "-t", "nat", "-A", "PREROUTING",
                "-p", proto, "--dport", &host_port_s,
                "-j", "DNAT", "--to-destination", &dest,
            ]).with_context(|| format!("PREROUTING DNAT for '{spec}'"))?;

            // FORWARD: accept new inbound connections forwarded to the pod.
            // Not needed for host-only ports because OUTPUT DNAT traffic is
            // locally generated and never enters the FORWARD chain.
            iptables_cmd(&[
                "-I", "FORWARD", "1",
                "-p", proto, "-d", pod_ip, "--dport", &container_port_s,
                "-j", "ACCEPT",
            ]).with_context(|| format!("FORWARD ACCEPT for '{spec}'"))?;
        }

        // OUTPUT: locally generated traffic on host → pod (all scope modes).
        iptables_cmd(&[
            "-t", "nat", "-A", "OUTPUT",
            "-d", "127.0.0.1", "-p", proto, "--dport", &host_port_s,
            "-j", "DNAT", "--to-destination", &dest,
        ]).with_context(|| format!("OUTPUT DNAT for '{spec}'"))?;

        // POSTROUTING SNAT: After OUTPUT DNAT rewrites dest from 127.0.0.1 to
        // pod_ip, the packet still has source 127.0.0.1.  The pod would reply
        // to 127.0.0.1 which stays inside its own namespace.  MASQUERADE on
        // the host veth rewrites the source to the host veth IP so the pod
        // sends the reply back across the veth pair.
        iptables_cmd(&[
            "-t", "nat", "-A", "POSTROUTING",
            "-s", "127.0.0.1", "-d", pod_ip, "-p", proto, "--dport", &container_port_s,
            "-o", host_veth, "-j", "MASQUERADE",
        ]).with_context(|| format!("POSTROUTING MASQUERADE for '{spec}'"))?;

        records.push(PortForwardRecord {
            proto: proto.to_string(),
            host_port,
            container_port,
            pod_ip: pod_ip.to_string(),
            host_only,
        });
    }

    // Save for cleanup on process exit or pod destruction
    let json = serde_json::to_string(&records).context("serialize port forward records")?;
    std::fs::write(pod_dir.join("port_forwards_active.json"), json)
        .context("save port forward records")?;

    Ok(())
}

/// Remove all iptables DNAT rules previously set up by `setup_port_forwards`.
///
/// Reads `{pod_dir}/port_forwards_active.json` and issues exact `-D` commands.
/// Silently returns if no active port forwards are recorded.
pub fn cleanup_port_forwards(pod_dir: &Path) {
    let path = pod_dir.join("port_forwards_active.json");
    let Ok(data) = std::fs::read_to_string(&path) else { return };
    let Ok(records) = serde_json::from_str::<Vec<PortForwardRecord>>(&data) else {
        std::fs::remove_file(&path).ok();
        return;
    };

    for r in &records {
        let host_port_s = r.host_port.to_string();
        let container_port_s = r.container_port.to_string();
        let dest = format!("{}:{}", r.pod_ip, r.container_port);

        if !r.host_only {
            iptables_cmd(&[
                "-t", "nat", "-D", "PREROUTING",
                "-p", &r.proto, "--dport", &host_port_s,
                "-j", "DNAT", "--to-destination", &dest,
            ]).ok();

            iptables_cmd(&[
                "-D", "FORWARD",
                "-p", &r.proto, "-d", &r.pod_ip, "--dport", &container_port_s,
                "-j", "ACCEPT",
            ]).ok();
        }

        iptables_cmd(&[
            "-t", "nat", "-D", "OUTPUT",
            "-d", "127.0.0.1", "-p", &r.proto, "--dport", &host_port_s,
            "-j", "DNAT", "--to-destination", &dest,
        ]).ok();
    }

    std::fs::remove_file(&path).ok();
}

// ---------------------------------------------------------------------------
// Internal (pod-to-pod) port forwarding
// ---------------------------------------------------------------------------

/// Persisted record for an active internal (pod-to-pod) port forward.
#[derive(Serialize, Deserialize)]
struct InternalPortRecord {
    proto: String,
    container_port: u16,
    pod_ip: String,
    /// CIDR of the pod subnet (e.g. "10.200.0.0/16") — used for cleanup.
    pod_subnet: String,
}

/// Parse an internal port spec: "container_port[/proto]".
fn parse_internal_port(spec: &str) -> Result<(u16, &str)> {
    let (port_str, proto) = match spec.rsplit_once('/') {
        Some((p, proto)) => (p, proto),
        None => (spec, "tcp"),
    };
    let port: u16 = port_str.parse()
        .with_context(|| format!("invalid port in internal port spec '{spec}'"))?;
    Ok((port, proto))
}

/// Allow other pods to reach this pod on the specified container ports.
///
/// Adds FORWARD rules scoped to the pod subnet (e.g. 10.200.0.0/16) — only
/// other pods can initiate connections. The host and external machines are
/// unaffected. No host port mapping; pods access each other directly by IP.
///
/// `subnet_base` is the pod's subnet base (e.g. "10.200") from NetworkState.
pub fn setup_internal_ports(pod_dir: &Path, pod_ip: &str, subnet_base: &str, ports: &[String]) -> Result<()> {
    if ports.is_empty() {
        return Ok(());
    }

    // Derive the /16 CIDR from the subnet base (e.g. "10.200" → "10.200.0.0/16")
    let pod_subnet = format!("{subnet_base}.0.0/16");

    let mut records = Vec::new();
    for spec in ports {
        let (container_port, proto) = parse_internal_port(spec)?;
        let port_s = container_port.to_string();

        // Allow inbound new connections from any pod in the subnet
        iptables_cmd(&[
            "-I", "FORWARD", "1",
            "-s", &pod_subnet, "-p", proto, "-d", pod_ip, "--dport", &port_s,
            "-j", "ACCEPT",
        ]).with_context(|| format!("FORWARD inbound for internal port '{spec}'"))?;

        // Allow return traffic from this pod back to the pod subnet
        iptables_cmd(&[
            "-I", "FORWARD", "1",
            "-s", pod_ip, "-p", proto, "--sport", &port_s, "-d", &pod_subnet,
            "-j", "ACCEPT",
        ]).with_context(|| format!("FORWARD return for internal port '{spec}'"))?;

        records.push(InternalPortRecord {
            proto: proto.to_string(),
            container_port,
            pod_ip: pod_ip.to_string(),
            pod_subnet: pod_subnet.clone(),
        });
    }

    let json = serde_json::to_string(&records).context("serialize internal port records")?;
    std::fs::write(pod_dir.join("internal_ports_active.json"), json)
        .context("save internal port records")?;

    Ok(())
}

/// Remove all FORWARD rules added by `setup_internal_ports`.
pub fn cleanup_internal_ports(pod_dir: &Path) {
    let path = pod_dir.join("internal_ports_active.json");
    let Ok(data) = std::fs::read_to_string(&path) else { return };
    let Ok(records) = serde_json::from_str::<Vec<InternalPortRecord>>(&data) else {
        std::fs::remove_file(&path).ok();
        return;
    };

    for r in &records {
        let port_s = r.container_port.to_string();

        iptables_cmd(&[
            "-D", "FORWARD",
            "-s", &r.pod_subnet, "-p", &r.proto, "-d", &r.pod_ip, "--dport", &port_s,
            "-j", "ACCEPT",
        ]).ok();

        iptables_cmd(&[
            "-D", "FORWARD",
            "-s", &r.pod_ip, "-p", &r.proto, "--sport", &port_s, "-d", &r.pod_subnet,
            "-j", "ACCEPT",
        ]).ok();
    }

    std::fs::remove_file(&path).ok();
}

// ---------------------------------------------------------------------------
// Live port mutation (add / remove individual rules without pod restart)
// ---------------------------------------------------------------------------

/// Add a single port-forward rule and append it to `port_forwards_active.json`.
///
/// `spec` follows the same format as `setup_port_forwards` ("8080:3000", "127.0.0.1:8080:3000", etc.).
/// Idempotent on the iptables level (duplicate rules are harmless but silently skipped via host_port check).
pub fn add_port_forward(pod_dir: &Path, host_veth: &str, pod_ip: &str, spec: &str) -> Result<()> {
    let (host_port, container_port, proto, host_only) = parse_port_spec(spec)?;
    let host_port_s = host_port.to_string();
    let container_port_s = container_port.to_string();
    let dest = format!("{pod_ip}:{container_port}");

    // Load existing records, check for duplicate host port
    let path = pod_dir.join("port_forwards_active.json");
    let mut records: Vec<PortForwardRecord> = if path.exists() {
        let data = std::fs::read_to_string(&path).context("read port_forwards_active.json")?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        vec![]
    };
    if records.iter().any(|r| r.host_port == host_port && r.proto == proto) {
        anyhow::bail!("port forward {proto}/{host_port} is already active");
    }

    // Enable route_localnet on lo + host veth (idempotent)
    for iface in &["lo", host_veth] {
        std::fs::write(
            format!("/proc/sys/net/ipv4/conf/{iface}/route_localnet"),
            "1",
        ).ok();
    }

    if !host_only {
        iptables_cmd(&[
            "-t", "nat", "-A", "PREROUTING",
            "-p", proto, "--dport", &host_port_s,
            "-j", "DNAT", "--to-destination", &dest,
        ]).with_context(|| format!("PREROUTING DNAT for '{spec}'"))?;

        iptables_cmd(&[
            "-I", "FORWARD", "1",
            "-p", proto, "-d", pod_ip, "--dport", &container_port_s,
            "-j", "ACCEPT",
        ]).with_context(|| format!("FORWARD ACCEPT for '{spec}'"))?;
    }

    iptables_cmd(&[
        "-t", "nat", "-A", "OUTPUT",
        "-d", "127.0.0.1", "-p", proto, "--dport", &host_port_s,
        "-j", "DNAT", "--to-destination", &dest,
    ]).with_context(|| format!("OUTPUT DNAT for '{spec}'"))?;

    records.push(PortForwardRecord {
        proto: proto.to_string(),
        host_port,
        container_port,
        pod_ip: pod_ip.to_string(),
        host_only,
    });
    let json = serde_json::to_string(&records).context("serialize port forward records")?;
    std::fs::write(&path, json).context("save port_forwards_active.json")?;
    Ok(())
}

/// Remove a port-forward rule by host port (and optional proto) and update the state file.
///
/// `spec` may be just a host port ("8080") or include proto ("8080/udp"). Defaults to tcp.
pub fn remove_port_forward(pod_dir: &Path, spec: &str) -> Result<()> {
    let (host_port, proto) = match spec.rsplit_once('/') {
        Some((p, proto)) => (p.parse::<u16>().context("invalid port")?, proto),
        None => (spec.parse::<u16>().context("invalid port")?, "tcp"),
    };

    let path = pod_dir.join("port_forwards_active.json");
    let data = std::fs::read_to_string(&path)
        .with_context(|| "no active port forwards (pod_forwards_active.json not found)")?;
    let mut records: Vec<PortForwardRecord> = serde_json::from_str(&data)
        .context("parse port_forwards_active.json")?;

    let pos = records.iter().position(|r| r.host_port == host_port && r.proto == proto)
        .with_context(|| format!("no active {proto}/{host_port} port forward found"))?;
    let r = records.remove(pos);

    let host_port_s = r.host_port.to_string();
    let container_port_s = r.container_port.to_string();
    let dest = format!("{}:{}", r.pod_ip, r.container_port);

    if !r.host_only {
        iptables_cmd(&[
            "-t", "nat", "-D", "PREROUTING",
            "-p", &r.proto, "--dport", &host_port_s,
            "-j", "DNAT", "--to-destination", &dest,
        ]).ok();
        iptables_cmd(&[
            "-D", "FORWARD",
            "-p", &r.proto, "-d", &r.pod_ip, "--dport", &container_port_s,
            "-j", "ACCEPT",
        ]).ok();
    }
    iptables_cmd(&[
        "-t", "nat", "-D", "OUTPUT",
        "-d", "127.0.0.1", "-p", &r.proto, "--dport", &host_port_s,
        "-j", "DNAT", "--to-destination", &dest,
    ]).ok();

    let json = serde_json::to_string(&records).context("serialize port forward records")?;
    std::fs::write(&path, json).context("save port_forwards_active.json")?;
    Ok(())
}

/// Add a single internal-port rule and append it to `internal_ports_active.json`.
///
/// `spec` follows the format "container_port[/proto]" (e.g. "3000", "3000/udp").
pub fn add_internal_port(pod_dir: &Path, pod_ip: &str, subnet_base: &str, spec: &str) -> Result<()> {
    let (container_port, proto) = parse_internal_port(spec)?;
    let port_s = container_port.to_string();
    let pod_subnet = format!("{subnet_base}.0.0/16");

    let path = pod_dir.join("internal_ports_active.json");
    let mut records: Vec<InternalPortRecord> = if path.exists() {
        let data = std::fs::read_to_string(&path).context("read internal_ports_active.json")?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        vec![]
    };
    if records.iter().any(|r| r.container_port == container_port && r.proto == proto) {
        anyhow::bail!("internal port {proto}/{container_port} is already active");
    }

    iptables_cmd(&[
        "-I", "FORWARD", "1",
        "-s", &pod_subnet, "-p", proto, "-d", pod_ip, "--dport", &port_s,
        "-j", "ACCEPT",
    ]).with_context(|| format!("FORWARD inbound for internal port '{spec}'"))?;

    iptables_cmd(&[
        "-I", "FORWARD", "1",
        "-s", pod_ip, "-p", proto, "--sport", &port_s, "-d", &pod_subnet,
        "-j", "ACCEPT",
    ]).with_context(|| format!("FORWARD return for internal port '{spec}'"))?;

    records.push(InternalPortRecord {
        proto: proto.to_string(),
        container_port,
        pod_ip: pod_ip.to_string(),
        pod_subnet,
    });
    let json = serde_json::to_string(&records).context("serialize internal port records")?;
    std::fs::write(&path, json).context("save internal_ports_active.json")?;
    Ok(())
}

/// Remove an internal-port rule by container port and update the state file.
///
/// `spec` may be just a port number ("3000") or include proto ("3000/udp"). Defaults to tcp.
pub fn remove_internal_port(pod_dir: &Path, spec: &str) -> Result<()> {
    let (container_port, proto) = parse_internal_port(spec)?;

    let path = pod_dir.join("internal_ports_active.json");
    let data = std::fs::read_to_string(&path)
        .with_context(|| "no active internal ports (internal_ports_active.json not found)")?;
    let mut records: Vec<InternalPortRecord> = serde_json::from_str(&data)
        .context("parse internal_ports_active.json")?;

    let pos = records.iter().position(|r| r.container_port == container_port && r.proto == proto)
        .with_context(|| format!("no active {proto}/{container_port} internal port found"))?;
    let r = records.remove(pos);

    let port_s = r.container_port.to_string();
    iptables_cmd(&[
        "-D", "FORWARD",
        "-s", &r.pod_subnet, "-p", &r.proto, "-d", &r.pod_ip, "--dport", &port_s,
        "-j", "ACCEPT",
    ]).ok();
    iptables_cmd(&[
        "-D", "FORWARD",
        "-s", &r.pod_ip, "-p", &r.proto, "--sport", &port_s, "-d", &r.pod_subnet,
        "-j", "ACCEPT",
    ]).ok();

    let json = serde_json::to_string(&records).context("serialize internal port records")?;
    std::fs::write(&path, json).context("save internal_ports_active.json")?;
    Ok(())
}

/// Read currently active port forwards from state files (for status display).
/// Returns (port_forwards, internal_ports) as raw JSON strings, or empty vecs.
pub fn read_active_ports(pod_dir: &Path) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let pf = pod_dir.join("port_forwards_active.json");
    let ip = pod_dir.join("internal_ports_active.json");

    let forwards = std::fs::read_to_string(&pf)
        .ok()
        .and_then(|d| serde_json::from_str::<Vec<serde_json::Value>>(&d).ok())
        .unwrap_or_default();

    let internals = std::fs::read_to_string(&ip)
        .ok()
        .and_then(|d| serde_json::from_str::<Vec<serde_json::Value>>(&d).ok())
        .unwrap_or_default();

    (forwards, internals)
}

/// Run `iptables` with the given arguments.
fn iptables_cmd(args: &[&str]) -> Result<()> {
    let output = Command::new("iptables")
        .args(args)
        .output()
        .with_context(|| format!("run: iptables {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("iptables {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pod firewall (expose)
// ---------------------------------------------------------------------------

/// Extract the container port from a port-forward spec.
///
/// Formats: "host:container", "host:container/proto", "127.0.0.1:host:container",
/// "127.0.0.1:host:container/proto", "port" (same port).
fn extract_container_port(spec: &str) -> Option<u16> {
    // Strip localhost prefix if present
    let rest = spec.strip_prefix("127.0.0.1:").unwrap_or(spec);

    // Strip protocol suffix
    let (port_part, _) = match rest.rsplit_once('/') {
        Some((p, _proto)) => (p, _proto),
        None => (rest, "tcp"),
    };

    // "host:container" or just "port"
    match port_part.rsplit_once(':') {
        Some((_host, container)) => container.parse::<u16>().ok(),
        None => port_part.parse::<u16>().ok(),
    }
}

/// Extract the port from an internal port spec ("port" or "port/proto").
fn extract_internal_port(spec: &str) -> Option<u16> {
    let port_str = match spec.rsplit_once('/') {
        Some((p, _)) => p,
        None => spec,
    };
    port_str.parse::<u16>().ok()
}

/// Set up firewall rules so only explicitly exposed ports are reachable on the pod IP.
///
/// Collects allowed ports from: `expose`, `ports`, `public_ports`, `internal_ports`.
/// If *none* of these are configured the firewall is skipped entirely
/// (backward compatibility — pod is fully open). When any port config exists, a default
/// DROP is installed on FORWARD for the pod IP, with ACCEPT rules for each allowed port
/// and for established/related return traffic.
pub fn setup_pod_firewall(
    pod_dir: &Path,
    pod_ip: &str,
    expose: &[u16],
    ports: &[String],
    public_ports: &[String],
    internal_ports: &[String],
) -> Result<()> {
    // Collect all ports that should be accessible
    let mut allowed_ports: std::collections::HashSet<u16> = std::collections::HashSet::new();

    // Explicit expose list
    for p in expose {
        allowed_ports.insert(*p);
    }

    // Ports from port forwarding (extract container port)
    for spec in ports.iter().chain(public_ports.iter()) {
        if let Some(container_port) = extract_container_port(spec) {
            allowed_ports.insert(container_port);
        }
    }

    // Internal ports
    for spec in internal_ports {
        if let Some(port) = extract_internal_port(spec) {
            allowed_ports.insert(port);
        }
    }

    // If nothing configured at all, skip firewall (backward compat)
    if allowed_ports.is_empty() && expose.is_empty() {
        return Ok(());
    }

    // ACCEPT established/related (return traffic always allowed)
    iptables_cmd(&[
        "-I", "FORWARD", "1",
        "-d", pod_ip,
        "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED",
        "-j", "ACCEPT",
    ])?;

    // ACCEPT each allowed port (TCP + UDP)
    for port in &allowed_ports {
        let port_s = port.to_string();
        iptables_cmd(&[
            "-I", "FORWARD", "1",
            "-d", pod_ip, "-p", "tcp", "--dport", &port_s,
            "-j", "ACCEPT",
        ])?;
        iptables_cmd(&[
            "-I", "FORWARD", "1",
            "-d", pod_ip, "-p", "udp", "--dport", &port_s,
            "-j", "ACCEPT",
        ])?;
    }

    // DEFAULT DROP all other inbound to pod
    iptables_cmd(&[
        "-A", "FORWARD",
        "-d", pod_ip,
        "-j", "DROP",
    ])?;

    // Save state for cleanup
    let record = serde_json::json!({
        "pod_ip": pod_ip,
        "allowed_ports": allowed_ports.iter().collect::<Vec<_>>(),
    });
    std::fs::write(
        pod_dir.join("firewall_active.json"),
        serde_json::to_string(&record)?,
    )?;

    Ok(())
}

/// Remove all firewall rules installed by [`setup_pod_firewall`].
///
/// Reads `firewall_active.json` from the pod directory to find which rules to
/// remove. Silent on errors — best-effort cleanup.
pub fn cleanup_pod_firewall(pod_dir: &Path) {
    let path = pod_dir.join("firewall_active.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(record) = serde_json::from_str::<serde_json::Value>(&content) {
            let pod_ip = record["pod_ip"].as_str().unwrap_or("");
            if !pod_ip.is_empty() {
                // Remove the DROP rule
                let _ = iptables_cmd(&["-D", "FORWARD", "-d", pod_ip, "-j", "DROP"]);

                // Remove ACCEPT rules for each port
                if let Some(ports) = record["allowed_ports"].as_array() {
                    for p in ports {
                        if let Some(port) = p.as_u64() {
                            let port_s = port.to_string();
                            let _ = iptables_cmd(&[
                                "-D", "FORWARD",
                                "-d", pod_ip, "-p", "tcp", "--dport", &port_s,
                                "-j", "ACCEPT",
                            ]);
                            let _ = iptables_cmd(&[
                                "-D", "FORWARD",
                                "-d", pod_ip, "-p", "udp", "--dport", &port_s,
                                "-j", "ACCEPT",
                            ]);
                        }
                    }
                }

                // Remove conntrack rule
                let _ = iptables_cmd(&[
                    "-D", "FORWARD",
                    "-d", pod_ip,
                    "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED",
                    "-j", "ACCEPT",
                ]);
            }
        }
    }
    std::fs::remove_file(&path).ok();
}

/// Read the current firewall state from `firewall_active.json`.
///
/// Returns `(pod_ip, allowed_ports)`. If no firewall is active, returns
/// empty values.
pub fn read_firewall_state(pod_dir: &Path) -> (String, Vec<u16>) {
    let path = pod_dir.join("firewall_active.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(record) = serde_json::from_str::<serde_json::Value>(&content) {
            let pod_ip = record["pod_ip"].as_str().unwrap_or("").to_string();
            let ports = record["allowed_ports"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_u64().map(|p| p as u16))
                        .collect()
                })
                .unwrap_or_default();
            return (pod_ip, ports);
        }
    }
    (String::new(), vec![])
}

/// Add a port to the firewall allow-list of a running pod.
///
/// Inserts ACCEPT rules for TCP+UDP and updates `firewall_active.json`.
/// If no firewall is active (no state file), this is a no-op and returns Ok.
pub fn expose_firewall_port(pod_dir: &Path, port: u16) -> Result<()> {
    let path = pod_dir.join("firewall_active.json");
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path).context("read firewall_active.json")?;
    let mut record: serde_json::Value =
        serde_json::from_str(&content).context("parse firewall_active.json")?;

    let pod_ip = record["pod_ip"].as_str().unwrap_or("").to_string();
    if pod_ip.is_empty() {
        anyhow::bail!("firewall state has no pod_ip");
    }

    // Check if already exposed
    let ports = record["allowed_ports"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_u64().map(|p| p as u16))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if ports.contains(&port) {
        return Ok(()); // already exposed
    }

    let port_s = port.to_string();

    // Insert ACCEPT rules BEFORE the DROP rule (position 1)
    iptables_cmd(&[
        "-I", "FORWARD", "1",
        "-d", &pod_ip, "-p", "tcp", "--dport", &port_s,
        "-j", "ACCEPT",
    ])?;
    iptables_cmd(&[
        "-I", "FORWARD", "1",
        "-d", &pod_ip, "-p", "udp", "--dport", &port_s,
        "-j", "ACCEPT",
    ])?;

    // Update state file
    let mut new_ports = ports;
    new_ports.push(port);
    record["allowed_ports"] = serde_json::json!(new_ports);
    std::fs::write(&path, serde_json::to_string(&record)?)?;

    Ok(())
}

/// Remove a port from the firewall allow-list of a running pod.
///
/// Deletes ACCEPT rules for TCP+UDP and updates `firewall_active.json`.
/// If no firewall is active (no state file), this is a no-op and returns Ok.
pub fn unexpose_firewall_port(pod_dir: &Path, port: u16) -> Result<()> {
    let path = pod_dir.join("firewall_active.json");
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path).context("read firewall_active.json")?;
    let mut record: serde_json::Value =
        serde_json::from_str(&content).context("parse firewall_active.json")?;

    let pod_ip = record["pod_ip"].as_str().unwrap_or("").to_string();
    if pod_ip.is_empty() {
        anyhow::bail!("firewall state has no pod_ip");
    }

    let ports = record["allowed_ports"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_u64().map(|p| p as u16))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !ports.contains(&port) {
        return Ok(()); // not exposed
    }

    let port_s = port.to_string();

    // Remove ACCEPT rules
    let _ = iptables_cmd(&[
        "-D", "FORWARD",
        "-d", &pod_ip, "-p", "tcp", "--dport", &port_s,
        "-j", "ACCEPT",
    ]);
    let _ = iptables_cmd(&[
        "-D", "FORWARD",
        "-d", &pod_ip, "-p", "udp", "--dport", &port_s,
        "-j", "ACCEPT",
    ]);

    // Update state file
    let new_ports: Vec<u16> = ports.into_iter().filter(|p| *p != port).collect();
    record["allowed_ports"] = serde_json::json!(new_ports);
    std::fs::write(&path, serde_json::to_string(&record)?)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_spec_basic() {
        let (h, c, p, lo) = parse_port_spec("8080:3000").unwrap();
        assert_eq!((h, c, p, lo), (8080, 3000, "tcp", false));
    }

    #[test]
    fn parse_port_spec_with_proto() {
        let (h, c, p, lo) = parse_port_spec("8080:3000/udp").unwrap();
        assert_eq!((h, c, p, lo), (8080, 3000, "udp", false));
    }

    #[test]
    fn parse_port_spec_same_port() {
        let (h, c, p, lo) = parse_port_spec("3000").unwrap();
        assert_eq!((h, c, p, lo), (3000, 3000, "tcp", false));
    }

    #[test]
    fn parse_port_spec_localhost_only() {
        let (h, c, p, lo) = parse_port_spec("127.0.0.1:8080:3000").unwrap();
        assert_eq!((h, c, p, lo), (8080, 3000, "tcp", true));
    }

    #[test]
    fn parse_port_spec_localhost_only_with_proto() {
        let (h, c, p, lo) = parse_port_spec("127.0.0.1:8080:3000/tcp").unwrap();
        assert_eq!((h, c, p, lo), (8080, 3000, "tcp", true));
    }

    #[test]
    fn parse_port_spec_localhost_same_port() {
        let (h, c, p, lo) = parse_port_spec("127.0.0.1:3000").unwrap();
        assert_eq!((h, c, p, lo), (3000, 3000, "tcp", true));
    }

    #[test]
    fn parse_port_spec_invalid() {
        assert!(parse_port_spec("notaport:3000").is_err());
        assert!(parse_port_spec("8080:notaport").is_err());
    }

    #[test]
    fn parse_internal_port_basic() {
        let (port, proto) = parse_internal_port("3000").unwrap();
        assert_eq!((port, proto), (3000, "tcp"));
    }

    #[test]
    fn parse_internal_port_with_proto() {
        let (port, proto) = parse_internal_port("5353/udp").unwrap();
        assert_eq!((port, proto), (5353, "udp"));
    }

    #[test]
    fn parse_internal_port_invalid() {
        assert!(parse_internal_port("notaport").is_err());
        assert!(parse_internal_port("notaport/tcp").is_err());
    }

    // -- extract_container_port / extract_internal_port (firewall helpers) ---

    #[test]
    fn extract_container_port_host_container() {
        assert_eq!(extract_container_port("8080:3000"), Some(3000));
    }

    #[test]
    fn extract_container_port_with_proto() {
        assert_eq!(extract_container_port("8080:3000/udp"), Some(3000));
    }

    #[test]
    fn extract_container_port_same_port() {
        assert_eq!(extract_container_port("3000"), Some(3000));
    }

    #[test]
    fn extract_container_port_localhost() {
        assert_eq!(extract_container_port("127.0.0.1:8080:3000"), Some(3000));
    }

    #[test]
    fn extract_container_port_localhost_with_proto() {
        assert_eq!(extract_container_port("127.0.0.1:8080:3000/tcp"), Some(3000));
    }

    #[test]
    fn extract_container_port_localhost_same() {
        assert_eq!(extract_container_port("127.0.0.1:3000"), Some(3000));
    }

    #[test]
    fn extract_container_port_invalid() {
        assert_eq!(extract_container_port("notaport:abc"), None);
    }

    #[test]
    fn extract_internal_port_basic() {
        assert_eq!(extract_internal_port("5000"), Some(5000));
    }

    #[test]
    fn extract_internal_port_with_proto() {
        assert_eq!(extract_internal_port("5353/udp"), Some(5353));
    }

    #[test]
    fn extract_internal_port_invalid() {
        assert_eq!(extract_internal_port("abc"), None);
    }

    #[test]
    fn parse_route_interface_standard() {
        let output = "8.8.8.8 via 192.168.1.1 dev eth0 src 192.168.1.100 uid 0";
        assert_eq!(parse_route_interface(output).unwrap(), "eth0");
    }

    #[test]
    fn parse_route_interface_wlan() {
        let output = "8.8.8.8 via 10.0.0.1 dev wlan0 src 10.0.0.50 uid 1000\n    cache";
        assert_eq!(parse_route_interface(output).unwrap(), "wlan0");
    }

    #[test]
    fn parse_route_interface_missing_dev() {
        let output = "8.8.8.8 via 192.168.1.1 src 192.168.1.100";
        assert!(parse_route_interface(output).is_err());
    }

    #[test]
    fn veth_config_from_index() {
        let config = VethConfig::from_index(5, "abc", "envpod-abc", DEFAULT_SUBNET_BASE);
        assert_eq!(config.host_ip, "10.200.5.1");
        assert_eq!(config.pod_ip, "10.200.5.2");
        assert_eq!(config.subnet, "10.200.5.0/30");
        assert_eq!(config.host_veth, "veth-abc-h");
        assert_eq!(config.pod_veth, "veth-abc-p");
    }

    #[test]
    fn veth_config_custom_subnet() {
        let config = VethConfig::from_index(3, "xyz", "envpod-xyz", "10.201");
        assert_eq!(config.host_ip, "10.201.3.1");
        assert_eq!(config.pod_ip, "10.201.3.2");
        assert_eq!(config.subnet, "10.201.3.0/30");
    }

    #[test]
    fn netns_exists_returns_false_for_nonexistent() {
        assert!(!netns_exists("envpod-nonexistent-test-99999"));
    }

    #[test]
    fn veth_exists_returns_false_for_nonexistent() {
        assert!(!veth_exists("veth-nonexistent-99999-h"));
    }

    #[test]
    fn ensure_pod_index_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_pod_index(tmp.path(), 42).unwrap();
        ensure_pod_index(tmp.path(), 42).unwrap(); // second call should not error
        assert!(tmp.path().join("netns_index/42").exists());
    }

    #[test]
    fn pod_index_allocation() {
        let tmp = tempfile::tempdir().unwrap();
        // Use 10.199 to avoid collision with live pods on 10.200
        let idx1 = allocate_pod_index(tmp.path(), "10.199").unwrap();
        let idx2 = allocate_pod_index(tmp.path(), "10.199").unwrap();
        assert_eq!(idx1, 1);
        assert_eq!(idx2, 2);

        release_pod_index(tmp.path(), idx1);
        let idx3 = allocate_pod_index(tmp.path(), "10.199").unwrap();
        assert_eq!(idx3, 1); // reuses released index
    }

    #[test]
    fn host_interface_cache_hit() {
        let tmp = tempfile::tempdir().unwrap();
        // Write a cached value pointing to "lo" (loopback always exists)
        std::fs::write(tmp.path().join(".host_interface"), "lo").unwrap();
        let iface = detect_host_interface_cached(Some(tmp.path())).unwrap();
        assert_eq!(iface, "lo");
    }

    #[test]
    fn host_interface_cache_stale() {
        let tmp = tempfile::tempdir().unwrap();
        // Write a cached value for a non-existent interface
        std::fs::write(tmp.path().join(".host_interface"), "nonexistent99").unwrap();
        // Should fall through to live detection (or fail if no network)
        let result = detect_host_interface_cached(Some(tmp.path()));
        // We don't assert success (CI may not have network), but it shouldn't
        // return the stale "nonexistent99" value
        if let Ok(iface) = result {
            assert_ne!(iface, "nonexistent99");
        }
    }

    #[test]
    fn host_interface_cache_miss_no_dir() {
        // No cache file — falls through to live detection
        let tmp = tempfile::tempdir().unwrap();
        let result = detect_host_interface_cached(Some(tmp.path()));
        // Just verify it doesn't panic; success depends on network availability
        let _ = result;
    }

    // -- Integration tests (require root) --

    #[test]
    #[ignore = "requires root"]
    fn create_and_destroy_netns() {
        let name = create_netns("test-netns").unwrap();
        assert!(Path::new(&format!("/run/netns/{name}")).exists());

        // Clean up
        ip_cmd(&["netns", "del", &name]).unwrap();
        assert!(!Path::new(&format!("/run/netns/{name}")).exists());
    }

    #[test]
    #[ignore = "requires root"]
    fn veth_connectivity() {
        let netns_name = create_netns("test-veth").unwrap();
        let config = VethConfig::from_index(250, "testveth", &netns_name, DEFAULT_SUBNET_BASE);

        setup_veth(&config).unwrap();

        // Verify we can ping from host to pod's veth IP via the netns
        let output = Command::new("ip")
            .args(["netns", "exec", &netns_name, "ping", "-c", "1", "-W", "1", &config.host_ip])
            .output()
            .unwrap();
        assert!(output.status.success(), "ping from pod to host should succeed");

        // Clean up
        ip_cmd(&["link", "del", &config.host_veth]).ok();
        ip_cmd(&["netns", "del", &netns_name]).unwrap();
    }
}
