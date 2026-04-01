// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Budget enforcement (CE edition).
//!
//! CE supports time budgets with graceful shutdown and warnings.
//! Premium adds: multi-dimension (requests, bandwidth, storage),
//! freeze/notify actions, extend/reset CLI, and renewable budgets.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audit::{AuditAction, AuditEntry, AuditLog};
use crate::config::BudgetConfig;

/// Budget enforcer for CE — time-based with graceful shutdown.
pub struct BudgetEnforcer {
    max_duration_secs: u64,
    warning_secs: Option<u64>,
    grace_period: Duration,
    pod_name: String,
    pod_dir: PathBuf,
    pid: u32,
    started_at: Instant,
}

impl BudgetEnforcer {
    /// Create from BudgetConfig. Returns None if no budget configured.
    pub fn from_config(
        config: &BudgetConfig,
        pod_name: &str,
        pod_dir: PathBuf,
        pid: u32,
    ) -> Option<Self> {
        let max_secs = config.max_duration.as_ref()
            .and_then(|d| crate::config::parse_duration_string(d))?;

        let warning_secs = config.warning.as_ref()
            .and_then(|w| crate::config::parse_duration_string(w));

        let grace_secs = crate::config::parse_duration_string(&config.grace_period)
            .unwrap_or(30);

        Some(Self {
            max_duration_secs: max_secs,
            warning_secs,
            grace_period: Duration::from_secs(grace_secs),
            pod_name: pod_name.to_string(),
            pod_dir,
            pid,
            started_at: Instant::now(),
        })
    }

    /// Run the enforcement loop. Call this in a tokio::spawn.
    pub async fn run(self: Arc<Self>) {
        let check_interval = Duration::from_secs(10);
        let mut warned = false;

        loop {
            tokio::time::sleep(check_interval).await;
            let elapsed = self.started_at.elapsed().as_secs();

            // Warning check
            if !warned {
                let should_warn = if let Some(warn_before) = self.warning_secs {
                    elapsed >= self.max_duration_secs.saturating_sub(warn_before)
                } else {
                    // Default: warn at 90%
                    elapsed >= (self.max_duration_secs as f64 * 0.9) as u64
                };

                if should_warn && elapsed < self.max_duration_secs {
                    warned = true;
                    let remaining = self.max_duration_secs - elapsed;
                    eprintln!(
                        "  \x1b[33m!\x1b[0m Budget warning: {} remaining (max_duration={})",
                        format_duration(remaining),
                        format_duration(self.max_duration_secs),
                    );
                    self.audit(AuditAction::BudgetWarning,
                        &format!("{} remaining", format_duration(remaining)));
                }
            }

            // Exceeded check
            if elapsed >= self.max_duration_secs {
                eprintln!(
                    "  \x1b[31m!\x1b[0m Budget exceeded: max_duration={} — stopping pod",
                    format_duration(self.max_duration_secs),
                );
                self.audit(AuditAction::BudgetExceeded,
                    &format!("max_duration={} ({}s)", format_duration(self.max_duration_secs), self.max_duration_secs));

                self.graceful_stop().await;
                break;
            }
        }
    }

    /// SIGTERM → grace period → SIGKILL
    async fn graceful_stop(&self) {
        let pid = nix::unistd::Pid::from_raw(self.pid as i32);

        eprintln!("  Sending SIGTERM (grace period: {}s)...", self.grace_period.as_secs());
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).ok();

        let start = Instant::now();
        while start.elapsed() < self.grace_period {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if nix::sys::signal::kill(pid, None).is_err() {
                eprintln!("  Process exited gracefully");
                return;
            }
        }

        eprintln!("  Grace period expired — sending SIGKILL");
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL).ok();
    }

    fn audit(&self, action: AuditAction, detail: &str) {
        let log = AuditLog::new(&self.pod_dir);
        let entry = AuditEntry {
            timestamp: chrono::Utc::now(),
            pod_name: self.pod_name.clone(),
            action,
            detail: detail.to_string(),
            success: true,
        };
        log.append(&entry).ok();
    }
}

/// Format seconds as human-readable duration.
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 { format!("{h}h{m}m") } else { format!("{h}h") }
    } else {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        if h > 0 { format!("{d}d{h}h") } else { format!("{d}d") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(90), "1m30s");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(7260), "2h1m");
        assert_eq!(format_duration(86400), "1d");
        assert_eq!(format_duration(90000), "1d1h");
    }

    #[test]
    fn test_no_config() {
        let config = BudgetConfig::default();
        let enforcer = BudgetEnforcer::from_config(&config, "test", PathBuf::from("/tmp"), 1);
        assert!(enforcer.is_none());
    }
}
