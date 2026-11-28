//! Project configuration for the `local-ci-hub` front door: the TOML
//! `local-ci-hub.toml` shape the Python edition established
//! (`py/local_ci_hub/config.py`), with the same sections, defaults and
//! discovery order.
//!
//! Two deliberate tightenings over the Python loader, both named in
//! #2467: unknown keys refuse (every section denies unknown fields, as
//! [`crate::deployment`] already does for deployment data), and a
//! `[receipt] digest_command` refuses — digests are computed by the
//! one canonical serializer in [`crate::digest`], never delegated to a
//! subprocess (a delegated digest may exist only as a test oracle).
//!
//! Relative paths resolve against the configuration root (the config
//! file's directory), exactly as the Python loader resolves them.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Built-in default project name.
pub const DEFAULT_NAME: &str = "example";
/// Built-in default ledger team.
pub const DEFAULT_TEAM: &str = "team";
/// Built-in default ledger location, relative to the config root.
pub const DEFAULT_LEDGER_PATH: &str = "ci-ledger";
/// Built-in default producer name.
pub const DEFAULT_PRODUCER: &str = "local-ci-hub";
/// Built-in default validate admission slots.
pub const DEFAULT_ADMISSION_SLOTS: u32 = 2;

/// Raw `[project]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSection {
    name: Option<String>,
    #[serde(default)]
    repos: Vec<toml::Value>,
    repo: Option<String>,
}

/// Raw `[validate]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateSection {
    command: Option<Vec<String>>,
    #[serde(default)]
    cells: Vec<String>,
    admission_slots: Option<u32>,
}

/// Raw `[ledger.location]` table.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerLocation {
    path: Option<String>,
    repo_path: Option<String>,
}

/// Raw `[ledger]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerSection {
    #[serde(default)]
    location: LedgerLocation,
    team: Option<String>,
    publish: Option<String>,
    evidence_root: Option<String>,
    #[serde(default)]
    counted_repos: Vec<String>,
    repo: Option<String>,
    #[serde(default)]
    repo_aliases: Vec<String>,
    producer: Option<String>,
    producers: Option<Vec<String>>,
}

/// Raw `[remote]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteSection {
    provider: Option<String>,
    #[serde(default)]
    workflows: Vec<String>,
}

/// Raw `[receipt]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptSection {
    qualifying: Option<Vec<String>>,
    require_full_evidence: Option<bool>,
    release_payload: Option<toml::Table>,
    digest_command: Option<Vec<String>>,
    admission_kinds: Option<Vec<String>>,
    #[serde(default)]
    known_gaps: Vec<String>,
    #[serde(default)]
    era_records: Vec<String>,
}

/// Raw `[health]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthSection {
    #[serde(default)]
    probes: Vec<toml::Table>,
}

/// Raw `[land]` section.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LandSection {
    state_dir: Option<String>,
}

/// The whole TOML document, pre-defaulting.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    project: ProjectSection,
    #[serde(default)]
    validate: ValidateSection,
    #[serde(default)]
    ledger: LedgerSection,
    #[serde(default)]
    remote: RemoteSection,
    #[serde(default)]
    receipt: ReceiptSection,
    #[serde(default)]
    health: HealthSection,
    #[serde(default)]
    land: LandSection,
}

