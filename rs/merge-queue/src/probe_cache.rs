//! Persistent cache for planning probes.
//!
//! A probe result depends only on commit IDs and on the merge configuration, never on the
//! current base except through the base key itself:
//!
//! * a base probe (changed files and conflicts of one head against the base) is keyed by
//!   `(config hash, base, head)`;
//! * a pairwise probe (ancestry and conflicts between two heads) is keyed by
//!   `(config hash, min(head), max(head))`, so it survives base movement and queue churn.
//!
//! The cache lives in the queue state directory (`probe-cache.json`) and therefore persists
//! across runs and processes. Stored conflict paths are raw `git merge-tree` output; the
//! regenerable-path filter is applied by the caller after retrieval, so a change to that
//! filter alone stays correct even before the config hash notices it. A change to a head is
//! a new key by construction; a change to merge attributes or regenerate configuration
//! changes the config hash and misses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::state::write_json;

/// Schema identifier of the on-disk cache file.
pub const PROBE_CACHE_SCHEMA: &str = "merge-queue-probe-cache/v2";
/// File name of the cache inside the queue state directory.
pub const PROBE_CACHE_FILE: &str = "probe-cache.json";

/// One cached base probe: raw results for `(base, head)`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BaseEntry {
    /// Files changed from the merge base of `base` and `head` through `head`, sorted.
    pub files: Vec<String>,
    /// Raw conflicting paths against the base, sorted (before regenerable filtering).
    pub conflicts: Vec<String>,
    /// Commits `head` adds over `base` (`rev-list BASE..HEAD`): another queued head in this set
    /// is an ancestor of this one, so pairwise ancestry needs no git call per pair.
    pub range: Vec<String>,
}

/// Which of two heads is an ancestor of the other, relative to the sorted key `(lo, hi)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PairAncestry {
    /// Neither head is an ancestor of the other.
    None,
    /// `lo` is an ancestor of `hi`.
    LoFirst,
    /// `hi` is an ancestor of `lo`.
    HiFirst,
}

/// One cached pairwise probe for two heads.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PairEntry {
    /// Ancestry relation between the two heads.
    pub ancestry: PairAncestry,
    /// Raw conflicting paths, sorted. `None` when no conflict probe was needed (an ancestry
    /// relation, or a pair skipped as file-disjoint at probe time); a later caller whose
    /// file sets are not disjoint re-probes just the conflicts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
}

impl Default for PairEntry {
    fn default() -> Self {
        Self {
            ancestry: PairAncestry::None,
            conflicts: None,
        }
    }
}

/// On-disk cache document.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProbeCacheFile {
    /// Schema identifier.
    pub schema: String,
    /// Base probes by `config:base:head`.
    pub base: BTreeMap<String, BaseEntry>,
    /// Pairwise probes by `config:lo:hi`.
    pub pairs: BTreeMap<String, PairEntry>,
}

/// An in-memory probe cache, optionally backed by `probe-cache.json` in a state directory.
#[derive(Clone, Debug, Default)]
pub struct ProbeCache {
    path: Option<PathBuf>,
    file: ProbeCacheFile,
    dirty: bool,
}

impl ProbeCache {
    /// An ephemeral cache that is never written to disk.
    pub fn ephemeral() -> Self {
        Self::default()
    }

