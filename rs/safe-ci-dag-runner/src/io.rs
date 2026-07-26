//! Canonical JSON (de)serialization for a [`DagConfig`].
//!
//! Direct port of `py/safe_ci_dag_runner/io.py`. This is the on-disk / interchange form the
//! CLI loads via `--dag FILE` and the shared fixture format for the cross-language differential
//! tests. Parsing is STRICT and fails loudly on a malformed document ([`DagJsonError`]), never
//! silently defaulting a wrong-typed field.
//!
//! Serialization ([`dag_to_json`]) is hand-rolled to reproduce Python's
//! `json.dumps(indent=2, ensure_ascii=False)` byte-for-byte: the fixed non-alphabetical key
//! order, 2-space indent, inline empty `{}` / `[]`, and float formatting (`1.0`, `90.0`,
//! `1.25`). Non-ASCII characters are emitted as raw UTF-8 on BOTH sides (Python passes
//! `ensure_ascii=False`), and both escape the same JSON control set (`"`, `\`, `\n`, `\t`,
//! `\r`, `\b`, `\f`, and `\u00XX` for other code points < 0x20), so the `json` output is
//! byte-identical for every input — including multi-line / quote / backslash / unicode
//! descriptions.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use crate::model::{
    DagConfig, ResourceHint, Step, StepClass, DEFAULT_JOBS_FLAG, DEFAULT_STEP_TIMEOUT,
};

const DEFAULT_MEM_CAP_FLOOR: i64 = 8 * 1024 * 1024 * 1024;

/// Raised when a DAG JSON document is malformed (mirrors Python's `DagJsonError`).
#[derive(Debug, Clone)]
pub struct DagJsonError(pub String);

impl fmt::Display for DagJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DagJsonError {}

fn err(msg: impl Into<String>) -> DagJsonError {
    DagJsonError(msg.into())
}

// --- typed narrowing helpers (serde_json::Value; narrow explicitly, mirror Python strictness) ---

fn as_obj<'a>(
    value: &'a Value,
    where_: &str,
) -> Result<&'a serde_json::Map<String, Value>, DagJsonError> {
    match value {
        Value::Object(m) => Ok(m),
        other => Err(err(format!(
            "{where_}: expected an object, got {}",
            type_name(other)
        ))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Reject bools masquerading as ints and floats that are not exact integers (Python's
/// `isinstance(val, bool) or not isinstance(val, int)`).
fn number_as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(_) => None,
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn req_str(
    m: &serde_json::Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<String, DagJsonError> {
    match m.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(err(format!("{where_}: field '{key}' must be a string"))),
    }
}

fn opt_str(
    m: &serde_json::Map<String, Value>,
    key: &str,
    default: &str,
) -> Result<String, DagJsonError> {
    match m.get(key) {
        None => Ok(default.to_string()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(err(format!("field '{key}' must be a string"))),
    }
}

fn opt_int(
    m: &serde_json::Map<String, Value>,
    key: &str,
    default: i64,
) -> Result<i64, DagJsonError> {
    match m.get(key) {
        None => Ok(default),
        Some(v) => number_as_int(v).ok_or_else(|| err(format!("field '{key}' must be an integer"))),
    }
}

fn opt_str_or_none(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(err(format!("field '{key}' must be a string or null"))),
    }
}

fn opt_int_or_none(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<i64>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => number_as_int(v)
            .map(Some)
            .ok_or_else(|| err(format!("field '{key}' must be an integer or null"))),
    }
}

fn opt_float(
    m: &serde_json::Map<String, Value>,
    key: &str,
    default: f64,
) -> Result<f64, DagJsonError> {
    match m.get(key) {
        None => Ok(default),
        Some(Value::Bool(_)) => Err(err(format!("field '{key}' must be a number"))),
        Some(Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| err(format!("field '{key}' must be a number"))),
        Some(_) => Err(err(format!("field '{key}' must be a number"))),
    }
}

fn opt_float_or_none(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(_)) => Err(err(format!("field '{key}' must be a number or null"))),
        Some(Value::Number(n)) => n
            .as_f64()
            .map(Some)
            .ok_or_else(|| err(format!("field '{key}' must be a number or null"))),
        Some(_) => Err(err(format!("field '{key}' must be a number or null"))),
    }
}

