//! Classify a change's paths against a test-footprint policy.
//!
//! Test selection and a merge queue ask the same question of every change: which parts of the
//! repository does it touch, does it change the CI itself, is it documentation only, and does
//! any path match no rule at all? This crate answers it once, from one policy file, so both
//! read the same buckets.
//!
//! The policy is a resolved footprint file (`version` 2), read as is:
//!
//! ```json
//! {
//!   "version": 2,
//!   "force_full": ["Cargo.lock", "ci/**"],
//!   "ci_irrelevant": ["**/*.md", "docs/**"],
//!   "groups": {"LINT": ["lint.fmt", "lint.clippy"]},
//!   "footprints": [{"paths": ["core/**/*.rs"], "nodes": ["test.core", "@LINT"]}],
//!   "backend_rules": [{"id": "fast", "tier": "backend", "paths": ["fast/**"],
//!                      "vendor_paths": ["fast-sys/**"], "backends": ["fast"],
//!                      "lanes": ["fast"], "extra_nodes": ["test.fast"]}]
//! }
//! ```
//!
//! Each changed path is matched in a fixed order. A `force_full` match forces the full suite.
//! Otherwise every footprint and backend rule that matches contributes its nodes. A path that
//! matched nothing is inert if it matches `ci_irrelevant`, and unknown otherwise. An unknown
//! path, an empty change, a `core`-tier backend rule, or a non-inert change that selected no
//! node gives [`Verdict::Full`]; a change whose every path is inert gives [`Verdict::Skip`];
//! anything else gives [`Verdict::Selective`] with the selected nodes. The classifier never
//! narrows past that: a consumer may still widen a selective verdict with its own policy.
//!
//! A backend rule key `NAME_paths` other than `paths` holds globs relative to the directory
//! `NAME/` (a vendored or submodule tree): `vendor_paths: ["fast-sys/**"]` matches the changed
//! path `vendor/fast-sys/src/lib.rs`.
//!
//! Globs are matched over `/`-separated paths: `**` matches any run of characters including
//! `/` (and `dir/**/x` also matches `dir/x`), `*` any run without `/`, `?` one character other
//! than `/`, and every other character itself. A pattern matches the whole path, never a
//! suffix. The golden vectors in `testdata/vectors.json` are the contract for these semantics.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

/// The policy `version` this crate reads.
pub const POLICY_VERSION: u64 = 2;

/// Match `path` against glob `pattern` (see the crate documentation).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    glob_inner(pattern.as_bytes(), path.as_bytes())
}

fn glob_inner(p: &[u8], t: &[u8]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => {
            let mut rest = &p[2..];
            if rest.first() == Some(&b'/') {
                // `**/` also matches zero directories.
                rest = &rest[1..];
            }
            (0..=t.len()).any(|i| glob_inner(rest, &t[i..]))
        }
        Some(b'*') => {
            for i in 0..=t.len() {
                if glob_inner(&p[1..], &t[i..]) {
                    return true;
                }
                if i < t.len() && t[i] == b'/' {
                    return false;
                }
            }
            false
        }
        Some(b'?') => !t.is_empty() && t[0] != b'/' && glob_inner(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob_inner(&p[1..], &t[1..]),
    }
}

fn matches_any(globs: &[String], path: &str) -> bool {
    globs.iter().any(|glob| glob_match(glob, path))
}

/// The three outcomes, named as test selection names them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Every changed path is inert (documentation and the like).
    Skip,
    /// The change maps to the selected nodes.
    Selective,
    /// Run everything: a forcing path, an unknown path, a core rule, or nothing to go on.
    Full,
}

impl Verdict {
    /// `skip`, `selective` or `full`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Selective => "selective",
            Self::Full => "full",
        }
    }
}

/// A footprint: paths and the nodes a change to them selects.
#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    /// Globs.
    pub paths: Vec<String>,
    /// Node references; `@NAME` names a group.
    pub nodes: Vec<String>,
    /// Every backend's end-to-end cells may be affected.
    pub e2e_all: bool,
    /// Backends whose end-to-end cells may be affected.
    pub e2e_backends: Vec<String>,
    /// Component name: the optional `component` key, else the first literal directory of the
    /// first path glob, else `footprint-N`.
    pub component: String,
    /// The entry as written, for keys this crate does not interpret.
    pub raw: Value,
}

