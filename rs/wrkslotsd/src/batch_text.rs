//! Python-shaped human renderings for the recover-batch writers.
//!
//! Each renderer takes the same payload `Value` the JSON edition
//! prints and reproduces the Python edition's human lines (or its
//! exact JSON spelling) byte for byte.

use serde_json::{json, Value};

pub(crate) fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut sorted = serde_json::Map::new();
            for (key, item) in entries {
                sorted.insert(key.clone(), sort_keys(item));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `json.dumps(..., sort_keys=True)` spelling: `, ` and `: `.
pub(crate) fn py_compact_pub(value: &Value) -> String {
    py_compact(value)
}

fn py_compact(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => serde_json::to_string(text).unwrap_or_default(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_compact).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(key, item)| format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap_or_default(),
                        py_compact(item)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn compact_json(value: &Value) -> String {
    format!("{}\n", py_compact(value))
}

fn pretty_json(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(&sort_keys(value)).unwrap_or_default()
    )
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn count(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn lower_bool(value: &Value, key: &str) -> String {
    match value.get(key).and_then(Value::as_bool) {
        Some(true) => "true".to_string(),
        _ => "false".to_string(),
    }
}

/// `recover-absent-agent-rows`: header, outcome rows, recovery note.
/// Returns the text and whether any row refused (exit status 1).
pub(crate) fn agent_rows(value: &Value, format_json: bool) -> (String, bool) {
    let counts = value.get("counts").cloned().unwrap_or_else(|| json!({}));
    let refused = count(&counts, "refused") > 0;
    if format_json {
        return (compact_json(value), refused);
    }
    let seconds = value.get("seconds").cloned().unwrap_or_else(|| json!(0));
    let mut out = format!(
        "mode={} requested={} would_recover={} would_resume={} recovered={} already_recovered={} refused={} seconds={}\n",
        text(value, "mode"),
        count(value, "requested"),
        count(&counts, "would-recover"),
        count(&counts, "would-resume"),
        count(&counts, "recovered"),
        count(&counts, "already-recovered"),
        count(&counts, "refused"),
        seconds,
    );
    if let Some(rows) = value.get("rows").and_then(Value::as_array) {
        for row in rows {
            let outcome = text(row, "outcome");
            let mut line = format!(
                "{}: {} generation={}",
                outcome.to_uppercase(),
                text(row, "slot"),
                count(row, "generation"),
            );
            if outcome == "refused" {
                line.push_str(&format!(" reason={}", text(row, "reason")));
            }
            out.push_str(&line);
            out.push('\n');
            if let Some(lines) = row.get("output").and_then(Value::as_array) {
                for line in lines {
                    out.push_str(&format!("  {}\n", line.as_str().unwrap_or_default()));
                }
            }
        }
    }
    if value.get("recovery_required").and_then(Value::as_bool) == Some(true) {
        out.push_str("RECOVERY REQUIRED: run 'wrkslots recover' before another recovery\n");
    }
    (out, refused)
}

/// `recover-absent-validate-rows`: ROW lines, or the indented payload.
pub(crate) fn validate_rows(value: &Value, format_json: bool) -> String {
    if format_json {
        return pretty_json(value);
    }
    let mut out = String::new();
    if let Some(rows) = value.get("rows").and_then(Value::as_array) {
        for row in rows {
            out.push_str(&format!(
                "ROW machine={} slot={} generation={} outcome={} detail={}\n",
                text(row, "machine"),
                text(row, "slot"),
                count(row, "generation"),
                text(row, "outcome"),
                serde_json::to_string(&row.get("detail").cloned().unwrap_or_default())
                    .unwrap_or_default(),
            ));
        }
    }
    out
}

/// `recover-ownerless-validate-batch`: counts header, RETAINED rows.
pub(crate) fn ownerless_recover(value: &Value, format_json: bool) -> String {
    if format_json {
        return compact_json(value);
    }
    let removed = value
        .get("removed")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let retained = value
        .get("retained")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = format!(
        "requested={} removed={} retained={} process_censuses={}\n",
        count(value, "requested"),
        removed,
        retained.len(),
        count(value, "process_censuses"),
    );
    for row in &retained {
        out.push_str(&format!(
            "RETAINED: {} blocks_entry={} reason={}\n",
            text(row, "checkout"),
            lower_bool(row, "blocks_entry"),
            text(row, "reason"),
        ));
    }
    out
}

/// `classify-ownerless-validate-batch`: counts header, state rows.
pub(crate) fn ownerless_classify(value: &Value, format_json: bool) -> String {
    if format_json {
        return compact_json(value);
    }
    let results = value
        .get("classifications")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let blocking = results
        .iter()
        .filter(|row| row.get("blocks_entry").and_then(Value::as_bool) == Some(true))
        .count();
    let mut out = format!(
        "requested={} blocking={} nonblocking={} process_censuses={}\n",
        count(value, "requested"),
        blocking,
        results.len().saturating_sub(blocking),
        count(value, "process_censuses"),
    );
    for row in &results {
        out.push_str(&format!(
            "{}: {} blocks_entry={} reason={}\n",
            text(row, "state"),
            text(row, "checkout"),
            lower_bool(row, "blocks_entry"),
            text(row, "reason"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ownerless_renderers_match_python_lines() {
        let recover = json!({
            "requested": 2, "process_censuses": 3,
            "removed": [{"checkout": "a"}],
            "retained": [{"checkout": "b", "blocks_entry": true, "reason": "busy"}],
        });
        assert_eq!(
            ownerless_recover(&recover, false),
            "requested=2 removed=1 retained=1 process_censuses=3\nRETAINED: b blocks_entry=true reason=busy\n"
        );
        let classify = json!({
            "requested": 2, "process_censuses": 1,
            "classifications": [
                {"state": "blocked", "checkout": "a", "blocks_entry": true, "reason": "live"},
                {"state": "free", "checkout": "b", "blocks_entry": false, "reason": "gone"},
            ],
        });
        assert_eq!(
            ownerless_classify(&classify, false),
            "requested=2 blocking=1 nonblocking=1 process_censuses=1\nblocked: a blocks_entry=true reason=live\nfree: b blocks_entry=false reason=gone\n"
        );
    }
}
