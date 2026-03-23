// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Pod and agent identity — Ed25519 keypairs, agent registry, JWT tokens.
//!
//! Every pod gets an Ed25519 keypair at init time. Agents are registered
//! (via pod.yaml or at runtime) and receive JWT tokens signed by the pod key.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AgentConfig;

// ── Pod Identity ────────────────────────────────────────────────────────────

/// Serialized pod identity metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodIdentityMeta {
    pub pod_id: Uuid,
    pub pod_name: String,
    pub public_key_hex: String,
    pub created_at: DateTime<Utc>,
}

/// Generate and persist an Ed25519 keypair for a new pod.
///
/// Creates:
/// - `{pod_dir}/identity/pod.key`  (private, mode 0600)
/// - `{pod_dir}/identity/pod.pub`  (public, mode 0644)
/// - `{pod_dir}/identity/pod.json` (metadata)
pub fn generate_pod_identity(
    pod_dir: &Path,
    pod_id: Uuid,
    pod_name: &str,
) -> Result<PodIdentityMeta> {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let identity_dir = pod_dir.join("identity");
    std::fs::create_dir_all(&identity_dir)
        .context("create identity directory")?;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Write private key (mode 0600)
    let key_path = identity_dir.join("pod.key");
    std::fs::write(&key_path, signing_key.to_bytes())
        .context("write pod private key")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Write public key
    let pub_path = identity_dir.join("pod.pub");
    std::fs::write(&pub_path, verifying_key.to_bytes())
        .context("write pod public key")?;

    let meta = PodIdentityMeta {
        pod_id,
        pod_name: pod_name.to_string(),
        public_key_hex: hex_encode(&verifying_key.to_bytes()),
        created_at: Utc::now(),
    };

    let json = serde_json::to_string_pretty(&meta)?;
    std::fs::write(identity_dir.join("pod.json"), json)
        .context("write pod identity metadata")?;

    Ok(meta)
}

/// Load the pod identity metadata. Returns None if no identity exists.
pub fn load_pod_identity(pod_dir: &Path) -> Result<Option<PodIdentityMeta>> {
    let path = pod_dir.join("identity/pod.json");
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .context("read pod identity metadata")?;
    let meta: PodIdentityMeta = serde_json::from_str(&data)
        .context("parse pod identity metadata")?;
    Ok(Some(meta))
}

/// Load the pod's Ed25519 signing key from disk.
fn load_signing_key(pod_dir: &Path) -> Result<ed25519_dalek::SigningKey> {
    let key_path = pod_dir.join("identity/pod.key");
    let bytes = std::fs::read(&key_path)
        .context("read pod private key")?;
    let key_bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid pod key length"))?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&key_bytes))
}

// ── Agent Registry ──────────────────────────────────────────────────────────

/// A registered agent within a pod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    pub agent_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub vault_keys: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Container for all agents in a pod.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentRegistry {
    pub agents: Vec<AgentRegistration>,
}

fn registry_path(pod_dir: &Path) -> std::path::PathBuf {
    pod_dir.join("identity/agents.json")
}

fn load_registry(pod_dir: &Path) -> Result<AgentRegistry> {
    let path = registry_path(pod_dir);
    if !path.exists() {
        return Ok(AgentRegistry::default());
    }
    let data = std::fs::read_to_string(&path)
        .context("read agent registry")?;
    serde_json::from_str(&data).context("parse agent registry")
}

fn save_registry(pod_dir: &Path, registry: &AgentRegistry) -> Result<()> {
    let identity_dir = pod_dir.join("identity");
    std::fs::create_dir_all(&identity_dir)?;
    let json = serde_json::to_string_pretty(registry)?;
    std::fs::write(registry_path(pod_dir), json)
        .context("write agent registry")
}

/// Register a new agent. Returns error if name already taken.
pub fn register_agent(
    pod_dir: &Path,
    name: &str,
    capabilities: Vec<String>,
    vault_keys: Vec<String>,
) -> Result<AgentRegistration> {
    let mut registry = load_registry(pod_dir)?;

    if registry.agents.iter().any(|a| a.name == name) {
        anyhow::bail!("agent '{name}' already registered");
    }

    let agent = AgentRegistration {
        agent_id: Uuid::new_v4(),
        name: name.to_string(),
        capabilities,
        vault_keys,
        created_at: Utc::now(),
    };

    registry.agents.push(agent.clone());
    save_registry(pod_dir, &registry)?;
    Ok(agent)
}

