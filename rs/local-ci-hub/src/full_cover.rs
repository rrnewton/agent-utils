//! Pure full-selection core and cover-evidence helpers (mirror of
//! the authority's `is_full_validation` semantics).
//!
//! In the authority's code this predicate gates the main-green-hours
//! classification only; the receipt qualifying predicate keeps its
//! own exact profile+selection clauses, and so does this crate's
//! [`crate::verdict`]. This module therefore serves the metrics path
//! (#2469) and any future caller that adopts the shared predicate;
//! the receipt path deliberately does not route through it.
//!
//! The core decision is pure: it consults only `selection_mode`,
//! never `profile`. Cover evidence for `label` selections compares
//! the full node set declared by a repo's DAG file at the run's exact
//! target commit with the nodes a run journal proves were planned
//! (started or skipped). Any absence of evidence fails closed, so
//! callers without a journal or DAG can simply pass `None` and still
//! evaluate a row safely: `label` then never covers, which is always
//! the safe answer.

use crate::event::is_lower_hex40;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The mirror-full core decision.
///
/// Mirrors `is_full_validation` exactly:
///
/// - `selection_mode == "full"` counts as a full validation.
/// - `selection_mode == "label"` counts only when the full node set
///   is non-empty and every full node appears in the run's planned
///   nodes (a covered label run then stands in for a full run).
/// - Anything else, including a missing selection mode, never
///   counts. `profile` is deliberately never consulted here.
///
/// `full_nodes`/`planned_nodes` are `None` when the underlying
/// evidence (DAG file at target, run journal) is absent or
/// unreadable; that always fails closed for `label`.
pub fn is_full_selection(
    selection_mode: Option<&str>,
    full_nodes: Option<&BTreeSet<String>>,
    planned_nodes: Option<&BTreeSet<String>>,
) -> bool {
    match selection_mode {
        Some("full") => true,
        Some("label") => match (full_nodes, planned_nodes) {
            (Some(full), Some(planned)) => !full.is_empty() && full.is_subset(planned),
            _ => false,
        },
        _ => false,
    }
}

/// The planned nodes a run journal proves were selected: the `step`
/// of every `step_start` and `step_skip` event (skipped dependents
/// count as selected, because they were part of the plan).
///
/// Unreadable input returns `None` so the caller fails closed; a
/// readable journal with no matching events returns an empty set.
/// Malformed lines and entries without a string `step` are skipped,
/// never fatal.
pub fn planned_nodes_from_journal(path: &Path) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(planned_nodes_from_journal_text(&text))
}

fn planned_nodes_from_journal_text(text: &str) -> BTreeSet<String> {
    let mut planned = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(event) = value.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        if event != "step_start" && event != "step_skip" {
            continue;
        }
        if let Some(step) = value.get("step").and_then(|v| v.as_str()) {
            if !step.is_empty() {
                planned.insert(step.to_string());
            }
        }
    }
    planned
}

/// The full node ids a DAG document declares for `full_label`.
///
/// Document shape: a JSON object with a `steps` array. Each step
/// names its node through the first present of `name`, `job`,
/// `step`, `id`; a `group` field, when present, prefixes the id as
/// `<group>.<id>`. A step is a full node when its `labels` array
/// contains `full_label`. Returns `None` for a document that is not
/// this shape (an empty full set is normalised to `None` by
/// [`full_nodes_at`], matching the fail-closed mirror; here an empty
/// set is returned as-is for direct callers).
pub fn full_nodes_from_dag_text(text: &str, full_label: &str) -> Option<BTreeSet<String>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let steps = value.get("steps")?.as_array()?;
    let mut nodes = BTreeSet::new();
    for step in steps {
        let Some(labels) = step.get("labels").and_then(|v| v.as_array()) else {
            continue;
        };
        let labelled_full = labels
            .iter()
            .any(|l| l.as_str().is_some_and(|s| s == full_label));
        if !labelled_full {
            continue;
        }
        let node = ["name", "job", "step", "id"]
            .iter()
            .find_map(|key| step.get(*key).and_then(|v| v.as_str()))?;
        if node.is_empty() {
            continue;
        }
        let id = match step.get("group").and_then(|v| v.as_str()) {
            Some(group) if !group.is_empty() => format!("{group}.{node}"),
            _ => node.to_string(),
        };
        nodes.insert(id);
    }
    Some(nodes)
}

/// Per-target memo of `full_nodes_at` results.
pub type FullNodesCache = BTreeMap<String, Option<BTreeSet<String>>>;

/// The full node set declared at `target` in `repo`'s DAG file.
///
/// `target` must be a 40-hex commit id; anything else returns
/// `None`, as does any git or parse failure. Results (including
/// failures) memoise per target inside `cache`. An empty
/// resolved full set returns `None`: no label run can cover an
/// empty plan, and the mirror normalises empty to absent.
pub fn full_nodes_at(
    repo: &Path,
    target: &str,
    dag_path: &str,
    full_label: &str,
    cache: &mut FullNodesCache,
) -> Option<BTreeSet<String>> {
    if let Some(hit) = cache.get(target) {
        return hit.clone();
    }
    let resolved = full_nodes_at_uncached(repo, target, dag_path, full_label);
    let stored = match resolved {
        Some(nodes) if !nodes.is_empty() => Some(nodes),
        _ => None,
    };
    cache.insert(target.to_string(), stored.clone());
    stored
}