/// A backend-affinity rule.
#[derive(Clone, Debug, PartialEq)]
pub struct BackendRule {
    /// Rule identifier.
    pub id: String,
    /// Globs over the repository.
    pub paths: Vec<String>,
    /// Globs relative to a directory: `(directory, globs)` for each `NAME_paths` key.
    pub scoped_paths: Vec<(String, Vec<String>)>,
    /// Backends its end-to-end cells belong to (`*`: every backend, never listed).
    pub backends: Vec<String>,
    /// Lanes it declares.
    pub lanes: Vec<String>,
    /// Nodes a `backend`-tier match selects.
    pub extra_nodes: Vec<String>,
    /// `backend` (scoped) or `core` (forces the full suite).
    pub tier: String,
}

impl BackendRule {
    fn matches(&self, path: &str) -> bool {
        matches_any(&self.paths, path)
            || self.scoped_paths.iter().any(|(dir, globs)| {
                path.strip_prefix(dir.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
                    .is_some_and(|inner| matches_any(globs, inner))
            })
    }
}

/// A loaded policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    /// Paths that force the full suite.
    pub force_full: Vec<String>,
    /// Paths that are inert when nothing else matches them.
    pub ci_irrelevant: Vec<String>,
    /// Node groups, `@NAME` in node lists.
    pub groups: BTreeMap<String, Vec<String>>,
    /// Footprints, in file order.
    pub footprints: Vec<Footprint>,
    /// Backend rules, in file order.
    pub backend_rules: Vec<BackendRule>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The first directory of `glob` when it is literal (`core/**/*.rs` gives `core`).
fn literal_dir(glob: &str) -> Option<String> {
    let (first, _) = glob.split_once('/')?;
    (!first.is_empty() && !first.contains(['*', '?'])).then(|| first.to_owned())
}

impl Policy {
    /// Parse a policy document.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(text).map_err(|error| format!("invalid policy JSON: {error}"))?;
        Self::from_value(&value)
    }

    /// Read a policy file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::from_json(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Build a policy from a parsed document.
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match value.get("version").and_then(Value::as_u64) {
            Some(POLICY_VERSION) => {}
            other => {
                return Err(format!(
                "policy version {} is not supported (this build reads version {POLICY_VERSION})",
                other.map_or("missing".to_owned(), |v| v.to_string())
            ))
            }
        }
        let groups = value
            .get("groups")
            .and_then(Value::as_object)
            .map(|groups| {
                groups
                    .iter()
                    .map(|(name, members)| (name.clone(), strings(members)))
                    .collect()
            })
            .unwrap_or_default();
        let footprints = value
            .get("footprints")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| {
                let paths = strings(&entry["paths"]);
                let component = entry
                    .get("component")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| paths.first().and_then(|glob| literal_dir(glob)))
                    .unwrap_or_else(|| format!("footprint-{index}"));
                Footprint {
                    nodes: strings(&entry["nodes"]),
                    e2e_all: entry["e2e_all"].as_bool().unwrap_or(false),
                    e2e_backends: strings(&entry["e2e_backends"]),
                    component,
                    raw: entry.clone(),
                    paths,
                }
            })
            .collect();
        let backend_rules = value
            .get("backend_rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|rule| BackendRule {
                id: rule["id"].as_str().unwrap_or_default().to_owned(),
                paths: strings(&rule["paths"]),
                scoped_paths: rule
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(key, globs)| {
                        let dir = key.strip_suffix("_paths")?;
                        (!dir.is_empty()).then(|| (dir.to_owned(), strings(globs)))
                    })
                    .collect(),
                backends: strings(&rule["backends"]),
                lanes: strings(&rule["lanes"]),
                extra_nodes: strings(&rule["extra_nodes"]),
                tier: rule["tier"].as_str().unwrap_or("backend").to_owned(),
            })
            .collect();
        let policy = Self {
            force_full: strings(&value["force_full"]),
            ci_irrelevant: strings(&value["ci_irrelevant"]),
            groups,
            footprints,
            backend_rules,
        };
        // Every group a rule names must exist, so classification never fails half way.
        let mut probe = BTreeSet::new();
        for refs in policy
            .footprints
            .iter()
            .map(|footprint| &footprint.nodes)
            .chain(policy.backend_rules.iter().map(|rule| &rule.extra_nodes))
        {
            policy.expand(refs, &mut probe, &mut BTreeSet::new())?;
        }
        Ok(policy)
    }

    /// Expand node references, resolving `@GROUP` aliases recursively (a cycle stops).
    fn expand(
        &self,
        refs: &[String],
        out: &mut BTreeSet<String>,
        seen: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        for reference in refs {
            match reference.strip_prefix('@') {
                Some(name) => {
                    if !seen.insert(name.to_owned()) {
                        continue;
                    }
                    let members = self
                        .groups
                        .get(name)
                        .ok_or_else(|| format!("unknown group @{name}"))?;
                    self.expand(members, out, seen)?;
                }
                None => {
                    out.insert(reference.clone());
                }
            }
        }
        Ok(())
    }
}

