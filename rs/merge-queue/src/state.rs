//! Durable queue state under one directory.
//!
//! ```text
//! <state>/
//!   config.json            fixed: queue configuration
//!   lock                   fixed: writer lock (flock)
//!   union.attributes       fixed: generated from `union_paths`, when set
//!   items/<pr>.json        one record per submitted change
//!   runs/<run-id>/         one directory per `run`: plan, events, validations, report
//!   obligations/<id>.json  one record per focused landing still owed a complete run
//!   scratch/<name>/        candidate worktrees, removed after use
//! ```
//!
//! Growing collections live in their own subdirectories, never beside the fixed files.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::git::CombineMode;
use crate::model::{Item, Obligation};

/// Current configuration schema identifier.
pub const CONFIG_SCHEMA: &str = "merge-queue-config/v1";
/// Default maximum number of changes per batch.
pub const DEFAULT_MAX_BATCH: usize = 4;
/// Default speculative depth and validation width.
pub const DEFAULT_DEPTH: usize = 2;
/// Default priority for a submitted change; lower lands first.
pub const DEFAULT_PRIORITY: i64 = pr_landing_planner::priority::DEFAULT_PRIORITY;

/// How validation runs.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ValidatorConfig {
    /// `command` (a shell template) or `dagrun` (a dagrun DAG, step-level results).
    pub kind: String,
    /// Shell command printing the complete test inventory, one ID per line (optional).
    pub list: String,
    /// Shell command printing the policy's selection for the candidate (`$MQ_BASE`, `$MQ_HEAD`).
    /// Empty means the policy is `all`.
    pub select: String,
    /// Only lines starting with this prefix are test IDs in `select`/`list` output (prefix removed).
    pub id_prefix: String,
    /// Name recorded for the selection policy, e.g. `path-routing`.
    pub policy: String,
    /// Shell command that runs a selection (`command` kind). See the user guide for its contract.
    pub run: String,
    /// dagrun executable for the `dagrun` kind, relative to the candidate checkout or absolute.
    pub dagrun: String,
    /// DAG file for the `dagrun` kind, relative to the candidate checkout.
    pub dag: String,
    /// Command prefix every validation runs under, such as a host admission wrapper.
    pub admission: Vec<String>,
    /// Seconds before one validation is killed and reported as errors (0 = no limit).
    pub timeout_secs: u64,
    /// Also delete ignored files (build caches) from a reused checkout before each validation.
    /// Off by default: caches make validations fast, at the cost of trusting the build to be
    /// hermetic with respect to them.
    pub clean_ignored: bool,
}

/// How a validated candidate reaches the base branch.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LandConfig {
    /// `push` (fast-forward the remote base to the candidate), `command`, or `none`.
    pub driver: String,
    /// For `command`: shell template run once per change, in order. See the user guide.
    pub command: String,
    /// For `command`: allow batches of more than one change. The base then passes through each
    /// change's intermediate tree, which was not validated on its own; off by default, which
    /// makes every batch a single change.
    pub allow_batches: bool,
}

/// Generated files the queue regenerates instead of treating their conflicts as real.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegenerateConfig {
    /// Path patterns of generated files: an exact path, a directory ending in `/`, or a pattern
    /// with `*` wildcards (matching any characters, `/` included).
    pub paths: Vec<String>,
    /// Shell command, run at the repository root of a checkout, that rewrites every generated
    /// file from its sources. It must be deterministic and touch nothing else.
    pub command: String,
    /// `conflict` (default): regenerate only when a change's conflicts are confined to generated
    /// paths. `always`: also regenerate after every change, so combined changes never leave a
    /// generated file stale.
    pub when: String,
}

impl RegenerateConfig {
    /// Whether regeneration is configured.
    pub fn enabled(&self) -> bool {
        !self.paths.is_empty() && !self.command.trim().is_empty()
    }

    /// Whether `path` is a generated file.
    pub fn covers(&self, path: &str) -> bool {
        self.enabled() && self.paths.iter().any(|pattern| path_matches(pattern, path))
    }

    /// Whether to regenerate after every change, not only after a generated-file conflict.
    pub fn always(&self) -> bool {
        self.when == "always"
    }
}

/// Match a path against a pattern: exact, directory prefix (`dir/`), or `*` wildcards.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(dir) = pattern.strip_suffix('/') {
        return path.starts_with(&format!("{dir}/"));
    }
    if !pattern.contains('*') {
        return pattern == path;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = path;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(after) => rest = after,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    rest.is_empty()
}

/// Identity for commits the queue creates.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdentityConfig {
    /// Name; empty uses Git's own configuration.
    pub name: String,
    /// Email; empty uses Git's own configuration.
    pub email: String,
}

