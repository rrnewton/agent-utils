//! The core's single canonical serializers and digests.
//!
//! No production path computes a digest outside this module, and no
//! production configuration delegates digesting to a subprocess. A
//! delegated digest may run only inside a parity test harness, as an
//! oracle checking these bytes.
//!
//! Two canonical forms live here and must not be conflated: the ledger
//! event digest (sorted keys, ASCII-escaped) and the receipt digest
//! (struct field order, raw UTF-8), whose bytes existing receipts already
//! depend on.

use crate::pyjson::{dumps_sorted, PyValue};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Canonicalization identifier for receipt digests.
pub const RECEIPT_CANONICALIZATION: &str = "serde_json::to_vec(HistoryRow)-v1";

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// SHA-256 hex of arbitrary bytes.
pub fn sha256_of(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Canonical event bytes: sorted keys, compact separators, ASCII escapes,
/// over the dict view.
pub fn canonical_event_bytes(value: &PyValue) -> Vec<u8> {
    dumps_sorted(value).into_bytes()
}

/// Ledger event digest over [`canonical_event_bytes`].
pub fn event_digest(value: &PyValue) -> String {
    sha256_hex(&canonical_event_bytes(value))
}

/// Event digest with any top-level `digest` field excluded, so verifying a
/// stored digest never hashes the digest itself.
pub fn event_digest_without_digest_field(value: &PyValue) -> String {
    match value {
        PyValue::Object(pairs) => {
            let stripped = PyValue::Object(
                pairs
                    .iter()
                    .filter(|(k, _)| k != "digest")
                    .cloned()
                    .collect(),
            );
            event_digest(&stripped)
        }
        other => event_digest(other),
    }
}

// --- Receipt canonical form (struct order, raw UTF-8) --------------------

/// Serialize one string exactly as `serde_json` does.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string always encodes")
}

/// Convert a retained value to `serde_json::Value` with consumer number
/// semantics (i64, else u64, else f64; non-finite refuses).
fn to_serde(value: &PyValue) -> Option<serde_json::Value> {
    match value {
        PyValue::Null => Some(serde_json::Value::Null),
        PyValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
        PyValue::Int(text) => {
            if let Ok(v) = text.parse::<i64>() {
                Some(serde_json::Value::Number(v.into()))
            } else if let Ok(v) = text.parse::<u64>() {
                Some(serde_json::Value::Number(v.into()))
            } else {
                let f: f64 = text.parse().ok()?;
                serde_json::Number::from_f64(f).map(serde_json::Value::Number)
            }
        }
        PyValue::Float(f) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number),
        PyValue::Str(s) => Some(serde_json::Value::String(s.clone())),
        PyValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_serde(item)?);
            }
            Some(serde_json::Value::Array(out))
        }
        PyValue::Object(pairs) => {
            let mut map = serde_json::Map::new();
            for (key, child) in pairs {
                map.insert(key.clone(), to_serde(child)?);
            }
            Some(serde_json::Value::Object(map))
        }
    }
}

fn emit(out: &mut String, first: &mut bool, key: &str, rendered: &str) {
    if !*first {
        out.push(',');
    }
    *first = false;
    out.push_str(&json_string(key));
    out.push(':');
    out.push_str(rendered);
}

fn render_opt_str(row: &PyValue, field: &str) -> Option<Option<String>> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(None),
        Some(PyValue::Str(s)) => Some(Some(s.clone())),
        _ => None,
    }
}

fn render_opt_bool(row: &PyValue, field: &str) -> Option<Option<bool>> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(None),
        Some(PyValue::Bool(b)) => Some(Some(*b)),
        _ => None,
    }
}

fn render_opt_i64(row: &PyValue, field: &str) -> Option<Option<i64>> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(None),
        Some(value) => value.as_i64().map(Some),
    }
}

fn render_opt_u64(row: &PyValue, field: &str) -> Option<Option<u64>> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(None),
        Some(value) => value.as_u64().map(Some),
    }
}

fn render_opt_f64(row: &PyValue, field: &str) -> Option<Option<f64>> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(None),
        Some(PyValue::Float(f)) if f.is_finite() => Some(Some(*f)),
        Some(PyValue::Int(text)) => text.parse::<f64>().ok().filter(|f| f.is_finite()).map(Some),
        _ => None,
    }
}

fn opt_text<T: ToString>(value: &Option<T>) -> String {
    match value {
        None => "null".to_owned(),
        Some(v) => v.to_string(),
    }
}

