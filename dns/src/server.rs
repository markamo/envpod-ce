// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Async UDP DNS server with policy-based filtering.
//!
//! Binds to a specific IP:53, receives DNS queries, checks them against
//! the pod's DnsPolicy, forwards allowed queries to upstream resolvers,
//! and returns NXDOMAIN for denied queries.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::serialize::binary::BinDecodable;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};

use crate::audit::DnsQueryLog;
use crate::resolver::{DnsPolicy, PolicyDecision};

/// Audit entry queued for async writing.
struct AuditMessage {
    pod_name: String,
    detail: String,
}

/// In-memory DNS response cache with TTL-based expiration.
///
/// Caches raw upstream response bytes keyed by (domain, query_type).
/// On cache hit, the response ID is patched to match the current query.
/// Docker's dnsmasq caches by default — this brings envpod to parity for
/// sustained workloads (loop of curls, repeated API calls, etc.).
struct DnsCache {
    entries: HashMap<(String, u16), DnsCacheEntry>,
    max_ttl: Duration,
}

struct DnsCacheEntry {
    response: Vec<u8>,
    expires_at: Instant,
}

impl DnsCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_ttl: Duration::from_secs(300), // 5 min cap
        }
    }

    /// Look up a cached response. Returns None if not found or expired.
    /// On hit, patches the DNS header ID (bytes 0-1) to match the current query.
    fn get(&mut self, domain: &str, qtype: u16, query_id: u16) -> Option<Vec<u8>> {
        let key = (domain.to_ascii_lowercase(), qtype);
        let entry = self.entries.get(&key)?;
        if Instant::now() < entry.expires_at {
            let mut resp = entry.response.clone();
            if resp.len() >= 2 {
                resp[0] = (query_id >> 8) as u8;
                resp[1] = query_id as u8;
            }
            Some(resp)
        } else {
            self.entries.remove(&key);
            None
        }
    }

    /// Cache an upstream DNS response using the minimum TTL from answer records.
    fn insert(&mut self, domain: &str, qtype: u16, response: &[u8]) {
        let key = (domain.to_ascii_lowercase(), qtype);
        let ttl_secs = Self::extract_min_ttl(response).unwrap_or(60);
        let ttl = Duration::from_secs(ttl_secs.min(self.max_ttl.as_secs()));
        self.entries.insert(key, DnsCacheEntry {
            response: response.to_vec(),
            expires_at: Instant::now() + ttl,
        });
    }

    fn extract_min_ttl(data: &[u8]) -> Option<u64> {
        let msg = Message::from_bytes(data).ok()?;
        msg.answers()
            .iter()
            .chain(msg.name_servers().iter())
            .map(|r| r.ttl())
            .min()
            .map(|t| t as u64)
    }
}

/// DNS server that filters queries based on pod policy.
pub struct DnsServer {
    bind_addr: SocketAddr,
    policy: Arc<RwLock<DnsPolicy>>,
    upstream: Vec<SocketAddr>,
    pod_name: String,
    /// Path to the pod's audit.jsonl file for per-query audit logging.
    audit_path: Option<PathBuf>,
    /// Unix socket path of the envpod-dns daemon for `*.pods.local` resolution.
    /// When set, pod name lookups are forwarded to the central daemon (no file reads).
    /// When None or daemon unreachable, `*.pods.local` returns NXDOMAIN.
    daemon_sock: Option<PathBuf>,
    /// When true, AAAA (IPv6) queries return NODATA immediately without forwarding
    /// to upstream. Pods always have IPv6 disabled (disable_ipv6=1), so AAAA queries
    /// are wasted upstream round-trips.
    filter_aaaa: bool,
}

/// Handle returned by `DnsServer::spawn()` — used to shut down the server.
pub struct DnsServerHandle {
    shutdown_tx: watch::Sender<bool>,
    join_handle: tokio::task::JoinHandle<()>,
    policy: Arc<RwLock<DnsPolicy>>,
}

impl DnsServerHandle {
    /// Signal the DNS server to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the server task to finish.
    pub async fn join(self) {
        let _ = self.join_handle.await;
    }

    /// Get a reference to the shared DNS policy (for live updates).
    pub fn policy(&self) -> &Arc<RwLock<DnsPolicy>> {
        &self.policy
    }
}