/// Queue configuration stored in `config.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Schema identifier.
    pub schema: String,
    /// Repository directory the queue operates on.
    pub repo: PathBuf,
    /// Remote holding the base branch and change heads; empty for a purely local repository.
    pub remote: String,
    /// Base branch name.
    pub base: String,
    /// Command prefix for network Git operations.
    pub net_wrapper: Vec<String>,
    /// How changes are combined onto a candidate.
    pub combine: CombineMode,
    /// Maximum changes per batch.
    pub max_batch: usize,
    /// Speculative depth: how many batches validate concurrently, each on top of the previous.
    /// Also the number of concurrent culprit-search probes.
    pub depth: usize,
    /// Largest window the adaptive (AIMD) window may grow to: +1 per landed batch, halved per
    /// red batch, never below 1. 0 (the default) means `depth`, a fixed window.
    pub max_depth: usize,
    /// Paths (Git attribute patterns) merged with Git's `union` driver when combining changes and
    /// probing conflicts. Only for line-oriented, append-only files; validation still checks the
    /// result.
    pub union_paths: Vec<String>,
    /// Generated files regenerated on the candidate instead of blocking it.
    pub regenerate: RegenerateConfig,
    /// Exact-commit rule: never carry a validation result to a different commit. A remainder
    /// after an ejection reruns the batch's whole selection, and a tree lands only with green
    /// coverage of its selection recorded at that exact commit.
    pub exact_sha: bool,
    /// Identity for commits the queue creates.
    pub identity: IdentityConfig,
    /// Validation configuration.
    pub validator: ValidatorConfig,
    /// Landing configuration.
    pub land: LandConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA.to_owned(),
            repo: PathBuf::new(),
            remote: String::new(),
            base: "main".to_owned(),
            net_wrapper: Vec::new(),
            combine: CombineMode::Rebase,
            max_batch: DEFAULT_MAX_BATCH,
            depth: DEFAULT_DEPTH,
            max_depth: 0,
            union_paths: Vec::new(),
            regenerate: RegenerateConfig::default(),
            exact_sha: false,
            identity: IdentityConfig::default(),
            validator: ValidatorConfig::default(),
            land: LandConfig::default(),
        }
    }
}

/// Handle on a state directory.
#[derive(Clone, Debug)]
pub struct State {
    /// Root of the state directory.
    pub root: PathBuf,
}

/// Exclusive writer lock, released on drop.
pub struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("invalid {}: {error}", path.display()))
}

/// Write JSON atomically (temporary file plus rename).
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
    text.push('\n');
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, text)
        .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|error| format!("cannot rename into {}: {error}", path.display()))
}

impl State {
    /// Open (without creating) the state directory at `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path of the configuration file.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// Directory of item records.
    pub fn items_dir(&self) -> PathBuf {
        self.root.join("items")
    }

    /// Directory of run records.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Directory of widening obligations.
    pub fn obligations_dir(&self) -> PathBuf {
        self.root.join("obligations")
    }

    /// Directory of candidate worktrees.
    pub fn scratch_dir(&self) -> PathBuf {
        self.root.join("scratch")
    }

    /// Whether a configuration exists.
    pub fn exists(&self) -> bool {
        self.config_path().is_file()
    }

    /// Create the directory layout and write the configuration.
    pub fn init(&self, config: &Config) -> Result<(), String> {
        for dir in [self.items_dir(), self.runs_dir(), self.obligations_dir()] {
            std::fs::create_dir_all(&dir)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        }
        write_json(&self.config_path(), config)
    }

    /// Load the configuration.
    pub fn config(&self) -> Result<Config, String> {
        if !self.exists() {
            return Err(format!(
                "no merge queue at {} (run `merge-queue init` first)",
                self.root.display()
            ));
        }
        let config: Config = read_json(&self.config_path())?;
        if config.schema != CONFIG_SCHEMA {
            return Err(format!(
                "{} has schema {:?}; this merge-queue reads {CONFIG_SCHEMA:?}",
                self.config_path().display(),
                config.schema
            ));
        }
        Ok(config)
    }

    /// Take the exclusive writer lock, failing fast if another writer holds it.
    pub fn lock(&self) -> Result<Lock, String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("cannot create {}: {error}", self.root.display()))?;
        let path = self.root.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        file.try_lock_exclusive().map_err(|_| {
            format!(
                "another merge-queue writer holds {} (a run in progress?)",
                path.display()
            )
        })?;
        Ok(Lock(file))
    }

    fn item_path(&self, pr: i64) -> PathBuf {
        self.items_dir().join(format!("{pr}.json"))
    }

    /// Load one item record, if present.
    pub fn item(&self, pr: i64) -> Result<Option<Item>, String> {
        let path = self.item_path(pr);
        if path.is_file() {
            read_json(&path).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Store one item record.
    pub fn save_item(&self, item: &Item) -> Result<(), String> {
        write_json(&self.item_path(item.pr), item)
    }

    /// Load every item record, ordered by PR number.
    pub fn items(&self) -> Result<Vec<Item>, String> {
        let mut items: Vec<Item> = load_dir(&self.items_dir())?;
        items.sort_by_key(|item| item.pr);
        Ok(items)
    }

    /// Store one obligation record.
    pub fn save_obligation(&self, obligation: &Obligation) -> Result<(), String> {
        write_json(
            &self
                .obligations_dir()
                .join(format!("{}.json", obligation.id)),
            obligation,
        )
    }

    /// Load every obligation, oldest first.
    pub fn obligations(&self) -> Result<Vec<Obligation>, String> {
        let mut all: Vec<Obligation> = load_dir(&self.obligations_dir())?;
        all.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
        Ok(all)
    }

    /// Create a fresh run directory and return its ID and path.
    pub fn new_run(&self) -> Result<(String, PathBuf), String> {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        for n in 0..1000 {
            let id = if n == 0 {
                stamp.clone()
            } else {
                format!("{stamp}-{n}")
            };
            let dir = self.runs_dir().join(&id);
            if std::fs::create_dir_all(self.runs_dir()).is_ok() && std::fs::create_dir(&dir).is_ok()
            {
                return Ok((id, dir));
            }
        }
        Err("cannot allocate a run directory".to_owned())
    }
}

fn load_dir<T: DeserializeOwned>(dir: &Path) -> Result<Vec<T>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            out.push(read_json(&path)?);
        }
    }
    Ok(out)
}
