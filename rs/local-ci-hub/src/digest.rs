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

/// Exact key set of a dict-view object (duplicate written pairs have
/// already collapsed to their last value, as in a `serde_json::Value`).
fn exact_keys(value: &PyValue, keys: &[&str]) -> bool {
    match value.as_object() {
        Some(pairs) => {
            pairs.len() == keys.len()
                && keys
                    .iter()
                    .all(|key| pairs.iter().any(|(name, _)| name == key))
        }
        None => false,
    }
}

fn req_str<'a>(value: &'a PyValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(PyValue::as_str)
}

fn req_u64(value: &PyValue, key: &str) -> Option<u64> {
    value.get(key).and_then(PyValue::as_u64)
}

fn req_bool(value: &PyValue, key: &str) -> Option<bool> {
    match value.get(key) {
        Some(PyValue::Bool(flag)) => Some(*flag),
        _ => None,
    }
}

/// One nullable typed string: present value or explicit `null`.
fn opt_str_json(value: &PyValue, key: &str) -> Option<String> {
    match value.get(key) {
        None | Some(PyValue::Null) => Some("null".to_owned()),
        Some(child) => child.as_str().map(json_string),
    }
}

fn opt_bool_json(value: &PyValue, key: &str) -> Option<String> {
    match value.get(key) {
        None | Some(PyValue::Null) => Some("null".to_owned()),
        Some(PyValue::Bool(flag)) => Some(flag.to_string()),
        _ => None,
    }
}

fn str_list_json(value: &PyValue, key: &str) -> Option<String> {
    match value.get(key) {
        Some(PyValue::Array(items)) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(json_string(item.as_str()?));
            }
            Some(format!("[{}]", parts.join(",")))
        }
        _ => None,
    }
}

const IDENTITY_KEYS: [&str; 5] = ["lane", "category", "test", "mode", "backend"];