/// One project's validated configuration, with Python-edition defaults
/// applied and relative paths resolved against `root`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectConfig {
    /// The file this config was loaded from (the would-be path when
    /// no file exists and defaults apply).
    pub path: PathBuf,
    /// Root relative paths resolved against.
    pub root: PathBuf,
    /// True when a real file was parsed; false for local-only defaults.
    pub loaded: bool,
    /// `[project] name`.
    pub name: String,
    /// `[project] repos` (opaque project data, carried verbatim).
    pub repos: Vec<toml::Value>,
    /// Repository name rows are judged for (`[project] repo`, else
    /// `[ledger] repo`).
    pub repo: Option<String>,
    /// `[validate] command`.
    pub validate_command: Vec<String>,
    /// `[validate] cells`.
    pub cells: Vec<String>,
    /// `[validate] admission_slots`.
    pub admission_slots: u32,
    /// Resolved `[ledger.location] path`/`repo_path`: the checkout or
    /// directory holding the ledger shards.
    pub ledger_dir: PathBuf,
    /// Whether `[ledger.location]` named a path at all (the read side
    /// only lets an explicit location steer its `--repo` default).
    pub ledger_dir_explicit: bool,
    /// `[ledger] team`.
    pub team: String,
    /// `[ledger] publish` (`none` | `git`).
    pub publish: String,
    /// Resolved `[ledger] evidence_root`, when present.
    pub evidence_root: Option<PathBuf>,
    /// `[ledger] counted_repos`: repositories whose rows count toward
    /// verdicts (empty counts every repository).
    pub counted_repos: Vec<String>,
    /// `[ledger] repo_aliases`.
    pub repo_aliases: Vec<String>,
    /// `[ledger] producer`.
    pub producer: String,
    /// `[ledger] producers`.
    pub producers: Vec<String>,
    /// `[remote] provider` (`none` | `github`).
    pub provider: String,
    /// `[remote] workflows`.
    pub workflows: Vec<String>,
    /// `[receipt] qualifying` rules (subset of `local-ledger-pass`,
    /// `remote-pass`).
    pub qualifying: Vec<String>,
    /// `[receipt] require_full_evidence` (legacy switch, kept so
    /// existing configurations still load).
    pub require_full_evidence: bool,
    /// `[receipt.release_payload]` (project data, carried verbatim).
    pub release_payload: Option<toml::Table>,
    /// `[receipt] admission_kinds`.
    pub admission_kinds: Vec<String>,
    /// `[receipt] known_gaps`.
    pub known_gaps: Vec<String>,
    /// `[receipt] era_records`.
    pub era_records: Vec<String>,
    /// `[health] probes` (project data, carried verbatim).
    pub probes: Vec<toml::Table>,
    /// Resolved `[land] state_dir`.
    pub state_dir: PathBuf,
}

fn resolve(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn home_state_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home).join(".local/state/local-ci-hub"),
        _ => PathBuf::from(".local/state/local-ci-hub"),
    }
}

impl ProjectConfig {
    /// Local-only defaults for a project with no config file, exactly
    /// as the Python loader's no-file branch: ledger under
    /// `<root>/ci-ledger`, state under `<root>/ignored/local-ci-hub`.
    pub fn defaults(root: &Path) -> Self {
        let mut config = Self::from_raw(RawConfig::default(), root.to_path_buf(), false)
            .expect("default raw config is valid");
        config.path = PathBuf::from("local-ci-hub.toml");
        config.ledger_dir = root.join(DEFAULT_LEDGER_PATH);
        config.state_dir = root.join("ignored/local-ci-hub");
        config
    }

    /// Parse and validate one TOML document, resolving relative paths
    /// against `root`.
    pub fn from_toml(text: &str, root: &Path) -> Result<Self, String> {
        let raw: RawConfig =
            toml::from_str(text).map_err(|e| format!("invalid project config: {e}"))?;
        Self::from_raw(raw, root.to_path_buf(), true)
    }