impl DnsServer {
    /// Create a new DNS server.
    ///
    /// - `bind_ip`: IP address to bind (host-side veth IP)
    /// - `policy`: DNS filtering policy for this pod
    /// - `upstream`: upstream DNS server addresses
    /// - `pod_name`: pod name for logging
    pub fn new(
        bind_ip: Ipv4Addr,
        policy: DnsPolicy,
        upstream: Vec<SocketAddr>,
        pod_name: String,
    ) -> Self {
        Self {
            bind_addr: SocketAddr::new(bind_ip.into(), 53),
            policy: Arc::new(RwLock::new(policy)),
            upstream,
            pod_name,
            audit_path: None,
            daemon_sock: None,
            filter_aaaa: false,
        }
    }

    /// Create a DNS server bound to a custom port (useful for testing without root).
    pub fn new_with_port(
        bind_ip: Ipv4Addr,
        port: u16,
        policy: DnsPolicy,
        upstream: Vec<SocketAddr>,
        pod_name: String,
    ) -> Self {
        Self {
            bind_addr: SocketAddr::new(bind_ip.into(), port),
            policy: Arc::new(RwLock::new(policy)),
            upstream,
            pod_name,
            audit_path: None,
            daemon_sock: None,
            filter_aaaa: false,
        }
    }

    /// Create a DNS server with a pre-shared policy (for live mutation).
    pub fn new_with_shared_policy(
        bind_ip: Ipv4Addr,
        policy: Arc<RwLock<DnsPolicy>>,
        upstream: Vec<SocketAddr>,
        pod_name: String,
    ) -> Self {
        Self {
            bind_addr: SocketAddr::new(bind_ip.into(), 53),
            policy,
            upstream,
            pod_name,
            audit_path: None,
            daemon_sock: None,
            filter_aaaa: false,
        }
    }

    /// Set the envpod-dns daemon socket path for `*.pods.local` resolution.
    /// When set, pod name lookups are forwarded to the daemon (in-memory, no file reads).
    pub fn with_daemon_sock(mut self, sock: PathBuf) -> Self {
        self.daemon_sock = Some(sock);
        self
    }

    /// Filter AAAA (IPv6) queries — return NODATA immediately without upstream.
    /// Pods have IPv6 disabled (disable_ipv6=1), so AAAA upstream queries are wasted.
    pub fn with_filter_aaaa(mut self, filter: bool) -> Self {
        self.filter_aaaa = filter;
        self
    }

    /// Set the audit log path for per-query DNS audit logging.
    ///
    /// When set, every DNS query decision is appended to this file in JSONL
    /// format (compatible with the pod's `audit.jsonl`).
    pub fn with_audit_path(mut self, path: PathBuf) -> Self {
        self.audit_path = Some(path);
        self
    }

    /// Spawn the DNS server as a tokio task. Returns a handle for shutdown.
    pub async fn spawn(self) -> Result<DnsServerHandle> {
        // Create a dual-stack (IPv6 + IPv4) socket so the server can receive
        // DNS queries from both AF_INET and AF_INET6 clients. Modern resolvers
        // (nslookup, glibc) often use AF_INET6 sockets even for IPv4 targets,
        // sending to ::ffff:X.X.X.X. Without dual-stack, those queries silently
        // miss our IPv4-only socket.
        let socket = Self::bind_dual_stack(self.bind_addr)
            .await
            .with_context(|| format!("bind DNS server to {}", self.bind_addr))?;

        tracing::info!(
            addr = %self.bind_addr,
            pod = %self.pod_name,
            "DNS server started (dual-stack)"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let policy = self.policy.clone();

        // Spawn async audit writer — non-blocking channel replaces sync file I/O
        // in the DNS hot path. Writer holds a persistent file handle and drains
        // the channel, so DNS queries never block on disk.
        let audit_tx = Self::spawn_audit_writer(self.audit_path);

        let join_handle = tokio::spawn(Self::run_loop(
            socket,
            self.policy,
            self.upstream,
            self.pod_name,
            audit_tx,
            self.daemon_sock,
            self.filter_aaaa,
            shutdown_rx,
        ));

        Ok(DnsServerHandle {
            shutdown_tx,
            join_handle,
            policy,
        })
    }

    /// Spawn a background task that writes audit entries to disk.
    ///
    /// Returns an mpsc sender. Callers send `AuditMessage` without blocking.
    /// The writer holds a persistent file handle (no open/close per entry)
    /// and flushes after draining all available messages.
    fn spawn_audit_writer(audit_path: Option<PathBuf>) -> Option<mpsc::UnboundedSender<AuditMessage>> {
        let path = audit_path?;

        let (tx, mut rx) = mpsc::unbounded_channel::<AuditMessage>();

        tokio::spawn(async move {
            use std::io::Write;

            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path);

            let mut file = match file {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open DNS audit file, audit disabled");
                    return;
                }
            };

            while let Some(msg) = rx.recv().await {
                let entry = serde_json::json!({
                    "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    "pod_name": msg.pod_name,
                    "action": "dns_query",
                    "detail": msg.detail,
                    "success": true,
                });
                let _ = writeln!(file, "{}", entry);

                // Drain any buffered messages before flushing
                while let Ok(msg) = rx.try_recv() {
                    let entry = serde_json::json!({
                        "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                        "pod_name": msg.pod_name,
                        "action": "dns_query",
                        "detail": msg.detail,
                        "success": true,
                    });
                    let _ = writeln!(file, "{}", entry);
                }

                let _ = file.flush();
            }
        });

