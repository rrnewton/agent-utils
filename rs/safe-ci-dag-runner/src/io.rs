//! Strict JSON and YAML serialization for [`crate::DagConfig`].

// Canonical JSON (de)serialization for a [`DagConfig`].
//
// Direct port of `py/safe_ci_dag_runner/io.py`. This is the on-disk / interchange form the
// CLI loads via `--dag FILE` and the shared fixture format for the cross-language differential
// tests. Parsing is STRICT and fails loudly on a malformed document ([`DagJsonError`]), never
// silently defaulting a wrong-typed field.
//
// Serialization ([`dag_to_json`]) is hand-rolled to reproduce Python's
// `json.dumps(indent=2, ensure_ascii=False)` byte-for-byte: the fixed non-alphabetical key
// order, 2-space indent, inline empty `{}` / `[]`, and float formatting that matches CPython's
// `repr(float)` for every finite value — fixed notation (`1.0`, `90.0`, `1.25`, `0.0001`) and
// scientific notation alike (`1e+20`, `1e-07`, `1.5e+16`; see [`json_float`], which documents the
// single negligible exact-halfway-tie residual). Non-ASCII
// characters are emitted as raw UTF-8 on BOTH sides (Python passes `ensure_ascii=False`), and
// both escape the same JSON control set (`"`, `\`, `\n`, `\t`, `\r`, `\b`, `\f`, and `\u00XX` for
// other code points < 0x20), so the `json` output is byte-identical for every input — including
// multi-line / quote / backslash / unicode descriptions. serde_json is built with the
// `float_roundtrip` feature so a float literal PARSES to the same `f64` CPython's `json.loads`
// produces, which byte-identical re-emission depends on.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use crate::model::{
    write_domain_violations, DagConfig, ResourceHint, Step, StepClass, WriteDomainGuarantee,
    WriteDomainPolicy, DEFAULT_JOBS_FLAG, DEFAULT_STEP_TIMEOUT,
};

const DEFAULT_MEM_CAP_FLOOR: i64 = 8 * 1024 * 1024 * 1024;

/// Error returned when a DAG document violates the interchange schema.
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

// Reject bools masquerading as ints and floats that are not exact integers (Python's
// `isinstance(val, bool) or not isinstance(val, int)`).
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

fn present_str_list(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, DagJsonError> {
    if !m.contains_key(key) {
        return Ok(None);
    }
    match m.get(key) {
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    _ => return Err(err(format!("field '{key}' must contain only strings"))),
                }
            }
            Ok(Some(out))
        }
        _ => Err(err(format!("field '{key}' must be a list of strings"))),
    }
}

