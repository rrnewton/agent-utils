//! The per-PR signal log: one append-only JSONL per change under `state/signals/`,
//! written only through this module (the run loop, intake, and `merge-queue signal`).
//! It is the history the success-probability model reads: heads, focused tests,
//! reviews, and every validation a change took part in - alone or in a batch,
//! cleared or not. Nothing in the landing path reads it yet.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::decisions::Decision;
use crate::model::now;

/// Schema identifier of one signal line.
pub const SIGNAL_SCHEMA: &str = "merge-queue-signal/v1";

/// The signals directory inside a queue state root.
pub fn dir(state_root: &Path) -> PathBuf {
    state_root.join("signals")
}

fn file(state_root: &Path, pr: i64) -> PathBuf {
    dir(state_root).join(format!("{pr}.jsonl"))
}

/// Append one signal for `pr`. `fields` is a JSON object merged into the record;
/// `schema`, `at`, `pr` and `kind` are always the log's own.
pub fn append(state_root: &Path, pr: i64, kind: &str, fields: Value) -> Result<(), String> {
    let mut record = Map::new();
    record.insert("schema".to_owned(), json!(SIGNAL_SCHEMA));
    record.insert("at".to_owned(), json!(now()));
    record.insert("pr".to_owned(), json!(pr));
    record.insert("kind".to_owned(), json!(kind));
    if let Value::Object(extra) = fields {
        for (key, value) in extra {
            record.entry(key).or_insert(value);
        }
    }
    append_record(state_root, pr, &Value::Object(record))
}