fn opt_bool(
    m: &serde_json::Map<String, Value>,
    key: &str,
    default: bool,
) -> Result<bool, DagJsonError> {
    match m.get(key) {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(err(format!("field '{key}' must be a boolean"))),
    }
}

fn opt_str_list(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    _ => return Err(err(format!("field '{key}' must contain only strings"))),
                }
            }
            Ok(out)
        }
        Some(_) => Err(err(format!("field '{key}' must be a list of strings"))),
    }
}

fn opt_str_int_map(
    m: &serde_json::Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<BTreeMap<String, i64>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(v) => {
            let obj = as_obj(v, &format!("{where_}.{key}"))?;
            let mut out = BTreeMap::new();
            for (name, num) in obj {
                let n = number_as_int(num)
                    .ok_or_else(|| err(format!("{where_}.{key}.{name}: must be an integer")))?;
                out.insert(name.clone(), n);
            }
            Ok(out)
        }
    }
}

fn opt_str_str_map(
    m: &serde_json::Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<BTreeMap<String, String>, DagJsonError> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(v) => {
            let obj = as_obj(v, &format!("{where_}.{key}"))?;
            let mut out = BTreeMap::new();
            for (name, text) in obj {
                match text {
                    Value::String(s) => {
                        out.insert(name.clone(), s.clone());
                    }
                    _ => return Err(err(format!("{where_}.{key}.{name}: must be a string"))),
                }
            }
            Ok(out)
        }
    }
}

fn hint_from(value: Option<&Value>, where_: &str) -> Result<ResourceHint, DagJsonError> {
    let value = match value {
        None | Some(Value::Null) => return Ok(ResourceHint::default()),
        Some(v) => v,
    };
    let obj = as_obj(value, where_)?;
    let cls_name = opt_str(obj, "classification", StepClass::Light.value())?;
    let classification = StepClass::from_value(&cls_name).ok_or_else(|| {
        err(format!(
            "{where_}.classification: unknown value '{cls_name}'"
        ))
    })?;
    Ok(ResourceHint {
        resources: opt_str_int_map(obj, "resources", where_)?,
        est_duration_s: opt_float(obj, "est_duration_s", 0.0)?,
        rss_baseline_bytes: opt_int_or_none(obj, "rss_baseline_bytes")?,
        hard_mem_max_bytes: opt_int_or_none(obj, "hard_mem_max_bytes")?,
        classification,
        preferred_inner_jobs: opt_int_or_none(obj, "preferred_inner_jobs")?,
        measured_effective_cores: opt_float_or_none(obj, "measured_effective_cores")?,
        measured_cpu_utilization: opt_float_or_none(obj, "measured_cpu_utilization")?,
    })
}

/// Parse a DAG JSON document into a [`DagConfig`]. Returns [`DagJsonError`] on any malformed
/// field, mirroring the Python `dag_from_json` strictness.
pub fn dag_from_json(text: &str) -> Result<DagConfig, DagJsonError> {
    let raw: Value = serde_json::from_str(text).map_err(|e| err(format!("invalid JSON: {e}")))?;
    let doc = as_obj(&raw, "<root>")?;
    let default_step_timeout = opt_int(doc, "default_step_timeout", DEFAULT_STEP_TIMEOUT)?;
    let steps_raw = match doc.get("steps") {
        Some(Value::Array(items)) => items,
        _ => return Err(err("<root>: 'steps' must be a list")),
    };
    let mut steps: Vec<Step> = Vec::with_capacity(steps_raw.len());
    for (i, entry) in steps_raw.iter().enumerate() {
        let where_ = format!("steps[{i}]");
        let sm = as_obj(entry, &where_)?;
        steps.push(Step {
            group: req_str(sm, "group", &where_)?,
            job: req_str(sm, "job", &where_)?,
            desc: opt_str(sm, "desc", "")?,
            description: opt_str(sm, "description", "")?,
            cmd: req_str(sm, "cmd", &where_)?,
            deps: opt_str_list(sm, "deps")?,
            env: opt_str_str_map(sm, "env", &where_)?,
            hint: hint_from(sm.get("hint"), &format!("{where_}.hint"))?,
            networkonly: opt_bool(sm, "networkonly", false)?,
            engine_only: opt_bool(sm, "engine_only", false)?,
            timeout: opt_int(sm, "timeout", default_step_timeout)?,
            jobs_flag: opt_str_or_none(sm, "jobs_flag")?,
        });
    }
    Ok(DagConfig {
        steps,
        description: opt_str(doc, "description", "")?,
        resource_caps: opt_str_int_map(doc, "resource_caps", "<root>")?,
        mem_cap_factor: opt_float(doc, "mem_cap_factor", 1.25)?,
        mem_cap_floor_bytes: opt_int(doc, "mem_cap_floor_bytes", DEFAULT_MEM_CAP_FLOOR)?,
        outer_mem_safety_factor: opt_float(doc, "outer_mem_safety_factor", 1.0)?,
        default_step_timeout,
        default_jobs_flag: opt_str(doc, "default_jobs_flag", DEFAULT_JOBS_FLAG)?,
    })
}