        Some(tx)
    }

    /// Bind a UDP socket that accepts both IPv4 and IPv6 DNS queries.
    ///
    /// Creates an AF_INET6 socket with IPV6_V6ONLY=false, bound to the
    /// IPv4-mapped IPv6 address (::ffff:X.X.X.X). This receives:
    /// - IPv4 packets to X.X.X.X:port (from `echo > /dev/udp/...`, bash)
    /// - IPv6 packets to ::ffff:X.X.X.X:port (from nslookup, glibc, curl)
    ///
    /// Falls back to IPv4-only if IPv6 binding fails.
    async fn bind_dual_stack(addr: SocketAddr) -> Result<UdpSocket> {
        use socket2::{Domain, Protocol, Socket, Type};

        // Extract the IPv4 address and port
        let (ipv4, port) = match addr {
            SocketAddr::V4(v4) => (*v4.ip(), v4.port()),
            SocketAddr::V6(_) => {
                // Already IPv6 — just bind directly
                let std_sock = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))
                    .context("create IPv6 socket")?;
                std_sock.set_only_v6(false).ok();
                std_sock.set_reuse_address(true).ok();
                std_sock.set_nonblocking(true).context("set nonblocking")?;
                std_sock.bind(&addr.into()).context("bind IPv6")?;
                return UdpSocket::from_std(std_sock.into())
                    .context("convert to tokio socket");
            }
        };

        // Try dual-stack IPv6 socket first
        let v6_mapped = ipv4.to_ipv6_mapped();
        let v6_addr = std::net::SocketAddrV6::new(v6_mapped, port, 0, 0);

        let result: std::io::Result<Socket> = (|| {
            let sock = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
            sock.set_only_v6(false)?; // dual-stack: receive IPv4 + IPv6
            sock.set_reuse_address(true)?;
            sock.set_nonblocking(true)?;
            sock.bind(&SocketAddr::V6(v6_addr).into())?;
            Ok(sock)
        })();

        match result {
            Ok(sock) => {
                tracing::debug!("[dns] bound dual-stack socket on [{v6_mapped}]:{port}");
                UdpSocket::from_std(sock.into()).context("convert to tokio socket")
            }
            Err(e) => {
                tracing::debug!("[dns] dual-stack bind failed ({e}), falling back to IPv4");
                UdpSocket::bind(addr).await.context("bind IPv4 fallback")
            }
        }
    }

    async fn run_loop(
        socket: UdpSocket,
        policy: Arc<RwLock<DnsPolicy>>,
        upstream: Vec<SocketAddr>,
        pod_name: String,
        audit_tx: Option<mpsc::UnboundedSender<AuditMessage>>,
        daemon_sock: Option<PathBuf>,
        filter_aaaa: bool,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let socket = Arc::new(socket);
        let audit_tx = Arc::new(audit_tx);
        let daemon_sock = Arc::new(daemon_sock);
        let cache = Arc::new(Mutex::new(DnsCache::new()));
        let mut buf = vec![0u8; 4096];
        let mut pkt_count: u64 = 0;

        tracing::debug!(
            "[dns] run_loop active — listening on {:?}, upstream: {:?}",
            socket.local_addr(),
            upstream
        );

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, src)) => {
                            pkt_count += 1;
                            let data = buf[..len].to_vec();
                            let policy = policy.clone();
                            let upstream = upstream.clone();
                            let pod_name = pod_name.clone();
                            let sock = socket.clone();
                            let pkt_num = pkt_count;
                            let audit_tx = audit_tx.clone();
                            let daemon_sock = daemon_sock.clone();
                            let cache = cache.clone();

                            // Spawn each query handler as a separate task so
                            // slow upstream forwarding doesn't block the receive loop.
                            tokio::spawn(async move {
                                tracing::debug!("[dns] pkt#{pkt_num} received {len} bytes from {src}");
                                match Self::handle_query(&data, &policy, &upstream, &pod_name, &audit_tx, &daemon_sock, &cache, filter_aaaa).await {
                                    Ok(response) => {
                                        tracing::debug!("[dns] pkt#{pkt_num} from {src} → resolved ({} bytes)", response.len());
                                        if let Err(e) = sock.send_to(&response, src).await {
                                            tracing::warn!("[dns] pkt#{pkt_num} failed to send response to {src}: {e}");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("[dns] pkt#{pkt_num} from {src} → error: {e}");
                                        if let Ok(servfail) = Self::build_servfail(&data) {
                                            let _ = sock.send_to(&servfail, src).await;
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "[dns] recv_from error");
                        }
                    }
                }
                result = shutdown_rx.changed() => {
                    match result {
                        Ok(()) => tracing::debug!("[dns] shutdown signal received"),
                        Err(_) => tracing::debug!("[dns] shutdown sender dropped"),
                    }
                    break;
                }
            }
        }
        tracing::debug!("[dns] run_loop exited for pod '{pod_name}' (total packets: {pkt_count})");
    }

    async fn handle_query(
        data: &[u8],
        policy: &Arc<RwLock<DnsPolicy>>,
        upstream: &[SocketAddr],
        pod_name: &str,
        audit_tx: &Arc<Option<mpsc::UnboundedSender<AuditMessage>>>,
        daemon_sock: &Arc<Option<PathBuf>>,
        cache: &Arc<Mutex<DnsCache>>,
        filter_aaaa: bool,
    ) -> Result<Vec<u8>> {
        use hickory_proto::rr::record_type::RecordType;

        let start = Instant::now();

        let request = Message::from_bytes(data).context("parse DNS query")?;

        // Extract the queried domain from the first question
        let question = request
            .queries()
            .first()
            .context("DNS query has no questions")?;

        let domain = question.name().to_string();
        let query_type = question.query_type();

        // Short-circuit AAAA queries when IPv6 is disabled in the pod.
        // Returns NODATA (NOERROR + empty answer) instantly — saves an upstream
        // round-trip per unique domain since pods always have disable_ipv6=1.
        if filter_aaaa && query_type == RecordType::AAAA {
            tracing::debug!(pod = %pod_name, domain = %domain, "AAAA filtered (IPv6 disabled)");
            return Self::build_nodata(&request);
        }

        // Pod discovery: intercept *.pods.local before any policy check.
        // Forwards to the central envpod-dns daemon via Unix socket (no file reads).
        // Fail-safe: daemon unreachable or pod not permitted → NXDOMAIN.
        {
            let d = domain.to_ascii_lowercase();
            let d = d.trim_end_matches('.');
            if let Some(queried_pod) = d.strip_suffix(".pods.local") {
                if !queried_pod.is_empty() {
                    return Self::handle_pod_lookup(
                        &request, queried_pod, daemon_sock, audit_tx,
                        pod_name, &domain, &format!("{query_type}"),
                    ).await;
                }
            }
        }

        // Check policy (acquire read lock — microseconds, safe in async)
        let decision = {
            let policy_guard = policy.read().expect("DNS policy lock poisoned");
            policy_guard.check(&domain)
        };
        let elapsed = start.elapsed();

        let _log = DnsQueryLog {
            timestamp: chrono::Utc::now(),
            pod_name: pod_name.to_string(),
            domain: domain.clone(),
            query_type: format!("{query_type}"),
            decision: format!("{decision:?}"),
            latency_us: elapsed.as_micros() as u64,
            upstream_used: None,
        };

        // Send audit entry to async writer (non-blocking)
        Self::send_audit(audit_tx, pod_name, &domain, &format!("{query_type}"), &decision);

        match &decision {
            PolicyDecision::Allow => {
                // Check response cache before hitting upstream
                let qtype_u16 = u16::from(query_type);
                if let Some(cached) = cache.lock().expect("dns cache").get(&domain, qtype_u16, request.id()) {
                    tracing::debug!(
                        pod = %pod_name,
                        domain = %domain,
                        "DNS cache hit"
                    );
                    return Ok(cached);
                }

                tracing::info!(
                    pod = %pod_name,
                    domain = %domain,
                    decision = "allow",
                    latency_us = %_log.latency_us,
                    "DNS query"
                );
                let response = Self::forward_to_upstream(data, upstream, pod_name).await?;

                // Cache upstream response (TTL extracted from answer records)
                cache.lock().expect("dns cache").insert(&domain, qtype_u16, &response);

                Ok(response)
            }
            PolicyDecision::Deny => {
                tracing::info!(
                    pod = %pod_name,
                    domain = %domain,
                    decision = "deny",
                    "DNS query blocked"
                );
                Self::build_nxdomain(&request)
            }
            PolicyDecision::Remap(target) => {
                tracing::info!(
                    pod = %pod_name,
                    domain = %domain,
                    target = %target,
                    decision = "remap",
                    "DNS query remapped"
                );
                Self::handle_remap(&request, data, target, upstream, pod_name).await
            }
        }
    }

    /// Send a DNS audit entry to the background writer (non-blocking).
    ///
    /// The entry is queued via an unbounded mpsc channel. The background writer
    /// holds a persistent file handle and batches writes, so DNS query latency
    /// is never blocked by disk I/O.
    fn send_audit(
        audit_tx: &Option<mpsc::UnboundedSender<AuditMessage>>,
        pod_name: &str,
        domain: &str,
        query_type: &str,
        decision: &PolicyDecision,
    ) {
        let Some(tx) = audit_tx else { return };

        let decision_str = match decision {
            PolicyDecision::Allow => "allow",
            PolicyDecision::Deny => "deny",
            PolicyDecision::Remap(_) => "remap",
        };

        let detail = match decision {
            PolicyDecision::Remap(target) => {
                format!("domain={domain} type={query_type} decision={decision_str} target={target}")
            }
            _ => format!("domain={domain} type={query_type} decision={decision_str}"),
        };

        let _ = tx.send(AuditMessage {
            pod_name: pod_name.to_string(),
            detail,
        });
    }

    /// Resolve a `*.pods.local` query via the central envpod-dns daemon.
    ///
    /// Forwards the lookup to the daemon over a Unix socket (no file reads).
    /// The daemon enforces bilateral policy: target must have `allow_discovery: true`
    /// AND the querying pod must have the target in its `allow_pods` list.
    ///
    /// Fail-safe: daemon unreachable → NXDOMAIN (same as pod not found).
    async fn handle_pod_lookup(
        request: &Message,
        queried_pod: &str,
        daemon_sock: &Arc<Option<PathBuf>>,
        audit_tx: &Arc<Option<mpsc::UnboundedSender<AuditMessage>>>,
        querying_pod: &str,
        domain: &str,
        query_type_str: &str,
    ) -> Result<Vec<u8>> {
        use hickory_proto::rr::rdata::A;
        use hickory_proto::rr::record_type::RecordType;
        use hickory_proto::rr::{RData, Record};

        // Query the daemon — fail-safe: any error → NXDOMAIN
        let maybe_ip = match daemon_sock.as_ref() {
            Some(sock) => {
                crate::daemon_client::lookup(sock, queried_pod, querying_pod)
                    .await
                    .unwrap_or(None)
            }
            None => None,
        };

        match maybe_ip {
            Some(ipv4) => {
                Self::send_audit(
                    audit_tx.as_ref(),
                    querying_pod,
                    domain,
                    query_type_str,
                    &PolicyDecision::Remap(ipv4.to_string()),
                );
                tracing::info!(
                    pod = querying_pod,
                    lookup = queried_pod,
                    ip = %ipv4,
                    "pod discovery lookup"
                );

                let mut response = Message::new();
                response.set_id(request.id());
                response.set_message_type(MessageType::Response);
                response.set_op_code(OpCode::Query);
                response.set_recursion_desired(request.recursion_desired());
                response.set_recursion_available(true);
                response.set_response_code(ResponseCode::NoError);
                for q in request.queries() {
                    response.add_query(q.clone());
                }
                // Only add A record for A queries; AAAA/other get NoError + no answers
                let qt = request.queries().first().map(|q| q.query_type());
                if qt == Some(RecordType::A) {
                    let name = request.queries().first().unwrap().name().clone();
                    let record = Record::from_rdata(name, 60, RData::A(A(ipv4)));
                    response.add_answer(record);
                }
                Ok(response.to_vec()?)
            }
            None => {
                Self::send_audit(
                    audit_tx.as_ref(),
                    querying_pod,
                    domain,
                    query_type_str,
                    &PolicyDecision::Deny,
                );
                tracing::debug!(
                    pod = querying_pod,
                    lookup = queried_pod,
                    "pod discovery: NXDOMAIN (not found, not permitted, or daemon unreachable)"
                );
                Self::build_nxdomain(request)
            }
        }
    }

    /// Handle a remapped DNS query.
    ///
    /// If the target is an IP address, builds a synthetic A/AAAA response.
    /// If the target is a domain, forwards a query for the target domain
    /// and returns the response with the original request ID.
    async fn handle_remap(
        request: &Message,
        _raw_data: &[u8],
        target: &str,
        upstream: &[SocketAddr],
        pod_name: &str,
    ) -> Result<Vec<u8>> {
        use hickory_proto::rr::rdata::{A, AAAA};
        use hickory_proto::rr::{Name, RData, Record};

        // If target is an IPv4 address, build a synthetic A response
        if let Ok(ipv4) = target.parse::<std::net::Ipv4Addr>() {
            let name = request.queries().first()
                .context("no question in request")?.name().clone();

            let record = Record::from_rdata(name, 300, RData::A(A(ipv4)));

            let mut response = Message::new();
            response.set_id(request.id());
            response.set_message_type(MessageType::Response);
            response.set_op_code(OpCode::Query);
            response.set_recursion_desired(request.recursion_desired());
            response.set_recursion_available(true);
            response.set_response_code(ResponseCode::NoError);

            for q in request.queries() {
                response.add_query(q.clone());
            }
            response.add_answer(record);

            return Ok(response.to_vec()?);
        }

        // If target is an IPv6 address, build a synthetic AAAA response
        if let Ok(ipv6) = target.parse::<std::net::Ipv6Addr>() {
            let name = request.queries().first()
                .context("no question in request")?.name().clone();

            let record = Record::from_rdata(name, 300, RData::AAAA(AAAA(ipv6)));

            let mut response = Message::new();
            response.set_id(request.id());
            response.set_message_type(MessageType::Response);
            response.set_op_code(OpCode::Query);
            response.set_recursion_desired(request.recursion_desired());
            response.set_recursion_available(true);
            response.set_response_code(ResponseCode::NoError);

            for q in request.queries() {
                response.add_query(q.clone());
            }
            response.add_answer(record);

            return Ok(response.to_vec()?);
        }

        // Target is a domain — build a new query for the target domain,
        // forward to upstream, and return the response with original request ID.
        let target_name = Name::from_ascii(target)
            .with_context(|| format!("parse remap target domain: {target}"))?;

        let query_type = request.queries().first()
            .context("no question in request")?.query_type();

        let mut target_request = Message::new();
        target_request.set_id(request.id());
        target_request.set_message_type(MessageType::Query);
        target_request.set_op_code(OpCode::Query);
        target_request.set_recursion_desired(true);

        let mut query = hickory_proto::op::Query::new();
        query.set_name(target_name);
        query.set_query_type(query_type);
        target_request.add_query(query);

        let target_data = target_request.to_vec()?;
        Self::forward_to_upstream(&target_data, upstream, pod_name).await
    }

    /// Forward a raw DNS query to an upstream resolver and return the response.
    async fn forward_to_upstream(
        data: &[u8],
        upstream: &[SocketAddr],
        pod_name: &str,
    ) -> Result<Vec<u8>> {
        anyhow::ensure!(!upstream.is_empty(), "no upstream DNS servers configured");

        // Try each upstream in order
        for server in upstream {
            match Self::query_upstream(data, *server).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    tracing::warn!(
                        pod = %pod_name,
                        upstream = %server,
                        error = %e,
                        "upstream DNS query failed, trying next"
                    );
                }
            }
        }

        anyhow::bail!("all upstream DNS servers failed")
    }

    /// Send a raw DNS query to a single upstream server and return the response.
    async fn query_upstream(data: &[u8], server: SocketAddr) -> Result<Vec<u8>> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("bind upstream socket")?;

        socket
            .send_to(data, server)
            .await
            .context("send to upstream")?;

        let mut buf = vec![0u8; 4096];
        let timeout = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.recv_from(&mut buf),
        )
        .await
        .context("upstream DNS timeout")?
        .context("upstream recv")?;

        Ok(buf[..timeout.0].to_vec())
    }

    /// Build a NODATA response (NOERROR + empty answer section).
    /// Used for AAAA queries when IPv6 is disabled — tells the client
    /// "domain exists but has no AAAA records", so it falls back to A immediately.
    fn build_nodata(request: &Message) -> Result<Vec<u8>> {
        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_recursion_desired(request.recursion_desired());
        response.set_recursion_available(true);
        response.set_response_code(ResponseCode::NoError);

        for q in request.queries() {
            response.add_query(q.clone());
        }

        Ok(response.to_vec()?)
    }

    /// Build an NXDOMAIN response for a denied query.
    fn build_nxdomain(request: &Message) -> Result<Vec<u8>> {
        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_recursion_desired(request.recursion_desired());
        response.set_recursion_available(true);
        response.set_response_code(ResponseCode::NXDomain);

        // Copy the question section
        for q in request.queries() {
            response.add_query(q.clone());
        }

        Ok(response.to_vec()?)
    }

    /// Build a SERVFAIL response from raw query bytes.
    fn build_servfail(data: &[u8]) -> Result<Vec<u8>> {
        let request = Message::from_bytes(data)?;
        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_response_code(ResponseCode::ServFail);

        for q in request.queries() {
            response.add_query(q.clone());
        }

        Ok(response.to_vec()?)
    }
}