fn coverage_canonical(coverage: &PyValue) -> Option<String> {
    let planned = match coverage.get("planned_test_nodes") {
        None => 0,
        Some(value) => value.as_u64()?,
    };
    let executed = match coverage.get("executed_test_nodes") {
        None => 0,
        Some(value) => value.as_u64()?,
    };
    let node_list = |field: &str| -> Option<Option<Vec<String>>> {
        match coverage.get(field) {
            None | Some(PyValue::Null) => Some(None),
            Some(PyValue::Array(items)) => {
                let mut names = Vec::with_capacity(items.len());
                for item in items {
                    names.push(item.as_str()?.to_owned());
                }
                Some(Some(names))
            }
            _ => None,
        }
    };
    let zero = node_list("zero_executed_nodes")?;
    let absent = node_list("absent_nodes")?;
    let list_text = |list: &Option<Vec<String>>| -> String {
        match list {
            None => "null".to_owned(),
            Some(names) => {
                format!(
                    "[{}]",
                    names
                        .iter()
                        .map(|n| json_string(n))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
    };
    // Struct order; the decode-only run identity never serializes, and
    // unknown coverage keys are dropped by the typed shape.
    Some(format!(
        "{{\"planned_test_nodes\":{},\"executed_test_nodes\":{},\"zero_executed_nodes\":{},\"absent_nodes\":{}}}",
        planned,
        executed,
        list_text(&zero),
        list_text(&absent)
    ))
}

fn gate_canonical(gate: &PyValue) -> Option<String> {
    let name = gate.get("name")?.as_str()?;
    let mut out = String::from("{");
    let mut first = true;
    emit(&mut out, &mut first, "name", &json_string(name));
    for field in ["result", "kind"] {
        let value = render_opt_str(gate, field)?;
        let text = match &value {
            None => "null".to_owned(),
            Some(s) => json_string(s),
        };
        emit(&mut out, &mut first, field, &text);
    }
    let exit_code: Option<Option<i32>> = match gate.get("exit_code") {
        None | Some(PyValue::Null) => Some(None),
        Some(value) => value.as_i64().and_then(|v| i32::try_from(v).ok()).map(Some),
    };
    emit(&mut out, &mut first, "exit_code", &opt_text(&exit_code?));
    let real = render_opt_f64(gate, "real_seconds")?;
    let real_text = match real {
        None => "null".to_owned(),
        Some(f) => serde_json::Number::from_f64(f)?.to_string(),
    };
    emit(&mut out, &mut first, "real_seconds", &real_text);
    for field in ["source_node", "failure_origin"] {
        let value = render_opt_str(gate, field)?;
        let text = match &value {
            None => "null".to_owned(),
            Some(s) => json_string(s),
        };
        emit(&mut out, &mut first, field, &text);
    }
    // failed_substeps: skipped when absent; present null is malformed.
    match gate.get("failed_substeps") {
        None => {}
        Some(PyValue::Null) => return None,
        Some(PyValue::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                names.push(json_string(item.as_str()?));
            }
            emit(
                &mut out,
                &mut first,
                "failed_substeps",
                &format!("[{}]", names.join(",")),
            );
        }
        _ => return None,
    }
    const GATE_KNOWN: [&str; 8] = [
        "name",
        "result",
        "kind",
        "exit_code",
        "real_seconds",
        "source_node",
        "failure_origin",
        "failed_substeps",
    ];
    let mut extra: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    if let Some(pairs) = gate.as_object() {
        for (key, child) in pairs {
            if !GATE_KNOWN.contains(&key.as_str()) {
                extra.insert(key.clone(), to_serde(child)?);
            }
        }
    }
    for (key, child) in &extra {
        emit(
            &mut out,
            &mut first,
            key,
            &serde_json::to_string(child).ok()?,
        );
    }
    out.push('}');
    Some(out)
}

/// Receipt-row field order (struct declaration order). `gates` and the
/// flattened extensions come last; extensions sort by key.
const ROW_FIELDS: [&str; 37] = [
    "schema_version",
    "run_id",
    "started_at",
    "finished_at",
    "host",
    "slot",
    "repo",
    "cwd",
    "profile",
    "selection_mode",
    "build_mode",
    "prepared_cohort_modes",
    "commit",
    "commit_anchored",
    "tree_dirty",
    "result",
    "exit_code",
    "executed_tests",
    "passed_tests",
    "filtered_tests",
    "coverage",
    "full_coverage",
    "checks",
    "gates_run",
    "gates_expected",
    "failures",
    "dag_jobs",
    "concurrent_validates",
    "known_flaky_failure",
    "solo_rerun_confirmation",
    "real_seconds",
    "user_seconds",
    "sys_seconds",
    "log_file",
    "source",
    "cell_results",
    "test_results",
];

const SKIP_IF_NONE: [&str; 4] = [
    "build_mode",
    "prepared_cohort_modes",
    "passed_tests",
    "test_results",
];

/// Hidden-identity pre-check: a coverage run identity that is present,
/// non-null, and inconsistent with the row's run means the row has no
/// digest. Full admission verification lives with the predicate; this is
/// the digest gate only.
fn admission_identity_ok(row: &PyValue) -> bool {
    let Some(coverage) = row.get("coverage") else {
        return true;
    };
    if matches!(coverage, PyValue::Null) {
        return true;
    }
    for key in ["run_id", "admission_run_id"] {
        if let Some(id) = coverage.get(key) {
            match id {
                PyValue::Null => return false,
                PyValue::Str(text) => {
                    if let Some(run_id) = row.get("run_id").and_then(PyValue::as_str) {
                        if text != run_id {
                            return false;
                        }
                    }
                }
                _ => return false,
            }
        }
    }
    true
}

/// Canonical receipt-row JSON, or `None` when the row cannot be
/// canonicalized (wrong field types, non-finite numbers, or a failed
/// hidden-identity check). `None` means no digest exists; it is never a
/// different digest.
pub fn receipt_canonical_json(row: &PyValue) -> Option<String> {
    if !row.is_object() || !admission_identity_ok(row) {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    let mut extras: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    // Present-null build stamps are retained as extensions, matching the
    // typed decoder's re-insertion, instead of vanishing as skipped fields.
    for stamp in ["build_mode", "prepared_cohort_modes"] {
        if matches!(row.get(stamp), Some(PyValue::Null)) {
            extras.insert(stamp.to_owned(), serde_json::Value::Null);
        }
    }
    for field in ROW_FIELDS {
        let skip = SKIP_IF_NONE.contains(&field);
        let rendered: Option<String> = match field {
            "schema_version" => {
                let value: Option<Option<u32>> = match row.get(field) {
                    None | Some(PyValue::Null) => Some(None),
                    Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()).map(Some),
                };
                Some(opt_text(&value?))
            }
            "run_id" | "started_at" | "finished_at" | "host" | "slot" | "repo" | "cwd"
            | "profile" | "selection_mode" | "build_mode" | "commit" | "result" | "log_file"
            | "source" => {
                let value = render_opt_str(row, field)?;
                if skip && value.is_none() {
                    None
                } else {
                    Some(match &value {
                        None => "null".to_owned(),
                        Some(s) => json_string(s),
                    })
                }
            }
            "prepared_cohort_modes" => match row.get(field) {
                None | Some(PyValue::Null) => None,
                Some(PyValue::Object(_)) => {
                    Some(serde_json::to_string(&to_serde(row.get(field)?)?).ok()?)
                }
                _ => return None,
            },
            "commit_anchored"
            | "tree_dirty"
            | "full_coverage"
            | "known_flaky_failure"
            | "solo_rerun_confirmation" => Some(opt_text(&render_opt_bool(row, field)?)),
            "exit_code" => {
                let value: Option<Option<i32>> = match row.get(field) {
                    None | Some(PyValue::Null) => Some(None),
                    Some(v) => v.as_i64().and_then(|n| i32::try_from(n).ok()).map(Some),
                };
                Some(opt_text(&value?))
            }
            "executed_tests" | "passed_tests" | "filtered_tests" => {
                let value = render_opt_i64(row, field)?;
                if skip && value.is_none() {
                    None
                } else {
                    Some(opt_text(&value))
                }
            }
            "checks"
            | "gates_run"
            | "gates_expected"
            | "failures"
            | "dag_jobs"
            | "concurrent_validates" => Some(opt_text(&render_opt_u64(row, field)?)),
            "coverage" => match row.get(field) {
                None | Some(PyValue::Null) => Some("null".to_owned()),
                Some(coverage) => Some(coverage_canonical(coverage)?),
            },
            "real_seconds" | "user_seconds" | "sys_seconds" => {
                let value = render_opt_f64(row, field)?;
                Some(match value {
                    None => "null".to_owned(),
                    Some(f) => serde_json::Number::from_f64(f)?.to_string(),
                })
            }
            "cell_results" => match row.get(field) {
                None | Some(PyValue::Null) => Some("null".to_owned()),
                Some(value) => Some(serde_json::to_string(&to_serde(value)?).ok()?),
            },
            "test_results" => match row.get(field) {
                None | Some(PyValue::Null) => None,
                Some(value) => Some(serde_json::to_string(&to_serde(value)?).ok()?),
            },
            _ => return None,
        };
        if let Some(text) = rendered {
            emit(&mut out, &mut first, field, &text);
        }
    }
    // gates: always present (empty list when the row has none).
    let gates_text = match row.get("gates") {
        None | Some(PyValue::Null) => "[]".to_owned(),
        Some(PyValue::Array(items)) => {
            let mut parts = Vec::with_capacity(items.len());
            for gate in items {
                parts.push(gate_canonical(gate)?);
            }
            format!("[{}]", parts.join(","))
        }
        _ => return None,
    };
    emit(&mut out, &mut first, "gates", &gates_text);
    if let Some(pairs) = row.as_object() {
        for (key, child) in pairs {
            if !ROW_FIELDS.contains(&key.as_str()) && key != "gates" {
                extras.insert(key.clone(), to_serde(child)?);
            }
        }
    }
    for (key, child) in &extras {
        emit(
            &mut out,
            &mut first,
            key,
            &serde_json::to_string(child).ok()?,
        );
    }
    out.push('}');
    Some(out)
}

/// Receipt digest: SHA-256 hex of [`receipt_canonical_json`], or `None`
/// when the row has no digest.
pub fn receipt_digest(row: &PyValue) -> Option<String> {
    receipt_canonical_json(row).map(|text| sha256_hex(text.as_bytes()))
}