/// How one changed path was classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathClass {
    /// Matched `force_full`.
    ForceFull,
    /// Matched a footprint or a backend rule.
    Matched,
    /// Matched nothing but `ci_irrelevant`.
    Inert,
    /// Matched nothing.
    Unknown,
}

/// One changed path and what it matched.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PathMatch {
    /// The path.
    pub path: String,
    /// Its class.
    pub class: PathClass,
    /// Indexes of the footprints it matched.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<usize>,
    /// Identifiers of the backend rules it matched.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub backend_rules: Vec<String>,
}

/// The classification of one change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Classification {
    /// The outcome.
    pub verdict: Verdict,
    /// Nodes the matched rules named (groups expanded). Under [`Verdict::Full`] every node
    /// runs, whatever this lists.
    pub nodes: BTreeSet<String>,
    /// Every backend's end-to-end cells may be affected.
    pub e2e_all: bool,
    /// Backends whose end-to-end cells may be affected.
    pub e2e_backends: BTreeSet<String>,
    /// Backend rules matched, in first-match order.
    pub backend_rules: Vec<String>,
    /// Lanes those rules declare, in first-match order.
    pub backend_lanes: Vec<String>,
    /// Components touched: matched footprints' components and `backend:ID` per rule.
    pub components: BTreeSet<String>,
    /// Paths that matched no rule.
    pub unknown: Vec<String>,
    /// Each path's class, in input order.
    pub paths: Vec<PathMatch>,
    /// Why, one line each.
    pub reasons: Vec<String>,
}

impl Classification {
    /// Whether every path forces the full suite (the CI and build configuration itself).
    pub fn only_force_full(&self) -> bool {
        !self.paths.is_empty() && self.paths.iter().all(|p| p.class == PathClass::ForceFull)
    }

    /// Whether every path is inert.
    pub fn only_inert(&self) -> bool {
        !self.paths.is_empty() && self.paths.iter().all(|p| p.class == PathClass::Inert)
    }
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|existing| existing == item) {
        list.push(item.to_owned());
    }
}

