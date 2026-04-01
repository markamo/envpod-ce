// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Per-pod authentication and identity (CE edition).
//!
//! Each pod gets an Ed25519 keypair + auth token on init.
//! Used for remote control authentication (Premium) and pod identity.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct PodIdentity {
    pub public_key: String,
    pub auth_token: String,
    pub identity_dir: PathBuf,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|e| anyhow::anyhow!("hex decode: {e}")))
        .collect()
}

pub fn generate_identity(pod_dir: &Path) -> Result<PodIdentity> {
    let identity_dir = pod_dir.join("identity");
    std::fs::create_dir_all(&identity_dir).context("create identity directory")?;

    let mut rng = rand::rngs::OsRng;
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rng);
    let verifying_key = signing_key.verifying_key();

    let private_hex = to_hex(&signing_key.to_bytes());
    let public_hex = to_hex(verifying_key.as_bytes());
    let auth_token = derive_token(&private_hex, &public_hex);

    let key_path = identity_dir.join("pod.key");
    let pub_path = identity_dir.join("pod.pub");
    let token_path = identity_dir.join("auth.token");

    std::fs::write(&key_path, &private_hex).context("write pod.key")?;
    std::fs::write(&pub_path, &public_hex).context("write pod.pub")?;
    std::fs::write(&token_path, &auth_token).context("write auth.token")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644))?;
    }

    Ok(PodIdentity { public_key: public_hex, auth_token, identity_dir })
}

pub fn load_identity(pod_dir: &Path) -> Result<PodIdentity> {
    let identity_dir = pod_dir.join("identity");
    let public_key = std::fs::read_to_string(identity_dir.join("pod.pub"))
        .context("read pod.pub")?.trim().to_string();
    let auth_token = std::fs::read_to_string(identity_dir.join("auth.token"))
        .context("read auth.token")?.trim().to_string();
    Ok(PodIdentity { public_key, auth_token, identity_dir })
}

pub fn verify_token(pod_dir: &Path, token: &str) -> bool {
    let token_path = pod_dir.join("identity/auth.token");
    match std::fs::read_to_string(&token_path) {
        Ok(stored) => stored.trim() == token.trim(),
        Err(_) => false,
    }
}

pub fn fingerprint(public_key: &str) -> String {
    if public_key.len() >= 16 {
        format!("{}...{}", &public_key[..8], &public_key[public_key.len()-8..])
    } else {
        public_key.to_string()
    }
}

fn derive_token(private_hex: &str, public_hex: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h1 = DefaultHasher::new();
    private_hex.hash(&mut h1);
    "envpod-pod-auth-v1".hash(&mut h1);
    let v1 = h1.finish();
    let mut h2 = DefaultHasher::new();
    public_hex.hash(&mut h2);
    v1.hash(&mut h2);
    let v2 = h2.finish();
    format!("{:016x}{:016x}", v1, v2)
}