/// Parse upstream DNS servers from the host's resolver config.
///
/// On systemd-resolved systems, reads the non-stub resolv.conf at
/// `/run/systemd/resolve/resolv.conf` which contains real upstream servers.
/// Falls back to `/etc/resolv.conf`, then to Google DNS (8.8.8.8).
///
/// Also filters out loopback stub addresses (127.0.0.53) since the DNS
/// server itself needs to forward to real upstream resolvers, not back
/// to the local stub.
pub fn parse_host_resolv_conf() -> Vec<SocketAddr> {
    // Prefer the non-stub systemd-resolved file (has real upstream servers)
    let servers = if std::path::Path::new("/run/systemd/resolve/resolv.conf").exists() {
        parse_resolv_conf_from("/run/systemd/resolve/resolv.conf")
    } else {
        parse_resolv_conf_from("/etc/resolv.conf")
    };

    // Filter out 127.0.0.53 (systemd-resolved stub) — we need real upstreams
    let real: Vec<SocketAddr> = servers
        .into_iter()
        .filter(|s| {
            !matches!(s.ip(), std::net::IpAddr::V4(ip) if ip == std::net::Ipv4Addr::new(127, 0, 0, 53))
        })
        .collect();

    if real.is_empty() {
        vec![SocketAddr::new("8.8.8.8".parse().unwrap(), 53)]
    } else {
        real
    }
}