fn full_nodes_at_uncached(
    repo: &Path,
    target: &str,
    dag_path: &str,
    full_label: &str,
) -> Option<BTreeSet<String>> {
    if !is_lower_hex40(target) {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["show", &format!("{target}:{dag_path}")])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    full_nodes_from_dag_text(&text, full_label)
}

/// Where a run's journal lives: an explicit record field wins when
/// it is a non-empty string; otherwise the artifacts layout
/// `<journal_root>/<run_id>.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalLocator {
    /// Root directory holding per-run journal files.
    pub root: PathBuf,
}

impl JournalLocator {
    /// Resolve the journal path for one run record.
    pub fn journal_path(&self, record_journal: Option<&str>, run_id: &str) -> PathBuf {
        match record_journal {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => self.root.join(format!("{run_id}.jsonl")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn only_selection_mode_decides() {
        // Case 16 (predicate truth table): profile never participates;
        // the function takes no profile at all.
        assert!(is_full_selection(Some("full"), None, None));
        let full = nodes(&["node-a", "node-b"]);
        let covered = nodes(&["node-a", "node-b", "node-c"]);
        let missing = nodes(&["node-a"]);
        assert!(is_full_selection(
            Some("label"),
            Some(&full),
            Some(&covered)
        ));
        assert!(!is_full_selection(
            Some("label"),
            Some(&full),
            Some(&missing)
        ));
        assert!(!is_full_selection(
            Some("only"),
            Some(&full),
            Some(&covered)
        ));
        assert!(!is_full_selection(
            Some("only-portable"),
            Some(&full),
            Some(&covered)
        ));
        assert!(!is_full_selection(None, Some(&full), Some(&covered)));
        assert!(!is_full_selection(Some("selected"), None, None));
    }

    #[test]
    fn label_needs_both_evidences_and_a_nonempty_full_set() {
        let full = nodes(&["node-a"]);
        let planned = nodes(&["node-a"]);
        let empty = BTreeSet::new();
        assert!(!is_full_selection(Some("label"), None, Some(&planned)));
        assert!(!is_full_selection(Some("label"), Some(&full), None));
        assert!(!is_full_selection(
            Some("label"),
            Some(&empty),
            Some(&planned)
        ));
    }

    #[test]
    fn planned_nodes_union_starts_and_skips() {
        let text = concat!(
            "{\"event\":\"step_start\",\"step\":\"node-a\"}\n",
            "{\"event\":\"step_skip\",\"step\":\"node-b\"}\n",
            "{\"event\":\"step_end\",\"step\":\"node-a\",\"ok\":true}\n",
            "not json\n",
            "{\"event\":\"step_start\",\"step\":\"\"}\n",
            "{\"event\":\"step_start\"}\n",
        );
        assert_eq!(
            planned_nodes_from_journal_text(text),
            nodes(&["node-a", "node-b"])
        );
        assert!(planned_nodes_from_journal_text("").is_empty());
    }

    #[test]
    fn full_nodes_read_grouped_labelled_steps() {
        let dag = r#"{"steps":[
            {"group":"gate","job":"node-a","labels":["full","fast"]},
            {"group":"gate","name":"node-b","labels":["full"]},
            {"id":"plain","labels":["nightly"]},
            {"step":"other","labels":["full"]}
        ]}"#;
        let got = full_nodes_from_dag_text(dag, "full").expect("dag parses");
        assert_eq!(got, nodes(&["gate.node-a", "gate.node-b", "other"]));
        assert_eq!(
            full_nodes_from_dag_text(dag, "nightly"),
            Some(nodes(&["plain"]))
        );
        assert!(full_nodes_from_dag_text("{nope", "full").is_none());
        assert!(full_nodes_from_dag_text("{\"nope\":1}", "full").is_none());
    }

    #[test]
    fn non_hex_targets_never_reach_git() {
        let mut cache = FullNodesCache::new();
        let repo = Path::new("/does/not/matter");
        for target in ["HEAD", "abc123", "", "branch-name"] {
            assert_eq!(
                full_nodes_at(repo, target, "dag.json", "full", &mut cache),
                None,
                "{target}"
            );
        }
        assert!(full_nodes_at(repo, &"a".repeat(40), "dag.json", "full", &mut cache).is_none());
    }

    #[test]
    fn journal_path_prefers_the_record_field() {
        let locator = JournalLocator {
            root: PathBuf::from("/journals"),
        };
        assert_eq!(
            locator.journal_path(Some("/elsewhere/run.log"), "run-1"),
            PathBuf::from("/elsewhere/run.log")
        );
        assert_eq!(
            locator.journal_path(Some(""), "run-1"),
            PathBuf::from("/journals/run-1.jsonl")
        );
        assert_eq!(
            locator.journal_path(None, "run-1"),
            PathBuf::from("/journals/run-1.jsonl")
        );
    }
}
