//! The single history fold: admitted events to terminal rows.
//!
//! One row is projected per non-correction event carrying an object
//! `legacy_row`, at that event's union position. A correction substitutes
//! its whole row for the superseded event's row (chains follow to the last
//! correction); there is no field-level merge. Rows are deduplicated by
//! whole-row identity after workspace-token restoration. Terminal fields
//! (`commit`, `result`, `host`, `finished_at`) are read from the row, never
//! from the envelope.

use crate::event::{EventKind, RawEvent, Refusal};
use crate::pyjson::{dumps_pairs, dumps_sorted, py_repr, PyValue};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Token producers write in place of the workspace root; readers restore it.
pub const WORKSPACE_TOKEN: &str = "{{WORKSPACE_ROOT}}";

/// Folded history: rows in projection order plus their printed lines.
#[derive(Clone, Debug)]
pub struct FoldOutcome {
    /// Restored terminal rows, first occurrence per identity.
    pub rows: Vec<PyValue>,
    /// One sorted-keys compact JSON line per row, in row order.
    pub lines: Vec<String>,
}

fn invalid(event_id: Option<String>, reason: String) -> Refusal {
    Refusal::InvalidEvent {
        event_id,
        codes: vec![reason],
    }
}

fn correction_invalid(event_id: &str, reason: String) -> Refusal {
    invalid(
        Some(event_id.to_owned()),
        format!(
            "HistoryRow correction {} is invalid: {reason}",
            py_repr(&PyValue::Str(event_id.to_owned()))
        ),
    )
}

fn is_failed_gate(gate: &PyValue) -> bool {
    if matches!(gate.get("aborted"), Some(PyValue::Bool(true))) {
        return false;
    }
    let text = match gate.get("result") {
        Some(value) if value.truthy() => value.as_str().unwrap_or(""),
        _ => gate.get("kind").and_then(PyValue::as_str).unwrap_or(""),
    };
    matches!(text, "fail" | "failed" | "timeout")
}

/// Gate failure-binding consistency, checked on corrected rows only.
fn require_gate_failure_bindings(row: &PyValue) -> Result<(), String> {
    let Some(PyValue::Array(gates)) = row.get("gates") else {
        return Ok(());
    };
    for (index, gate) in gates.iter().enumerate() {
        if !gate.is_object() {
            continue;
        }
        let origin = gate.get("failure_origin");
        let has_substeps_key = gate.contains_key("failed_substeps");
        let ok = match origin {
            None | Some(PyValue::Null) => !has_substeps_key,
            Some(PyValue::Str(text)) if text == "outer_gate" => {
                is_failed_gate(gate)
                    && matches!(gate.get("failed_substeps"), Some(PyValue::Array(items)) if items.is_empty())
            }
            Some(PyValue::Str(text)) if text == "lane_substep" => {
                is_failed_gate(gate)
                    && matches!(gate.get("failed_substeps"), Some(PyValue::Array(items))
                        if !items.is_empty() && items.iter().all(|i| matches!(i, PyValue::Str(s) if !s.is_empty())))
            }
            _ => false,
        };
        if !ok {
            let name = gate
                .get("name")
                .and_then(PyValue::as_str)
                .map(|n| py_repr(&PyValue::Str(n.to_owned())))
                .unwrap_or_else(|| format!("'index {index}'"));
            return Err(format!(
                "gate {name} has inconsistent failure_origin/failed_substeps"
            ));
        }
    }
    Ok(())
}

struct Corrections {
    replacements: HashMap<String, (String, PyValue)>,
    correction_ids: BTreeSet<String>,
}

fn legacy_row_corrections(events: &[RawEvent]) -> Result<Corrections, Refusal> {
    let mut by_id: HashMap<String, &RawEvent> = HashMap::new();
    for event in events {
        if let Some(id) = event.get_str("event_id") {
            if !id.is_empty() {
                by_id.insert(id.to_owned(), event);
            }
        }
    }
    let mut replacements: HashMap<String, (String, PyValue)> = HashMap::new();
    let mut correction_ids: BTreeSet<String> = BTreeSet::new();
    for event in events {
        if event.kind() != Some(EventKind::RunCorrect) || !event.value.contains_key("legacy_row") {
            continue;
        }
        let Some(event_id) = event.get_str("event_id").filter(|s| !s.is_empty()) else {
            return Err(invalid(None, "event_id is missing".to_owned()));
        };
        let event_id = event_id.to_owned();
        let refuse = |reason: String| correction_invalid(&event_id, reason);
        // Orphan references refuse here (the main check pass has no id set).
        let reference = event
            .get("enriches")
            .filter(|v| v.truthy())
            .or_else(|| event.get("supersedes"));
        if let Some(reference) = reference {
            let known = reference
                .as_str()
                .map(|id| by_id.contains_key(id))
                .unwrap_or(false);
            if reference.truthy() && !known {
                return Err(refuse("event violates unresolvable-reference".to_owned()));
            }
        }
        let Some(supersedes) = event.get_str("supersedes").filter(|s| !s.is_empty()) else {
            return Err(refuse("supersedes is missing".to_owned()));
        };
        let supersedes = supersedes.to_owned();
        let Some(target) = by_id.get(&supersedes).copied() else {
            return Err(refuse(format!(
                "supersedes unknown event {}",
                py_repr(&PyValue::Str(supersedes.clone()))
            )));
        };
        match event.get_str("reason") {
            Some(reason) if !reason.trim().is_empty() => {}
            _ => return Err(refuse("reason is missing".to_owned())),
        }
        for field in ["run_id", "team", "host"] {
            if event.get(field) != target.get(field) {
                return Err(refuse(format!("{field} differs from superseded event")));
            }
        }
        let Some(corrected) = event.get("legacy_row").filter(|r| r.is_object()) else {
            return Err(refuse("legacy_row is not an object".to_owned()));
        };
        if !matches!(target.get("legacy_row"), Some(row) if row.is_object()) {
            return Err(refuse("superseded event has no legacy_row".to_owned()));
        }
        require_gate_failure_bindings(corrected).map_err(refuse)?;
        if replacements.contains_key(&supersedes) {
            return Err(refuse(format!(
                "a second correction also supersedes {}",
                py_repr(&PyValue::Str(supersedes))
            )));
        }
        replacements.insert(supersedes, (event_id.clone(), corrected.clone()));
        correction_ids.insert(event_id);
    }
    Ok(Corrections {
        replacements,
        correction_ids,
    })
}