/// Parse upstream DNS servers from a resolv.conf file.
pub fn parse_resolv_conf_from(path: &str) -> Vec<SocketAddr> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![SocketAddr::new("8.8.8.8".parse().unwrap(), 53)],
    };

    parse_resolv_conf(&contents)
}

/// Parse nameserver lines from resolv.conf content.
pub fn parse_resolv_conf(content: &str) -> Vec<SocketAddr> {
    let servers: Vec<SocketAddr> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("nameserver") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(ip) = parts[1].parse::<std::net::IpAddr>() {
                        return Some(SocketAddr::new(ip, 53));
                    }
                }
            }
            None
        })
        .collect();

    if servers.is_empty() {
        // Fallback to Google DNS
        vec![SocketAddr::new("8.8.8.8".parse().unwrap(), 53)]
    } else {
        servers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resolv_conf_extracts_nameservers() {
        let content = "\
# Generated by NetworkManager
nameserver 192.168.1.1
nameserver 8.8.8.8
search home.lan
";
        let servers = parse_resolv_conf(content);
        assert_eq!(servers.len(), 2);
        assert_eq!(
            servers[0],
            SocketAddr::new("192.168.1.1".parse().unwrap(), 53)
        );
        assert_eq!(
            servers[1],
            SocketAddr::new("8.8.8.8".parse().unwrap(), 53)
        );
    }

    #[test]
    fn parse_resolv_conf_handles_ipv6() {
        let content = "nameserver ::1\nnameserver 8.8.4.4\n";
        let servers = parse_resolv_conf(content);
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn parse_resolv_conf_empty_falls_back() {
        let servers = parse_resolv_conf("");
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0],
            SocketAddr::new("8.8.8.8".parse().unwrap(), 53)
        );
    }

    #[test]
    fn parse_resolv_conf_comments_only() {
        let content = "# comment\n# another comment\n";
        let servers = parse_resolv_conf(content);
        assert_eq!(servers.len(), 1); // fallback
    }

    #[test]
    fn nxdomain_response_format() {
        use hickory_proto::op::{Message, MessageType, OpCode, Query};
        use hickory_proto::rr::record_type::RecordType;
        use hickory_proto::rr::Name;

        let mut request = Message::new();
        request.set_id(1234);
        request.set_message_type(MessageType::Query);
        request.set_op_code(OpCode::Query);
        request.set_recursion_desired(true);

        let mut query = Query::new();
        query.set_name(Name::from_ascii("evil.com.").unwrap());
        query.set_query_type(RecordType::A);
        request.add_query(query);

        let response_bytes = DnsServer::build_nxdomain(&request).unwrap();
        let response = Message::from_bytes(&response_bytes).unwrap();

        assert_eq!(response.id(), 1234);
        assert_eq!(response.message_type(), MessageType::Response);
        assert_eq!(response.response_code(), ResponseCode::NXDomain);
        assert_eq!(response.queries().len(), 1);
        assert!(response.recursion_available());
    }

    /// Integration test: start a DnsServer on a high port, send real UDP queries,
    /// verify that whitelisted domains pass and unlisted domains get NXDOMAIN.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn dns_server_filters_queries() {
        use crate::resolver::DnsPolicyMode;
        use hickory_proto::op::{Message, MessageType, OpCode, Query};
        use hickory_proto::rr::record_type::RecordType;
        use hickory_proto::rr::Name;
        use std::collections::HashMap;
        use tokio::net::UdpSocket;

        let policy = DnsPolicy {
            mode: DnsPolicyMode::Whitelist,
            allowed_domains: vec!["anthropic.com".into()],
            denied_domains: Vec::new(),
            remap: HashMap::new(),
        };

        // Use port 0 so the OS picks a free port; we need to know the actual port.
        // DnsServer binds internally, so we use a known high port instead.
        let port: u16 = 15353;
        let upstream = parse_resolv_conf("nameserver 8.8.8.8\n");

        let server = DnsServer::new_with_port(
            Ipv4Addr::new(127, 0, 0, 1),
            port,
            policy,
            upstream,
            "test-dns".into(),
        );
        let handle = server.spawn().await.expect("DNS server should start");

        // Give server a moment to be ready
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

        // --- Query 1: allowed domain (anthropic.com) ---
        {
            let mut request = Message::new();
            request.set_id(1001);
            request.set_message_type(MessageType::Query);
            request.set_op_code(OpCode::Query);
            request.set_recursion_desired(true);
            let mut q = Query::new();
            q.set_name(Name::from_ascii("api.anthropic.com.").unwrap());
            q.set_query_type(RecordType::A);
            request.add_query(q);

            let sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
            sock.send_to(&request.to_vec().unwrap(), server_addr).await.unwrap();

            let mut buf = vec![0u8; 4096];
            let (len, _) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                sock.recv_from(&mut buf),
            )
            .await
            .expect("should get response for allowed domain")
            .unwrap();

            let response = Message::from_bytes(&buf[..len]).unwrap();
            assert_eq!(response.id(), 1001);
            // Should NOT be NXDOMAIN — upstream will resolve it
            assert_ne!(
                response.response_code(),
                ResponseCode::NXDomain,
                "allowed domain should not get NXDOMAIN"
            );
        }

        // --- Query 2: denied domain (evil.com) ---
        {
            let mut request = Message::new();
            request.set_id(1002);
            request.set_message_type(MessageType::Query);
            request.set_op_code(OpCode::Query);
            request.set_recursion_desired(true);
            let mut q = Query::new();
            q.set_name(Name::from_ascii("evil.com.").unwrap());
            q.set_query_type(RecordType::A);
            request.add_query(q);

            let sock = UdpSocket::bind("0.0.0.0:0").await.unwrap();
            sock.send_to(&request.to_vec().unwrap(), server_addr).await.unwrap();

            let mut buf = vec![0u8; 4096];
            let (len, _) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                sock.recv_from(&mut buf),
            )
            .await
            .expect("should get response for denied domain")
            .unwrap();

            let response = Message::from_bytes(&buf[..len]).unwrap();
            assert_eq!(response.id(), 1002);
            assert_eq!(
                response.response_code(),
                ResponseCode::NXDomain,
                "denied domain should get NXDOMAIN"
            );
        }

        // Shut down
        handle.shutdown();
        handle.join().await;
    }
}
