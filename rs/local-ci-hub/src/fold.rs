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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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

/// The envelope fields of a correction's target that validation compares,
/// captured without retaining the target event itself.
#[derive(Clone, Debug)]
pub struct CorrectionTarget {
    /// Target `run_id` dict-view value, when present.
    pub run_id: Option<PyValue>,
    /// Target `team` dict-view value, when present.
    pub team: Option<PyValue>,
    /// Target `host` dict-view value, when present.
    pub host: Option<PyValue>,
    /// Whether the target carries an object `legacy_row`.
    pub has_object_row: bool,
}

impl CorrectionTarget {
    /// Capture the compared fields from one event.
    pub fn from_event(event: &RawEvent) -> Self {
        Self {
            run_id: event.get("run_id").cloned(),
            team: event.get("team").cloned(),
            host: event.get("host").cloned(),
            has_object_row: matches!(event.get("legacy_row"), Some(row) if row.is_object()),
        }
    }

    fn get(&self, field: &str) -> Option<&PyValue> {
        match field {
            "run_id" => self.run_id.as_ref(),
            "team" => self.team.as_ref(),
            "host" => self.host.as_ref(),
            _ => None,
        }
    }
}

/// Validated correction chains: superseded id to its correction, plus the
/// set of correction event ids (which project no row of their own). Only
/// corrections are retained, never the full event history.
#[derive(Clone, Debug, Default)]
pub struct CorrectionIndex {
    replacements: HashMap<String, (String, PyValue)>,
    correction_ids: HashSet<String>,
}

impl CorrectionIndex {
    /// Whether `source_id` (Python `str(event_id)`) names a correction.
    pub fn is_correction(&self, source_id: &str) -> bool {
        self.correction_ids.contains(source_id)
    }

    /// Follow the correction chain from `source_row` to its terminal row,
    /// cloning only the terminal row. Cycle and identity semantics match
    /// the slice fold exactly.
    pub fn terminal_row(&self, source_id: &str, source_row: &PyValue) -> Result<PyValue, Refusal> {
        let mut current = source_id.to_owned();
        let mut terminal: &PyValue = source_row;
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(current.clone());
        while let Some((next_id, next_row)) = self.replacements.get(&current) {
            if !visited.insert(next_id.clone()) {
                return Err(invalid(
                    Some(source_id.to_owned()),
                    format!(
                        "HistoryRow correction cycle starts at {}",
                        py_repr(&PyValue::Str(source_id.to_owned()))
                    ),
                ));
            }
            current = next_id.clone();
            terminal = next_row;
        }
        Ok(terminal.clone())
    }
}