/// Remove an agent by name. Returns the removed agent if found.
pub fn remove_agent(pod_dir: &Path, name: &str) -> Result<Option<AgentRegistration>> {
    let mut registry = load_registry(pod_dir)?;
    let pos = registry.agents.iter().position(|a| a.name == name);
    match pos {
        Some(i) => {
            let removed = registry.agents.remove(i);
            save_registry(pod_dir, &registry)?;
            Ok(Some(removed))
        }
        None => Ok(None),
    }
}

/// List all registered agents.
pub fn list_agents(pod_dir: &Path) -> Result<Vec<AgentRegistration>> {
    Ok(load_registry(pod_dir)?.agents)
}

/// Look up an agent by name.
pub fn get_agent(pod_dir: &Path, name: &str) -> Result<Option<AgentRegistration>> {
    let registry = load_registry(pod_dir)?;
    Ok(registry.agents.into_iter().find(|a| a.name == name))
}

/// Sync agents declared in pod.yaml into the registry.
/// Adds agents from config that aren't already registered.
/// Does NOT remove runtime-registered agents missing from config.
pub fn sync_agents_from_config(pod_dir: &Path, config_agents: &[AgentConfig]) -> Result<()> {
    let mut registry = load_registry(pod_dir)?;

    for cfg in config_agents {
        if registry.agents.iter().any(|a| a.name == cfg.name) {
            continue;
        }
        registry.agents.push(AgentRegistration {
            agent_id: Uuid::new_v4(),
            name: cfg.name.clone(),
            capabilities: cfg.capabilities.clone(),
            vault_keys: cfg.vault_keys.clone(),
            created_at: Utc::now(),
        });
    }

    save_registry(pod_dir, &registry)
}

// ── JWT Tokens ──────────────────────────────────────────────────────────────

/// JWT claims for an agent token.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentClaims {
    pub iss: String,
    pub sub: String,
    pub pod_id: String,
    pub pod_name: String,
    pub agent_id: String,
    pub agent_name: String,
    pub capabilities: Vec<String>,
    pub vault_keys: Vec<String>,
    pub iat: i64,
    pub exp: i64,
}

/// Generate a JWT token for an agent, signed by the pod's Ed25519 key.
pub fn generate_agent_token(
    pod_dir: &Path,
    pod_id: Uuid,
    pod_name: &str,
    agent: &AgentRegistration,
    ttl_secs: u64,
) -> Result<String> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    let signing_key = load_signing_key(pod_dir)?;
    let now = Utc::now().timestamp();

    let claims = AgentClaims {
        iss: "envpod".to_string(),
        sub: format!("pod:{}/agent:{}", pod_name, agent.name),
        pod_id: pod_id.to_string(),
        pod_name: pod_name.to_string(),
        agent_id: agent.agent_id.to_string(),
        agent_name: agent.name.clone(),
        capabilities: agent.capabilities.clone(),
        vault_keys: agent.vault_keys.clone(),
        iat: now,
        exp: now + ttl_secs as i64,
    };

    let header = Header::new(Algorithm::EdDSA);
    let key = EncodingKey::from_ed_der(&signing_key_to_pkcs8(&signing_key));

    encode(&header, &claims, &key).context("sign agent JWT")
}

/// Verify a JWT token against the pod's public key. Returns decoded claims.
pub fn verify_agent_token(pod_dir: &Path, token: &str) -> Result<AgentClaims> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    let pub_path = pod_dir.join("identity/pod.pub");
    let pub_bytes = std::fs::read(&pub_path)
        .context("read pod public key")?;

    let key = DecodingKey::from_ed_der(&pub_bytes);
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_issuer(&["envpod"]);

    let token_data = decode::<AgentClaims>(token, &key, &validation)
        .context("verify agent JWT")?;

    Ok(token_data.claims)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Convert ed25519-dalek SigningKey to PKCS#8 DER for jsonwebtoken.
