// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! DNS policy engine — pure logic, no I/O.
//!
//! Decides whether a DNS query should be allowed, denied, or remapped
//! based on the pod's DNS configuration (allowlist/denylist/remap).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// DNS filtering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsPolicyMode {
    /// Only explicitly allowed domains resolve.
    #[serde(alias = "whitelist")]
    Allowlist,
    /// All domains resolve except explicitly denied.
    #[serde(alias = "blacklist")]
    Denylist,
    /// All domains resolve; queries are logged only.
    Monitor,
}

/// Policy configuration for DNS filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsPolicy {
    pub mode: DnsPolicyMode,
    /// Domains allowed (used in allowlist mode).
    pub allowed_domains: Vec<String>,
    /// Domains denied (used in denylist mode).
    pub denied_domains: Vec<String>,
    /// Domain → address remapping (applied before allow/deny).
    pub remap: HashMap<String, String>,
}

/// Result of a policy check on a DNS query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Allow the query to proceed to upstream resolver.
    Allow,
    /// Deny the query — return NXDOMAIN.
    Deny,
    /// Remap the domain to a different target address.
    Remap(String),
}

impl DnsPolicy {
    /// Check a domain against the policy.
    ///
    /// Domain matching is case-insensitive and trailing-dot normalized.
    /// Suffix matching: `anthropic.com` matches `api.anthropic.com`.
    pub fn check(&self, domain: &str) -> PolicyDecision {
        let domain = normalize_domain(domain);

        // Remap takes priority over allow/deny.
        if let Some(target) = self.remap_match(&domain) {
            return PolicyDecision::Remap(target);
        }

        match self.mode {
            DnsPolicyMode::Allowlist => {
                if self.matches_list(&domain, &self.allowed_domains) {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Deny
                }
            }
            DnsPolicyMode::Denylist => {
                if self.matches_list(&domain, &self.denied_domains) {
                    PolicyDecision::Deny
                } else {
                    PolicyDecision::Allow
                }
            }
            DnsPolicyMode::Monitor => PolicyDecision::Allow,
        }
    }

    /// Check if domain matches any entry in the remap table.
    fn remap_match(&self, domain: &str) -> Option<String> {
        for (pattern, target) in &self.remap {
            let pattern = normalize_domain(pattern);
            if domain_matches(domain, &pattern) {
                return Some(target.clone());
            }
        }
        None
    }

    /// Check if domain matches any entry in a domain list.
    fn matches_list(&self, domain: &str, list: &[String]) -> bool {
        list.iter().any(|entry| {
            let entry = normalize_domain(entry);
            domain_matches(domain, &entry)
        })
    }
}

/// Normalize a domain: lowercase, strip trailing dot.
fn normalize_domain(domain: &str) -> String {
    domain.to_ascii_lowercase().trim_end_matches('.').to_string()
}