/// Restore the workspace token in every string value (keys untouched).
pub fn restore_workspace(value: &PyValue, root: &str) -> PyValue {
    match value {
        PyValue::Str(text) => PyValue::Str(text.replace(WORKSPACE_TOKEN, root)),
        PyValue::Array(items) => PyValue::Array(
            items
                .iter()
                .map(|item| restore_workspace(item, root))
                .collect(),
        ),
        PyValue::Object(pairs) => PyValue::Object(
            pairs
                .iter()
                .map(|(key, child)| (key.clone(), restore_workspace(child, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn has_duplicate_keys(value: &PyValue) -> bool {
    match value {
        PyValue::Object(pairs) => {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for (key, child) in pairs {
                if !seen.insert(key.as_str()) {
                    return true;
                }
                if has_duplicate_keys(child) {
                    return true;
                }
            }
            false
        }
        PyValue::Array(items) => items.iter().any(has_duplicate_keys),
        _ => false,
    }
}

/// Whole-row identity used for dedup.
fn row_identity(row: &PyValue) -> String {
    if has_duplicate_keys(row) {
        dumps_pairs(row)
    } else {
        dumps_sorted(row)
    }
}

/// Fold unioned, checked events into terminal rows.
///
/// `workspace_root` is the explicit replacement text for
/// [`WORKSPACE_TOKEN`]; discovering it is deployment configuration, not
/// fold behaviour.
pub fn fold_rows(events: &[RawEvent], workspace_root: &str) -> Result<FoldOutcome, Refusal> {
    let corrections = legacy_row_corrections(events)?;
    let mut rows: Vec<PyValue> = Vec::new();
    let mut seen_identities: BTreeSet<String> = BTreeSet::new();
    for event in events {
        let source_id = crate::pyjson::py_str(event.get("event_id"));
        if corrections.correction_ids.contains(&source_id) {
            continue;
        }
        let Some(source_row) = event.get("legacy_row").filter(|r| r.is_object()) else {
            continue;
        };
        // Follow the correction chain to its terminal row.
        let mut current = source_id.clone();
        let mut terminal = source_row.clone();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        visited.insert(current.clone());
        while let Some((next_id, next_row)) = corrections.replacements.get(&current) {
            if !visited.insert(next_id.clone()) {
                return Err(invalid(
                    Some(source_id.clone()),
                    format!(
                        "HistoryRow correction cycle starts at {}",
                        py_repr(&PyValue::Str(source_id.clone()))
                    ),
                ));
            }
            current = next_id.clone();
            terminal = next_row.clone();
        }
        let restored = restore_workspace(&terminal, workspace_root);
        let identity = row_identity(&restored);
        if seen_identities.insert(identity) {
            rows.push(restored);
        }
    }
    let lines = rows.iter().map(dumps_sorted).collect();
    Ok(FoldOutcome { rows, lines })
}

/// Terminal comparison object for one row: the fields byte-compared by the
/// fold differential, present keys only (absent stays absent, null stays
/// null), read from the row itself.
pub fn terminal_row(row: &PyValue) -> PyValue {
    let mut pairs: Vec<(String, PyValue)> = Vec::new();
    for field in ["run_id", "commit", "result", "host", "finished_at"] {
        if let Some(value) = row.get(field) {
            pairs.push((field.to_owned(), value.clone()));
        }
    }
    PyValue::Object(pairs)
}

/// Sorted terminal JSONL lines for byte comparison (multiset view).
pub fn terminal_lines_sorted(rows: &[PyValue]) -> Vec<String> {
    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| dumps_sorted(&terminal_row(row)))
        .collect();
    lines.sort();
    lines
}

/// Index folded rows by exact `commit` string for verdict queries.
pub fn rows_by_commit(rows: &[PyValue]) -> BTreeMap<String, Vec<PyValue>> {
    let mut by_commit: BTreeMap<String, Vec<PyValue>> = BTreeMap::new();
    for row in rows {
        if let Some(commit) = row.get("commit").and_then(PyValue::as_str) {
            by_commit
                .entry(commit.to_owned())
                .or_default()
                .push(row.clone());
        }
    }
    by_commit
}
