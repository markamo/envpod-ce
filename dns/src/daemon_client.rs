// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Minimal client for querying the envpod-dns daemon from the per-pod DNS server.
//!
//! Kept intentionally minimal — just enough to send a lookup request and parse
//! the response. Cannot import envpod-core (would be a circular dependency).

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Query the envpod-dns daemon for a pod name resolution.
///
/// Returns `Some(ip)` if the lookup succeeds (target discoverable + source allowed),
/// `None` for NXDOMAIN (pod not registered, not discoverable, or not in allow_pods),
/// or an `Err` if the daemon is unreachable.
///
/// The caller should treat `Err` the same as `None` (NXDOMAIN) — the daemon
/// being down is a safe failure mode.
pub async fn lookup(
    sock_path: &Path,
    queried_pod: &str,
    from_pod: &str,
) -> Result<Option<Ipv4Addr>> {
    let mut stream = UnixStream::connect(sock_path)
        .await
        .map_err(|e| anyhow::anyhow!("envpod-dns daemon unreachable: {e}"))?;

    let req = format!(
        "{{\"op\":\"lookup\",\"name\":{},\"from_pod\":{}}}\n",
        serde_json::to_string(queried_pod)?,
        serde_json::to_string(from_pod)?,
    );
    stream.write_all(req.as_bytes()).await?;

    let (read_half, _) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let val: serde_json::Value = serde_json::from_str(line.trim())?;

    if val.get("nxdomain").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(None);
    }
    if let Some(ip_str) = val.get("ip").and_then(|v| v.as_str()) {
        let ip: Ipv4Addr = ip_str.parse()
            .map_err(|_| anyhow::anyhow!("daemon returned invalid IP: {ip_str}"))?;
        return Ok(Some(ip));
    }
    if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
        anyhow::bail!("daemon error: {err}");
    }

    Ok(None)
}
