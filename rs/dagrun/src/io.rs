//! Strict JSON and YAML serialization for [`crate::DagConfig`].
//!
//! The strict JSON reader and byte-exact writer are re-exported here from the
//! model crate. The YAML interchange and the filesystem `include`
//! composition stay in this crate: YAML parses into a [`serde_json::Value`]
//! and delegates to the same strict narrowing, and `include` needs a source
//! directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::DagConfig;

use crate::model::STRUCTURED_TEST_RESULTS_KIND;
#[doc(hidden)]
pub use dagrun_model::io::json_str;
use dagrun_model::io::{
    as_obj, err, opt_str_int_map, raw_from_json, refuse_unknown_keys, req_str, steps_list_error,
    uncarried_config_key,
};
pub use dagrun_model::io::{dag_from_json, dag_from_value, dag_to_json, DagJsonError};

fn raw_from_yaml(text: &str) -> Result<Value, DagJsonError> {
    serde_norway::from_str(text).map_err(|e| err(format!("invalid YAML: {e}")))
}

/// Parse one self-contained YAML DAG into a [`DagConfig`].
///
/// YAML is isomorphic to the JSON schema. `include` requires filesystem context and is accepted
/// only by [`dag_from_path`].
pub fn dag_from_yaml(text: &str) -> Result<DagConfig, DagJsonError> {
    dag_from_value(&raw_from_yaml(text)?)
}

const INCLUDE_KEYS: [&str; 3] = ["after", "namespace", "path"];
/// Maximum recursively included edges below the root document.
pub const MAX_DAG_INCLUDE_DEPTH: usize = 128;
/// Maximum included file instances; the root document is not counted.
pub const MAX_DAG_INCLUDE_INSTANCES: usize = 4096;
/// Maximum steps after file composition. Context-free parsers retain their existing behavior.
pub const MAX_DAG_FLATTENED_STEPS: usize = 100_000;
const GLOBAL_POLICY_KEYS: [&str; 7] = [
    "mem_cap_factor",
    "mem_cap_floor_bytes",
    "outer_mem_safety_factor",
    "default_step_timeout",
    "default_jobs_flag",
    "default_jobs_env",
    "write_domain_policy",
];

fn valid_namespace_segment(namespace: &str) -> bool {
    let mut chars = namespace.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn include_entries(
    doc: &serde_json::Map<String, Value>,
    source: &str,
) -> Result<Vec<(String, String, Vec<String>)>, DagJsonError> {
    let Some(raw) = doc.get("include") else {
        return Ok(Vec::new());
    };
    let Value::Array(values) = raw else {
        return Err(err(format!("{source}.include: must be a list")));
    };
    let mut entries = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let where_ = format!("{source}.include[{index}]");
        let entry = as_obj(value, &where_)?;
        refuse_unknown_keys(entry, &INCLUDE_KEYS, &where_)?;
        let path = req_str(entry, "path", &where_)?;
        let namespace = req_str(entry, "namespace", &where_)?;
        if path.is_empty() {
            return Err(err(format!("{where_}.path: must be non-empty")));
        }
        if !valid_namespace_segment(&namespace) {
            return Err(err(format!(
                "{where_}.namespace: must match [A-Za-z0-9][A-Za-z0-9_-]*"
            )));
        }
        let after = match entry.get("after") {
            None => Vec::new(),
            Some(Value::Array(values)) => {
                let mut after = Vec::with_capacity(values.len());
                for value in values {
                    let Value::String(tag) = value else {
                        return Err(err(format!("{where_}.after: must contain only strings")));
                    };
                    after.push(tag.clone());
                }
                after
            }
            Some(_) => return Err(err(format!("{where_}.after: must be a list of strings"))),
        };
        if after.iter().any(String::is_empty) {
            return Err(err(format!("{where_}.after: tags must be non-empty")));
        }
        if after.iter().collect::<BTreeSet<_>>().len() != after.len() {
            return Err(err(format!(
                "{where_}.after: duplicate tags are not allowed"
            )));
        }
        entries.push((path, namespace, after));
    }
    Ok(entries)
}

fn qualify(namespace: &str, value: &str) -> String {
    if namespace.is_empty() {
        value.to_string()
    } else {
        format!("{namespace}.{value}")
    }
}

