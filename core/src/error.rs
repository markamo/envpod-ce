// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvpodError {
    #[error("pod not found: {0}")]
    PodNotFound(String),

    #[error("pod already exists: {0}")]
    PodAlreadyExists(String),

    #[error("pod is not running: {0}")]
    PodNotRunning(String),

    #[error("pod is already running: {0}")]
    PodAlreadyRunning(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("mount error: {path}: {reason}")]
    Mount { path: String, reason: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("resource limit exceeded: {resource}")]
    ResourceLimitExceeded { resource: String },

    #[error("namespace error: {0}")]
    Namespace(String),

    #[error("overlay error: {0}")]
    Overlay(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
