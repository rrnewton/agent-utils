//! Strict JSON and YAML serialization for [`crate::DagConfig`].

// Canonical JSON (de)serialization for a [`DagConfig`].
//
// Direct port of `py/dagrun/io.py`. This is the on-disk / interchange form the CLI loads via
// `--dag FILE` and the shared fixture format for the cross-language differential tests.
// Parsing is STRICT and fails loudly on a malformed document ([`DagJsonError`]), never
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
    graph_structure_violations, resolve_jobs_env, validate_cmdtype_config, write_domain_violations,
    CmdType, DagConfig, DagManifest, IntentionalSkipReason, ResourceHint, Step, StepClass,
    WriteDomainGuarantee, WriteDomainPolicy, DEFAULT_JOBS_FLAG,
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

/// Refuse an `explains` declaration that cannot mean what it says.
///
/// `explains` buys a step an exemption from eager-exit cancellation, so a declaration that is
/// quietly wrong is worse than one that is missing: the step looks protected in the document and
/// is reaped anyway, or protects itself for a reason nobody can audit.
///
/// UNKNOWN TAG names a node that does not exist, so the exemption can never trigger -- the
/// misspelling case, silent without a check. SELF-REFERENCE would let any step opt itself out of
/// cancellation with one self-naming line, which is the blanket opt-out this relationship exists
/// to avoid. A CYCLE (A explains B explains A, directly or through a chain) makes each member
/// shield the next, so the whole cycle becomes uncancellable and eager-exit silently stops
/// applying to it, with every individual line still looking reasonable.
///
/// The two editions of this crate must refuse the same documents.
fn refuse_unusable_explains(steps: &[Step]) -> Result<(), DagJsonError> {
    let by_tag: std::collections::BTreeMap<String, &Step> =
        steps.iter().map(|s| (s.tag(), s)).collect();
    let mut problems: std::collections::BTreeSet<String> = Default::default();
    for step in steps {
        let tag = step.tag();
        for target in &step.explains {
            if *target == tag {
                problems.insert(format!(
                    "step {tag}: explains itself; a step cannot diagnose its own failure"
                ));
            } else if !by_tag.contains_key(target) {
                problems.insert(format!("step {tag}: explains unknown node '{target}'"));
            }
        }
    }
    if !problems.is_empty() {
        let joined: Vec<String> = problems.into_iter().collect();
        return Err(err(joined.join("; ")));
    }

    // Iterative three-colour DFS over the explains relation, so a deep chain cannot blow the
    // stack and turn a validation error into a crash.
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        White,
        Grey,
        Black,
    }
    let mut colour: std::collections::BTreeMap<&str, Colour> =
        by_tag.keys().map(|k| (k.as_str(), Colour::White)).collect();
    for root in by_tag.keys() {
        if colour[root.as_str()] != Colour::White {
            continue;
        }
        let mut stack: Vec<(&str, bool)> = vec![(root.as_str(), false)];
        let mut path: Vec<&str> = Vec::new();
        while let Some((tag, leaving)) = stack.pop() {
            if leaving {
                colour.insert(tag, Colour::Black);
                path.pop();
                continue;
            }
            match colour[tag] {
                Colour::Black => continue,
                Colour::Grey => {
                    let start = path.iter().position(|t| *t == tag).unwrap_or(0);
                    let mut cycle: Vec<&str> = path[start..].to_vec();
                    cycle.push(tag);
                    return Err(err(format!(
                        "explains cycle: {}. Each step in the cycle would exempt the next from \
                         eager-exit, so the whole cycle becomes uncancellable and eager-exit \
                         silently stops applying to it. `explains` must describe a one-way \
                         'diagnoses' relation.",
                        cycle.join(" -> ")
                    )));
                }
                Colour::White => {}
            }
            colour.insert(tag, Colour::Grey);
            path.push(tag);
            stack.push((tag, true));
            let mut targets: Vec<&str> = by_tag[tag].explains.iter().map(|s| s.as_str()).collect();
            targets.sort_unstable();
            for target in targets {
                if colour.get(target) != Some(&Colour::Black) {
                    stack.push((target, false));
                }
            }
        }
    }
    Ok(())
}

fn err(msg: impl Into<String>) -> DagJsonError {
    DagJsonError(msg.into())
}

/// Every key a `steps[]` entry may carry. The step object is CLOSED.
///
/// The top level is deliberately open (see [`UNCARRIED_CONFIG_KEYS`]): a top-level key nobody has
/// ever implemented cannot masquerade as a setting that took effect, so tolerating it buys forward
/// compatibility at no cost. A STEP key is the opposite case. Every one of them is a per-node
/// instruction, and the near-misses are all real spellings a person writes by accident -- `dep`,
/// `cmds`, `timeouts`, `env_vars`, `description` vs `desc`. Silently ignored, the instruction is
/// simply not carried out and the document still says it was: the step runs with no timeout, no
/// dependency, no environment, and nothing anywhere reports it.
const STEP_KEYS: [&str; 25] = [
    "cmd",
    "cmdtype",
    "cpu_timeout",
    "deps",
    "desc",
    "description",
    "engine_only",
    "env",
    "explains",
    "fail_fast_family",
    "group",
    "hint",
    "integration_test_binaries",
    "job",
    "jobs_env",
    "jobs_flag",
    "labels",
    "networkonly",
    "skip_reason",
    "timeout",
    "write_domain_guarantee",
    "write_domains",
    // ⚠️ DECLARED HERE, CONSUMED DOWNSTREAM, NOT BY dagrun.
    // `requires_host_capability` drives a consuming planner's HOST-INAPPLICABLE
    // decision, while `manifest`, `result_manifests`, and `integration_test_binaries`
    // carry other consumer-owned selection facts. dagrun retains the declarations;
    // only its generic exact-result ownership helper interprets the selectors.
    //
    // Retained because this schema is CLOSED, and closing it without these fields
    // made dagrun REFUSE graphs that were already in use -- measured 2026-08-26,
    // exit 2 on `manifest` for one real graph and on `requires_host_capability`
    // for another. Keep this list in step with the Python edition's STEP_KEYS;
    // the two are asserted identical.
    "manifest",
    "result_manifests",
    "requires_host_capability",
];