// --------------------------------------------------------------------------- serialization

/// Quote and escape a string the way Python's `json.dumps` does for ASCII input.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Format a float like Python's `repr` / `json.dumps` (e.g. `1.0`, `90.0`, `1.25`). Rust's
/// `{:?}` shortest-round-trip formatting matches for all finite non-scientific values.
fn json_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    format!("{f:?}")
}

fn opt_int_json(v: Option<i64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "null".to_string(),
    }
}

fn opt_str_json(v: Option<&str>) -> String {
    match v {
        Some(s) => json_str(s),
        None => "null".to_string(),
    }
}

fn opt_float_json(v: Option<f64>) -> String {
    match v {
        Some(f) => json_float(f),
        None => "null".to_string(),
    }
}

/// Emit a sorted `str -> int` map at the given base indent (`{}` when empty).
fn emit_int_map(s: &mut String, map: &BTreeMap<String, i64>, base: usize) {
    if map.is_empty() {
        s.push_str("{}");
        return;
    }
    s.push_str("{\n");
    let inner = " ".repeat(base + 2);
    let n = map.len();
    for (i, (k, v)) in map.iter().enumerate() {
        s.push_str(&inner);
        s.push_str(&json_str(k));
        s.push_str(": ");
        s.push_str(&v.to_string());
        s.push_str(if i + 1 < n { ",\n" } else { "\n" });
    }
    s.push_str(&" ".repeat(base));
    s.push('}');
}

/// Emit a sorted `str -> str` map at the given base indent (`{}` when empty).
fn emit_str_map(s: &mut String, map: &BTreeMap<String, String>, base: usize) {
    if map.is_empty() {
        s.push_str("{}");
        return;
    }
    s.push_str("{\n");
    let inner = " ".repeat(base + 2);
    let n = map.len();
    for (i, (k, v)) in map.iter().enumerate() {
        s.push_str(&inner);
        s.push_str(&json_str(k));
        s.push_str(": ");
        s.push_str(&json_str(v));
        s.push_str(if i + 1 < n { ",\n" } else { "\n" });
    }
    s.push_str(&" ".repeat(base));
    s.push('}');
}

/// Emit a string list at the given base indent (`[]` when empty).
fn emit_str_list(s: &mut String, list: &[String], base: usize) {
    if list.is_empty() {
        s.push_str("[]");
        return;
    }
    s.push_str("[\n");
    let inner = " ".repeat(base + 2);
    let n = list.len();
    for (i, item) in list.iter().enumerate() {
        s.push_str(&inner);
        s.push_str(&json_str(item));
        s.push_str(if i + 1 < n { ",\n" } else { "\n" });
    }
    s.push_str(&" ".repeat(base));
    s.push(']');
}

fn emit_hint(s: &mut String, hint: &ResourceHint, base: usize) {
    // base is the indent of the enclosing key; the object's fields sit at base+2.
    let key = " ".repeat(base + 2);
    s.push_str("{\n");
    s.push_str(&key);
    s.push_str("\"resources\": ");
    emit_int_map(s, &hint.resources, base + 2);
    s.push_str(",\n");
    s.push_str(&key);
    s.push_str(&format!(
        "\"est_duration_s\": {},\n",
        json_float(hint.est_duration_s)
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"rss_baseline_bytes\": {},\n",
        opt_int_json(hint.rss_baseline_bytes)
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"hard_mem_max_bytes\": {},\n",
        opt_int_json(hint.hard_mem_max_bytes)
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"classification\": {},\n",
        json_str(hint.classification.value())
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"preferred_inner_jobs\": {},\n",
        opt_int_json(hint.preferred_inner_jobs)
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"measured_effective_cores\": {},\n",
        opt_float_json(hint.measured_effective_cores)
    ));
    s.push_str(&key);
    s.push_str(&format!(
        "\"measured_cpu_utilization\": {}\n",
        opt_float_json(hint.measured_cpu_utilization)
    ));
    s.push_str(&" ".repeat(base));
    s.push('}');
}

