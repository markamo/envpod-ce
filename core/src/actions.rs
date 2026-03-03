// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Action catalog for pod governance.
//!
//! The catalog is the host-defined menu of what agents are *allowed to do*.
//! Agents query available actions and call them by name with typed parameters.
//! envpod executes approved actions — agents never make the calls directly.
//!
//! Security model:
//! - Catalog lives at `{pod_dir}/actions.json` — host-side only, agent cannot write it.
//! - Params are validated against the schema before queuing — no injection.
//! - `blocked` tier is absolute: queued with Blocked status, cannot be approved.
//! - `immediate` tier executes synchronously (COW-protected by OverlayFS).
//! - `staged` tier requires human approval before envpod executes.
//! - `delayed` tier auto-executes after a timeout unless cancelled.
//!
//! Action format in `actions.json`:
//! ```json
//! [
//!   {
//!     "name": "send_email",
//!     "description": "Send an email via SendGrid",
//!     "tier": "staged",
//!     "params": [
//!       {"name": "to",      "description": "Recipient address", "required": true},
//!       {"name": "subject", "description": "Email subject",     "required": true},
//!       {"name": "body",    "description": "Plain-text body",   "required": false}
//!     ]
//!   }
//! ]
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::action_types::{ActionScope, ActionType};
use crate::queue::ActionTier;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single parameter definition for an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    /// Parameter name (used as key in the call's `params` map).
    pub name: String,
    /// Human-readable description shown in dashboard and `list_actions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this parameter must be present in the call.
    #[serde(default)]
    pub required: bool,
}

/// A single action definition in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDef {
    /// Unique action name. Used by agents to call it.
    pub name: String,
    /// Human-readable description of what the action does.
    pub description: String,
    /// Reversibility tier: immediate, delayed, staged, blocked.
    /// When `action_type` is set, this overrides the built-in default.
    #[serde(default = "default_tier")]
    pub tier: ActionTier,
    /// Built-in action type. When set, params schema is auto-derived and
    /// envpod executes the action on approval. When absent, the action is
    /// "custom" — params are user-defined and no built-in executor runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_type: Option<ActionType>,
    /// Executor configuration (non-secret values).
    /// Secrets must be in the vault — reference them by vault key name here.
    /// Example: `{"auth_vault_key": "STRIPE_SECRET_KEY", "from": "noreply@co.com"}`
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, String>,
    /// Parameter schema. Auto-derived from `action_type` if not set manually.
    #[serde(default)]
    pub params: Vec<ParamDef>,
}

fn default_tier() -> ActionTier {
    ActionTier::Staged
}

impl Default for ActionDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            tier: ActionTier::Staged,
            action_type: None,
            config: HashMap::new(),
            params: Vec::new(),
        }
    }
}

impl ActionDef {
    /// Effective scope: from built-in action_type if set, else Custom (internal).
    pub fn scope(&self) -> ActionScope {
        match &self.action_type {
            Some(t) => crate::action_types::scope(t),
            None => ActionScope::Internal, // custom, assume internal
        }
    }