/// Classify a change's paths (see the crate documentation for the rules).
pub fn classify(policy: &Policy, paths: &[String]) -> Classification {
    let mut out = Classification {
        verdict: Verdict::Full,
        nodes: BTreeSet::new(),
        e2e_all: false,
        e2e_backends: BTreeSet::new(),
        backend_rules: Vec::new(),
        backend_lanes: Vec::new(),
        components: BTreeSet::new(),
        unknown: Vec::new(),
        paths: Vec::new(),
        reasons: Vec::new(),
    };
    if paths.is_empty() {
        out.reasons
            .push("no changed-path information available → full suite".to_owned());
        return out;
    }
    let mut force = false;
    let mut all_inert = true;
    let mut core_rule: Option<String> = None;
    for path in paths {
        let mut entry = PathMatch {
            path: path.clone(),
            class: PathClass::Unknown,
            footprints: Vec::new(),
            backend_rules: Vec::new(),
        };
        if matches_any(&policy.force_full, path) {
            force = true;
            all_inert = false;
            entry.class = PathClass::ForceFull;
            out.reasons.push(format!("{path} → force_full"));
            out.paths.push(entry);
            continue;
        }
        for (index, footprint) in policy.footprints.iter().enumerate() {
            if matches_any(&footprint.paths, path) {
                // Groups were checked when the policy was loaded.
                let _ = policy.expand(&footprint.nodes, &mut out.nodes, &mut BTreeSet::new());
                out.e2e_all |= footprint.e2e_all;
                out.e2e_backends
                    .extend(footprint.e2e_backends.iter().cloned());
                out.components.insert(footprint.component.clone());
                entry.footprints.push(index);
            }
        }
        for rule in &policy.backend_rules {
            if rule.matches(path) {
                push_unique(&mut out.backend_rules, &rule.id);
                for lane in &rule.lanes {
                    push_unique(&mut out.backend_lanes, lane);
                }
                out.components.insert(format!("backend:{}", rule.id));
                if rule.tier == "core" {
                    core_rule = Some(rule.id.clone());
                } else {
                    let _ = policy.expand(&rule.extra_nodes, &mut out.nodes, &mut BTreeSet::new());
                    out.e2e_backends.extend(
                        rule.backends
                            .iter()
                            .filter(|backend| backend.as_str() != "*")
                            .cloned(),
                    );
                }
                entry.backend_rules.push(rule.id.clone());
            }
        }
        if !entry.footprints.is_empty() || !entry.backend_rules.is_empty() {
            all_inert = false;
            entry.class = PathClass::Matched;
        } else if matches_any(&policy.ci_irrelevant, path) {
            entry.class = PathClass::Inert;
        } else {
            all_inert = false;
            out.unknown.push(path.clone());
        }
        out.paths.push(entry);
    }
    out.verdict = if let Some(rule) = &core_rule {
        out.reasons
            .push(format!("shared core rule `{rule}` matched → full suite"));
        Verdict::Full
    } else if force {
        Verdict::Full
    } else if !out.unknown.is_empty() {
        out.reasons.push(format!(
            "{} unmapped path(s) (e.g. {}) → conservative full suite",
            out.unknown.len(),
            out.unknown
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
        Verdict::Full
    } else if out.nodes.is_empty() && all_inert {
        out.reasons
            .push("all changed paths are CI-irrelevant → skip".to_owned());
        Verdict::Skip
    } else if out.nodes.is_empty() {
        out.reasons
            .push("no footprint matched but paths not proven inert → full suite".to_owned());
        Verdict::Full
    } else {
        out.reasons
            .push(format!("{} node(s) selected", out.nodes.len()));
        Verdict::Selective
    };
    out
}

/// Parse `git diff --name-status` output into changed paths, counting both sides of a rename
/// or copy (its source may be in another component than its destination). A line holding only
/// a path passes through.
pub fn parse_name_status(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let Some(first) = fields.next() else {
            continue;
        };
        let status = first.trim();
        if status.starts_with('R') || status.starts_with('C') {
            out.extend(
                fields
                    .map(str::trim)
                    .filter(|field| !field.is_empty())
                    .map(str::to_owned),
            );
        } else if let Some(path) = fields.next().or(Some(status)) {
            let path = path.trim();
            if !path.is_empty() {
                out.push(path.to_owned());
            }
        }
    }
    out
}

/// The paths `head` changes since its merge base with `base` (`git diff --name-status -M
/// base...head` in `repo`), both sides of each rename counted.
pub fn changed_paths(repo: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-status", "-M", &format!("{base}...{head}")])
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff {base}...{head} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_name_status(&String::from_utf8_lossy(&output.stdout)))
}

/// One failed golden-vector case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorFailure {
    /// Which case.
    pub case: String,
    /// What differed.
    pub detail: String,
}