/// Every key a step's `hint` object may carry. Closed for the same reason as [`STEP_KEYS`];
/// `est_duration` for `est_duration_s` is the canonical silent drop.
const HINT_KEYS: [&str; 8] = [
    "classification",
    "est_duration_s",
    "hard_mem_max_bytes",
    "measured_cpu_utilization",
    "measured_effective_cores",
    "preferred_inner_jobs",
    "resources",
    "rss_baseline_bytes",
];

/// Every key a per-step manifest selector may carry.
const MANIFEST_KEYS: [&str; 5] = ["backend", "category", "lane", "mode", "test"];

/// Every key the top-level `write_domain_policy` object may carry. Closed: a misspelled
/// `require_explicit` turns a fail-closed policy into no policy at all, silently.
const WRITE_DOMAIN_POLICY_KEYS: [&str; 2] = ["allowed_domains", "require_explicit"];

/// Refuse keys a CLOSED schema object does not define, naming every one of them.
///
/// `resources` and `env` are caller-defined key spaces and are never narrowed here; only the
/// schema objects themselves are closed.
fn refuse_unknown_keys(
    m: &serde_json::Map<String, Value>,
    known: &[&str],
    where_: &str,
) -> Result<(), DagJsonError> {
    let unknown: Vec<String> = m
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .map(|key| format!("'{key}'"))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    // serde_json::Map preserves insertion order only with the `preserve_order` feature; sort so
    // the refusal is byte-identical to the Python edition's sorted list either way.
    let mut unknown = unknown;
    unknown.sort();
    // BOTH lists, and this one was the half that was missing. `known` was joined in DECLARATION
    // order while the Python edition joins `sorted(known)`, so the two agreed only for as long as
    // the Rust array happened to be alphabetical. It stopped being so the moment two fields were
    // appended to the end of it, and the differential went red on seven checks with the same
    // twenty-one field names in a different order. The sentence above already claimed this was
    // handled; now it is.
    let mut known: Vec<&str> = known.to_vec();
    known.sort_unstable();
    Err(err(format!(
        "{where_}: unknown field(s) {}. This object's schema is CLOSED, because an ignored field \
         reads exactly like one that took effect. Known fields: {}",
        unknown.join(", "),
        known.join(", ")
    )))
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

fn steps_list_error(doc: &serde_json::Map<String, Value>) -> DagJsonError {
    let mut keys: Vec<String> = doc.keys().map(|key| format!("'{key}'")).collect();
    keys.sort();
    let keys = if keys.is_empty() {
        "(none)".to_string()
    } else {
        keys.join(", ")
    };
    let found = match doc.get("steps") {
        None => format!("no 'steps' key (top-level keys: {keys})"),
        Some(value) => format!(
            "'steps' with type {} (top-level keys: {keys})",
            type_name(value)
        ),
    };
    err(format!(
        "<root>: expected a dagrun DAG document with a top-level 'steps' list; found {found}. \
         This may be a different document type. Pass a dagrun DAG file, or run `dagrun \
         quickstart` for the schema."
    ))
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

fn manifest_value_from(value: &Value, where_: &str) -> Result<DagManifest, DagJsonError> {
    let object = as_obj(value, where_)?;
    refuse_unknown_keys(object, &MANIFEST_KEYS, where_)?;
    let lane = req_str(object, "lane", where_)?;
    let category = req_str(object, "category", where_)?;
    if lane.is_empty() {
        return Err(err(format!("{where_}.lane: must be non-empty")));
    }
    if category.is_empty() {
        return Err(err(format!("{where_}.category: must be non-empty")));
    }
    let test = opt_str_or_none(object, "test")?;
    let mode = opt_str_or_none(object, "mode")?;
    let backend = opt_str_or_none(object, "backend")?;
    for (field, value) in [("test", &test), ("mode", &mode), ("backend", &backend)] {
        if value.as_deref() == Some("") {
            return Err(err(format!(
                "{where_}.{field}: must be non-empty when present"
            )));
        }
    }
    Ok(DagManifest {
        lane,
        category,
        test,
        mode,
        backend,
    })
}

fn manifest_from(value: Option<&Value>, where_: &str) -> Result<Option<DagManifest>, DagJsonError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => manifest_value_from(value, where_).map(Some),
    }
}

fn result_manifests_from(
    value: Option<&Value>,
    where_: &str,
) -> Result<Option<Vec<DagManifest>>, DagJsonError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Value::Array(values) = value else {
        return Err(err(format!(
            "{where_}: must be a list of manifest selectors or null"
        )));
    };
    let mut manifests = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let item_where = format!("{where_}[{index}]");
        let manifest = manifest_value_from(value, &item_where)?;
        if manifests.contains(&manifest) {
            return Err(err(format!(
                "{where_}: duplicate selector at index {index}"
            )));
        }
        manifests.push(manifest);
    }
    Ok(Some(manifests))
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

