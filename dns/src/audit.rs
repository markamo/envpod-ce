// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! DNS query audit logging.
//!
//! Structured log entries for every DNS query a pod makes,
//! recording the domain, decision, and timing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single DNS query audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsQueryLog {
    /// When the query was received.
    pub timestamp: DateTime<Utc>,
    /// Pod that made the query.
    pub pod_name: String,
    /// Domain that was queried.
    pub domain: String,
    /// DNS record type (A, AAAA, CNAME, etc.).
    pub query_type: String,
    /// Policy decision (Allow, Deny, Remap).
    pub decision: String,
    /// Time to process the query in microseconds.
    pub latency_us: u64,
    /// Upstream server used (if forwarded).
    pub upstream_used: Option<String>,
}