fn emit_step(s: &mut String, step: &Step, base: usize) {
    let ind = " ".repeat(base);
    let key = " ".repeat(base + 2);
    s.push_str(&ind);
    s.push_str("{\n");
    s.push_str(&key);
    s.push_str(&format!("\"group\": {},\n", json_str(&step.group)));
    s.push_str(&key);
    s.push_str(&format!("\"job\": {},\n", json_str(&step.job)));
    s.push_str(&key);
    s.push_str(&format!("\"desc\": {},\n", json_str(&step.desc)));
    s.push_str(&key);
    s.push_str(&format!(
        "\"description\": {},\n",
        json_str(&step.description)
    ));
    s.push_str(&key);
    s.push_str(&format!("\"cmd\": {},\n", json_str(&step.cmd)));
    s.push_str(&key);
    s.push_str("\"deps\": ");
    emit_str_list(s, &step.deps, base + 2);
    s.push_str(",\n");
    s.push_str(&key);
    s.push_str("\"env\": ");
    emit_str_map(s, &step.env, base + 2);
    s.push_str(",\n");
    s.push_str(&key);
    s.push_str(&format!("\"networkonly\": {},\n", step.networkonly));
    s.push_str(&key);
    s.push_str(&format!("\"engine_only\": {},\n", step.engine_only));
    s.push_str(&key);
    s.push_str(&format!("\"timeout\": {},\n", step.timeout));
    s.push_str(&key);
    s.push_str(&format!(
        "\"jobs_flag\": {},\n",
        opt_str_json(step.jobs_flag.as_deref())
    ));
    s.push_str(&key);
    s.push_str("\"hint\": ");
    emit_hint(s, &step.hint, base + 2);
    s.push('\n');
    s.push_str(&ind);
    s.push('}');
}