fn labels_from(
    m: &serde_json::Map<String, Value>,
    where_: &str,
) -> Result<Vec<String>, DagJsonError> {
    let labels = opt_str_list(m, "labels")?;
    if labels.iter().any(|label| label.trim().is_empty()) {
        return Err(err(format!("{where_}.labels: labels must be non-empty")));
    }
    let unique = labels.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != labels.len() {
        return Err(err(format!(
            "{where_}.labels: duplicate labels are not allowed"
        )));
    }
    Ok(labels)
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

fn integration_test_binaries_from(
    m: &serde_json::Map<String, Value>,
) -> Result<Option<Vec<String>>, DagJsonError> {
    let Some(values) = present_str_list(m, "integration_test_binaries")? else {
        return Ok(None);
    };
    let mut seen = BTreeSet::new();
    for value in &values {
        if value.trim().is_empty() {
            return Err(err(
                "field 'integration_test_binaries' must not contain empty names",
            ));
        }
        if !seen.insert(value) {
            return Err(err(format!(
                "field 'integration_test_binaries' contains duplicate name '{value}'"
            )));
        }
    }
    Ok(Some(values))
}

fn write_domain_policy(value: Option<&Value>) -> Result<WriteDomainPolicy, DagJsonError> {
    let Some(value) = value else {
        return Ok(WriteDomainPolicy::default());
    };
    if value.is_null() {
        return Ok(WriteDomainPolicy::default());
    }
    let obj = as_obj(value, "write_domain_policy")?;
    refuse_unknown_keys(obj, &WRITE_DOMAIN_POLICY_KEYS, "write_domain_policy")?;
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
    refuse_unknown_keys(obj, &HINT_KEYS, where_)?;
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
        // Planner-only provenance. It is intentionally not part of the authored DAG schema.
        rss_baseline_inner_jobs: None,
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

/// `DagConfig` fields that the DOCUMENT FORMAT deliberately does not carry.
///
/// Writing one of these at the top level of a DAG file today has no effect whatsoever: the
/// parser never looks at the key, the field takes its `DagConfig` default, and nothing says so.
/// That is the dropped-field bug from the reader's side — a configured cap silently replaced by
/// a default, with no report — so the loader REFUSES the key by name instead of ignoring it.
///
/// Genuinely unknown keys stay tolerated: a key nobody has ever implemented cannot masquerade as
/// a setting that took effect, whereas one that names a real field reads exactly like one that
/// did. `known_failures` is listed although this crate has no such field, because the key set is
/// a shared contract: both language editions of the runner must refuse the same keys byte for
/// byte, and a document is not more portable for being accepted by only one of them.
const UNCARRIED_CONFIG_KEYS: [&str; 6] = [
    "default_step_mem_cap_bytes",
    "default_step_cpu_count",
    "default_step_cpu_timeout",
    "cpu_timeout_multiplier",
    "cpu_timeout_platform",
    "known_failures",
];

/// `Some(refusal)` when the document sets a real config field the format cannot carry.
fn uncarried_config_key(doc: &serde_json::Map<String, Value>) -> Option<String> {
    let present: Vec<&str> = UNCARRIED_CONFIG_KEYS
        .iter()
        .copied()
        .filter(|key| doc.contains_key(*key))
        .collect();
    if present.is_empty() {
        return None;
    }
    Some(format!(
        "<root>: {} top-level key(s) name a DagConfig field the DAG document format does not \
         carry, so the value would be SILENTLY replaced by a default: {}. Set these on the \
         DagConfig at the call site (they are caller/platform policy, not properties of the \
         graph), or remove them.",
        present.len(),
        present.join(", ")
    ))
}

/// Construct a [`DagConfig`] from an already-parsed JSON/YAML value tree — the shared strict
/// narrowing behind both [`dag_from_json`] and [`dag_from_yaml`], so the two syntaxes cannot
/// drift in how they build the model.
pub fn dag_from_value(raw: &Value) -> Result<DagConfig, DagJsonError> {
    let doc = as_obj(raw, "<root>")?;
    if let Some(refusal) = uncarried_config_key(doc) {
        return Err(err(refusal));
    }
    // ABSENT IS NOT 1800: an omitted default leaves both it and the step at the 0 sentinel, and
    // `resolved_wall_timeout` derives the bound from the step's declared CPU budget (or falls
    // back to DEFAULT_STEP_TIMEOUT). Materializing 1800 here is what baked the load-sensitive
    // number into every graph.
    let default_step_timeout = opt_int(doc, "default_step_timeout", 0)?;
    let policy = write_domain_policy(doc.get("write_domain_policy"))?;
    let steps_raw = match doc.get("steps") {
        Some(Value::Array(items)) => items,
        _ => return Err(steps_list_error(doc)),
    };
    let mut steps: Vec<Step> = Vec::with_capacity(steps_raw.len());
    for (i, entry) in steps_raw.iter().enumerate() {
        let where_ = format!("steps[{i}]");
        let sm = as_obj(entry, &where_)?;
        // Name the offending step by TAG as well as by index wherever the document supplies one:
        // "steps[7]" sends a reader counting entries in a 200-step file. Only the closed-schema
        // refusal gets the decorated location; the per-field type errors keep their established
        // "steps[N].field" spelling.
        let named = match (sm.get("group"), sm.get("job")) {
            (Some(Value::String(group)), Some(Value::String(job))) => {
                format!("{where_} ({group}.{job})")
            }
            _ => where_.clone(),
        };
        refuse_unknown_keys(sm, &STEP_KEYS, &named)?;
        let cmdtype_text = opt_str(sm, "cmdtype", CmdType::Unknown.value())?;
        let cmdtype = CmdType::from_value(&cmdtype_text).ok_or_else(|| {
            err(format!(
                "{where_}.cmdtype: unknown value '{cmdtype_text}'; valid values: unknown, make, cargo-build, cargo-test, cargo-nextest, generic-dash-j-command, generic-with-flag"
            ))
        })?;
        let fail_fast_family = opt_str_or_none(sm, "fail_fast_family")?;
        if fail_fast_family
            .as_ref()
            .is_some_and(|family| family.trim().is_empty())
        {
            return Err(err(format!("{where_}.fail_fast_family: must be non-empty")));
        }
        steps.push(Step {
            group: req_str(sm, "group", &where_)?,
            job: req_str(sm, "job", &where_)?,
            desc: opt_str(sm, "desc", "")?,
            description: opt_str(sm, "description", "")?,
            labels: labels_from(sm, &where_)?,
            cmd: req_str(sm, "cmd", &where_)?,
            cmdtype,
            manifest: manifest_from(sm.get("manifest"), &format!("{where_}.manifest"))?,
            result_manifests: result_manifests_from(
                sm.get("result_manifests"),
                &format!("{where_}.result_manifests"),
            )?,
            integration_test_binaries: integration_test_binaries_from(sm)?,
            deps: opt_str_list(sm, "deps")?,
            env: opt_str_str_map(sm, "env", &where_)?,
            hint: hint_from(sm.get("hint"), &format!("{where_}.hint"))?,
            networkonly: opt_bool(sm, "networkonly", false)?,
            engine_only: opt_bool(sm, "engine_only", false)?,
            timeout: opt_int(sm, "timeout", default_step_timeout)?,
            cpu_timeout: opt_int(sm, "cpu_timeout", 0)?,
            jobs_flag: opt_str_or_none(sm, "jobs_flag")?,
            jobs_env: match opt_str_or_none(sm, "jobs_env")? {
                Some(value) => Some(resolve_jobs_env(Some(&value)).map_err(|error| {
                    err(format!("{where_}.jobs_env: {error}"))
                })?),
                None => None,
            },
            skip_reason: match opt_str_or_none(sm, "skip_reason")? {
                Some(text) => {
                    Some(IntentionalSkipReason::from_value(&text).ok_or_else(|| {
                        err(format!("{where_}.skip_reason: unknown value '{text}'"))
                    })?)
                }
                None => None,
            },
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
            explains: opt_str_list(sm, "explains")?,
            fail_fast_family,
        });
    }
    refuse_unusable_explains(&steps)?;
    let intentional: std::collections::BTreeSet<String> = steps
        .iter()
        .filter(|step| step.skip_reason.is_some())
        .map(Step::tag)
        .collect();
    for step in &steps {
        let blocked: Vec<String> = step
            .deps
            .iter()
            .filter(|dep| intentional.contains(*dep))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            return Err(err(format!(
                "step {}: dependency on intentionally skipped node(s) {} is undefined",
                step.tag(),
                blocked.join(", ")
            )));
        }
    }
    let default_jobs_env = match doc.get("default_jobs_env") {
        Some(_) => {
            let value = opt_str(doc, "default_jobs_env", "")?;
            resolve_jobs_env(Some(&value))
        }
        None => resolve_jobs_env(None),
    }
    .map_err(|error| err(format!("<root>.default_jobs_env: {error}")))?;
    let cfg = DagConfig {
        steps,
        description: opt_str(doc, "description", "")?,
        resource_caps: opt_str_int_map(doc, "resource_caps", "<root>")?,
        mem_cap_factor: opt_float(doc, "mem_cap_factor", 1.25)?,
        mem_cap_floor_bytes: opt_int(doc, "mem_cap_floor_bytes", DEFAULT_MEM_CAP_FLOOR)?,
        outer_mem_safety_factor: opt_float(doc, "outer_mem_safety_factor", 1.0)?,
        default_step_timeout,
        default_jobs_flag: opt_str(doc, "default_jobs_flag", DEFAULT_JOBS_FLAG)?,
        default_jobs_env,
        write_domain_policy: policy,
        // SMALL forcing-function default caps for undeclared steps: not parsed from the document
        // (mirrors the Python io parser, which relies on the DagConfig dataclass defaults), so a
        // parsed DAG gets the 1-GiB / 1-core / 10-s floor. Callers override via the DagConfig fields.
        ..DagConfig::default()
    };
    validate_cmdtype_config(&cfg).map_err(err)?;
    let violations = write_domain_violations(&cfg);
    if !violations.is_empty() {
        return Err(err(format!(
            "write-domain policy refused DAG before execution: {}",
            violations.join("; ")
        )));
    }
    // LAST, because it is the only check that needs the finished configuration (caps and steps
    // together) and because its findings are about the graph rather than about a field.
    let structural = graph_structure_violations(&cfg);
    if !structural.is_empty() {
        return Err(err(format!(
            "<root>: {} graph error(s) refused before execution: {}",
            structural.len(),
            structural.join("; ")
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

fn emit_manifest(s: &mut String, manifest: &DagManifest, base: usize) {
    let fields = [
        ("lane", Some(manifest.lane.as_str())),
        ("category", Some(manifest.category.as_str())),
        ("test", manifest.test.as_deref()),
        ("mode", manifest.mode.as_deref()),
        ("backend", manifest.backend.as_deref()),
    ];
    let present: Vec<(&str, &str)> = fields
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect();
    s.push_str("{\n");
    let key = " ".repeat(base + 2);
    for (index, (name, value)) in present.iter().enumerate() {
        s.push_str(&key);
        s.push_str(&format!("{}: {}", json_str(name), json_str(value)));
        s.push_str(if index + 1 < present.len() {
            ",\n"
        } else {
            "\n"
        });
    }
    s.push_str(&" ".repeat(base));
    s.push('}');
}

fn emit_manifest_list(s: &mut String, manifests: &[DagManifest], base: usize) {
    if manifests.is_empty() {
        s.push_str("[]");
        return;
    }
    s.push_str("[\n");
    for (index, manifest) in manifests.iter().enumerate() {
        s.push_str(&" ".repeat(base + 2));
        emit_manifest(s, manifest, base + 2);
        s.push_str(if index + 1 < manifests.len() {
            ",\n"
        } else {
            "\n"
        });
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
    if !step.labels.is_empty() {
        s.push_str(&key);
        s.push_str("\"labels\": ");
        emit_str_list(s, &step.labels, base + 2);
        s.push_str(",\n");
    }
    s.push_str(&key);
    s.push_str(&format!("\"cmd\": {},\n", json_str(&step.cmd)));
    if step.cmdtype != CmdType::Unknown {
        s.push_str(&key);
        s.push_str(&format!(
            "\"cmdtype\": {},\n",
            json_str(step.cmdtype.value())
        ));
    }
    if let Some(manifest) = &step.manifest {
        s.push_str(&key);
        s.push_str("\"manifest\": ");
        emit_manifest(s, manifest, base + 2);
        s.push_str(",\n");
    }
    if let Some(manifests) = &step.result_manifests {
        s.push_str(&key);
        s.push_str("\"result_manifests\": ");
        emit_manifest_list(s, manifests, base + 2);
        s.push_str(",\n");
    }
    if let Some(targets) = &step.integration_test_binaries {
        s.push_str(&key);
        s.push_str("\"integration_test_binaries\": ");
        emit_str_list(s, targets, base + 2);
        s.push_str(",\n");
    }
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
    // Both timeout fields are emitted only when SET. 0 is the "derive it" sentinel, and writing
    // it out would read as "no wall bound" — the opposite of what it means.
    if step.timeout != 0 {
        s.push_str(&key);
        s.push_str(&format!("\"timeout\": {},\n", step.timeout));
    }
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
    s.push_str(&format!(
        "\"jobs_env\": {},\n",
        opt_str_json(step.jobs_env.as_deref())
    ));
    if let Some(reason) = step.skip_reason {
        s.push_str(&key);
        s.push_str(&format!("\"skip_reason\": {},\n", json_str(reason.value())));
    }
    s.push_str(&key);
    s.push_str("\"hint\": ");
    emit_hint(s, &step.hint, base + 2);
    // `explains` is emitted only when declared, and in the same position as Python's serializer,
    // because the two editions are pinned together by a byte-identical JSON comparison. A graph
    // that does not use the relationship keeps a byte-identical document.
    let has_explains = !step.explains.is_empty();
    let has_fail_fast_family = step.fail_fast_family.is_some();
    let has_write_domains = step.write_domains.is_some() || step.write_domain_guarantee.is_some();
    if has_explains || has_fail_fast_family || has_write_domains {
        s.push_str(",\n");
    } else {
        s.push('\n');
    }
    if has_explains {
        s.push_str(&key);
        s.push_str("\"explains\": ");
        emit_str_list(s, &step.explains, base + 2);
        if has_fail_fast_family || has_write_domains {
            s.push_str(",\n");
        } else {
            s.push('\n');
        }
    }
    if let Some(family) = &step.fail_fast_family {
        s.push_str(&key);
        s.push_str(&format!("\"fail_fast_family\": {}", json_str(family)));
        if has_write_domains {
            s.push_str(",\n");
        } else {
            s.push('\n');
        }
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
    if cfg.default_step_timeout != 0 {
        s.push_str(&format!(
            "  \"default_step_timeout\": {},\n",
            cfg.default_step_timeout
        ));
    }
    s.push_str(&format!(
        "  \"default_jobs_flag\": {},\n",
        json_str(&cfg.default_jobs_flag)
    ));
    s.push_str(&format!(
        "  \"default_jobs_env\": {},\n",
        json_str(&cfg.default_jobs_env)
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
            "outer_mem_safety_factor": 1.1, "default_jobs_flag": "--jobs=",
            "default_jobs_env": "DEFAULT_JOBS", "steps": [
            {"group": "build", "job": "app", "desc": "compile", "labels": ["quick", "full"],
             "description": "line 1\nline 2 with \"quotes\" and \\backslash\\ and unicode é☃",
             "cmd": "make build",
             "jobs_flag": "-j%d", "jobs_env": "STEP_JOBS",
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
        assert_eq!(back.steps[0].jobs_env.as_deref(), Some("STEP_JOBS"));
        assert_eq!(back.steps[0].labels, ["quick", "full"]);
        assert!(back.steps[1].labels.is_empty());
        assert_eq!(back.steps[1].jobs_flag, None);
        assert_eq!(back.steps[1].jobs_env, None);
        assert_eq!(back.default_jobs_flag, "--jobs=");
        assert_eq!(back.default_jobs_env, "DEFAULT_JOBS");
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
        assert!(step.labels.is_empty());
        assert_eq!(cfg.description, "");
        assert!(step.deps.is_empty());
        assert!(step.env.is_empty());
        // 0 is the "derive it" sentinel, not "no bound": a minimal step declares no wall
        // budget and no CPU budget, so the resolved bound is still the 1800-second backstop.
        assert_eq!(step.timeout, 0);
        assert_eq!(cfg.default_step_timeout, 0);
        assert_eq!(
            crate::model::resolved_wall_timeout(step, cfg.default_step_timeout, 1.0),
            1800
        );
        assert_eq!(step.hint.classification, StepClass::Light);
        assert_eq!(step.jobs_flag, None);
        assert_eq!(step.manifest, None);
        assert_eq!(step.result_manifests, None);
        assert_eq!(step.integration_test_binaries, None);
        assert!(cfg.resource_caps.is_empty());
        assert_eq!(cfg.mem_cap_factor, 1.25);
        assert_eq!(cfg.default_jobs_flag, "-j");
    }

    #[test]
    fn labels_refuse_malformed_values() {
        for (labels, expected) in [
            (r#""quick""#, "field 'labels' must be a list of strings"),
            (
                r#"["quick", 1]"#,
                "field 'labels' must contain only strings",
            ),
            (r#"[""]"#, "labels must be non-empty"),
            (r#"["quick", "quick"]"#, "duplicate labels are not allowed"),
        ] {
            let document = format!(
                r#"{{"steps":[{{"group":"g","job":"j","cmd":"true","labels":{labels}}}]}}"#
            );
            let error = dag_from_json(&document).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn manifest_selection_roundtrips_and_refuses_malformed_values() {
        let doc = r#"{"steps":[{"group":"e2e","job":"manifest_applications","cmd":"true",
            "manifest":{"lane":"portable","category":"applications"}}]}"#;
        let cfg = dag_from_json(doc).unwrap();
        assert_eq!(
            cfg.steps[0].manifest,
            Some(DagManifest {
                lane: "portable".into(),
                category: "applications".into(),
                test: None,
                mode: None,
                backend: None,
            })
        );
        assert_eq!(cfg.steps[0].result_manifests, None);
        let encoded = dag_to_json(&cfg);
        assert_eq!(dag_to_json(&dag_from_json(&encoded).unwrap()), encoded);

        for (value, expected) in [
            (
                r#"{"lane":"portable"}"#,
                "manifest: field 'category' must be a string",
            ),
            (
                r#"{"lane":"","category":"applications"}"#,
                "manifest.lane: must be non-empty",
            ),
            (
                r#"{"lane":"portable","category":"applications","future":"value"}"#,
                "manifest: unknown field(s) 'future'",
            ),
        ] {
            let input = format!(
                r#"{{"steps":[{{"group":"e2e","job":"manifest_applications","cmd":"true","manifest":{value}}}]}}"#
            );
            let error = dag_from_json(&input).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn result_manifests_roundtrip_preserves_absent_and_explicit_empty() {
        let doc = r#"{"steps":[
            {"group":"e2e","job":"legacy","cmd":"true",
             "manifest":{"lane":"portable","category":"applications"}},
            {"group":"e2e","job":"none","cmd":"true",
             "manifest":{"lane":"portable","category":"applications"},
             "result_manifests":[]},
            {"group":"e2e","job":"many","cmd":"true","result_manifests":[
                {"lane":"portable","category":"applications","mode":"verify","backend":"ptrace"},
                {"lane":"portable","category":"c-programs","test":"c-programs/add-key-enosys",
                 "mode":"run","backend":"kvm"}
             ]}
        ]}"#;
        let cfg = dag_from_json(doc).unwrap();
        assert_eq!(cfg.steps[0].result_manifests, None);
        assert_eq!(cfg.steps[0].effective_result_manifests().len(), 1);
        assert_eq!(cfg.steps[1].result_manifests, Some(Vec::new()));
        assert!(cfg.steps[1].effective_result_manifests().is_empty());
        assert_eq!(cfg.steps[2].effective_result_manifests().len(), 2);
        assert_eq!(
            cfg.steps[2].result_manifests.as_ref().unwrap()[1],
            DagManifest {
                lane: "portable".into(),
                category: "c-programs".into(),
                test: Some("c-programs/add-key-enosys".into()),
                mode: Some("run".into()),
                backend: Some("kvm".into()),
            }
        );
        let encoded = dag_to_json(&cfg);
        let encoded_value: Value = serde_json::from_str(&encoded).unwrap();
        let encoded_steps = encoded_value["steps"].as_array().unwrap();
        assert!(encoded_steps[0].get("result_manifests").is_none());
        assert_eq!(encoded_steps[1]["result_manifests"], serde_json::json!([]));
        assert_eq!(dag_to_json(&dag_from_json(&encoded).unwrap()), encoded);
    }

    #[test]
    fn result_manifests_refuse_malformed_and_duplicate_selectors() {
        for (value, expected) in [
            (
                r#"{"lane":"portable","category":"applications"}"#,
                "result_manifests: must be a list of manifest selectors or null",
            ),
            (r#"[null]"#, "result_manifests[0]: expected an object"),
            (
                r#"[{"lane":"portable"}]"#,
                "result_manifests[0]: field 'category' must be a string",
            ),
            (
                r#"[{"lane":"portable","category":"applications","mode":""}]"#,
                "result_manifests[0].mode: must be non-empty when present",
            ),
            (
                r#"[{"lane":"portable","category":"applications"},{"lane":"portable","category":"applications"}]"#,
                "result_manifests: duplicate selector at index 1",
            ),
            (
                r#"[{"lane":"portable","category":"applications","future":1}]"#,
                "result_manifests[0]: unknown field(s) 'future'",
            ),
        ] {
            let input = format!(
                r#"{{"steps":[{{"group":"e2e","job":"results","cmd":"true","result_manifests":{value}}}]}}"#
            );
            let error = dag_from_json(&input).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn integration_test_binaries_roundtrip_and_refuse_malformed_values() {
        let doc = r#"{"steps":[{"group":"test","job":"cli","cmd":"true",
            "integration_test_binaries":["unit_alpha","unit_beta"]}]}"#;
        let cfg = dag_from_json(doc).unwrap();
        assert_eq!(
            cfg.steps[0].integration_test_binaries,
            Some(vec!["unit_alpha".into(), "unit_beta".into()])
        );
        let encoded = dag_to_json(&cfg);
        assert_eq!(dag_to_json(&dag_from_json(&encoded).unwrap()), encoded);

        for (value, expected) in [
            (
                r#""unit_alpha""#,
                "field 'integration_test_binaries' must be a list of strings",
            ),
            (
                r#"["unit_alpha",7]"#,
                "field 'integration_test_binaries' must contain only strings",
            ),
            (
                r#"["unit_alpha",""]"#,
                "field 'integration_test_binaries' must not contain empty names",
            ),
            (
                r#"["unit_alpha","unit_alpha"]"#,
                "field 'integration_test_binaries' contains duplicate name 'unit_alpha'",
            ),
        ] {
            let input = format!(
                r#"{{"steps":[{{"group":"test","job":"cli","cmd":"true","integration_test_binaries":{value}}}]}}"#
            );
            let error = dag_from_json(&input).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn fail_fast_family_roundtrips_and_rejects_empty() {
        let doc = r#"{"steps":[
            {"group":"g","job":"scoped","cmd":"true",
             "fail_fast_family":"family-a"},
            {"group":"g","job":"global","cmd":"true"}]}"#;
        let cfg = dag_from_json(doc).unwrap();
        assert_eq!(cfg.steps[0].fail_fast_family.as_deref(), Some("family-a"));
        assert_eq!(cfg.steps[1].fail_fast_family, None);

        let encoded = dag_to_json(&cfg);
        assert_eq!(encoded.matches("\"fail_fast_family\"").count(), 1);
        assert_eq!(dag_to_json(&dag_from_json(&encoded).unwrap()), encoded);

        let error = dag_from_json(
            r#"{"steps":[{"group":"g","job":"j","cmd":"true",
                "fail_fast_family":"   "}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("fail_fast_family: must be non-empty"),
            "{error}"
        );
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
    fn typed_intentional_skip_roundtrips_and_rejects_dependents() {
        let cfg = dag_from_json(
            r#"{"steps":[{"group":"g","job":"empty","cmd":"false","skip_reason":"empty-manifest-bucket"}]}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.steps[0].skip_reason,
            Some(IntentionalSkipReason::EmptyManifestBucket)
        );
        assert!(dag_to_json(&cfg).contains("\"skip_reason\": \"empty-manifest-bucket\""));

        assert!(dag_from_json(
            r#"{"steps":[{"group":"g","job":"x","cmd":"true","skip_reason":"unknown"}]}"#
        )
        .is_err());
        assert!(dag_from_json(
            r#"{"steps":[{"group":"g","job":"empty","cmd":"false","skip_reason":"empty-manifest-bucket"},{"group":"g","job":"consumer","cmd":"true","deps":["g.empty"]}]}"#
        )
        .is_err());
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
    fn a_non_dag_document_names_what_was_read_and_what_to_do() {
        let error = dag_from_yaml("schema: 2\nbucket: example\ntest: []\n")
            .unwrap_err()
            .0;
        assert_eq!(
            error,
            "<root>: expected a dagrun DAG document with a top-level 'steps' list; found no \
             'steps' key (top-level keys: 'bucket', 'schema', 'test'). This may be a different \
             document type. Pass a dagrun DAG file, or run `dagrun quickstart` for the schema."
        );

        let wrong_type = dag_from_json(r#"{"steps":"not a list"}"#).unwrap_err().0;
        assert_eq!(
            wrong_type,
            "<root>: expected a dagrun DAG document with a top-level 'steps' list; found \
             'steps' with type str (top-level keys: 'steps'). This may be a different document \
             type. Pass a dagrun DAG file, or run `dagrun quickstart` for the schema."
        );
    }

    #[test]
    fn malformed_jobs_env_fields_are_refused_by_location() {
        let top = dag_from_json(r#"{"default_jobs_env":"not a name","steps":[]}"#)
            .unwrap_err()
            .to_string();
        assert!(top.contains("<root>.default_jobs_env"), "{top}");
        assert!(
            top.contains("not a valid environment variable name"),
            "{top}"
        );

        let step = dag_from_json(
            r#"{"steps":[{"group":"g","job":"j","cmd":"true","jobs_env":"bad=name"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(step.contains("steps[0].jobs_env"), "{step}");
        assert!(
            step.contains("not a valid environment variable name"),
            "{step}"
        );
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

    // ---------------------------------------------- uncarried top-level config keys (#21)

    // The six keys the loader must refuse, WRITTEN OUT rather than read from the production
    // constant.
    //
    // Iterating `UNCARRIED_CONFIG_KEYS` here was a tautology: deleting two names from the
    // production array deleted the two cases that would have caught it, and the whole suite
    // stayed green while two previously-refused keys went back to silently defaulting. A
    // literal list is the only kind that can fail. It is also the parity contract the other
    // edition's `test_config_carry.py` repeats verbatim, and the cross differential now drives
    // both binaries with each of these keys.
    const REFUSED_KEYS: [&str; 6] = [
        "default_step_mem_cap_bytes",
        "default_step_cpu_count",
        "default_step_cpu_timeout",
        "cpu_timeout_multiplier",
        "cpu_timeout_platform",
        "known_failures",
    ];

    #[test]
    fn a_top_level_key_the_format_cannot_carry_is_refused_by_name() {
        for key in REFUSED_KEYS {
            let doc = format!(r#"{{"{key}": 5, "steps": []}}"#);
            let error = dag_from_json(&doc)
                .expect_err(&format!("'{key}' silently reverted to a default"))
                .0;
            assert!(error.contains(key), "refusal must name the key: {error}");
            assert!(
                error.contains("SILENTLY replaced by a default"),
                "refusal must say what would otherwise happen: {error}"
            );
        }
    }

    #[test]
    fn the_refused_key_set_is_exactly_the_six_names_the_contract_lists() {
        // The other direction, so the literal list cannot silently GROW either: a key added to
        // the production array without being added to the shared contract (and to the Python
        // edition, and to the cross differential) is a document that loads on one build and is
        // rejected on the other.
        // Compared as SLICES so a length change is a test failure by name, not a type error.
        assert_eq!(UNCARRIED_CONFIG_KEYS.as_slice(), REFUSED_KEYS.as_slice());
    }

    #[test]
    fn the_refusal_counts_and_names_every_offending_key_at_once() {
        let doc = r#"{"default_step_cpu_timeout": 5, "cpu_timeout_multiplier": 2.0, "steps": []}"#;
        let error = dag_from_json(doc).unwrap_err().0;
        assert!(error.contains("2 top-level key(s)"), "{error}");
        assert!(
            error.contains("default_step_cpu_timeout, cpu_timeout_multiplier"),
            "{error}"
        );
    }

    #[test]
    fn a_key_naming_no_config_field_at_all_is_still_tolerated() {
        // Forward compatibility: a key nobody has ever implemented cannot masquerade as a setting
        // that took effect, so it is NOT the dropped-field bug and stays accepted.
        let cfg = dag_from_json(r#"{"future_thing": 5, "steps": []}"#)
            .expect("an unimplemented key is not a dropped field");
        assert!(cfg.steps.is_empty());
    }

    // ------------------------------------------------- the loader's graph contract (closed
    // schema, duplicate tags, missing deps, cycles, unsatisfiable demands)
    //
    // Every case below was ACCEPTED by this loader before, and the first four then produced a
    // wrong run rather than an error: an ignored field, a step that silently never ran, a starve
    // discovered only after unrelated work had completed, and a stack-overflow abort.

    #[test]
    fn an_unknown_step_field_is_refused_and_named_with_its_step() {
        let error =
            dag_from_json(r#"{"steps":[{"group":"a","job":"one","cmd":"true","bogus_field":42}]}"#)
                .expect_err("an unknown step field silently did nothing")
                .0;
        assert!(error.contains("steps[0] (a.one)"), "{error}");
        assert!(error.contains("'bogus_field'"), "{error}");
        // The known-field list is part of the message: the whole point is that the author meant
        // one of them.
        assert!(error.contains("cpu_timeout"), "{error}");

        // Two at once are named together, sorted, so a reader fixes both in one pass.
        let both = dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","cmd":"true","zeta":1,"alpha":2}]}"#,
        )
        .unwrap_err()
        .0;
        assert!(both.contains("'alpha', 'zeta'"), "{both}");

        // The nested schema objects are closed too. `est_duration` for `est_duration_s` is the
        // canonical silent drop: the estimate is simply not carried and planning uses 0.
        let hint = dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","cmd":"true","hint":{"est_duration":9}}]}"#,
        )
        .unwrap_err()
        .0;
        assert!(hint.contains("steps[0].hint"), "{hint}");
        assert!(hint.contains("'est_duration'"), "{hint}");

        let policy =
            dag_from_json(r#"{"steps":[],"write_domain_policy":{"require_explicits":true}}"#)
                .unwrap_err()
                .0;
        assert!(policy.contains("write_domain_policy"), "{policy}");
        assert!(policy.contains("'require_explicits'"), "{policy}");
    }

    #[test]
    fn a_step_declaring_only_known_fields_still_loads() {
        // The other side, so "refuse every step" cannot pass the case above.
        dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","desc":"d","description":"long",
                "cmd":"true","deps":[],"env":{"K":"V"},"networkonly":false,
                "engine_only":false,"timeout":5,"cpu_timeout":3,
                "cmdtype":"generic-with-flag","jobs_flag":"-j",
                "jobs_env":"J","explains":[],"fail_fast_family":"fam",
                "hint":{"resources":{},"est_duration_s":1.0,"classification":"light"}}]}"#,
        )
        .expect("a step using only declared fields must load");
    }

    #[test]
    fn a_duplicate_step_tag_is_refused_rather_than_silently_dropping_a_step() {
        // THE SILENT ONE. Two steps, one tag: the runner executed exactly one of them and then
        // reported "2 passed". Nothing anywhere said a declared command had not been run.
        let error = dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","cmd":"echo FIRST"},
                         {"group":"a","job":"one","cmd":"echo SECOND"}]}"#,
        )
        .expect_err("a duplicate tag silently dropped a step")
        .0;
        assert!(error.contains("duplicate step tag 'a.one'"), "{error}");
        assert!(error.contains("declared 2 times"), "{error}");
        assert!(error.contains("vanish silently"), "{error}");
    }

    #[test]
    fn a_missing_dependency_is_refused_at_load_not_after_a_full_build() {
        let error = dag_from_json(
            r#"{"steps":[{"group":"z","job":"zero","cmd":"true"},
                         {"group":"a","job":"one","cmd":"true","deps":["b.missing"]}]}"#,
        )
        .expect_err("a dangling dependency loaded fine and starved mid-run")
        .0;
        assert!(
            error.contains("step a.one: depends on 'b.missing', which no step declares"),
            "{error}"
        );
    }

    #[test]
    fn a_dependency_cycle_is_refused_and_the_refusal_names_the_cycle() {
        // The CRASH case. Accepted by the loader, this reached the bottom-level walk and aborted
        // the process with a stack overflow (core dump); the Python edition raised RecursionError.
        let error = dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","cmd":"true","deps":["b.two"]},
                         {"group":"b","job":"two","cmd":"true","deps":["a.one"]}]}"#,
        )
        .expect_err("a cycle loaded fine and then crashed the planner")
        .0;
        assert!(
            error.contains("dependency cycle: a.one -> b.two -> a.one"),
            "the refusal must NAME the cycle: {error}"
        );

        // A self-edge is a one-node cycle and is named the same way.
        let self_edge =
            dag_from_json(r#"{"steps":[{"group":"a","job":"one","cmd":"true","deps":["a.one"]}]}"#)
                .unwrap_err()
                .0;
        assert!(
            self_edge.contains("dependency cycle: a.one -> a.one"),
            "{self_edge}"
        );

        // A long chain is walked ITERATIVELY, so the cycle check cannot itself be the crash.
        let mut steps: Vec<String> = Vec::new();
        for i in 0..5000 {
            let dep = if i == 0 {
                "\"g.s4999\"".to_string()
            } else {
                format!("\"g.s{}\"", i - 1)
            };
            steps.push(format!(
                r#"{{"group":"g","job":"s{i}","cmd":"true","deps":[{dep}]}}"#
            ));
        }
        let deep = dag_from_json(&format!(r#"{{"steps":[{}]}}"#, steps.join(",")))
            .unwrap_err()
            .0;
        assert!(deep.contains("dependency cycle: "), "{deep}");
    }

    #[test]
    fn a_demand_above_a_positive_cap_is_refused_but_a_cap_of_zero_stays_a_deliberate_block() {
        let error = dag_from_json(
            r#"{"resource_caps":{"browser":1},
                "steps":[{"group":"a","job":"one","cmd":"true",
                          "hint":{"resources":{"browser":2}}}]}"#,
        )
        .expect_err("an unsatisfiable demand loaded fine")
        .0;
        assert!(
            error.contains(
                "step a.one: demands browser=2 but resource_caps declares browser=1, so it can \
                 never be admitted"
            ),
            "{error}"
        );

        // A cap of exactly 0 is documented as "blocked on purpose", so it is NOT a load error.
        // Asserting the boundary from both sides is what stops this becoming a blanket ban.
        dag_from_json(
            r#"{"resource_caps":{"browser":0},
                "steps":[{"group":"a","job":"one","cmd":"true",
                          "hint":{"resources":{"browser":1}}}]}"#,
        )
        .expect("a cap of 0 is the documented deliberate block, not a load error");

        // An intentionally-skipped step never launches, so its dormant demand is not an error.
        dag_from_json(
            r#"{"resource_caps":{"browser":1},
                "steps":[{"group":"a","job":"one","cmd":"true",
                          "skip_reason":"empty-manifest-bucket",
                          "hint":{"resources":{"browser":9}}}]}"#,
        )
        .expect("a skipped step's dormant demand cannot starve anything");
    }

    #[test]
    fn a_graph_with_several_faults_reports_them_all_at_once() {
        let error = dag_from_json(
            r#"{"resource_caps":{"browser":1},
                "steps":[{"group":"a","job":"one","cmd":"true","deps":["nope.gone"]},
                         {"group":"b","job":"two","cmd":"true","deps":["c.three"]},
                         {"group":"c","job":"three","cmd":"true","deps":["b.two"]},
                         {"group":"d","job":"four","cmd":"true",
                          "hint":{"resources":{"browser":5}}}]}"#,
        )
        .unwrap_err()
        .0;
        assert!(error.contains("3 graph error(s)"), "{error}");
        assert!(error.contains("'nope.gone'"), "{error}");
        assert!(
            error.contains("dependency cycle: b.two -> c.three -> b.two"),
            "{error}"
        );
        assert!(error.contains("demands browser=5"), "{error}");
    }

    #[test]
    fn a_duplicate_tag_short_circuits_the_edge_checks() {
        // While two steps share a tag, every statement about "the step named X" is ambiguous, so
        // the loader reports the ambiguity and nothing built on top of it.
        let error = dag_from_json(
            r#"{"steps":[{"group":"a","job":"one","cmd":"true","deps":["nope.gone"]},
                         {"group":"a","job":"one","cmd":"true"}]}"#,
        )
        .unwrap_err()
        .0;
        assert!(error.contains("1 graph error(s)"), "{error}");
        assert!(error.contains("duplicate step tag"), "{error}");
        assert!(!error.contains("nope.gone"), "{error}");
    }

    #[test]
    fn every_key_the_serializer_emits_survives_a_round_trip() {
        // The carry assertion applied to this crate's OWN loader/serializer pair: whatever
        // dag_to_json writes, dag_from_json must read back to the same configuration, or the
        // format itself is silently substituting defaults.
        let doc = r#"{
            "description": "a real lane",
            "resource_caps": {"widget_guest": 1, "manifest_guest": 4},
            "mem_cap_factor": 1.5,
            "mem_cap_floor_bytes": 4294967296,
            "outer_mem_safety_factor": 1.2,
            "default_step_timeout": 600,
            "default_jobs_flag": "--jobs {n}",
            "write_domain_policy": {"require_explicit": true, "allowed_domains": ["shared"]},
            "steps": [{"group": "g", "job": "j", "cmd": "true", "write_domains": []}]
        }"#;
        let cfg = dag_from_json(doc).unwrap();
        let again = dag_from_json(&dag_to_json(&cfg)).unwrap();
        assert_eq!(
            crate::model::dag_config_carry_diff(&cfg, &again),
            Vec::<String>::new()
        );
        // and the values really were non-default, so an all-defaults round trip cannot pass by
        // accident.
        assert_eq!(cfg.default_step_timeout, 600);
        assert_eq!(cfg.resource_caps.len(), 2);
    }
}
