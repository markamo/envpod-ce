// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! File-based credential vault for pods with encryption at rest.
//!
//! Secrets are stored as individual files under `{pod_dir}/vault/`,
//! one file per key (mode 0600), encrypted with ChaCha20-Poly1305 (AEAD).
//! A per-pod 256-bit key is stored at `{pod_dir}/vault.key` (mode 0600).
//! Secrets are injected as environment variables when the pod process
//! starts — they never appear in the agent's context window or
//! configuration files.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// File-based credential vault scoped to a single pod.
pub struct Vault {
    dir: PathBuf,
    key: [u8; KEY_LEN],
}

impl Vault {
    /// Open (or create) the vault for a pod directory.
    ///
    /// Loads (or generates) the encryption key and auto-migrates any
    /// plaintext vault files to encrypted format.
    pub fn new(pod_dir: &Path) -> Result<Self> {
        let dir = pod_dir.join("vault");
        fs::create_dir_all(&dir)
            .with_context(|| format!("create vault dir: {}", dir.display()))?;
        // Restrict vault directory to owner only
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("set vault dir permissions: {}", dir.display()))?;

        let key = load_or_create_key(pod_dir)?;

        let vault = Self { dir, key };
        vault.migrate_plaintext_to_encrypted()?;
        Ok(vault)
    }

    /// Store a secret. Overwrites if key already exists.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        validate_key(key)?;
        let path = self.dir.join(key);
        let encrypted = encrypt(&self.key, value.as_bytes())?;
        fs::write(&path, encrypted)
            .with_context(|| format!("write vault key: {key}"))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("set vault key permissions: {key}"))?;
        Ok(())
    }

    /// Retrieve a secret. Returns None if the key doesn't exist.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        validate_key(key)?;
        let path = self.dir.join(key);
        match fs::read(&path) {
            Ok(data) => {
                let plaintext = decrypt(&self.key, &data)
                    .with_context(|| format!("decrypt vault key: {key}"))?;
                let value = String::from_utf8(plaintext)
                    .with_context(|| format!("vault key {key} is not valid UTF-8"))?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context(format!("read vault key: {key}"))),
        }
    }

    /// List all keys in the vault.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        if !self.dir.exists() {
            return Ok(keys);
        }
        for entry in fs::read_dir(&self.dir).context("read vault dir")? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    keys.push(name.to_string());
                }
            }
        }
        keys.sort();
        Ok(keys)
    }

    /// Remove a secret. No error if key doesn't exist.
    pub fn remove(&self, key: &str) -> Result<()> {
        validate_key(key)?;
        let path = self.dir.join(key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("remove vault key: {key}"))),
        }
    }

    /// Write all secrets to `{pod_dir}/vault_env` as a plaintext KEY=value file.
    ///
    /// This file is bind-mounted read-only into the pod at `/run/envpod/secrets.env`
    /// at pod start, giving the agent live read access without a restart.
    /// File is 0644 (world-readable within the pod boundary; agent already has
    /// the same secrets via env var injection).
    pub fn refresh_env_file(&self, pod_dir: &Path) -> Result<()> {
        let secrets = self.load_all()?;
        write_env_file(pod_dir, &secrets)
    }

    /// Load all secrets as a key→value map (for env injection).
    pub fn load_all(&self) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        if !self.dir.exists() {
            return Ok(map);
        }
        for entry in fs::read_dir(&self.dir).context("read vault dir")? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    let data = fs::read(entry.path())?;
                    let plaintext = decrypt(&self.key, &data)
                        .with_context(|| format!("decrypt vault key: {name}"))?;
                    let value = String::from_utf8(plaintext)
                        .with_context(|| format!("vault key {name} is not valid UTF-8"))?;
                    map.insert(name.to_string(), value);
                }
            }
        }
        Ok(map)
    }

    /// Migrate any plaintext vault files to encrypted format.
    ///
    /// Detection heuristic: encrypted files are at least NONCE_LEN + 16 bytes
    /// (12-byte nonce + 16-byte Poly1305 tag, even for empty plaintext).
    /// Files shorter than that, or files that fail decryption, are treated
    /// as plaintext and re-encrypted in place.
    fn migrate_plaintext_to_encrypted(&self) -> Result<()> {
        if !self.dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.dir).context("read vault dir for migration")? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let data = fs::read(&path)?;

            // If the file is too short to be encrypted, it must be plaintext
            let is_plaintext = if data.len() < NONCE_LEN + 16 {
                true
            } else {
                // Try to decrypt — if it fails, assume plaintext
                decrypt(&self.key, &data).is_err()
            };

            if is_plaintext {
                let encrypted = encrypt(&self.key, &data)?;
                fs::write(&path, encrypted)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok(())
    }
}