fn write_domain_policy(value: Option<&Value>) -> Result<WriteDomainPolicy, DagJsonError> {
    let Some(value) = value else {
        return Ok(WriteDomainPolicy::default());
    };
    if value.is_null() {
        return Ok(WriteDomainPolicy::default());
    }
    let obj = as_obj(value, "write_domain_policy")?;
    let allowed = opt_str_list(obj, "allowed_domains")?;
    let mut allowed_domains = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for name in allowed {
        if name.is_empty() {
            return Err(err(
                "write_domain_policy.allowed_domains must not contain empty names",
            ));
        }
        if !allowed_domains.insert(name.clone()) {
            duplicates.insert(name);
        }
    }
    if !duplicates.is_empty() {
        return Err(err(format!(
            "write_domain_policy.allowed_domains contains duplicates: {}",
            duplicates.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(WriteDomainPolicy {
        require_explicit: opt_bool(obj, "require_explicit", false)?,
        allowed_domains,
    })
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

/// Parse a JSON DAG document with strict field and type validation.
pub fn dag_from_json(text: &str) -> Result<DagConfig, DagJsonError> {
    let raw: Value = serde_json::from_str(text).map_err(|e| err(format!("invalid JSON: {e}")))?;
    dag_from_value(&raw)
}

/// Parse a DAG YAML document into a [`DagConfig`]. YAML is ISOMORPHIC to the JSON schema: it is
/// deserialized into the same `serde_json::Value` intermediate and funneled through
/// [`dag_from_value`], so JSON and YAML construct the model identically. Returns [`DagJsonError`]
/// on any malformed field, mirroring the JSON strictness.
pub fn dag_from_yaml(text: &str) -> Result<DagConfig, DagJsonError> {
    let raw: Value = serde_norway::from_str(text).map_err(|e| err(format!("invalid YAML: {e}")))?;
    dag_from_value(&raw)
}

/// Construct a [`DagConfig`] from an already-parsed JSON/YAML value tree — the shared strict
/// narrowing behind both [`dag_from_json`] and [`dag_from_yaml`], so the two syntaxes cannot
/// drift in how they build the model.
pub fn dag_from_value(raw: &Value) -> Result<DagConfig, DagJsonError> {
    let doc = as_obj(raw, "<root>")?;
    let default_step_timeout = opt_int(doc, "default_step_timeout", DEFAULT_STEP_TIMEOUT)?;
    let policy = write_domain_policy(doc.get("write_domain_policy"))?;
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
            cpu_timeout: opt_int(sm, "cpu_timeout", 0)?,
            jobs_flag: opt_str_or_none(sm, "jobs_flag")?,
            write_domains: present_str_list(sm, "write_domains")?,
            write_domain_guarantee: match sm.get("write_domain_guarantee") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => Some(
                    WriteDomainGuarantee::from_value(value).ok_or_else(|| {
                        err(format!(
                            "{where_}.write_domain_guarantee: unknown value '{value}'"
                        ))
                    })?,
                ),
                Some(_) => {
                    return Err(err(format!(
                        "{where_}.write_domain_guarantee: field 'write_domain_guarantee' must be a string"
                    )))
                }
            },
        });
    }
    let cfg = DagConfig {
        steps,
        description: opt_str(doc, "description", "")?,
        resource_caps: opt_str_int_map(doc, "resource_caps", "<root>")?,
        mem_cap_factor: opt_float(doc, "mem_cap_factor", 1.25)?,
        mem_cap_floor_bytes: opt_int(doc, "mem_cap_floor_bytes", DEFAULT_MEM_CAP_FLOOR)?,
        outer_mem_safety_factor: opt_float(doc, "outer_mem_safety_factor", 1.0)?,
        default_step_timeout,
        default_jobs_flag: opt_str(doc, "default_jobs_flag", DEFAULT_JOBS_FLAG)?,
        write_domain_policy: policy,
        // SMALL forcing-function default caps for undeclared steps: not parsed from the document
        // (mirrors the Python io parser, which relies on the DagConfig dataclass defaults), so a
        // parsed DAG gets the 1-GiB / 1-core / 10-s floor. Callers override via the DagConfig fields.
        ..DagConfig::default()
    };
    let violations = write_domain_violations(&cfg);
    if !violations.is_empty() {
        return Err(err(format!(
            "write-domain policy refused DAG before execution: {}",
            violations.join("; ")
        )));
    }
    Ok(cfg)
}

// --------------------------------------------------------------------------- serialization

// Quote and escape a string the way Python's `json.dumps` does for ASCII input.
pub(crate) fn json_str(s: &str) -> String {
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

// Format a float exactly like Python's `repr(float)` / `json.dumps` — e.g. `1.0`, `90.0`,
// `1.25`, `0.0001`, but also the scientific forms `1e+20`, `1e-07`, `1.5e+16`, `1e+100`.
//
// Rust's default float formatting gives the same shortest round-trip *digits* as CPython, but
// `{:?}` formats the exponent differently (`1e20` / `1e-7`, no sign, no zero-pad), which broke
// byte-for-byte parity for any float Python renders in scientific notation. This reproduces
// CPython's `float_repr`:
//  * the shortest round-trip digit string (taken from Rust's `{}` / Display, which is shortest and
//    always fixed-point — `{:e}` is NOT shortest and must not be used),
//  * fixed vs. scientific chosen by CPython's rule (scientific iff the decimal point position is
//    `<= -4` or `> 16`),
//  * a signed, at-least-two-digit exponent (`e+NN` / `e-NN`),
//  * and a trailing `.0` on any integer-valued float in fixed notation.
//
// This matches CPython for every finite `f64` EXCEPT one negligible residual: when a value is
// EXACTLY halfway between two equally-short decimals, CPython rounds the tie to even while Rust's
// Display rounds it half-up (e.g. `-887777373534812.25` -> CPython `...812.2`, here `...812.3`).
// Detecting an exact tie needs arbitrary-precision arithmetic (a dependency this crate avoids),
// and such values are unreachable for realistic config floats; see the pinning unit test.
//
// Non-finite inputs cannot occur (the reader rejects them and the model never holds them); the
// guards below are a defensive fallback that never executes on real data.
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
    // Rust's Display (`{}`) is the shortest round-trip decimal in PLAIN fixed-point form — never
    // an exponent — e.g. "100", "1.25", "0.0001", "100000000000000000000". (Rust's `{:e}` is NOT
    // shortest and can be a ULP off, so it must not be used here.) Decompose Display into a
    // canonical (sign, significant-digit string, decimal-point position `decpt`) triple: `decpt`
    // is the count of digits to the left of the point, i.e. value == <digits> with the point after
    // `decpt` of them. Then re-render with CPython's repr rules.
    let disp = format!("{f}");
    let (neg, body) = match disp.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, disp.as_str()),
    };
    let (int_part, frac_part) = match body.split_once('.') {
        Some((i, fr)) => (i, fr),
        None => (body, ""),
    };
    let mut all = String::with_capacity(int_part.len() + frac_part.len());
    all.push_str(int_part);
    all.push_str(frac_part);

    // Strip leading zeros (shifting decpt) and trailing zeros to get the significant digits.
    let first_nonzero = all.bytes().position(|b| b != b'0');
    let (digits, decpt): (&str, i32) = match first_nonzero {
        // The value is exactly zero: CPython renders it as "0.0" / "-0.0".
        None => {
            return if neg {
                "-0.0".to_string()
            } else {
                "0.0".to_string()
            }
        }
        Some(lead) => {
            let last_nonzero = all.bytes().rposition(|b| b != b'0').unwrap_or(lead);
            (
                &all[lead..=last_nonzero],
                int_part.len() as i32 - lead as i32,
            )
        }
    };

    // CPython uses scientific notation for repr iff decpt <= -4 or decpt > 16.
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        // Scientific: d[.ddd]e{+|-}NN, exponent = decpt - 1, magnitude zero-padded to >= 2 digits.
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let exp_val = decpt - 1;
        if exp_val < 0 {
            out.push('-');
        } else {
            out.push('+');
        }
        out.push_str(&format!("{:02}", exp_val.unsigned_abs()));
    } else if decpt <= 0 {
        // 0.000ddd
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(digits);
    } else if (decpt as usize) >= digits.len() {
        // ddd000.0  (integer-valued: pad with zeros, add the trailing ".0")
        out.push_str(digits);
        for _ in 0..(decpt as usize - digits.len()) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        // dd.ddd  (decimal point inside the digit run)
        let cut = decpt as usize;
        out.push_str(&digits[..cut]);
        out.push('.');
        out.push_str(&digits[cut..]);
    }
    out
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
    // Emitted only when set, so existing DAGs (all cpu_timeout=0) stay byte-for-byte
    // unchanged and absence parses back to 0. Mirrors the Python serializer exactly.
    if step.cpu_timeout != 0 {
        s.push_str(&key);
        s.push_str(&format!("\"cpu_timeout\": {},\n", step.cpu_timeout));
    }
    s.push_str(&key);
    s.push_str(&format!(
        "\"jobs_flag\": {},\n",
        opt_str_json(step.jobs_flag.as_deref())
    ));
    s.push_str(&key);
    s.push_str("\"hint\": ");
    emit_hint(s, &step.hint, base + 2);
    if step.write_domains.is_some() || step.write_domain_guarantee.is_some() {
        s.push_str(",\n");
    } else {
        s.push('\n');
    }
    if let Some(domains) = &step.write_domains {
        s.push_str(&key);
        s.push_str("\"write_domains\": ");
        emit_str_list(s, domains, base + 2);
        if step.write_domain_guarantee.is_some() {
            s.push_str(",\n");
        } else {
            s.push('\n');
        }
    }
    if let Some(guarantee) = step.write_domain_guarantee {
        s.push_str(&key);
        s.push_str(&format!(
            "\"write_domain_guarantee\": {}\n",
            json_str(guarantee.value())
        ));
    }
    s.push_str(&ind);
    s.push('}');
}