/// Ed25519 PKCS#8 format: fixed prefix + 32-byte private key + 32-byte public key.
fn signing_key_to_pkcs8(key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    // PKCS#8 v2 wrapper for Ed25519:
    // SEQUENCE {
    //   INTEGER 0 (version)
    //   SEQUENCE { OID 1.3.101.112 (Ed25519) }
    //   OCTET STRING { OCTET STRING { 32-byte private key } }
    //   [1] EXPLICIT BIT STRING { 32-byte public key }
    // }
    // Simplified: use the fixed prefix from RFC 8410
    let mut der = Vec::with_capacity(48);
    // Fixed PKCS#8 prefix for Ed25519 private key (v0, no public key embedded)
    der.extend_from_slice(&[
        0x30, 0x2e, // SEQUENCE, 46 bytes
        0x02, 0x01, 0x00, // INTEGER 0 (version)
        0x30, 0x05, // SEQUENCE, 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
        0x04, 0x22, // OCTET STRING, 34 bytes
        0x04, 0x20, // OCTET STRING, 32 bytes (the actual key)
    ]);
    der.extend_from_slice(&key.to_bytes());
    der
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_load_pod_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_id = Uuid::new_v4();
        let meta = generate_pod_identity(tmp.path(), pod_id, "test-pod").unwrap();

        assert_eq!(meta.pod_id, pod_id);
        assert_eq!(meta.pod_name, "test-pod");
        assert_eq!(meta.public_key_hex.len(), 64); // 32 bytes = 64 hex chars

        // Verify files exist
        assert!(tmp.path().join("identity/pod.key").exists());
        assert!(tmp.path().join("identity/pod.pub").exists());
        assert!(tmp.path().join("identity/pod.json").exists());

        // Load back
        let loaded = load_pod_identity(tmp.path()).unwrap().unwrap();
        assert_eq!(loaded.pod_id, pod_id);
        assert_eq!(loaded.public_key_hex, meta.public_key_hex);
    }

    #[test]
    fn load_nonexistent_identity_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_pod_identity(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn register_and_list_agents() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("identity")).unwrap();

        let a1 = register_agent(
            tmp.path(), "coder",
            vec!["read".into(), "write".into()],
            vec!["API_KEY".into()],
        ).unwrap();
        assert_eq!(a1.name, "coder");

        let a2 = register_agent(
            tmp.path(), "reviewer",
            vec!["read".into()],
            vec![],
        ).unwrap();
        assert_ne!(a1.agent_id, a2.agent_id);

        let agents = list_agents(tmp.path()).unwrap();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn duplicate_agent_name_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("identity")).unwrap();

        register_agent(tmp.path(), "coder", vec![], vec![]).unwrap();
        let err = register_agent(tmp.path(), "coder", vec![], vec![]);
        assert!(err.is_err());
    }

    #[test]
    fn remove_agent_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("identity")).unwrap();

        register_agent(tmp.path(), "coder", vec![], vec![]).unwrap();
        register_agent(tmp.path(), "reviewer", vec![], vec![]).unwrap();

        let removed = remove_agent(tmp.path(), "coder").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "coder");

        let agents = list_agents(tmp.path()).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "reviewer");

        // Removing again returns None
        assert!(remove_agent(tmp.path(), "coder").unwrap().is_none());
    }

    #[test]
    fn get_agent_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("identity")).unwrap();

        register_agent(tmp.path(), "coder", vec!["write".into()], vec![]).unwrap();

        let found = get_agent(tmp.path(), "coder").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().capabilities, vec!["write"]);

        assert!(get_agent(tmp.path(), "nope").unwrap().is_none());
    }

    #[test]
    fn sync_agents_from_config_adds_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("identity")).unwrap();

        // Pre-register one agent
        register_agent(tmp.path(), "coder", vec![], vec![]).unwrap();

        // Sync with config that has coder (exists) + reviewer (new)
        let config = vec![
            AgentConfig { name: "coder".into(), capabilities: vec![], vault_keys: vec![] },
            AgentConfig { name: "reviewer".into(), capabilities: vec!["read".into()], vault_keys: vec![] },
        ];
        sync_agents_from_config(tmp.path(), &config).unwrap();

        let agents = list_agents(tmp.path()).unwrap();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn generate_and_verify_jwt() {
        let tmp = tempfile::tempdir().unwrap();
        let pod_id = Uuid::new_v4();

        // Generate pod identity first
        generate_pod_identity(tmp.path(), pod_id, "test-pod").unwrap();

        let agent = AgentRegistration {
            agent_id: Uuid::new_v4(),
            name: "coder".into(),
            capabilities: vec!["read".into(), "write".into()],
            vault_keys: vec!["API_KEY".into()],
            created_at: Utc::now(),
        };

        let token = generate_agent_token(tmp.path(), pod_id, "test-pod", &agent, 3600).unwrap();
        assert!(!token.is_empty());

        // Verify
        let claims = verify_agent_token(tmp.path(), &token).unwrap();
        assert_eq!(claims.iss, "envpod");
        assert_eq!(claims.pod_name, "test-pod");
        assert_eq!(claims.agent_name, "coder");
        assert_eq!(claims.capabilities, vec!["read", "write"]);
        assert_eq!(claims.vault_keys, vec!["API_KEY"]);
        assert_eq!(claims.sub, "pod:test-pod/agent:coder");
    }
}
