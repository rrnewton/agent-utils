//! Deployment configuration for the `local-ci-hub` front door.
//!
//! Deployment vocabulary (repository location, pinned head, field names,
//! fixture producers) arrives as data in a JSON file, never as literals
//! in clause logic. The front door reads the file through `--config`;
//! this module owns its shape, validation, and mapping back to the
//! documented command-line flags so tests can pin that mapping without
//! running the binary.

use serde::Deserialize;
use std::path::Path;

/// Built-in default for `--repo`.
pub const DEFAULT_REPO: &str = ".";
/// Built-in default for `--git-ref`.
pub const DEFAULT_GIT_REF: &str = "HEAD";
/// Built-in default for `--binding-tree-field`.
pub const DEFAULT_BINDING_TREE_FIELD: &str = "tree";
/// Built-in default for `--evidence-head-field`.
pub const DEFAULT_EVIDENCE_HEAD_FIELD: &str = "validated_head_sha";

/// One deployment's front-door settings, as stored in a `--config` file.
///
/// Every field is optional in the file: an absent key (or, for the two
/// path-or-null keys, an explicit `null`) keeps the built-in default.
/// List keys replace, never append to, any default list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    /// Git repository holding the ledger shards (`--repo`).
    pub repo: Option<String>,
    /// Ref or commit whose git objects are the read authority
    /// (`--git-ref`).
    pub git_ref: Option<String>,
    /// Evidence archive root holding `run-evidence/` objects
    /// (`--evidence-root`); `null` means no evidence source.
    pub evidence_root: Option<String>,
    /// Field naming the validated tree for derived evidence bindings
    /// (`--binding-tree-field`).
    pub binding_tree_field: Option<String>,
    /// Extra base-evidence field names (repeatable `--base-field`).
    #[serde(default)]
    pub base_fields: Vec<String>,
    /// Admission-fixture producer names (repeatable
    /// `--fixture-producer`).
    #[serde(default)]
    pub fixture_producers: Vec<String>,
    /// Field naming the validated head for typed evidence
    /// canonicalization (`--evidence-head-field`).
    pub evidence_head_field: Option<String>,
    /// Index file path (`--index`); `null` means an ephemeral
    /// in-memory index.
    pub index: Option<String>,
}

fn require_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("deployment config field {field} must not be empty"));
    }
    Ok(())
}

impl DeploymentConfig {
    /// Parse and validate one config document (JSON text).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let config: Self =
            serde_json::from_str(text).map_err(|e| format!("invalid deployment config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Load and validate the config file at `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read deployment config {}: {e}", path.display()))?;
        Self::from_json(&text)
    }

    /// Check every present value is usable as deployment data.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(value) = &self.repo {
            require_non_empty("repo", value)?;
        }
        if let Some(value) = &self.git_ref {
            require_non_empty("git_ref", value)?;
        }
        if let Some(value) = &self.evidence_root {
            require_non_empty("evidence_root", value)?;
        }
        if let Some(value) = &self.binding_tree_field {
            require_non_empty("binding_tree_field", value)?;
        }
        for value in &self.base_fields {
            require_non_empty("base_fields", value)?;
        }
        for value in &self.fixture_producers {
            require_non_empty("fixture_producers", value)?;
        }
        if let Some(value) = &self.evidence_head_field {
            require_non_empty("evidence_head_field", value)?;
        }
        if let Some(value) = &self.index {
            require_non_empty("index", value)?;
        }
        Ok(())
    }

    /// The documented flags this config stands for, in help order.
    ///
    /// Absent scalars and a `null` evidence root or index emit no flag:
    /// they mean "keep the default" (`--evidence-root` unset, ephemeral
    /// index). Empty lists emit no repeatable flag.
    pub fn flags(&self) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();
        if let Some(value) = &self.repo {
            flags.push("--repo".to_owned());
            flags.push(value.clone());
        }
        if let Some(value) = &self.git_ref {
            flags.push("--git-ref".to_owned());
            flags.push(value.clone());
        }
        if let Some(value) = &self.evidence_root {
            flags.push("--evidence-root".to_owned());
            flags.push(value.clone());
        }
        if let Some(value) = &self.binding_tree_field {
            flags.push("--binding-tree-field".to_owned());
            flags.push(value.clone());
        }
        for value in &self.base_fields {
            flags.push("--base-field".to_owned());
            flags.push(value.clone());
        }
        for value in &self.fixture_producers {
            flags.push("--fixture-producer".to_owned());
            flags.push(value.clone());
        }
        if let Some(value) = &self.evidence_head_field {
            flags.push("--evidence-head-field".to_owned());
            flags.push(value.clone());
        }
        if let Some(value) = &self.index {
            flags.push("--index".to_owned());
            flags.push(value.clone());
        }
        flags
    }
}