    /// Effective parameter schema: built-in if action_type is set, else manual params.
    pub fn effective_params(&self) -> Vec<ParamDef> {
        if let Some(t) = &self.action_type {
            let built_in = crate::action_types::schema(t);
            if !built_in.is_empty() {
                return built_in;
            }
        }
        self.params.clone()
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// Host-managed action catalog backed by `{pod_dir}/actions.json`.
/// Re-read from disk on each query — live hot-reload, no restart needed.
pub struct ActionCatalog {
    path: PathBuf,
}

impl ActionCatalog {
    pub fn new(pod_dir: &Path) -> Self {
        Self { path: pod_dir.join("actions.json") }
    }

    /// Load all defined actions. Returns empty catalog if file is absent.
    pub fn load(&self) -> Result<Vec<ActionDef>> {
        match std::fs::read_to_string(&self.path) {
            Ok(json) => serde_json::from_str(&json).context("parse actions.json"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(anyhow::Error::new(e).context("read actions.json")),
        }
    }

    /// Save actions (pretty JSON). Overwrites the file.
    pub fn save(&self, actions: &[ActionDef]) -> Result<()> {
        let json = serde_json::to_string_pretty(actions).context("serialize actions")?;
        std::fs::write(&self.path, json).context("write actions.json")?;
        Ok(())
    }

    /// Add or replace an action by name.
    pub fn upsert(&self, action: ActionDef) -> Result<()> {
        let mut actions = self.load()?;
        if let Some(pos) = actions.iter().position(|a| a.name == action.name) {
            actions[pos] = action;
        } else {
            actions.push(action);
        }
        self.save(&actions)
    }

    /// Remove an action by name. Returns true if it existed.
    pub fn remove(&self, name: &str) -> Result<bool> {
        let mut actions = self.load()?;
        let before = actions.len();
        actions.retain(|a| a.name != name);
        if actions.len() < before {
            self.save(&actions)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Look up a single action by exact name.
    pub fn get(&self, name: &str) -> Result<Option<ActionDef>> {
        Ok(self.load()?.into_iter().find(|a| a.name == name))
    }

    /// Validate a call: check action exists, required params present, no unknown keys.
    ///
    /// Uses `effective_params()` — built-in schema when `action_type` is set.
    /// Returns the `ActionDef` on success (caller uses its tier and action_type).
    pub fn validate_call(
        &self,
        action_name: &str,
        params: &HashMap<String, serde_json::Value>,
    ) -> Result<ActionDef> {
        let def = self
            .get(action_name)?
            .with_context(|| format!("action not found: '{action_name}'"))?;

        let effective = def.effective_params();

        // Check required params
        for p in &effective {
            if p.required && !params.contains_key(&p.name) {
                bail!(
                    "action '{}': missing required param '{}'",
                    action_name,
                    p.name
                );
            }
        }

        // Check no unknown params (only when schema is non-empty)
        if !effective.is_empty() {
            let known: std::collections::HashSet<&str> =
                effective.iter().map(|p| p.name.as_str()).collect();
            for key in params.keys() {
                if !known.contains(key.as_str()) {
                    bail!(
                        "action '{}': unknown param '{}' (expected: {})",
                        action_name,
                        key,
                        effective
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }

        Ok(def)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_catalog(dir: &Path) -> ActionCatalog {
        ActionCatalog::new(dir)
    }

    fn email_action() -> ActionDef {
        ActionDef {
            name: "send_email".to_string(),
            description: "Send an email".to_string(),
            tier: ActionTier::Staged,
            params: vec![
                ParamDef { name: "to".to_string(), description: None, required: true },
                ParamDef { name: "subject".to_string(), description: None, required: true },
                ParamDef { name: "body".to_string(), description: None, required: false },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn empty_catalog_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        assert!(catalog.load().unwrap().is_empty());
    }

    #[test]
    fn upsert_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();

        let actions = catalog.load().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].name, "send_email");
        assert_eq!(actions[0].tier, ActionTier::Staged);
    }

    #[test]
    fn upsert_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();

        let updated = ActionDef {
            name: "send_email".to_string(),
            description: "Updated description".to_string(),
            tier: ActionTier::Delayed,
            params: vec![],
            ..Default::default()
        };
        catalog.upsert(updated).unwrap();

        let actions = catalog.load().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].description, "Updated description");
        assert_eq!(actions[0].tier, ActionTier::Delayed);
    }

    #[test]
    fn remove_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();
        assert!(catalog.remove("send_email").unwrap());
        assert!(catalog.load().unwrap().is_empty());
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        assert!(!catalog.remove("missing").unwrap());
    }

    #[test]
    fn validate_call_success() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();

        let params: HashMap<String, serde_json::Value> = [
            ("to".to_string(), serde_json::json!("user@example.com")),
            ("subject".to_string(), serde_json::json!("Hello")),
        ]
        .into_iter()
        .collect();
        let def = catalog.validate_call("send_email", &params).unwrap();
        assert_eq!(def.name, "send_email");
    }

    #[test]
    fn validate_call_missing_required_param() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();

        let params: HashMap<String, serde_json::Value> =
            [("to".to_string(), serde_json::json!("user@example.com"))]
                .into_iter()
                .collect();
        let err = catalog.validate_call("send_email", &params).unwrap_err();
        assert!(err.to_string().contains("missing required param 'subject'"));
    }

    #[test]
    fn validate_call_unknown_param() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();

        let params: HashMap<String, serde_json::Value> = [
            ("to".to_string(), serde_json::json!("user@example.com")),
            ("subject".to_string(), serde_json::json!("Hello")),
            ("extra".to_string(), serde_json::json!("bad")),
        ]
        .into_iter()
        .collect();
        let err = catalog.validate_call("send_email", &params).unwrap_err();
        assert!(err.to_string().contains("unknown param 'extra'"));
    }

    #[test]
    fn validate_call_action_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        let params = HashMap::new();
        let err = catalog.validate_call("missing_action", &params).unwrap_err();
        assert!(err.to_string().contains("action not found"));
    }

    #[test]
    fn get_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        catalog.upsert(email_action()).unwrap();
        assert!(catalog.get("send_email").unwrap().is_some());
        assert!(catalog.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn save_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = make_catalog(tmp.path());
        let actions = vec![
            email_action(),
            ActionDef {
                name: "charge_customer".to_string(),
                description: "Charge via Stripe".to_string(),
                tier: ActionTier::Staged,
                params: vec![
                    ParamDef { name: "customer_id".to_string(), description: None, required: true },
                    ParamDef { name: "amount_cents".to_string(), description: None, required: true },
                ],
                ..Default::default()
            },
        ];
        catalog.save(&actions).unwrap();
        let loaded = catalog.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].name, "charge_customer");
    }
}