    /// Load and validate the config file at `path`; relative paths
    /// resolve against the file's directory.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read project config {}: {e}", path.display()))?;
        let root = path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mut config = Self::from_toml(&text, &root)?;
        config.path = path.to_path_buf();
        Ok(config)
    }

    /// The repository identities whose rows count toward verdicts:
    /// `counted_repos ∪ repo_aliases ∪ {repo}`, exactly the Python
    /// receipt allow-list. The set may be empty (then no row naming
    /// a repository counts); callers with no project config at all
    /// keep the deployment reader's count-everything default instead.
    pub fn counted_repo_set(&self) -> BTreeSet<String> {
        let mut set: BTreeSet<String> = self.counted_repos.iter().cloned().collect();
        set.extend(self.repo_aliases.iter().cloned());
        if let Some(repo) = &self.repo {
            set.insert(repo.clone());
        }
        set
    }

    /// Discover a project config under `base`, in the Python loader's
    /// order: `local-ci-hub.toml`, then `ci/local-ci-hub.toml`.
    pub fn discover(base: &Path) -> Option<PathBuf> {
        [
            base.join("local-ci-hub.toml"),
            base.join("ci/local-ci-hub.toml"),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
    }

    fn from_raw(raw: RawConfig, root: PathBuf, loaded: bool) -> Result<Self, String> {
        if raw.receipt.digest_command.is_some() {
            return Err(
                "project config [receipt] digest_command is refused: digests are \
                 computed by the core's one canonical serializer, never delegated \
                 to a subprocess"
                    .to_owned(),
            );
        }
        let qualifying = raw
            .receipt
            .qualifying
            .unwrap_or_else(|| vec!["local-ledger-pass".to_owned()]);
        let unknown: Vec<&String> = qualifying
            .iter()
            .filter(|q| q.as_str() != "local-ledger-pass" && q.as_str() != "remote-pass")
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "unknown qualifying rules: {}",
                unknown
                    .iter()
                    .map(|q| q.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let provider = raw.remote.provider.unwrap_or_else(|| "none".to_owned());
        if provider == "none" && qualifying.iter().any(|q| q == "remote-pass") {
            return Err("qualifying includes remote-pass but remote.provider is none".to_owned());
        }
        let ledger_explicit = raw.ledger.location.path.clone().or(raw
            .ledger
            .location
            .repo_path
            .clone());
        let ledger_dir_explicit = ledger_explicit.is_some();
        let ledger_raw = ledger_explicit.unwrap_or_else(|| DEFAULT_LEDGER_PATH.to_owned());
        let state_dir = match &raw.land.state_dir {
            Some(value) => resolve(&root, value),
            None if loaded => home_state_dir(),
            None => root.join("ignored/local-ci-hub"),
        };
        Ok(Self {
            path: PathBuf::from("local-ci-hub.toml"),
            loaded,
            root: root.clone(),
            name: raw.project.name.unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            repos: raw.project.repos,
            repo: raw.project.repo.or(raw.ledger.repo),
            validate_command: raw
                .validate
                .command
                .unwrap_or_else(|| vec!["make".to_owned(), "validate".to_owned()]),
            cells: raw.validate.cells,
            admission_slots: raw
                .validate
                .admission_slots
                .unwrap_or(DEFAULT_ADMISSION_SLOTS),
            ledger_dir: resolve(&root, &ledger_raw),
            ledger_dir_explicit,
            team: raw.ledger.team.unwrap_or_else(|| DEFAULT_TEAM.to_owned()),
            publish: raw.ledger.publish.unwrap_or_else(|| "none".to_owned()),
            evidence_root: raw
                .ledger
                .evidence_root
                .as_deref()
                .map(|value| resolve(&root, value)),
            counted_repos: raw.ledger.counted_repos,
            repo_aliases: raw.ledger.repo_aliases,
            producer: raw
                .ledger
                .producer
                .unwrap_or_else(|| DEFAULT_PRODUCER.to_owned()),
            producers: raw
                .ledger
                .producers
                .unwrap_or_else(|| vec![DEFAULT_PRODUCER.to_owned()]),
            provider,
            workflows: raw.remote.workflows,
            qualifying,
            require_full_evidence: raw.receipt.require_full_evidence.unwrap_or(true),
            release_payload: raw.receipt.release_payload,
            admission_kinds: raw
                .receipt
                .admission_kinds
                .unwrap_or_else(|| vec!["host-limits-live".to_owned()]),
            known_gaps: raw.receipt.known_gaps,
            era_records: raw.receipt.era_records,
            probes: raw.health.probes,
            state_dir,
        })
    }
}