fn append_record(state_root: &Path, pr: i64, record: &Value) -> Result<(), String> {
    let dir = dir(state_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = file(state_root, pr);
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    writeln!(
        out,
        "{}",
        serde_json::to_string(record).map_err(|error| error.to_string())?
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Every signal recorded for `pr`, oldest first. A torn last line is ignored.
pub fn read(state_root: &Path, pr: i64) -> Result<Vec<Value>, String> {
    let text = match std::fs::read_to_string(file(state_root, pr)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read signals for #{pr}: {error}")),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Change numbers that have a signal file.
pub fn prs(state_root: &Path) -> Vec<i64> {
    let Ok(entries) = std::fs::read_dir(dir(state_root)) else {
        return Vec::new();
    };
    let mut out: Vec<i64> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
                .and_then(|stem| stem.parse().ok())
        })
        .collect();
    out.sort_unstable();
    out
}

/// The `validated` signals one run's decisions produce: per change, the batch it
/// rode in, the candidate, the verdict, and the attribution the run reached.
pub fn validated_signals(decisions: &[Decision]) -> Vec<(i64, Value)> {
    let mut out = Vec::new();
    for decision in decisions {
        let batch: Vec<i64> = decision.changes.iter().map(|c| c.pr).collect();
        for change in &decision.changes {
            let attribution = if decision.culprit == Some(change.pr) {
                "culprit"
            } else if decision.landed.is_some() || decision.outcome == "green" {
                "cleared"
            } else {
                "undetermined"
            };
            out.push((
                change.pr,
                json!({
                    "head": change.head,
                    "batch": batch,
                    "candidate": decision.commit,
                    "base": decision.base,
                    "run": decision.run,
                    "validation": decision.validation,
                    "purpose": decision.purpose,
                    "verdict": decision.outcome,
                    "failing": decision.failed,
                    "attribution": attribution,
                    "at": decision.at,
                }),
            ));
        }
    }
    out
}

/// Append the `validated` signals for one run's decisions. Records carry the
/// validation's own end time, so a later backfill synthesizes the identical event.
pub fn append_validated(state_root: &Path, decisions: &[Decision]) -> Result<usize, String> {
    let signals = validated_signals(decisions);
    let count = signals.len();
    for (pr, fields) in signals {
        let at = fields
            .get("at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut record = Map::new();
        record.insert("schema".to_owned(), json!(SIGNAL_SCHEMA));
        record.insert("at".to_owned(), json!(at));
        record.insert("pr".to_owned(), json!(pr));
        record.insert("kind".to_owned(), json!("validated"));
        if let Value::Object(extra) = fields {
            for (key, value) in extra {
                record.entry(key).or_insert(value);
            }
        }
        append_record(state_root, pr, &Value::Object(record))?;
    }
    Ok(count)
}

/// Identity of a signal for backfill de-duplication: the event, not its timestamp
/// (a live record and its backfill synthesis can land a second apart).
fn dedupe_key(record: &Value) -> String {
    let get = |key: &str| record.get(key).cloned().unwrap_or(Value::Null);
    match record.get("kind").and_then(Value::as_str) {
        Some("validated") => format!(
            "validated:{}:{}:{}",
            get("candidate"),
            get("run"),
            get("validation")
        ),
        Some("head") => format!("head:{}:{}", get("head"), get("parent_head")),
        _ => format!("{}:{}", get("kind"), get("at")),
    }
}

/// Synthesize `head` and `validated` signals from the decision records, run event
/// logs and item records already in the state. Focused tests and reviews are never
/// invented: they exist only from their producers. Idempotent; returns how many
/// signals were added.
pub fn backfill(state_root: &Path) -> Result<usize, String> {
    let mut added = 0usize;
    // Validated: from every decision record file.
    let decisions_dir = state_root.join("decisions");
    let mut decisions: Vec<Decision> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&decisions_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with(".jsonl") || name == "rulings.jsonl" || name == "skips.jsonl" {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                for line in text.lines() {
                    if let Ok(decision) = serde_json::from_str::<Decision>(line) {
                        decisions.push(decision);
                    }
                }
            }
        }
    }
    let mut existing: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for pr in prs(state_root) {
        for record in read(state_root, pr)? {
            existing.entry(pr).or_default().insert(dedupe_key(&record));
        }
    }
    for (pr, fields) in validated_signals(&decisions) {
        let mut record = Map::new();
        record.insert("schema".to_owned(), json!(SIGNAL_SCHEMA));
        record.insert("at".to_owned(), fields["at"].clone());
        record.insert("pr".to_owned(), json!(pr));
        record.insert("kind".to_owned(), json!("validated"));
        record.insert("backfilled".to_owned(), json!(true));
        if let Value::Object(extra) = fields {
            for (key, value) in extra {
                record.entry(key).or_insert(value);
            }
        }
        let record = Value::Object(record);
        if existing.entry(pr).or_default().insert(dedupe_key(&record)) {
            append_record(state_root, pr, &record)?;
            added += 1;
        }
    }
    // Heads: from item histories (a `submitted` event names the head it carried).
    let items_dir = state_root.join("items");
    if let Ok(entries) = std::fs::read_dir(&items_dir) {
        for entry in entries.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(item) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let Some(pr) = item.get("pr").and_then(Value::as_i64) else {
                continue;
            };
            let Some(history) = item.get("history").and_then(Value::as_array) else {
                continue;
            };
            let mut parent = String::new();
            for event in history {
                if event.get("kind").and_then(Value::as_str) != Some("submitted") {
                    continue;
                }
                let detail = event
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(head) = detail
                    .strip_prefix("head ")
                    .and_then(|rest| rest.split(';').next())
                    .map(str::trim)
                    .filter(|head| !head.is_empty())
                else {
                    continue;
                };
                let record = json!({
                    "schema": SIGNAL_SCHEMA,
                    "at": event.get("at").cloned().unwrap_or(Value::Null),
                    "pr": pr,
                    "kind": "head",
                    "head": head,
                    "parent_head": parent,
                    "reason": "unknown",
                    "backfilled": true,
                });
                if existing.entry(pr).or_default().insert(dedupe_key(&record)) {
                    append_record(state_root, pr, &record)?;
                    added += 1;
                }
                parent = head.to_owned();
            }
        }
    }
    Ok(added)
}

/// Append signals not yet exported to `to` (one `<pr>.jsonl` per change there),
/// remembering per-change line counts in `signals/export-marks.json`. Returns how
/// many lines were exported.
pub fn export(state_root: &Path, to: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(to)
        .map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    let marks_path = dir(state_root).join("export-marks.json");
    let mut marks: BTreeMap<String, usize> = std::fs::read_to_string(&marks_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut exported = 0usize;
    for pr in prs(state_root) {
        let records = read(state_root, pr)?;
        let done = marks.get(&pr.to_string()).copied().unwrap_or(0);
        if records.len() <= done {
            continue;
        }
        let path = to.join(format!("{pr}.jsonl"));
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        for record in &records[done..] {
            writeln!(
                out,
                "{}",
                serde_json::to_string(record).map_err(|error| error.to_string())?
            )
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            exported += 1;
        }
        marks.insert(pr.to_string(), records.len());
    }
    if exported > 0 {
        let text =
            serde_json::to_string(&marks).map_err(|error| format!("cannot encode: {error}"))?;
        std::fs::write(&marks_path, text)
            .map_err(|error| format!("cannot write {}: {error}", marks_path.display()))?;
    }
    Ok(exported)
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::git::testrepo::TempDir;

    #[test]
    fn append_read_and_export_only_new_lines() {
        let tmp = TempDir::new("signal");
        append(
            &tmp.0,
            7,
            "focused-tests",
            json!({"head": "abc", "actor": "dev"}),
        )
        .unwrap();
        append(&tmp.0, 7, "review", json!({"verdict": "approve"})).unwrap();
        let records = read(&tmp.0, 7).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["kind"], "focused-tests");
        assert_eq!(records[0]["schema"], SIGNAL_SCHEMA);
        let out = tmp.0.join("exported");
        assert_eq!(export(&tmp.0, &out).unwrap(), 2);
        assert_eq!(export(&tmp.0, &out).unwrap(), 0);
        append(&tmp.0, 7, "head", json!({"head": "def"})).unwrap();
        assert_eq!(export(&tmp.0, &out).unwrap(), 1);
        let exported = std::fs::read_to_string(out.join("7.jsonl")).unwrap();
        assert_eq!(exported.lines().count(), 3);
    }
}