    /// Load the cache stored in the state directory `dir`; a missing, unreadable or
    /// foreign-schema file is an empty cache, never an error.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(PROBE_CACHE_FILE);
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<ProbeCacheFile>(&text).ok())
            .filter(|file| file.schema == PROBE_CACHE_SCHEMA)
            .unwrap_or_else(|| ProbeCacheFile {
                schema: PROBE_CACHE_SCHEMA.to_owned(),
                ..ProbeCacheFile::default()
            });
        Self {
            path: Some(path),
            file,
            dirty: false,
        }
    }

    /// Whether this cache persists to disk.
    pub fn persistent(&self) -> bool {
        self.path.is_some()
    }

    /// Number of cached base probes.
    pub fn base_len(&self) -> usize {
        self.file.base.len()
    }

    /// Number of cached pairwise probes.
    pub fn pair_len(&self) -> usize {
        self.file.pairs.len()
    }

    /// Cache key for a base probe.
    pub fn base_key(config_hash: &str, base: &str, head: &str) -> String {
        format!("{config_hash}:{base}:{head}")
    }

    /// Cache key for a pairwise probe; the two heads are sorted so `(a, b)` and `(b, a)`
    /// share one entry.
    pub fn pair_key(config_hash: &str, head_a: &str, head_b: &str) -> (String, String, String) {
        let (lo, hi) = if head_a <= head_b {
            (head_a, head_b)
        } else {
            (head_b, head_a)
        };
        (
            format!("{config_hash}:{lo}:{hi}"),
            lo.to_owned(),
            hi.to_owned(),
        )
    }

    /// The cached base probe for `key`, if any.
    pub fn base(&self, key: &str) -> Option<&BaseEntry> {
        self.file.base.get(key)
    }

    /// Record a base probe.
    pub fn put_base(&mut self, key: String, entry: BaseEntry) {
        if self.file.base.get(&key) != Some(&entry) {
            self.file.base.insert(key, entry);
            self.dirty = true;
        }
    }

    /// The cached pairwise probe for `key`, if any.
    pub fn pair(&self, key: &str) -> Option<&PairEntry> {
        self.file.pairs.get(key)
    }

    /// Record (or complete) a pairwise probe, keeping any already-known conflicts.
    pub fn put_pair(&mut self, key: String, entry: PairEntry) {
        let merged = match self.file.pairs.get(&key) {
            Some(old) if old.ancestry == entry.ancestry => PairEntry {
                ancestry: entry.ancestry,
                conflicts: entry.conflicts.or_else(|| old.conflicts.clone()),
            },
            _ => entry,
        };
        if self.file.pairs.get(&key) != Some(&merged) {
            self.file.pairs.insert(key, merged);
            self.dirty = true;
        }
    }

    /// Drop entries that can no longer be asked for: another base, or a head that is not queued
    /// now (under any merge configuration). Keeps the file proportional to the queue.
    pub fn retain(&mut self, base: &str, heads: &std::collections::BTreeSet<&str>) {
        let before = (self.file.base.len(), self.file.pairs.len());
        self.file.base.retain(|key, _| {
            let mut parts = key.rsplitn(3, ':');
            let head = parts.next().unwrap_or_default();
            let entry_base = parts.next().unwrap_or_default();
            entry_base == base && heads.contains(head)
        });
        self.file.pairs.retain(|key, _| {
            let mut parts = key.rsplitn(3, ':');
            let hi = parts.next().unwrap_or_default();
            let lo = parts.next().unwrap_or_default();
            heads.contains(lo) && heads.contains(hi)
        });
        if (self.file.base.len(), self.file.pairs.len()) != before {
            self.dirty = true;
        }
    }

    /// Write the cache back to disk when it changed; ephemeral caches do nothing.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(path) = &self.path {
            write_json(path, &self.file)?;
        }
        self.dirty = false;
        Ok(())
    }
}

/// Everything outside the commits themselves that can change what `git merge-tree` answers in
/// this repository: the repository's `info/attributes`, the effective global attributes file,
/// the attributes file at `HEAD` (bare repositories read it there), merge and diff
/// configuration (rename detection, custom drivers, renormalisation), and the Git version.
pub fn merge_environment(git: &Git) -> String {
    let read = |path: &str| -> String {
        if path.is_empty() {
            return String::new();
        }
        let expanded = match path.strip_prefix("~/") {
            Some(rest) => std::env::var("HOME")
                .map(|home| format!("{home}/{rest}"))
                .unwrap_or_default(),
            None => path.to_owned(),
        };
        let full = std::path::Path::new(&expanded);
        let full = if full.is_absolute() {
            full.to_path_buf()
        } else {
            git.dir.join(full)
        };
        std::fs::read_to_string(full).unwrap_or_default()
    };
    let info = git
        .run(&["rev-parse", "--git-path", "info/attributes"])
        .map(|path| read(&path))
        .unwrap_or_default();
    let global = git
        .run(&["config", "--get", "core.attributesFile"])
        .map(|path| read(&path))
        .unwrap_or_default();
    let head_attributes = git
        .run(&["rev-parse", "--verify", "--quiet", "HEAD:.gitattributes"])
        .unwrap_or_default();
    let merge_config = git
        .run(&["config", "--get-regexp", "^(merge|diff)\\."])
        .unwrap_or_default();
    let version = git.run(&["--version"]).unwrap_or_default();
    format!(
        "info-attributes={info}\0global-attributes={global}\0head-attributes={head_attributes}\0merge-config={merge_config}\0git={version}"
    )
}