/// Write a `KEY=value` plaintext env file to `{pod_dir}/vault_env`.
///
/// Called after every vault mutation so the in-pod bind-mounted file stays
/// in sync without requiring a pod restart.
pub fn write_env_file(pod_dir: &Path, secrets: &HashMap<String, String>) -> Result<()> {
    let path = pod_dir.join("vault_env");
    let mut content = String::new();
    let mut keys: Vec<&String> = secrets.keys().collect();
    keys.sort();
    for key in keys {
        let value = &secrets[key];
        // Quote values that contain whitespace or special shell chars
        let needs_quote = value.chars().any(|c| matches!(c, ' ' | '\t' | '"' | '\'' | '\\' | '$' | '`' | '!'));
        if needs_quote {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            content.push_str(&format!("export {key}=\"{escaped}\"\n"));
        } else {
            content.push_str(&format!("export {key}={value}\n"));
        }
    }
    fs::write(&path, &content)
        .with_context(|| format!("write vault_env: {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
        .with_context(|| format!("chmod vault_env: {}", path.display()))?;
    Ok(())
}

/// Parse a `.env` file and return KEY→value pairs.
///
/// Handles:
/// - Blank lines and `# comments` → skipped
/// - `KEY=value` → literal value
/// - `KEY="value"` or `KEY='value'` → strips surrounding quotes
/// - `KEY=` → empty string
pub fn parse_env_file(content: &str) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip optional "export " prefix
        let line = line.strip_prefix("export ").unwrap_or(line);
        let eq = line.find('=').ok_or_else(|| {
            anyhow::anyhow!("line {}: missing '=' in env file: {line}", lineno + 1)
        })?;
        let key = line[..eq].trim();
        let raw_val = line[eq + 1..].trim();
        validate_key(key)?;
        let value = strip_quotes(raw_val);
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

/// Strip matching surrounding single or double quotes from a value.
fn strip_quotes(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"')) ||
       (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Validate that a vault key contains only safe characters (alphanumeric + underscore).
fn validate_key(key: &str) -> Result<()> {
    anyhow::ensure!(!key.is_empty(), "vault key must not be empty");
    anyhow::ensure!(
        key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "vault key must contain only alphanumeric characters and underscores, got: '{key}'"
    );
    Ok(())
}

/// Load the per-pod encryption key, or generate one if it doesn't exist.
fn load_or_create_key(pod_dir: &Path) -> Result<[u8; KEY_LEN]> {
    let key_path = pod_dir.join("vault.key");

    if key_path.exists() {
        let data = fs::read(&key_path).context("read vault.key")?;
        anyhow::ensure!(
            data.len() == KEY_LEN,
            "vault.key has wrong size: expected {KEY_LEN}, got {}",
            data.len()
        );
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&data);
        Ok(key)
    } else {
        let key = generate_key();
        fs::write(&key_path, key).context("write vault.key")?;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))
            .context("set vault.key permissions")?;
        Ok(key)
    }
}

/// Generate a random 256-bit key from the OS CSPRNG.
fn generate_key() -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);
    key
}