fn rewrite_string_field(object: &mut serde_json::Map<String, Value>, key: &str, namespace: &str) {
    if let Some(Value::String(value)) = object.get_mut(key) {
        *value = qualify(namespace, value);
    }
}

fn rewrite_string_list(object: &mut serde_json::Map<String, Value>, key: &str, namespace: &str) {
    if let Some(Value::Array(values)) = object.get_mut(key) {
        for value in values {
            if let Value::String(text) = value {
                *text = qualify(namespace, text);
            }
        }
    }
}

fn rewrite_step(mut value: Value, namespace: &str) -> Value {
    if namespace.is_empty() {
        return value;
    }
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    rewrite_string_field(object, "group", namespace);
    rewrite_string_list(object, "deps", namespace);
    rewrite_string_list(object, "explains", namespace);
    if let Some(Value::Array(deps)) = object.get_mut("artifact_deps") {
        for dep in deps {
            if let Some(dep_object) = dep.as_object_mut() {
                rewrite_string_field(dep_object, "step", namespace);
            }
        }
    }
    if object
        .get("fail_fast_family")
        .and_then(Value::as_str)
        .is_some_and(|family| !family.trim().is_empty())
    {
        rewrite_string_field(object, "fail_fast_family", namespace);
    }
    if let Some(Value::Array(declarations)) = object.get_mut("result_manifests") {
        for declaration in declarations {
            let Some(manifest) = declaration.as_object_mut() else {
                continue;
            };
            if manifest.get("kind").and_then(Value::as_str) == Some(STRUCTURED_TEST_RESULTS_KIND)
                && manifest
                    .get("owner")
                    .and_then(Value::as_str)
                    .is_some_and(|owner| !owner.is_empty())
            {
                rewrite_string_field(manifest, "owner", namespace);
            }
        }
    }
    value
}

fn policy_config(doc: &serde_json::Map<String, Value>) -> Result<DagConfig, DagJsonError> {
    let mut policy = doc.clone();
    policy.remove("include");
    policy.insert("steps".to_string(), Value::Array(Vec::new()));
    dag_from_value(&Value::Object(policy))
}

fn same_policy(key: &str, left: &DagConfig, right: &DagConfig) -> bool {
    match key {
        "mem_cap_factor" => left.mem_cap_factor == right.mem_cap_factor,
        "mem_cap_floor_bytes" => left.mem_cap_floor_bytes == right.mem_cap_floor_bytes,
        "outer_mem_safety_factor" => left.outer_mem_safety_factor == right.outer_mem_safety_factor,
        "default_step_timeout" => left.default_step_timeout == right.default_step_timeout,
        "default_jobs_flag" => left.default_jobs_flag == right.default_jobs_flag,
        "default_jobs_env" => left.default_jobs_env == right.default_jobs_env,
        "write_domain_policy" => left.write_domain_policy == right.write_domain_policy,
        _ => unreachable!("GLOBAL_POLICY_KEYS contains an unknown field"),
    }
}

fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|suffix| suffix.to_str())
        .is_some_and(|suffix| matches!(suffix.to_ascii_lowercase().as_str(), "yaml" | "yml"))
}

fn read_dag_source(
    path: &Path,
    syntax_path: &Path,
) -> Result<serde_json::Map<String, Value>, DagJsonError> {
    let text = fs::read_to_string(path)
        .map_err(|error| err(format!("{}: cannot read DAG: {error}", path.display())))?;
    let raw = if is_yaml_path(syntax_path) {
        raw_from_yaml(&text)
    } else {
        raw_from_json(&text)
    }
    .map_err(|error| err(format!("{}: {error}", path.display())))?;
    as_obj(&raw, "<root>")
        .cloned()
        .map_err(|error| err(format!("{}: {error}", path.display())))
}

fn path_looks_expanded(raw: &str) -> bool {
    let looks_like_url = raw.split_once("://").is_some_and(|(scheme, _)| {
        let mut chars = scheme.chars();
        chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
            && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    });
    looks_like_url
        || raw.starts_with('~')
        || raw.contains('$')
        || raw.chars().any(|ch| matches!(ch, '*' | '?' | '['))
}

#[derive(Debug)]
struct StepSource {
    path: PathBuf,
    index: usize,
    namespace: String,
}