/// Build the correction index from correction candidates in fold order.
///
/// `known` answers whether a non-empty string event id exists anywhere in
/// the history; `target` resolves a superseded id to its compared fields.
/// Both are lookups, so a streaming caller can answer them from an index
/// instead of retaining every event. Validation order and refusal texts
/// match the original whole-slice pass exactly.
pub fn build_correction_index<'a, I>(
    candidates: I,
    known: &dyn Fn(&str) -> bool,
    target: &dyn Fn(&str) -> Option<CorrectionTarget>,
) -> Result<CorrectionIndex, Refusal>
where
    I: Iterator<Item = &'a RawEvent>,
{
    let mut replacements: HashMap<String, (String, PyValue)> = HashMap::new();
    let mut correction_ids: HashSet<String> = HashSet::new();
    for event in candidates {
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
            let is_known = reference.as_str().map(known).unwrap_or(false);
            if reference.truthy() && !is_known {
                return Err(refuse("event violates unresolvable-reference".to_owned()));
            }
        }
        let Some(supersedes) = event.get_str("supersedes").filter(|s| !s.is_empty()) else {
            return Err(refuse("supersedes is missing".to_owned()));
        };
        let supersedes = supersedes.to_owned();
        let Some(target) = target(&supersedes) else {
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
        if !target.has_object_row {
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
    Ok(CorrectionIndex {
        replacements,
        correction_ids,
    })
}

fn legacy_row_corrections(events: &[RawEvent]) -> Result<CorrectionIndex, Refusal> {
    let mut by_id: HashMap<String, &RawEvent> = HashMap::new();
    for event in events {
        if let Some(id) = event.get_str("event_id") {
            if !id.is_empty() {
                by_id.insert(id.to_owned(), event);
            }
        }
    }
    build_correction_index(events.iter(), &|id| by_id.contains_key(id), &|id| {
        by_id
            .get(id)
            .map(|event| CorrectionTarget::from_event(event))
    })
}

/// Restore the workspace token in every string value (keys untouched),
/// mutating in place so folding a terminal row never copies its tree twice.
pub fn restore_workspace_in_place(value: &mut PyValue, root: &str) {
    match value {
        PyValue::Str(text) => {
            if text.contains(WORKSPACE_TOKEN) {
                *text = text.replace(WORKSPACE_TOKEN, root);
            }
        }
        PyValue::Array(items) => {
            for item in items {
                restore_workspace_in_place(item, root);
            }
        }
        PyValue::Object(pairs) => {
            for (_, child) in pairs {
                restore_workspace_in_place(child, root);
            }
        }
        _ => {}
    }
}

/// Restore the workspace token in every string value (keys untouched).
pub fn restore_workspace(value: &PyValue, root: &str) -> PyValue {
    let mut restored = value.clone();
    restore_workspace_in_place(&mut restored, root);
    restored
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

/// Printed line and dedup hash for one restored row. The canonical line is
/// serialized once; the hash (over the identity encoding) is all the dedup
/// set retains, never the full row text.
pub fn row_line_and_hash(row: &PyValue) -> (String, String) {
    let line = dumps_sorted(row);
    let hash = if has_duplicate_keys(row) {
        crate::digest::sha256_of(dumps_pairs(row).as_bytes())
    } else {
        crate::digest::sha256_of(line.as_bytes())
    };
    (line, hash)
}

/// Emit one source event's terminal row through `sink`, unless the event
/// projects no row or its restored identity was already emitted. Returns
/// whether a row was emitted. The event is borrowed; nothing is retained.
pub fn emit_terminal_row(
    corrections: &CorrectionIndex,
    seen_hashes: &mut HashSet<String>,
    event: &RawEvent,
    workspace_root: &str,
    sink: &mut dyn FnMut(PyValue, String),
) -> Result<bool, Refusal> {
    let source_id = crate::pyjson::py_str(event.get("event_id"));
    if corrections.is_correction(&source_id) {
        return Ok(false);
    }
    let Some(source_row) = event.get("legacy_row").filter(|r| r.is_object()) else {
        return Ok(false);
    };
    let mut restored = corrections.terminal_row(&source_id, source_row)?;
    restore_workspace_in_place(&mut restored, workspace_root);
    let (line, hash) = row_line_and_hash(&restored);
    if !seen_hashes.insert(hash) {
        return Ok(false);
    }
    sink(restored, line);
    Ok(true)
}

/// Fold unioned, checked events into terminal rows.
///
/// `workspace_root` is the explicit replacement text for
/// [`WORKSPACE_TOKEN`]; discovering it is deployment configuration, not
/// fold behaviour.
pub fn fold_rows(events: &[RawEvent], workspace_root: &str) -> Result<FoldOutcome, Refusal> {
    let corrections = legacy_row_corrections(events)?;
    let mut rows: Vec<PyValue> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut seen_hashes: HashSet<String> = HashSet::new();
    for event in events {
        emit_terminal_row(
            &corrections,
            &mut seen_hashes,
            event,
            workspace_root,
            &mut |row, line| {
                rows.push(row);
                lines.push(line);
            },
        )?;
    }
    Ok(FoldOutcome { rows, lines })
}

/// Stream the fold over a replayable event source without retaining the
/// history. `source` is called once per pass and must yield the history in
/// union order (duplicates allowed; they are collapsed by identity hash,
/// conflicting bodies refuse) on every call. Each folded row is handed to
/// `sink` with its canonical line and then dropped, so peak retention is
/// the correction index plus one event and one row at a time. Returns the
/// number of rows emitted.
pub fn fold_rows_streaming<I, F>(
    mut source: F,
    workspace_root: &str,
    mut sink: impl FnMut(PyValue, String),
) -> Result<usize, Refusal>
where
    I: Iterator<Item = Result<RawEvent, Refusal>>,
    F: FnMut() -> I,
{
    // Pass 1: collapse duplicates, remember string ids, and retain only
    // correction candidates (in fold order) for validation.
    let mut first_identity: HashMap<String, String> = HashMap::new();
    let mut known_ids: HashSet<String> = HashSet::new();
    let mut candidates: Vec<RawEvent> = Vec::new();
    let mut superseded_ids: HashSet<String> = HashSet::new();
    for item in source() {
        let event = item?;
        if event.kind() == Some(EventKind::RunAnnotate) {
            crate::event::check_annotate_event(&event)?;
        }
        let grouping = event.grouping_key().into_owned();
        match first_identity.get(&grouping) {
            None => {
                first_identity.insert(grouping, event.identity_hash());
            }
            Some(first) => {
                if *first != event.identity_hash() {
                    return Err(Refusal::ConflictingEventId { event_id: grouping });
                }
                continue;
            }
        }
        if let Some(id) = event.get_str("event_id").filter(|s| !s.is_empty()) {
            known_ids.insert(id.to_owned());
        }
        if event.kind() == Some(EventKind::RunCorrect) && event.value.contains_key("legacy_row") {
            if let Some(id) = event.get_str("supersedes").filter(|s| !s.is_empty()) {
                superseded_ids.insert(id.to_owned());
            }
            candidates.push(event);
        }
    }
    drop(first_identity);
    // Pass 2: capture compared fields for superseded targets only.
    let mut targets: HashMap<String, CorrectionTarget> = HashMap::new();
    if !superseded_ids.is_empty() {
        let mut seen: HashSet<String> = HashSet::new();
        for item in source() {
            let event = item?;
            let grouping = event.grouping_key().into_owned();
            if !seen.insert(grouping) {
                continue;
            }
            if let Some(id) = event.get_str("event_id").filter(|s| !s.is_empty()) {
                if superseded_ids.contains(id) {
                    targets.insert(id.to_owned(), CorrectionTarget::from_event(&event));
                }
            }
        }
    }
    let corrections =
        build_correction_index(candidates.iter(), &|id| known_ids.contains(id), &|id| {
            targets.get(id).cloned()
        })?;
    drop(candidates);
    drop(targets);
    drop(known_ids);
    // Pass 3: project rows one at a time; each raw event is dropped as
    // soon as its terminal row (if any) has been folded and emitted.
    let mut first_identity: HashMap<String, String> = HashMap::new();
    let mut seen_hashes: HashSet<String> = HashSet::new();
    let mut emitted = 0usize;
    for item in source() {
        let event = item?;
        let grouping = event.grouping_key().into_owned();
        match first_identity.get(&grouping) {
            None => {
                first_identity.insert(grouping, event.identity_hash());
            }
            Some(first) => {
                if *first != event.identity_hash() {
                    return Err(Refusal::ConflictingEventId { event_id: grouping });
                }
                continue;
            }
        }
        if emit_terminal_row(
            &corrections,
            &mut seen_hashes,
            &event,
            workspace_root,
            &mut sink,
        )? {
            emitted += 1;
        }
    }
    Ok(emitted)
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
