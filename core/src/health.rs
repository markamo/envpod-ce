// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Pod health checks — basic single-check liveness probe with auto-recovery.
//!
//! CE supports one health check per pod with a simple action (restart/freeze/alert).
//! For multiple checks, recovery sequences, live mutation, and notifications,
//! upgrade to envpod Premium.
//!
//! Configurable in pod.yaml:
//!   health:
//!     endpoint: /health         # HTTP GET — 200 = healthy
//!     interval: 30              # seconds between checks
//!     timeout: 5                # seconds before check times out
//!     retries: 3                # consecutive failures before action
//!     action: restart           # restart | alert | freeze
//!     grace_period: 10          # seconds for graceful shutdown

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What to do when a pod fails its health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthAction {
    Restart,
    Alert,
    Freeze,
}

impl Default for HealthAction {
    fn default() -> Self {
        Self::Restart
    }
}

fn default_interval() -> u64 { 30 }
fn default_timeout() -> u64 { 5 }
fn default_retries() -> u32 { 3 }
fn default_grace() -> u64 { 10 }

/// Health check configuration — CE: single check only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthConfig {
    /// HTTP GET endpoint path (e.g., "/health").
    pub endpoint: Option<String>,
    /// Port for HTTP check.
    pub port: Option<u16>,
    /// Shell command to run inside pod. Exit 0 = healthy.
    pub command: Option<String>,
    /// Seconds between checks. Default: 30.
    #[serde(default = "default_interval")]
    pub interval: u64,
    /// Seconds before check times out. Default: 5.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Consecutive failures before action triggers. Default: 3.
    #[serde(default = "default_retries")]
    pub retries: u32,
    /// What to do when retries exhausted. Default: restart.
    #[serde(default)]
    pub action: HealthAction,
    /// Graceful shutdown timeout in seconds before SIGKILL. Default: 10.
    #[serde(default = "default_grace")]
    pub grace_period: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            port: None,
            command: None,
            interval: default_interval(),
            timeout: default_timeout(),
            retries: default_retries(),
            action: HealthAction::default(),
            grace_period: default_grace(),
        }
    }
}

impl HealthConfig {
    /// Returns true if a health check is configured.
    pub fn is_enabled(&self) -> bool {
        self.endpoint.is_some() || self.command.is_some()
    }
}

/// Health check status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy(u32),
    Unknown,
}

/// Simple health checker — tracks consecutive failures for a single check.
pub struct HealthChecker {
    pub config: HealthConfig,
    pub pod_name: String,
    pub pod_ip: Option<String>,
    pub consecutive_failures: u32,
    pub last_check: Option<DateTime<Utc>>,
    pub status: HealthStatus,
}

impl HealthChecker {
    pub fn new(config: HealthConfig, pod_name: String, pod_ip: Option<String>) -> Self {
        Self {
            config,
            pod_name,
            pod_ip,
            consecutive_failures: 0,
            last_check: None,
            status: HealthStatus::Unknown,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_enabled()
    }

    /// Check health and update status. Returns current status.
    pub fn check(&mut self) -> HealthStatus {
        let healthy = if let Some(ref endpoint) = self.config.endpoint {
            let port = self.config.port.unwrap_or(80);
            let ip = self.pod_ip.as_deref().unwrap_or("127.0.0.1");
            let url = format!("http://{}:{}{}", ip, port, endpoint);
            check_http(&url, self.config.timeout)
        } else if let Some(ref command) = self.config.command {
            check_command(&self.pod_name, command, self.config.timeout)
        } else {
            true
        };

        self.last_check = Some(Utc::now());

        if healthy {
            self.consecutive_failures = 0;
            self.status = HealthStatus::Healthy;
        } else {
            self.consecutive_failures += 1;
            self.status = HealthStatus::Unhealthy(self.consecutive_failures);
        }

        self.status.clone()
    }

    /// Returns the action to take if retries are exhausted.
    pub fn should_act(&self) -> Option<HealthAction> {
        if self.consecutive_failures >= self.config.retries {
            Some(self.config.action)
        } else {
            None
        }
    }

    /// Reset after recovery action taken.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.status = HealthStatus::Unknown;
    }
}

/// HTTP health check using curl.
pub fn check_http(url: &str, timeout_secs: u64) -> bool {
    std::process::Command::new("curl")
        .args(["-sf", "--max-time", &timeout_secs.to_string(), url])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Command health check inside pod.
pub fn check_command(pod_name: &str, command: &str, timeout_secs: u64) -> bool {
    std::process::Command::new("timeout")
        .args([&timeout_secs.to_string(), "envpod", "run", pod_name, "--root", "--", "sh", "-c", command])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_disabled() {
        let config = HealthConfig::default();
        assert!(!config.is_enabled());
        assert_eq!(config.interval, 30);
        assert_eq!(config.timeout, 5);
        assert_eq!(config.retries, 3);
        assert_eq!(config.grace_period, 10);
        assert_eq!(config.action, HealthAction::Restart);
    }

    #[test]
    fn config_with_endpoint_is_enabled() {
        let config = HealthConfig {
            endpoint: Some("/health".into()),
            ..Default::default()
        };
        assert!(config.is_enabled());
    }

    #[test]
    fn config_with_command_is_enabled() {
        let config = HealthConfig {
            command: Some("pgrep python3".into()),
            ..Default::default()
        };
        assert!(config.is_enabled());
    }

    #[test]
    fn checker_tracks_failures() {
        let config = HealthConfig {
            command: Some("false".into()),
            retries: 3,
            ..Default::default()
        };
        let mut checker = HealthChecker::new(config, "test".into(), None);
        assert_eq!(checker.consecutive_failures, 0);
        assert!(checker.should_act().is_none());
    }

    #[test]
    fn should_act_after_retries() {
        let config = HealthConfig {
            retries: 3,
            ..Default::default()
        };
        let mut checker = HealthChecker::new(config, "test".into(), None);
        checker.consecutive_failures = 3;
        assert_eq!(checker.should_act(), Some(HealthAction::Restart));
    }

    #[test]
    fn reset_clears_failures() {
        let config = HealthConfig::default();
        let mut checker = HealthChecker::new(config, "test".into(), None);
        checker.consecutive_failures = 5;
        checker.reset();
        assert_eq!(checker.consecutive_failures, 0);
        assert_eq!(checker.status, HealthStatus::Unknown);
    }

    #[test]
    fn config_deserialize() {
        let yaml = r#"
endpoint: /health
port: 9500
interval: 15
timeout: 3
retries: 5
action: freeze
grace_period: 20
"#;
        let config: HealthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoint, Some("/health".into()));
        assert_eq!(config.port, Some(9500));
        assert_eq!(config.interval, 15);
        assert_eq!(config.timeout, 3);
        assert_eq!(config.retries, 5);
        assert_eq!(config.action, HealthAction::Freeze);
        assert_eq!(config.grace_period, 20);
    }
}