struct IncludeLoader {
    root: PathBuf,
    root_dir: PathBuf,
    root_policy: DagConfig,
    steps: Vec<Value>,
    resource_caps: BTreeMap<String, i64>,
    cap_sources: BTreeMap<String, PathBuf>,
    step_sources: BTreeMap<String, StepSource>,
    seen: BTreeSet<(PathBuf, String)>,
    active: Vec<PathBuf>,
    include_instances: usize,
    declared_steps: usize,
    after_references: Vec<(String, PathBuf)>,
}

impl IncludeLoader {
    fn inject_after(
        &mut self,
        start: usize,
        end: usize,
        after: &[String],
        source: &Path,
    ) -> Result<(), DagJsonError> {
        if after.is_empty() {
            return Ok(());
        }
        let subgraph_tags: BTreeSet<String> = self.steps[start..end]
            .iter()
            .filter_map(|step| {
                let object = step.as_object()?;
                Some(format!(
                    "{}.{}",
                    object.get("group")?.as_str()?,
                    object.get("job")?.as_str()?
                ))
            })
            .collect();
        let mut internal: Vec<&str> = after
            .iter()
            .filter(|tag| subgraph_tags.contains(*tag))
            .map(String::as_str)
            .collect();
        internal.sort_unstable();
        if !internal.is_empty() {
            return Err(err(format!(
                "{}: include after must name outer steps, not tag(s) inside the included \
                 subgraph: {}",
                source.display(),
                internal.join(", ")
            )));
        }
        for step in &mut self.steps[start..end] {
            let Some(object) = step.as_object_mut() else {
                continue;
            };
            let existing: Vec<String> = match object.get("deps") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => {
                    let Some(values) = values
                        .iter()
                        .map(|value| value.as_str().map(ToString::to_string))
                        .collect::<Option<Vec<_>>>()
                    else {
                        continue;
                    };
                    values
                }
                Some(_) => continue,
            };
            if existing.iter().any(|dep| subgraph_tags.contains(dep)) {
                continue;
            }
            let mut deps = existing;
            for dependency in after {
                if !deps.contains(dependency) {
                    deps.push(dependency.clone());
                }
            }
            object.insert(
                "deps".to_string(),
                Value::Array(deps.into_iter().map(Value::String).collect()),
            );
        }
        Ok(())
    }

    fn resolve_include(&self, raw: &str, source: &Path) -> Result<PathBuf, DagJsonError> {
        let relative = Path::new(raw);
        if relative.is_absolute() {
            return Err(err(format!(
                "{}: include path '{raw}' must be relative",
                source.display()
            )));
        }
        if path_looks_expanded(raw) {
            return Err(err(format!(
                "{}: include path '{raw}' must be a literal relative path (URLs, globs, '~', and \
                 environment expansion are not supported)",
                source.display()
            )));
        }
        let candidate = source
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative);
        let resolved = fs::canonicalize(&candidate).map_err(|error| {
            err(format!(
                "{}: cannot resolve include path '{raw}': {error}",
                source.display()
            ))
        })?;
        if !resolved.starts_with(&self.root_dir) {
            return Err(err(format!(
                "{}: include path '{raw}' resolves outside top-level DAG directory {}",
                source.display(),
                self.root_dir.display()
            )));
        }
        if !resolved.is_file() {
            return Err(err(format!(
                "{}: include path '{raw}' does not name a regular file",
                source.display()
            )));
        }
        Ok(resolved)
    }

    fn visit(
        &mut self,
        source: PathBuf,
        namespace: String,
        document: Option<serde_json::Map<String, Value>>,
        syntax_path: PathBuf,
        depth: usize,
    ) -> Result<(usize, usize), DagJsonError> {
        if let Some(start) = self.active.iter().position(|path| path == &source) {
            let mut chain: Vec<String> = self.active[start..]
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            chain.push(source.display().to_string());
            return Err(err(format!("include cycle: {}", chain.join(" -> "))));
        }
        if !self.seen.insert((source.clone(), namespace.clone())) {
            let display = if namespace.is_empty() {
                "<root>"
            } else {
                namespace.as_str()
            };
            return Err(err(format!(
                "duplicate include: {} is loaded more than once as namespace '{display}'",
                source.display()
            )));
        }
        self.active.push(source.clone());
        let start = self.steps.len();
        let result = self.visit_inner(source, namespace, document, syntax_path, depth);
        self.active.pop();
        result.map(|()| (start, self.steps.len()))
    }

    fn visit_inner(
        &mut self,
        source: PathBuf,
        namespace: String,
        document: Option<serde_json::Map<String, Value>>,
        syntax_path: PathBuf,
        depth: usize,
    ) -> Result<(), DagJsonError> {
        let doc = match document {
            Some(doc) => doc,
            None => read_dag_source(&source, &syntax_path)?,
        };
        if let Some(refusal) = uncarried_config_key(&doc) {
            return Err(err(format!(
                "{}: {}",
                source.display(),
                refusal.strip_prefix("<root>: ").unwrap_or(&refusal)
            )));
        }
        let includes = include_entries(&doc, &source.display().to_string())?;
        let steps = match doc.get("steps") {
            Some(Value::Array(steps)) => steps.clone(),
            _ => {
                let error = steps_list_error(&doc).to_string();
                return Err(err(format!(
                    "{}: {}",
                    source.display(),
                    error.strip_prefix("<root>: ").unwrap_or(&error)
                )));
            }
        };
        self.declared_steps = self.declared_steps.saturating_add(steps.len());
        if self.declared_steps > MAX_DAG_FLATTENED_STEPS {
            return Err(err(format!(
                "{}: composed DAG exceeds {MAX_DAG_FLATTENED_STEPS} flattened steps",
                source.display()
            )));
        }
        let fragment_policy = policy_config(&doc).map_err(|error| {
            let detail = error.0.strip_prefix("<root>: ").unwrap_or(&error.0);
            err(format!("{}: {detail}", source.display()))
        })?;
        if source != self.root {
            for key in GLOBAL_POLICY_KEYS {
                if doc.contains_key(key) && !same_policy(key, &fragment_policy, &self.root_policy) {
                    return Err(err(format!(
                        "{}: global policy '{key}' conflicts with top-level DAG {}",
                        source.display(),
                        self.root.display()
                    )));
                }
            }
        }

        for (name, value) in opt_str_int_map(&doc, "resource_caps", &source.display().to_string())?
        {
            if let Some(previous) = self.resource_caps.get(&name) {
                if *previous != value {
                    let previous_source = self.cap_sources.get(&name).expect("cap source");
                    return Err(err(format!(
                        "{}: resource cap '{name}'={value} conflicts with {} value {previous}",
                        source.display(),
                        previous_source.display()
                    )));
                }
            }
            self.resource_caps.insert(name.clone(), value);
            self.cap_sources
                .entry(name)
                .or_insert_with(|| source.clone());
        }

        for (raw_path, segment, after) in includes {
            if depth >= MAX_DAG_INCLUDE_DEPTH {
                return Err(err(format!(
                    "{}: include nesting exceeds maximum depth {MAX_DAG_INCLUDE_DEPTH}",
                    source.display()
                )));
            }
            if self.include_instances >= MAX_DAG_INCLUDE_INSTANCES {
                return Err(err(format!(
                    "{}: composed DAG exceeds {MAX_DAG_INCLUDE_INSTANCES} include instances",
                    source.display()
                )));
            }
            self.include_instances += 1;
            let included = self.resolve_include(&raw_path, &source)?;
            let child_namespace = qualify(&namespace, &segment);
            let (child_start, child_end) = self.visit(
                included,
                child_namespace,
                None,
                PathBuf::from(&raw_path),
                depth + 1,
            )?;
            let qualified_after: Vec<String> =
                after.iter().map(|tag| qualify(&namespace, tag)).collect();
            self.after_references.extend(
                qualified_after
                    .iter()
                    .map(|tag| (tag.clone(), source.clone())),
            );
            self.inject_after(child_start, child_end, &qualified_after, &source)?;
        }

        for (index, entry) in steps.into_iter().enumerate() {
            let rewritten = rewrite_step(entry, &namespace);
            if let Some(object) = rewritten.as_object() {
                if let (Some(group), Some(job)) = (
                    object.get("group").and_then(Value::as_str),
                    object.get("job").and_then(Value::as_str),
                ) {
                    let tag = format!("{group}.{job}");
                    if let Some(previous) = self.step_sources.get(&tag) {
                        let is_plain_root_duplicate = source == self.root
                            && previous.path == self.root
                            && namespace.is_empty()
                            && previous.namespace.is_empty();
                        if !is_plain_root_duplicate {
                            let previous_namespace = if previous.namespace.is_empty() {
                                "<root>"
                            } else {
                                &previous.namespace
                            };
                            let this_namespace = if namespace.is_empty() {
                                "<root>"
                            } else {
                                &namespace
                            };
                            return Err(err(format!(
                                "duplicate step tag '{tag}': {} steps[{}] (namespace \
                                 '{previous_namespace}') and {} steps[{index}] (namespace \
                                 '{this_namespace}')",
                                previous.path.display(),
                                previous.index,
                                source.display()
                            )));
                        }
                    } else {
                        self.step_sources.insert(
                            tag,
                            StepSource {
                                path: source.clone(),
                                index,
                                namespace: namespace.clone(),
                            },
                        );
                    }
                }
            }
            self.steps.push(rewritten);
        }
        Ok(())
    }
}