/// Serialize a [`DagConfig`] to canonical, deterministic JSON (2-space indent), byte-identical
/// to Python's `dag_to_json` for ASCII input. No trailing newline (the CLI's print adds one).
pub fn dag_to_json(cfg: &DagConfig) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!(
        "  \"description\": {},\n",
        json_str(&cfg.description)
    ));
    s.push_str("  \"resource_caps\": ");
    emit_int_map(&mut s, &cfg.resource_caps, 2);
    s.push_str(",\n");
    s.push_str(&format!(
        "  \"mem_cap_factor\": {},\n",
        json_float(cfg.mem_cap_factor)
    ));
    s.push_str(&format!(
        "  \"mem_cap_floor_bytes\": {},\n",
        cfg.mem_cap_floor_bytes
    ));
    s.push_str(&format!(
        "  \"outer_mem_safety_factor\": {},\n",
        json_float(cfg.outer_mem_safety_factor)
    ));
    s.push_str(&format!(
        "  \"default_step_timeout\": {},\n",
        cfg.default_step_timeout
    ));
    s.push_str(&format!(
        "  \"default_jobs_flag\": {},\n",
        json_str(&cfg.default_jobs_flag)
    ));
    if cfg.steps.is_empty() {
        s.push_str("  \"steps\": []\n");
    } else {
        s.push_str("  \"steps\": [\n");
        let n = cfg.steps.len();
        for (i, step) in cfg.steps.iter().enumerate() {
            emit_step(&mut s, step, 4);
            s.push_str(if i + 1 < n { ",\n" } else { "\n" });
        }
        s.push_str("  ]\n");
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_stable() {
        let doc = r#"{"description": "the whole pipeline", "resource_caps": {"browser": 2},
            "mem_cap_factor": 1.25,
            "outer_mem_safety_factor": 1.1, "default_jobs_flag": "--jobs=", "steps": [
            {"group": "build", "job": "app", "desc": "compile",
             "description": "line 1\nline 2 with \"quotes\" and \\backslash\\ and unicode é☃",
             "cmd": "make build",
             "jobs_flag": "-j%d",
             "hint": {"est_duration_s": 90, "rss_baseline_bytes": 5368709120,
                      "classification": "cpu-bound", "preferred_inner_jobs": 8}},
            {"group": "e2e", "job": "smoke", "desc": "browser", "cmd": "make e2e",
             "deps": ["build.app"], "env": {"HEADLESS": "1"},
             "hint": {"resources": {"browser": 1}, "classification": "latency-bound"}}]}"#;
        let cfg = dag_from_json(doc).unwrap();
        let once = dag_to_json(&cfg);
        let twice = dag_to_json(&dag_from_json(&once).unwrap());
        assert_eq!(once, twice, "canonical JSON is a fixed point");
        let back = dag_from_json(&once).unwrap();
        assert_eq!(
            back.steps.iter().map(|s| s.tag()).collect::<Vec<_>>(),
            vec!["build.app", "e2e.smoke"]
        );
        assert_eq!(back.description, "the whole pipeline");
        assert_eq!(
            back.steps[0].description,
            "line 1\nline 2 with \"quotes\" and \\backslash\\ and unicode é☃"
        );
        assert_eq!(back.steps[1].description, "");
        assert_eq!(back.resource_caps.get("browser"), Some(&2));
        assert_eq!(back.steps[0].hint.classification, StepClass::CpuBound);
        assert_eq!(back.steps[0].hint.rss_baseline_bytes, Some(5368709120));
        assert_eq!(back.steps[0].jobs_flag.as_deref(), Some("-j%d"));
        assert_eq!(back.steps[1].jobs_flag, None);
        assert_eq!(back.default_jobs_flag, "--jobs=");
        assert_eq!(back.steps[1].hint.resources.get("browser"), Some(&1));
        assert_eq!(back.steps[1].env.get("HEADLESS"), Some(&"1".to_string()));
    }

    #[test]
    fn minimal_document_defaults() {
        let cfg =
            dag_from_json(r#"{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}"#).unwrap();
        let step = &cfg.steps[0];
        assert_eq!(step.tag(), "g.j");
        assert_eq!(step.desc, "");
        assert_eq!(step.description, "");
        assert_eq!(cfg.description, "");
        assert!(step.deps.is_empty());
        assert!(step.env.is_empty());
        assert_eq!(step.timeout, 1800);
        assert_eq!(step.hint.classification, StepClass::Light);
        assert_eq!(step.jobs_flag, None);
        assert!(cfg.resource_caps.is_empty());
        assert_eq!(cfg.mem_cap_factor, 1.25);
        assert_eq!(cfg.default_jobs_flag, "-j");
    }

    #[test]
    fn default_step_timeout_applied() {
        let doc = r#"{"default_step_timeout": 42, "steps": [
            {"group": "g", "job": "a", "cmd": "true"},
            {"group": "g", "job": "b", "cmd": "true", "timeout": 7}]}"#;
        let cfg = dag_from_json(doc).unwrap();
        let by_tag = cfg.by_tag();
        assert_eq!(by_tag["g.a"].timeout, 42);
        assert_eq!(by_tag["g.b"].timeout, 7);
        assert_eq!(cfg.default_step_timeout, 42);
    }

    #[test]
    fn strict_parse_errors() {
        let bad = [
            "not json at all",
            "[]",
            r#"{"steps": "not a list"}"#,
            r#"{"steps": [{"job": "j", "cmd": "c"}]}"#,
            r#"{"steps": [{"group": "g", "job": "j"}]}"#,
            r#"{"steps": [{"group": "g", "job": "j", "cmd": "c", "timeout": "x"}]}"#,
            r#"{"steps": [{"group": "g", "job": "j", "cmd": "c", "deps": [1]}]}"#,
            r#"{"steps": [{"group": "g", "job": "j", "cmd": "c", "hint": {"classification": "nope"}}]}"#,
            r#"{"steps": [{"group": "g", "job": "j", "cmd": "c", "hint": {"resources": {"x": "y"}}}]}"#,
        ];
        for doc in bad {
            assert!(dag_from_json(doc).is_err(), "expected error for: {doc}");
        }
    }
}