fn identity_json(value: &PyValue) -> Option<String> {
    if !exact_keys(value, &IDENTITY_KEYS) {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    for key in IDENTITY_KEYS {
        emit(
            &mut out,
            &mut first,
            key,
            &json_string(req_str(value, key)?),
        );
    }
    out.push('}');
    Some(out)
}

fn comparison_json(value: &PyValue) -> Option<String> {
    const KEYS: [&str; 16] = [
        "strictness",
        "display_name",
        "compare_logs",
        "compare_io_buffers",
        "record_envelope",
        "virtualize_time",
        "log_scope",
        "strip_lines",
        "canonicalize_addresses",
        "full_trace",
        "exact_remainder",
        "stripped_prefixes",
        "canonicalizations",
        "ignore_lines",
        "skip_commit",
        "skip_detlog",
    ];
    // The four `Option` fields may be absent (they deserialize to
    // `None` and serialize back as `null`); every other key is required
    // and no unknown key is allowed.
    const REQUIRED: [&str; 12] = [
        "strictness",
        "compare_logs",
        "log_scope",
        "strip_lines",
        "canonicalize_addresses",
        "full_trace",
        "exact_remainder",
        "stripped_prefixes",
        "canonicalizations",
        "ignore_lines",
        "skip_commit",
        "skip_detlog",
    ];
    let shape_ok = match value.as_object() {
        Some(pairs) => {
            pairs.iter().all(|(key, _)| KEYS.contains(&key.as_str()))
                && REQUIRED
                    .iter()
                    .all(|key| pairs.iter().any(|(name, _)| name == key))
        }
        None => false,
    };
    if !shape_ok {
        return None;
    }
    if !matches!(req_str(value, "strictness")?, "stripped" | "canonical")
        || !matches!(
            req_str(value, "log_scope")?,
            "deterministic" | "info" | "full_trace"
        )
    {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    emit(
        &mut out,
        &mut first,
        "strictness",
        &json_string(req_str(value, "strictness")?),
    );
    emit(
        &mut out,
        &mut first,
        "display_name",
        &opt_str_json(value, "display_name")?,
    );
    emit(
        &mut out,
        &mut first,
        "compare_logs",
        &req_bool(value, "compare_logs")?.to_string(),
    );
    emit(
        &mut out,
        &mut first,
        "compare_io_buffers",
        &opt_bool_json(value, "compare_io_buffers")?,
    );
    emit(
        &mut out,
        &mut first,
        "record_envelope",
        &opt_str_json(value, "record_envelope")?,
    );
    emit(
        &mut out,
        &mut first,
        "virtualize_time",
        &opt_bool_json(value, "virtualize_time")?,
    );
    emit(
        &mut out,
        &mut first,
        "log_scope",
        &json_string(req_str(value, "log_scope")?),
    );
    for key in [
        "strip_lines",
        "canonicalize_addresses",
        "full_trace",
        "exact_remainder",
    ] {
        emit(
            &mut out,
            &mut first,
            key,
            &req_bool(value, key)?.to_string(),
        );
    }
    emit(
        &mut out,
        &mut first,
        "stripped_prefixes",
        &str_list_json(value, "stripped_prefixes")?,
    );
    emit(
        &mut out,
        &mut first,
        "canonicalizations",
        &str_list_json(value, "canonicalizations")?,
    );
    for key in ["ignore_lines", "skip_commit", "skip_detlog"] {
        emit(
            &mut out,
            &mut first,
            key,
            &req_bool(value, key)?.to_string(),
        );
    }
    out.push('}');
    Some(out)
}

fn verdict_json(value: &PyValue) -> Option<String> {
    let state = value.get("state").and_then(PyValue::as_str)?;
    const TIERS: [&str; 4] = [
        "canonical-bitwise",
        "exit-and-stream-equality",
        "execution-only-self-consistent",
        "declared-but-unverifiable",
    ];
    let tier = req_str(value, "comparison_tier")?;
    if !TIERS.contains(&tier) {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    emit(&mut out, &mut first, "state", &json_string(state));
    emit(&mut out, &mut first, "comparison_tier", &json_string(tier));
    match state {
        "compared-and-matched" | "compared-and-diverged" => {
            if !exact_keys(
                value,
                &[
                    "state",
                    "comparison_tier",
                    "comparison",
                    "bitwise_parity",
                    "compared_log_messages",
                ],
            ) {
                return None;
            }
            emit(
                &mut out,
                &mut first,
                "comparison",
                &comparison_json(value.get("comparison")?)?,
            );
            emit(
                &mut out,
                &mut first,
                "bitwise_parity",
                &req_bool(value, "bitwise_parity")?.to_string(),
            );
            let counts = match value.get("compared_log_messages") {
                Some(PyValue::Null) => "null".to_owned(),
                Some(counts) if exact_keys(counts, &["left", "right"]) => {
                    let mut text = String::from("{");
                    let mut inner = true;
                    emit(
                        &mut text,
                        &mut inner,
                        "left",
                        &req_u64(counts, "left")?.to_string(),
                    );
                    emit(
                        &mut text,
                        &mut inner,
                        "right",
                        &req_u64(counts, "right")?.to_string(),
                    );
                    text.push('}');
                    text
                }
                _ => return None,
            };
            emit(&mut out, &mut first, "compared_log_messages", &counts);
        }
        "performs-no-comparison-by-design" | "unavailable-with-reason" => {
            if !exact_keys(value, &["state", "comparison_tier", "reason"]) {
                return None;
            }
            emit(
                &mut out,
                &mut first,
                "reason",
                &json_string(req_str(value, "reason")?),
            );
        }
        _ => return None,
    }
    out.push('}');
    Some(out)
}

fn binding_json(value: &PyValue) -> Option<String> {
    if !exact_keys(
        value,
        &[
            "run_id",
            "producer",
            "series_cell",
            "tree",
            "selected_attempt",
        ],
    ) {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    for key in ["run_id", "producer", "series_cell", "tree"] {
        emit(
            &mut out,
            &mut first,
            key,
            &json_string(req_str(value, key)?),
        );
    }
    emit(
        &mut out,
        &mut first,
        "selected_attempt",
        &req_u64(value, "selected_attempt")?.to_string(),
    );
    out.push('}');
    Some(out)
}

fn cell_json(value: &PyValue) -> Option<String> {
    let keys: Vec<&str> = value
        .as_object()?
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    let base = [
        "lane",
        "category",
        "test",
        "mode",
        "backend",
        "cell_verdict",
    ];
    let shape_ok = keys
        .iter()
        .all(|key| base.contains(key) || *key == "evidence_binding")
        && base.iter().all(|key| keys.contains(key));
    if !shape_ok {
        return None;
    }
    let mut out = String::from("{");
    let mut first = true;
    for key in IDENTITY_KEYS {
        emit(
            &mut out,
            &mut first,
            key,
            &json_string(req_str(value, key)?),
        );
    }
    emit(
        &mut out,
        &mut first,
        "cell_verdict",
        &verdict_json(value.get("cell_verdict")?)?,
    );
    if let Some(binding) = value.get("evidence_binding") {
        emit(
            &mut out,
            &mut first,
            "evidence_binding",
            &binding_json(binding)?,
        );
    }
    out.push('}');
    Some(out)
}

/// Canonical `cell_results` JSON. A value carrying the exact typed
/// evidence shape serializes in struct declaration order at every
/// depth (evidence, artifact, identities, cells, verdicts, bindings);
/// any other shape keeps the generic sorted-key form, exactly as the
/// typed reader's retained/raw fallback serializes it.
fn cell_results_canonical(value: &PyValue, head_field: &str) -> Option<String> {
    let typed = (|| -> Option<String> {
        let keys: [&str; 9] = [
            "run_id",
            head_field,
            "source_tree_dirty",
            "selected_count",
            "recorded_count",
            "population_sha256",
            "artifact",
            "selected",
            "cells",
        ];
        if !exact_keys(value, &keys) {
            return None;
        }
        let artifact = value.get("artifact")?;
        if !exact_keys(artifact, &["path", "sha256", "row_count"]) {
            return None;
        }
        let mut artifact_out = String::from("{");
        let mut afirst = true;
        emit(
            &mut artifact_out,
            &mut afirst,
            "path",
            &json_string(req_str(artifact, "path")?),
        );
        emit(
            &mut artifact_out,
            &mut afirst,
            "sha256",
            &json_string(req_str(artifact, "sha256")?),
        );
        emit(
            &mut artifact_out,
            &mut afirst,
            "row_count",
            &req_u64(artifact, "row_count")?.to_string(),
        );
        artifact_out.push('}');
        let selected = match value.get("selected") {
            Some(PyValue::Array(items)) => {
                let mut parts = Vec::with_capacity(items.len());
                for item in items {
                    parts.push(identity_json(item)?);
                }
                format!("[{}]", parts.join(","))
            }
            _ => return None,
        };
        let cells = match value.get("cells") {
            Some(PyValue::Array(items)) => {
                let mut parts = Vec::with_capacity(items.len());
                for item in items {
                    parts.push(cell_json(item)?);
                }
                format!("[{}]", parts.join(","))
            }
            _ => return None,
        };
        let mut out = String::from("{");
        let mut first = true;
        emit(
            &mut out,
            &mut first,
            "run_id",
            &json_string(req_str(value, "run_id")?),
        );
        emit(
            &mut out,
            &mut first,
            head_field,
            &json_string(req_str(value, head_field)?),
        );
        emit(
            &mut out,
            &mut first,
            "source_tree_dirty",
            &req_bool(value, "source_tree_dirty")?.to_string(),
        );
        emit(
            &mut out,
            &mut first,
            "selected_count",
            &req_u64(value, "selected_count")?.to_string(),
        );
        emit(
            &mut out,
            &mut first,
            "recorded_count",
            &req_u64(value, "recorded_count")?.to_string(),
        );
        emit(
            &mut out,
            &mut first,
            "population_sha256",
            &json_string(req_str(value, "population_sha256")?),
        );
        emit(&mut out, &mut first, "artifact", &artifact_out);
        emit(&mut out, &mut first, "selected", &selected);
        emit(&mut out, &mut first, "cells", &cells);
        out.push('}');
        Some(out)
    })();
    match typed {
        Some(text) => Some(text),
        None => serde_json::to_string(&to_serde(value)?).ok(),
    }
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
    receipt_canonical_json_with(row, "validated_head_sha")
}

/// Canonical receipt-row JSON with the deployment's validated-head
/// field name for typed cell evidence (deployment vocabulary arrives
/// as data, never as a literal in the clause logic).
pub fn receipt_canonical_json_with(row: &PyValue, head_field: &str) -> Option<String> {
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
                Some(value) => Some(cell_results_canonical(value, head_field)?),
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

/// Receipt digest with the deployment's validated-head field name.
pub fn receipt_digest_with(row: &PyValue, head_field: &str) -> Option<String> {
    receipt_canonical_json_with(row, head_field).map(|text| sha256_hex(text.as_bytes()))
}