/// Encrypt plaintext using ChaCha20-Poly1305 with a random nonce.
/// Returns: [12-byte nonce][ciphertext + 16-byte tag]
fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt ciphertext produced by `encrypt()`.
/// Input: [12-byte nonce][ciphertext + 16-byte tag]
fn decrypt(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        data.len() >= NONCE_LEN + 16,
        "encrypted data too short ({} bytes, minimum {})",
        data.len(),
        NONCE_LEN + 16
    );
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::from_slice(&data[..NONCE_LEN]);
    let ciphertext = &data[NONCE_LEN..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed (wrong key or corrupted data): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("API_KEY", "sk-secret123").unwrap();
        assert_eq!(vault.get("API_KEY").unwrap(), Some("sk-secret123".into()));
    }

    #[test]
    fn get_missing_key_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        assert_eq!(vault.get("NONEXISTENT").unwrap(), None);
    }

    #[test]
    fn list_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("B_KEY", "b").unwrap();
        vault.set("A_KEY", "a").unwrap();

        let keys = vault.list().unwrap();
        assert_eq!(keys, vec!["A_KEY", "B_KEY"]);
    }

    #[test]
    fn remove_key() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("TOKEN", "xyz").unwrap();
        vault.remove("TOKEN").unwrap();
        assert_eq!(vault.get("TOKEN").unwrap(), None);
    }

    #[test]
    fn remove_missing_key_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.remove("NOPE").unwrap();
    }

    #[test]
    fn load_all_returns_map() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("KEY_A", "val_a").unwrap();
        vault.set("KEY_B", "val_b").unwrap();

        let map = vault.load_all().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["KEY_A"], "val_a");
        assert_eq!(map["KEY_B"], "val_b");
    }

    #[test]
    fn invalid_key_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        assert!(vault.set("", "val").is_err());
        assert!(vault.set("key with spaces", "val").is_err());
        assert!(vault.set("key/slash", "val").is_err());
        assert!(vault.set("key.dot", "val").is_err());
        assert!(vault.set("../escape", "val").is_err());
    }

    #[test]
    fn overwrite_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("KEY", "old").unwrap();
        vault.set("KEY", "new").unwrap();
        assert_eq!(vault.get("KEY").unwrap(), Some("new".into()));
    }

    #[test]
    fn file_permissions_are_restricted() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        vault.set("SECRET", "data").unwrap();

        let path = tmp.path().join("vault/SECRET");
        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn file_contents_are_not_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();

        let secret = "super-secret-api-key-12345";
        vault.set("API_KEY", secret).unwrap();

        // Read raw file contents — should NOT contain the plaintext
        let raw = fs::read(tmp.path().join("vault/API_KEY")).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains(secret),
            "vault file should be encrypted, not plaintext"
        );
        // Encrypted file must be: nonce (12) + ciphertext (len) + tag (16)
        assert!(raw.len() >= NONCE_LEN + 16);
    }

    #[test]
    fn key_file_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let _vault = Vault::new(tmp.path()).unwrap();

        let key_path = tmp.path().join("vault.key");
        assert!(key_path.exists());
        let perms = fs::metadata(&key_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        // Key should be exactly 32 bytes
        let key_data = fs::read(&key_path).unwrap();
        assert_eq!(key_data.len(), KEY_LEN);
    }

    #[test]
    fn migration_from_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let vault_dir = tmp.path().join("vault");
        fs::create_dir_all(&vault_dir).unwrap();

        // Write plaintext files directly (simulating old vault)
        fs::write(vault_dir.join("OLD_KEY"), "plaintext-secret").unwrap();
        fs::write(vault_dir.join("ANOTHER"), "another-value").unwrap();

        // Creating a Vault should auto-migrate
        let vault = Vault::new(tmp.path()).unwrap();

        // Should be readable through the vault API
        assert_eq!(
            vault.get("OLD_KEY").unwrap(),
            Some("plaintext-secret".into())
        );
        assert_eq!(
            vault.get("ANOTHER").unwrap(),
            Some("another-value".into())
        );

        // Raw file should no longer be plaintext
        let raw = fs::read(vault_dir.join("OLD_KEY")).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains("plaintext-secret"),
            "migrated file should be encrypted"
        );
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();
        vault.set("SECRET", "value").unwrap();

        // Read the encrypted data
        let data = fs::read(tmp.path().join("vault/SECRET")).unwrap();

        // Try to decrypt with a different key
        let wrong_key = generate_key();
        assert!(
            decrypt(&wrong_key, &data).is_err(),
            "decryption with wrong key should fail"
        );
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = generate_key();
        let plaintext = b"hello, vault!";
        let encrypted = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn write_env_file_creates_plaintext_file() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();
        vault.set("API_KEY", "sk-test-123").unwrap();
        vault.set("DB_URL", "postgres://localhost/db").unwrap();
        vault.refresh_env_file(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join("vault_env")).unwrap();
        assert!(content.contains("export API_KEY=sk-test-123\n"), "got: {content}");
        assert!(content.contains("export DB_URL=postgres://localhost/db\n"), "got: {content}");
    }

    #[test]
    fn write_env_file_quotes_values_with_spaces() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();
        vault.set("MSG", "hello world").unwrap();
        vault.refresh_env_file(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join("vault_env")).unwrap();
        assert!(content.contains("export MSG=\"hello world\"\n"), "got: {content}");
    }

    #[test]
    fn write_env_file_empty_vault_creates_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path()).unwrap();
        vault.refresh_env_file(tmp.path()).unwrap();

        let content = std::fs::read_to_string(tmp.path().join("vault_env")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn parse_env_file_basic() {
        let content = "API_KEY=sk-123\nDB_URL=postgres://localhost\n";
        let map = parse_env_file(content).unwrap();
        assert_eq!(map["API_KEY"], "sk-123");
        assert_eq!(map["DB_URL"], "postgres://localhost");
    }

    #[test]
    fn parse_env_file_strips_quotes() {
        let content = "KEY1=\"quoted value\"\nKEY2='single'\n";
        let map = parse_env_file(content).unwrap();
        assert_eq!(map["KEY1"], "quoted value");
        assert_eq!(map["KEY2"], "single");
    }

    #[test]
    fn parse_env_file_skips_comments_and_blanks() {
        let content = "# comment\n\nKEY=val\n\n# another\n";
        let map = parse_env_file(content).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["KEY"], "val");
    }

    #[test]
    fn parse_env_file_empty_value() {
        let content = "EMPTY=\n";
        let map = parse_env_file(content).unwrap();
        assert_eq!(map["EMPTY"], "");
    }

    #[test]
    fn parse_env_file_invalid_key_rejected() {
        let content = "key with spaces=val\n";
        assert!(parse_env_file(content).is_err());
    }

    #[test]
    fn parse_env_file_missing_equals_rejected() {
        let content = "KEYNOEQUALS\n";
        assert!(parse_env_file(content).is_err());
    }

    #[test]
    fn encrypt_empty_value() {
        let key = generate_key();
        let encrypted = encrypt(&key, b"").unwrap();
        // nonce (12) + tag (16) = 28 bytes minimum
        assert_eq!(encrypted.len(), NONCE_LEN + 16);
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }
}