/// A stable hash of everything besides commit IDs that can change a probe result: the Git
/// `-c` settings (which carry the merge-attributes file) plus caller-supplied material such
/// as union paths and regenerate configuration. FNV-1a; the value only needs to be stable
/// across processes and sensitive to changes, not cryptographic.
pub fn probe_config_hash(git: &Git, extra: &str) -> String {
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    let mut hash: u64 = 0xcbf29ce484222325;
    for setting in &git.settings {
        feed(&mut hash, setting.as_bytes());
        feed(&mut hash, b"\0");
    }
    feed(&mut hash, b"\x01");
    feed(&mut hash, extra.as_bytes());
    format!("{hash:016x}")
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::git::testrepo::TempDir;

    #[test]
    fn merge_environment_changes_with_repository_attributes() {
        let tmp = TempDir::new("probe-env");
        let git = crate::git::testrepo::init(&tmp.0);
        let before = merge_environment(&git);
        std::fs::create_dir_all(tmp.0.join(".git/info")).unwrap();
        std::fs::write(
            tmp.0.join(".git/info/attributes"),
            "shared.txt merge=union\n",
        )
        .unwrap();
        let after = merge_environment(&git);
        assert_ne!(before, after);
        assert_ne!(
            probe_config_hash(&git, &before),
            probe_config_hash(&git, &after)
        );
    }

    fn base_entry() -> BaseEntry {
        BaseEntry {
            files: vec!["a.txt".to_owned()],
            conflicts: vec![],
            range: vec!["H".to_owned()],
        }
    }

    fn pair_entry() -> PairEntry {
        PairEntry {
            ancestry: PairAncestry::None,
            conflicts: Some(vec!["x.txt".to_owned()]),
        }
    }

    #[test]
    fn pair_key_is_order_independent() {
        let (ab, lo, hi) = ProbeCache::pair_key("cfg", "bbb", "aaa");
        let (ba, _, _) = ProbeCache::pair_key("cfg", "aaa", "bbb");
        assert_eq!(ab, ba);
        assert_eq!((lo.as_str(), hi.as_str()), ("aaa", "bbb"));
    }

    #[test]
    fn config_hash_tracks_settings_and_extra() {
        let mut git = Git::new("/tmp");
        let base = probe_config_hash(&git, "");
        assert_eq!(base, probe_config_hash(&Git::new("/elsewhere"), ""));
        git.settings = vec!["core.attributesFile=/x/union.attributes".to_owned()];
        assert_ne!(base, probe_config_hash(&git, ""));
        assert_ne!(
            probe_config_hash(&git, "regen:a"),
            probe_config_hash(&git, "regen:b")
        );
    }

    #[test]
    fn cache_survives_a_fresh_load() {
        let tmp = TempDir::new("probe-cache");
        let mut cache = ProbeCache::load(&tmp.0);
        cache.put_base(ProbeCache::base_key("cfg", "B", "H"), base_entry());
        cache.put_pair(ProbeCache::pair_key("cfg", "H1", "H2").0, pair_entry());
        cache.save().unwrap();
        assert!(tmp.0.join(PROBE_CACHE_FILE).is_file());

        // A fresh load stands in for a new process reading the same state directory.
        let reloaded = ProbeCache::load(&tmp.0);
        assert_eq!(reloaded.base_len(), 1);
        assert_eq!(reloaded.pair_len(), 1);
        assert_eq!(
            reloaded.base(&ProbeCache::base_key("cfg", "B", "H")),
            Some(&base_entry())
        );
        // A different head or config hash is a different key.
        assert_eq!(reloaded.base(&ProbeCache::base_key("cfg", "B", "H2")), None);
        assert_eq!(reloaded.base(&ProbeCache::base_key("zz", "B", "H")), None);
    }

    #[test]
    fn corrupt_cache_file_loads_empty() {
        let tmp = TempDir::new("probe-cache-corrupt");
        std::fs::write(tmp.0.join(PROBE_CACHE_FILE), "{not json").unwrap();
        let cache = ProbeCache::load(&tmp.0);
        assert_eq!(cache.base_len(), 0);
        assert_eq!(cache.pair_len(), 0);
    }
}