/// Check a golden-vector document (`testdata/vectors.json`'s format) against this build.
/// Returns the number of cases checked and every failure.
pub fn check_vectors(text: &str) -> Result<(usize, Vec<VectorFailure>), String> {
    let doc: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid vectors JSON: {error}"))?;
    if doc["schema"] != "change-classifier-vectors/v1" {
        return Err("vectors schema must be change-classifier-vectors/v1".to_owned());
    }
    let mut checked = 0;
    let mut failures = Vec::new();
    let mut fail = |case: String, detail: String| failures.push(VectorFailure { case, detail });
    for case in doc["globs"].as_array().into_iter().flatten() {
        checked += 1;
        let (pattern, path) = (
            case["pattern"].as_str().unwrap_or_default(),
            case["path"].as_str().unwrap_or_default(),
        );
        let want = case["match"].as_bool().unwrap_or(false);
        if glob_match(pattern, path) != want {
            fail(
                format!("glob {pattern:?} vs {path:?}"),
                format!("expected match={want}"),
            );
        }
    }
    for case in doc["name_status"].as_array().into_iter().flatten() {
        checked += 1;
        let got = parse_name_status(case["input"].as_str().unwrap_or_default());
        let want = strings(&case["paths"]);
        if got != want {
            fail(
                format!("name-status {:?}", case["input"]),
                format!("expected {want:?}, got {got:?}"),
            );
        }
    }
    let mut policies = BTreeMap::new();
    for (name, value) in doc["policies"].as_object().into_iter().flatten() {
        policies.insert(name.clone(), Policy::from_value(value)?);
    }
    for case in doc["classify"].as_array().into_iter().flatten() {
        checked += 1;
        let name = case["name"].as_str().unwrap_or("unnamed").to_owned();
        let Some(policy) = policies.get(case["policy"].as_str().unwrap_or_default()) else {
            fail(name, format!("unknown policy {}", case["policy"]));
            continue;
        };
        let got = classify(policy, &strings(&case["paths"]));
        let want = &case["expect"];
        if want["verdict"].as_str() != Some(got.verdict.as_str()) {
            fail(
                name.clone(),
                format!(
                    "verdict {} expected {}",
                    got.verdict.as_str(),
                    want["verdict"]
                ),
            );
        }
        let set = |key: &str| -> Option<BTreeSet<String>> {
            want.get(key)
                .map(|value| strings(value).into_iter().collect())
        };
        for (key, actual) in [
            ("nodes", &got.nodes),
            ("e2e_backends", &got.e2e_backends),
            ("components", &got.components),
        ] {
            if let Some(expected) = set(key) {
                if &expected != actual {
                    fail(
                        name.clone(),
                        format!("{key} {actual:?} expected {expected:?}"),
                    );
                }
            }
        }
        for (key, actual) in [
            ("backend_rules", &got.backend_rules),
            ("backend_lanes", &got.backend_lanes),
            ("unknown", &got.unknown),
        ] {
            if want.get(key).is_some() && &strings(&want[key]) != actual {
                fail(
                    name.clone(),
                    format!("{key} {actual:?} expected {}", want[key]),
                );
            }
        }
        if let Some(e2e_all) = want.get("e2e_all").and_then(Value::as_bool) {
            if e2e_all != got.e2e_all {
                fail(
                    name.clone(),
                    format!("e2e_all {} expected {e2e_all}", got.e2e_all),
                );
            }
        }
        if let Some(classes) = want.get("classes").and_then(Value::as_array) {
            let actual: Vec<Value> = got
                .paths
                .iter()
                .map(|p| serde_json::to_value(p.class).unwrap_or(Value::Null))
                .collect();
            if &actual != classes {
                fail(
                    name.clone(),
                    format!("classes {actual:?} expected {classes:?}"),
                );
            }
        }
    }
    Ok((checked, failures))
}

/// The golden vectors shipped with this crate: the contract for glob and ordering semantics.
pub const VECTORS: &str = include_str!("../testdata/vectors.json");

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn unknown_groups_are_refused_when_the_policy_loads() {
        let error = Policy::from_json(
            r#"{"version": 2, "footprints": [{"paths": ["a/**"], "nodes": ["@MISSING"]}]}"#,
        )
        .unwrap_err();
        assert!(error.contains("unknown group @MISSING"), "{error}");
        let error = Policy::from_json(r#"{"version": 3}"#).unwrap_err();
        assert!(error.contains("version 3"), "{error}");
    }

    #[test]
    fn group_cycles_stop() {
        let policy = Policy::from_json(
            r#"{"version": 2, "groups": {"A": ["x", "@B"], "B": ["y", "@A"]},
                "footprints": [{"paths": ["a/**"], "nodes": ["@A"]}]}"#,
        )
        .unwrap();
        let got = classify(&policy, &["a/f".to_owned()]);
        assert_eq!(got.nodes, ["x", "y"].map(str::to_owned).into());
    }
}