/// Serialize a DAG configuration to canonical two-space-indented JSON.
///
/// The returned string has no trailing newline.
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
    let policy_active = cfg.write_domain_policy.require_explicit
        || !cfg.write_domain_policy.allowed_domains.is_empty();
    if cfg.steps.is_empty() {
        s.push_str(if policy_active {
            "  \"steps\": [],\n"
        } else {
            "  \"steps\": []\n"
        });
    } else {
        s.push_str("  \"steps\": [\n");
        let n = cfg.steps.len();
        for (i, step) in cfg.steps.iter().enumerate() {
            emit_step(&mut s, step, 4);
            s.push_str(if i + 1 < n { ",\n" } else { "\n" });
        }
        s.push_str(if policy_active { "  ],\n" } else { "  ]\n" });
    }
    if policy_active {
        s.push_str("  \"write_domain_policy\": {\n");
        s.push_str(&format!(
            "    \"require_explicit\": {},\n",
            cfg.write_domain_policy.require_explicit
        ));
        s.push_str("    \"allowed_domains\": ");
        let domains: Vec<String> = cfg
            .write_domain_policy
            .allowed_domains
            .iter()
            .cloned()
            .collect();
        emit_str_list(&mut s, &domains, 4);
        s.push_str("\n  }\n");
    }
    s.push('}');
    s
}