/// Load a JSON/YAML DAG and flatten its recursively namespaced `include` fragments.
///
/// An include entry's optional `after` tags are resolved in the parent's namespace and injected
/// into every entry node of the included flattened subgraph.
pub fn dag_from_path(path: impl AsRef<Path>) -> Result<DagConfig, DagJsonError> {
    let requested = path.as_ref();
    let root = fs::canonicalize(requested)
        .map_err(|error| err(format!("{}: cannot read DAG: {error}", requested.display())))?;
    if !root.is_file() {
        return Err(err(format!(
            "{}: does not name a regular file",
            requested.display()
        )));
    }
    let root_dir = root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let root_doc = read_dag_source(&root, requested)?;
    include_entries(&root_doc, &root.display().to_string())?;
    let root_policy = policy_config(&root_doc).map_err(|error| {
        let detail = error.0.strip_prefix("<root>: ").unwrap_or(&error.0);
        err(format!("{}: {detail}", root.display()))
    })?;
    let mut loader = IncludeLoader {
        root: root.clone(),
        root_dir,
        root_policy,
        steps: Vec::new(),
        resource_caps: BTreeMap::new(),
        cap_sources: BTreeMap::new(),
        step_sources: BTreeMap::new(),
        seen: BTreeSet::new(),
        active: Vec::new(),
        include_instances: 0,
        declared_steps: 0,
        after_references: Vec::new(),
    };
    loader.visit(
        root.clone(),
        String::new(),
        Some(root_doc.clone()),
        requested.to_path_buf(),
        0,
    )?;
    for (tag, source) in &loader.after_references {
        if !loader.step_sources.contains_key(tag) {
            return Err(err(format!(
                "{}: include after references missing outer step '{tag}'",
                source.display()
            )));
        }
    }

    let mut flattened = root_doc;
    flattened.remove("include");
    flattened.insert("steps".to_string(), Value::Array(loader.steps));
    flattened.insert(
        "resource_caps".to_string(),
        Value::Object(
            loader
                .resource_caps
                .into_iter()
                .map(|(name, value)| (name, Value::Number(value.into())))
                .collect(),
        ),
    );
    dag_from_value(&Value::Object(flattened)).map_err(|error| {
        let detail = error.0.strip_prefix("<root>: ").unwrap_or(&error.0);
        err(format!("{}: {detail}", root.display()))
    })
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
    use crate::model::StructuredTestResultsManifest;

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
                "description": "multi\nline", "cmd": "true", "result_manifests": [
                {"lane":"portable","category":"applications"},
                {"kind":"structured-test-results","schema":2,
                 "path_env":"DAGRUN_TEST_COUNTS_PATH","owner":"g.j"}]}]}"#,
        )
        .unwrap();
        // dag_to_yaml output need not match Python, but must round-trip back to the same DagConfig.
        let back = dag_from_yaml(&dag_to_yaml(&cfg)).unwrap();
        assert_eq!(dag_to_json(&cfg), dag_to_json(&back));
        assert_eq!(back.steps[0].effective_result_manifests().len(), 1);
        assert_eq!(
            back.steps[0].structured_test_results_manifest().unwrap(),
            Some(&StructuredTestResultsManifest::current("g.j"))
        );
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
}