/// Check if `domain` matches `pattern` — exact or suffix subdomain match.
///
/// Supports `*.` prefix syntax (e.g., `*.anthropic.com` → matches subdomains).
/// The `*.` is stripped before matching — suffix matching already covers it.
///
/// `api.anthropic.com` matches pattern `anthropic.com`
/// `anthropic.com` matches pattern `anthropic.com`
/// `notanthropic.com` does NOT match pattern `anthropic.com`
/// `platform.claude.com` matches pattern `*.claude.com`
fn domain_matches(domain: &str, pattern: &str) -> bool {
    // Strip wildcard prefix — suffix matching handles subdomain matching.
    let pattern = pattern.strip_prefix("*.").unwrap_or(pattern);

    if domain == pattern {
        return true;
    }
    // Suffix match: domain must end with `.{pattern}`
    domain.ends_with(&format!(".{pattern}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist_policy(domains: &[&str]) -> DnsPolicy {
        DnsPolicy {
            mode: DnsPolicyMode::Allowlist,
            allowed_domains: domains.iter().map(|s| s.to_string()).collect(),
            denied_domains: Vec::new(),
            remap: HashMap::new(),
        }
    }

    fn denylist_policy(domains: &[&str]) -> DnsPolicy {
        DnsPolicy {
            mode: DnsPolicyMode::Denylist,
            allowed_domains: Vec::new(),
            denied_domains: domains.iter().map(|s| s.to_string()).collect(),
            remap: HashMap::new(),
        }
    }

    fn monitor_policy() -> DnsPolicy {
        DnsPolicy {
            mode: DnsPolicyMode::Monitor,
            allowed_domains: Vec::new(),
            denied_domains: Vec::new(),
            remap: HashMap::new(),
        }
    }

    // -- Allowlist mode --

    #[test]
    fn allowlist_allows_exact_match() {
        let policy = allowlist_policy(&["anthropic.com"]);
        assert_eq!(policy.check("anthropic.com"), PolicyDecision::Allow);
    }

    #[test]
    fn allowlist_allows_subdomain() {
        let policy = allowlist_policy(&["anthropic.com"]);
        assert_eq!(policy.check("api.anthropic.com"), PolicyDecision::Allow);
    }

    #[test]
    fn allowlist_denies_unlisted() {
        let policy = allowlist_policy(&["anthropic.com"]);
        assert_eq!(policy.check("evil.com"), PolicyDecision::Deny);
    }

    #[test]
    fn allowlist_denies_partial_suffix() {
        let policy = allowlist_policy(&["anthropic.com"]);
        // "notanthropic.com" should NOT match "anthropic.com"
        assert_eq!(policy.check("notanthropic.com"), PolicyDecision::Deny);
    }

    #[test]
    fn allowlist_empty_denies_all() {
        let policy = allowlist_policy(&[]);
        assert_eq!(policy.check("anything.com"), PolicyDecision::Deny);
    }

    // -- Denylist mode --

    #[test]
    fn denylist_denies_exact_match() {
        let policy = denylist_policy(&["evil.com"]);
        assert_eq!(policy.check("evil.com"), PolicyDecision::Deny);
    }

    #[test]
    fn denylist_denies_subdomain() {
        let policy = denylist_policy(&["evil.com"]);
        assert_eq!(policy.check("www.evil.com"), PolicyDecision::Deny);
    }

    #[test]
    fn denylist_allows_unlisted() {
        let policy = denylist_policy(&["evil.com"]);
        assert_eq!(policy.check("good.com"), PolicyDecision::Allow);
    }

    #[test]
    fn denylist_empty_allows_all() {
        let policy = denylist_policy(&[]);
        assert_eq!(policy.check("anything.com"), PolicyDecision::Allow);
    }

    // -- Monitor mode --

    #[test]
    fn monitor_allows_everything() {
        let policy = monitor_policy();
        assert_eq!(policy.check("evil.com"), PolicyDecision::Allow);
        assert_eq!(policy.check("anything.example.org"), PolicyDecision::Allow);
    }

    // -- Remap --

    #[test]
    fn remap_takes_priority() {
        let mut policy = allowlist_policy(&["anthropic.com"]);
        policy
            .remap
            .insert("internal.dev".into(), "127.0.0.1".into());
        assert_eq!(
            policy.check("internal.dev"),
            PolicyDecision::Remap("127.0.0.1".into())
        );
    }

    #[test]
    fn remap_matches_subdomain() {
        let mut policy = allowlist_policy(&[]);
        policy
            .remap
            .insert("example.com".into(), "10.0.0.1".into());
        assert_eq!(
            policy.check("api.example.com"),
            PolicyDecision::Remap("10.0.0.1".into())
        );
    }

    // -- Case insensitivity --

    #[test]
    fn case_insensitive_matching() {
        let policy = allowlist_policy(&["Anthropic.COM"]);
        assert_eq!(policy.check("ANTHROPIC.com"), PolicyDecision::Allow);
        assert_eq!(policy.check("api.ANTHROPIC.COM"), PolicyDecision::Allow);
    }

    // -- Trailing dot normalization --

    #[test]
    fn trailing_dot_normalized() {
        let policy = allowlist_policy(&["anthropic.com."]);
        assert_eq!(policy.check("anthropic.com"), PolicyDecision::Allow);
        assert_eq!(policy.check("anthropic.com."), PolicyDecision::Allow);
    }

    // -- Deep subdomain --

    #[test]
    fn deep_subdomain_match() {
        let policy = allowlist_policy(&["anthropic.com"]);
        assert_eq!(
            policy.check("a.b.c.anthropic.com"),
            PolicyDecision::Allow
        );
    }

    // -- Wildcard prefix syntax --

    #[test]
    fn wildcard_prefix_matches_subdomain() {
        let policy = allowlist_policy(&["*.claude.com"]);
        assert_eq!(policy.check("platform.claude.com"), PolicyDecision::Allow);
    }

    #[test]
    fn wildcard_prefix_matches_bare_domain() {
        let policy = allowlist_policy(&["*.claude.com"]);
        assert_eq!(policy.check("claude.com"), PolicyDecision::Allow);
    }

    #[test]
    fn wildcard_prefix_matches_deep_subdomain() {
        let policy = allowlist_policy(&["*.anthropic.com"]);
        assert_eq!(
            policy.check("a.b.anthropic.com"),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn wildcard_prefix_no_partial_match() {
        let policy = allowlist_policy(&["*.claude.com"]);
        assert_eq!(policy.check("notclaude.com"), PolicyDecision::Deny);
    }

    #[test]
    fn denylist_wildcard_denies_subdomain() {
        let policy = denylist_policy(&["*.internal"]);
        assert_eq!(policy.check("foo.internal"), PolicyDecision::Deny);
        assert_eq!(policy.check("bar.baz.internal"), PolicyDecision::Deny);
    }
}