/// Serialize a DAG configuration to a YAML document.
///
/// The result round-trips through [`dag_from_yaml`] to an equivalent configuration. Serialization
/// uses the canonical JSON value tree so both formats expose the same fields.
pub fn dag_to_yaml(cfg: &DagConfig) -> String {
    let json = dag_to_json(cfg);
    // Canonical JSON produced by dag_to_json is always valid, and a plain JSON value tree always
    // serializes to YAML, so neither step can fail in practice; the invariants are asserted here.
    let value: Value =
        serde_json::from_str(&json).expect("canonical JSON from dag_to_json must re-parse");
    serde_norway::to_string(&value).expect("a JSON value tree always serializes to YAML")
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
    fn yaml_is_isomorphic_to_json() {
        // Adversarial YAML: a literal (|-) block scalar with quotes + unicode, and a QUOTED
        // Norway-problem token ("no"), which must stay the STRING "no" (not the bool false).
        // The lines start at column 0 so the YAML indentation is exactly two spaces.
        let yaml = r#"description: the whole pipeline
resource_caps:
  browser: 1
steps:
  - group: build
    job: app
    desc: compile
    description: |-
      line 1
      line 2 with "quotes" and unicode é☃
    cmd: make build
    hint:
      classification: cpu-bound
      preferred_inner_jobs: 8
  - group: e2e
    job: smoke
    desc: browser
    description: "no"
    cmd: make e2e
    deps: [build.app]
    hint:
      resources:
        browser: 1
"#;
        // The JSON document that must load to the SAME DagConfig.
        let json = r#"{"description": "the whole pipeline", "resource_caps": {"browser": 1},
            "steps": [
            {"group": "build", "job": "app", "desc": "compile",
             "description": "line 1\nline 2 with \"quotes\" and unicode é☃",
             "cmd": "make build",
             "hint": {"classification": "cpu-bound", "preferred_inner_jobs": 8}},
            {"group": "e2e", "job": "smoke", "desc": "browser", "description": "no",
             "cmd": "make e2e", "deps": ["build.app"],
             "hint": {"resources": {"browser": 1}}}]}"#;
        let from_yaml = dag_from_yaml(yaml).unwrap();
        let from_json = dag_from_json(json).unwrap();
        // Isomorphism: identical canonical JSON regardless of input syntax.
        assert_eq!(dag_to_json(&from_yaml), dag_to_json(&from_json));
        // The quoted Norway token stayed a string; the literal block scalar chomped correctly.
        assert_eq!(from_yaml.steps[1].description, "no");
        assert_eq!(
            from_yaml.steps[0].description,
            "line 1\nline 2 with \"quotes\" and unicode é☃"
        );
    }

    #[test]
    fn yaml_emit_round_trips() {
        let cfg = dag_from_json(
            r#"{"description": "d", "steps": [{"group": "g", "job": "j", "desc": "x",
                "description": "multi\nline", "cmd": "true"}]}"#,
        )
        .unwrap();
        // dag_to_yaml output need not match Python, but must round-trip back to the same DagConfig.
        let back = dag_from_yaml(&dag_to_yaml(&cfg)).unwrap();
        assert_eq!(dag_to_json(&cfg), dag_to_json(&back));
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
    fn write_domain_policy_roundtrips_and_refuses_omission() {
        let good = r#"{"steps":[
            {"group":"g","job":"reader","cmd":"true","write_domains":[]},
            {"group":"g","job":"barrier","cmd":"true",
             "write_domains":["shared-cargo-target"],
             "write_domain_guarantee":"immutable-artifact-barrier"},
            {"group":"g","job":"shielded","cmd":"true","deps":["g.barrier"],
             "write_domains":["shared-cargo-target"],
             "write_domain_guarantee":"artifact-barrier-dependent"},
            {"group":"g","job":"writer","cmd":"true",
             "write_domains":["isolated-target"],
             "write_domain_guarantee":"explicitly-isolated"}],
            "write_domain_policy":{"require_explicit":true,
             "allowed_domains":["shared-cargo-target","isolated-target"]}}"#;
        let cfg = dag_from_json(good).unwrap();
        assert_eq!(cfg.steps[0].write_domains, Some(Vec::new()));
        assert_eq!(
            cfg.steps[3].write_domain_guarantee,
            Some(WriteDomainGuarantee::ExplicitlyIsolated)
        );
        let encoded = dag_to_json(&cfg);
        assert_eq!(dag_to_json(&dag_from_json(&encoded).unwrap()), encoded);

        let bad = [
            r#"{"steps":[{"group":"g","job":"j","cmd":"true"}],
                "write_domain_policy":{"require_explicit":true,"allowed_domains":[]}}"#,
            r#"{"steps":[{"group":"g","job":"j","cmd":"true",
                "write_domains":["typo"],"write_domain_guarantee":"artifact-producer"}],
                "write_domain_policy":{"require_explicit":true,
                "allowed_domains":["shared-cargo-target"]}}"#,
            r#"{"steps":[{"group":"g","job":"j","cmd":"true",
                "write_domains":["shared-cargo-target","shared-cargo-target"]}],
                "write_domain_policy":{"require_explicit":true,
                "allowed_domains":["shared-cargo-target"]}}"#,
            r#"{"steps":[{"group":"g","job":"j","cmd":"true",
                "write_domains":["shared-cargo-target"],
                "write_domain_guarantee":"artifact-barrier-dependent"}],
                "write_domain_policy":{"require_explicit":true,
                "allowed_domains":["shared-cargo-target"]}}"#,
        ];
        for doc in bad {
            let error = dag_from_json(doc).unwrap_err().to_string();
            assert!(
                error.contains("write-domain policy refused"),
                "unexpected refusal: {error}"
            );
        }
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

    #[test]
    fn json_float_matches_python_repr() {
        // (value, exact CPython repr(float) == json.dumps(float) output)
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (1.0, "1.0"),
            (90.0, "90.0"),
            (1.25, "1.25"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1073741824.0, "1073741824.0"),
            (100.0, "100.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e20, "1e+20"),
            (1e100, "1e+100"),
            (1e-7, "1e-07"),
            (1.5e16, "1.5e+16"),
            (1.2345678901234568e17, "1.2345678901234568e+17"),
            (3.5e-4, "0.00035"),
            (-3.5e-4, "-0.00035"),
        ];
        for (v, want) in cases {
            assert_eq!(&json_float(*v), want, "json_float({v}) mismatch");
        }
        // -0.0 preserves the sign, like CPython's repr(-0.0).
        assert_eq!(json_float(-0.0), "-0.0");
    }

    #[test]
    fn float_literal_round_trips_bit_exactly() {
        // float_roundtrip: a scientific literal must parse to the exact f64 and re-emit identically.
        let doc = r#"{"mem_cap_factor": 2.0951218323850843e-171, "steps": []}"#;
        let cfg = dag_from_json(doc).unwrap();
        assert_eq!(cfg.mem_cap_factor, 2.0951218323850843e-171);
        assert!(dag_to_json(&cfg).contains("\"mem_cap_factor\": 2.0951218323850843e-171"));
    }

    #[test]
    fn json_float_exact_halfway_tie_is_a_known_residual() {
        // The ONE documented float parity gap. -887777373534812.25 is EXACTLY halfway between the
        // two equally-short decimals ...812.2 and ...812.3. CPython's repr rounds such a tie to
        // EVEN (emits ...812.2); Rust's Display rounds it half-up (...812.3), which json_float
        // inherits. Correctly detecting an exact tie needs arbitrary-precision arithmetic (a
        // dependency this crate deliberately avoids), and such a value is essentially unreachable
        // for real config data (small, few-decimal floats). This test PINS the current behavior so
        // the residual is visible and any future change is caught.
        assert_eq!(json_float(-887777373534812.2), "-887777373534812.3");
    }
}
