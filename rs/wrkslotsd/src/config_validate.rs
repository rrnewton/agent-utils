//! `load_config` validation parity: resolve and validate `.wrkslots.yml`.
//!
//! Reproduces the existing loader's checks and normalized `Config` document
//! for a fixture root: root discovery by explicit `--root`, exact top-level
//! keys, schema, machine, relative-inside paths with symlink refusal, layout
//! and control directory, remote and landed-ref rules, liveness commands,
//! caps, glob/hook tuples, disk thresholds, and the image and sandbox
//! sections with their defaults spelled out. Read-only: unlike the existing
//! loader it never activates (mounts) image-backed slots; on a `worktree`
//! project (the default) the loaders agree exactly.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::yamlconfig;

type Result<T> = std::result::Result<T, String>;

const SCHEMA: i64 = 2;
const REQUIRED: [&str; 7] = [
    "schema",
    "worktrees_dir",
    "machine",
    "default_remote",
    "default_landed_ref",
    "heartbeat_ttl_seconds",
    "liveness_command",
];
const OPTIONAL: [&str; 13] = [
    "liveness_batch_command",
    "max_active_slots",
    "layout",
    "cache_globs",
    "repo_cache_globs",
    "post_provision_hooks",
    "salvage_push_remotes",
    "disk_advisory_bytes",
    "disk_provisioning_floor_bytes",
    "disk_emergency_bytes",
    "slot_representation",
    "image",
    "sandbox",
];

fn name_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(crate) fn validate_name(value: &str, label: &str) -> Result<String> {
    if name_valid(value) {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "invalid {label} {value:?}; use 1-64 letters, digits, '.', '_', or '-'"
        ))
    }
}

pub(crate) fn validate_remote(value: &str) -> Result<String> {
    let shaped = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
    if !shaped || value.starts_with('-') || value.ends_with('/') || value.contains("..") {
        return Err(format!("invalid remote name {value:?}"));
    }
    Ok(value.to_owned())
}

fn validate_ref(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.contains("..")
        || value.ends_with('/')
        || value.ends_with('.')
    {
        return Err(format!("invalid {label} {value:?}"));
    }
    Ok(())
}

pub(crate) fn validate_full_ref(value: &str, label: &str) -> Result<String> {
    validate_ref(value, label)?;
    let forbidden = [' ', '~', '^', ':', '?', '*', '[', '\\'];
    let parts: Vec<&str> = value.split('/').collect();
    let bad = !value.starts_with("refs/")
        || parts.len() < 3
        || parts
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || value.contains("@{")
        || value.chars().any(|c| (c as u32) < 32 || (c as u32) == 127)
        || value.chars().any(|c| forbidden.contains(&c));
    if bad {
        return Err(format!("invalid full {label} {value:?}"));
    }
    Ok(value.to_owned())
}

fn as_str<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| format!("{label} must be a string"))
}

fn as_int(value: &Value, label: &str, minimum: i64) -> Result<i64> {
    match value.as_i64() {
        Some(number) if !matches!(value, Value::Bool(_)) && number >= minimum => Ok(number),
        _ => Err(format!("{label} must be an integer >= {minimum}")),
    }
}

fn exact_keys(raw: &Map<String, Value>, label: &str) -> Result<()> {
    let missing: Vec<&str> = REQUIRED
        .iter()
        .filter(|key| !raw.contains_key(**key))
        .copied()
        .collect();
    let mut unknown: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|key| !REQUIRED.contains(key) && !OPTIONAL.contains(key))
        .collect();
    unknown.sort_unstable();
    if missing.is_empty() && unknown.is_empty() {
        return Ok(());
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {}", missing.join(", ")));
    }
    if !unknown.is_empty() {
        details.push(format!("unknown {}", unknown.join(", ")));
    }
    Err(format!(
        "{label} has invalid fields: {}",
        details.join("; ")
    ))
}

pub(crate) fn refuse_symlink(path: &Path, label: &str) -> Result<()> {
    if path.is_symlink() {
        return Err(format!("{label} must not be a symlink: {}", path.display()));
    }
    Ok(())
}

pub(crate) fn ensure_no_symlink_components(root: &Path, path: &Path, label: &str) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{label} escapes the project root: {}", path.display()))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if current.is_symlink() {
            return Err(format!("{label} crosses a symlink: {}", current.display()));
        }
    }
    Ok(())
}

/// Lexical normalization matching `os.path.normpath` for relative paths.
pub(crate) fn normpath(raw: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|last| *last != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.is_empty() {
        ".".to_owned()
    } else {
        out.join("/")
    }
}

fn relative_inside(root: &Path, raw: &str, label: &str) -> Result<(String, PathBuf)> {
    let parts: Vec<&str> = raw.split('/').collect();
    if raw.starts_with('/') || raw.is_empty() || parts.contains(&"..") {
        return Err(format!(
            "{label} must be a relative path inside {}: {raw:?}",
            root.display()
        ));
    }
    let lexical = normpath(raw);
    if lexical == "." {
        return Err(format!("{label} must not be the project root"));
    }
    let absolute = root.join(&lexical);
    ensure_no_symlink_components(root, &absolute, label)?;
    Ok((lexical, absolute))
}

pub(crate) fn validate_layout(value: &str) -> Result<String> {
    if matches!(value, "nested" | "flat") {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "invalid layout {value:?}; expected 'nested' or 'flat'"
        ))
    }
}

fn control_directory_for(worktrees: &Path, layout: &str) -> PathBuf {
    if worktrees.file_name().is_some_and(|name| name == "slots") {
        return worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
    }
    if layout == "nested" {
        worktrees.to_path_buf()
    } else {
        worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }
}

fn validate_cache_glob(value: &str) -> Result<String> {
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        return Err(
            "cache globs must be non-empty relative paths without surrounding space".to_owned(),
        );
    }
    if value.starts_with('/') || value.split('/').any(|part| part == "..") {
        return Err(format!(
            "cache glob must stay inside each checkout: {value:?}"
        ));
    }
    let normalized = normpath(value);
    if normalized.is_empty() || normalized == "." {
        return Err("cache glob must not name the checkout root".to_owned());
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts
        .iter()
        .any(|part| part.contains("**") && *part != "**")
    {
        return Err(
            "cache glob recursive wildcards ('**') must occupy a complete path component"
                .to_owned(),
        );
    }
    if parts.contains(&".git") {
        return Err("cache globs must never name Git administrative data".to_owned());
    }
    if parts[0].chars().any(|c| matches!(c, '*' | '?' | '[')) {
        return Err(
            "cache glob's first path component must be literal so it cannot match source broadly"
                .to_owned(),
        );
    }
    Ok(normalized)
}

fn string_tuple(
    value: &Value,
    label: &str,
    validator: fn(&str) -> Result<String>,
    unique: bool,
) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{label} must be a JSON array"))?;
    let mut result: Vec<String> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let raw = item
            .as_str()
            .ok_or_else(|| format!("{label}[{index}] must be a string"))?;
        let validated = validator(raw)?;
        if unique && result.contains(&validated) {
            return Err(format!("{label} contains duplicate entry {validated:?}"));
        }
        result.push(validated);
    }
    Ok(result)
}

pub(crate) fn validate_hook(value: &str) -> Result<String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err("post-provision hooks must be non-empty shell commands".to_owned());
    }
    Ok(value.to_owned())
}

fn validate_salvage_remote(value: &str) -> Result<String> {
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        return Err("salvage push remotes must be non-empty remote URLs".to_owned());
    }
    Ok(value.to_owned())
}

fn image_settings(raw: &Map<String, Value>) -> Result<Value> {
    let (mut ceiling, mut state_ceiling, mut backend) =
        (512_i64 * 1024_i64.pow(3), 128_i64 * 1024_i64.pow(3), "auto");
    if let Some(section) = raw.get("image") {
        let section = section
            .as_object()
            .ok_or_else(|| "configuration.image must be a JSON object".to_owned())?;
        let unknown: Vec<&str> = section
            .keys()
            .map(String::as_str)
            .filter(|key| !matches!(*key, "ceiling_bytes" | "state_ceiling_bytes" | "backend"))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "configuration.image has invalid fields: unknown {}",
                unknown.join(", ")
            ));
        }
        if let Some(value) = section.get("ceiling_bytes") {
            ceiling = as_int(value, "configuration.image.ceiling_bytes", 1024_i64.pow(3))?;
        }
        if let Some(value) = section.get("state_ceiling_bytes") {
            state_ceiling = as_int(
                value,
                "configuration.image.state_ceiling_bytes",
                1024_i64.pow(3),
            )?;
        }
        if let Some(value) = section.get("backend") {
            backend = as_str(value, "configuration.image.backend")?;
            if !matches!(backend, "auto" | "kernel" | "fuse") {
                return Err(
                    "configuration.image.backend must be one of auto, kernel, fuse".to_owned(),
                );
            }
        }
    }
    Ok(json!({
        "backend": backend,
        "ceiling_bytes": ceiling,
        "state_ceiling_bytes": state_ceiling,
    }))
}

const SANDBOX_KEYS: [&str; 14] = [
    "isolation",
    "home",
    "home_shared",
    "home_private",
    "home_private_files",
    "home_hidden",
    "home_expose",
    "outputs",
    "read_write",
    "env",
    "tmp_size",
    "protect_system",
    "coordinator_writable",
    "limits",
];
const LIMIT_KEYS: [&str; 7] = [
    "memory_max",
    "memory_high",
    "cpu_quota",
    "tasks_max",
    "io_weight",
    "all_slots_memory_max",
    "all_slots_cpu_quota",
];
const RENAMED_KEYS: [(&str, &str); 9] = [
    ("home_writable", "home_private"),
    ("home_paths", "home_expose (with home: hidden)"),
    ("memory_max", "limits.memory_max"),
    ("memory_high", "limits.memory_high"),
    ("cpu_quota", "limits.cpu_quota"),
    ("tasks_max", "limits.tasks_max"),
    ("io_weight", "limits.io_weight"),
    ("all_slots_memory_max", "limits.all_slots_memory_max"),
    ("all_slots_cpu_quota", "limits.all_slots_cpu_quota"),
];

fn relative_path(value: &str, label: &str) -> Result<String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.split('/').any(|part| part == "..")
        || normpath(value) == "."
    {
        return Err(format!(
            "{label} entries must be relative paths without '..': {value:?}"
        ));
    }
    Ok(normpath(value))
}

fn str_list(value: &Value, label: &str) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{label} must be a list of strings"))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{label} must be a list of strings"))
        })
        .collect()
}

fn relative_list(raw: &Map<String, Value>, key: &str, default: &[&str]) -> Result<Vec<String>> {
    let label = format!("configuration.sandbox.{key}");
    let values = match raw.get(key) {
        None => default.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        Some(value) => str_list(value, &label)?,
    };
    let mut result: Vec<String> = Vec::new();
    for item in values {
        let validated = relative_path(&item, &label)?;
        if !result.contains(&validated) {
            result.push(validated);
        }
    }
    Ok(result)
}

fn limit_shape(value: &str) -> bool {
    // ^(infinity|\d+(\.\d+)?[KMGT]?|\d+(\.\d+)?%)$
    if value == "infinity" {
        return true;
    }
    let (number, suffix) = if let Some(stripped) = value.strip_suffix('%') {
        (stripped, "%")
    } else if let Some(last) = value.chars().last() {
        if matches!(last, 'K' | 'M' | 'G' | 'T') {
            (&value[..value.len() - 1], "unit")
        } else {
            (value, "")
        }
    } else {
        return false;
    };
    let _ = suffix;
    let mut parts = number.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(fraction) => !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()),
    }
}

fn optional_limit(value: &Value, label: &str) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let text = value
        .as_str()
        .filter(|text| limit_shape(text))
        .ok_or_else(|| format!("{label} must look like 32G, 800%, or infinity, not {value}"))?;
    Ok(Value::String(text.to_owned()))
}

fn optional_positive_int(value: &Value, label: &str) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match value.as_i64() {
        Some(number) if number >= 1 => Ok(json!(number)),
        _ => Err(format!("{label} must be a positive integer or null")),
    }
}

fn sandbox_settings(raw: &Map<String, Value>) -> Result<Value> {
    let section = match raw.get("sandbox") {
        None => Map::new(),
        Some(value) => value
            .as_object()
            .ok_or_else(|| "configuration.sandbox must be a JSON object".to_owned())?
            .clone(),
    };
    let mut unknown: Vec<String> = section
        .keys()
        .filter(|key| !SANDBOX_KEYS.contains(&key.as_str()))
        .map(|key| {
            RENAMED_KEYS
                .iter()
                .find(|(old, _)| *old == key.as_str())
                .map_or_else(|| key.clone(), |(old, new)| format!("{old} (now {new})"))
        })
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        return Err(format!(
            "configuration.sandbox has unknown keys: {}",
            unknown.join(", ")
        ));
    }
    let text = |key: &str, default: &str| -> String {
        section
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_owned()
    };
    let isolation = text("isolation", "userns");
    if !matches!(isolation.as_str(), "userns" | "cgroup" | "root") {
        let hint = if isolation == "namespace" {
            " (namespace was renamed userns)"
        } else {
            ""
        };
        return Err(format!(
            "configuration.sandbox.isolation must be one of userns, cgroup, root{hint}"
        ));
    }
    let home = text("home", "ro");
    if !matches!(home.as_str(), "ro" | "hidden") {
        let hint = if matches!(home.as_str(), "select" | "none") {
            " (use hidden with home_expose)"
        } else {
            ""
        };
        return Err(format!(
            "configuration.sandbox.home must be one of ro, hidden{hint}"
        ));
    }
    let protect = match section.get("protect_system") {
        None => true,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return Err("configuration.sandbox.protect_system must be true or false".to_owned())
        }
    };
    let tmp_size = text("tmp_size", "16G");
    let tmp_ok = {
        let digits = tmp_size.trim_end_matches(['K', 'M', 'G', 'T', '%']);
        !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && tmp_size[digits.len()..].len() <= 1
    };
    if !tmp_ok {
        return Err(format!(
            "configuration.sandbox.tmp_size must look like 16G or 50%, not {tmp_size:?}"
        ));
    }
    let scope = text("coordinator_writable", "worktrees");
    if !matches!(scope.as_str(), "worktrees" | "project") {
        return Err(
            "configuration.sandbox.coordinator_writable must be one of worktrees, project"
                .to_owned(),
        );
    }
    let env = match section.get("env") {
        None => json!({}),
        Some(value) => {
            let object = value.as_object().ok_or_else(|| {
                "configuration.sandbox.env must be an object of NAME: value strings".to_owned()
            })?;
            for (key, item) in object {
                let name_ok = !key.is_empty()
                    && key
                        .bytes()
                        .next()
                        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if !name_ok {
                    return Err(format!(
                        "configuration.sandbox.env has an invalid variable name: {key:?}"
                    ));
                }
                let item = item.as_str().ok_or_else(|| {
                    format!("configuration.sandbox.env.{key} must be a one-line string")
                })?;
                if item.contains('\n') || item.contains('\0') {
                    return Err(format!(
                        "configuration.sandbox.env.{key} must be a one-line string"
                    ));
                }
            }
            value.clone()
        }
    };
    let limits = match section.get("limits") {
        None => json!({
            "all_slots_cpu_quota": null, "all_slots_memory_max": null,
            "cpu_quota": null, "io_weight": null, "memory_high": null,
            "memory_max": null, "tasks_max": 8192
        }),
        Some(value) => {
            let object = value
                .as_object()
                .ok_or_else(|| "configuration.sandbox.limits must be an object".to_owned())?;
            let unknown: Vec<&str> = object
                .keys()
                .map(String::as_str)
                .filter(|key| !LIMIT_KEYS.contains(key))
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "configuration.sandbox.limits has unknown keys: {}",
                    unknown.join(", ")
                ));
            }
            let get = |key: &str| object.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "all_slots_cpu_quota": optional_limit(&get("all_slots_cpu_quota"), "configuration.sandbox.limits.all_slots_cpu_quota")?,
                "all_slots_memory_max": optional_limit(&get("all_slots_memory_max"), "configuration.sandbox.limits.all_slots_memory_max")?,
                "cpu_quota": optional_limit(&get("cpu_quota"), "configuration.sandbox.limits.cpu_quota")?,
                "io_weight": optional_positive_int(&get("io_weight"), "configuration.sandbox.limits.io_weight")?,
                "memory_high": optional_limit(&get("memory_high"), "configuration.sandbox.limits.memory_high")?,
                "memory_max": optional_limit(&get("memory_max"), "configuration.sandbox.limits.memory_max")?,
                "tasks_max": match object.get("tasks_max") {
                    None => json!(8192),
                    Some(value) => optional_positive_int(value, "configuration.sandbox.limits.tasks_max")?,
                },
            })
        }
    };
    let private_files = relative_list(&section, "home_private_files", &[".claude.json"])?;
    for value in &private_files {
        if value.contains('/') {
            return Err(format!(
                "configuration.sandbox.home_private_files entries must be top-level $HOME names: {value:?}"
            ));
        }
    }
    let read_write = match section.get("read_write") {
        None => Vec::new(),
        Some(value) => {
            let label = "configuration.sandbox.read_write";
            let mut result: Vec<String> = Vec::new();
            for item in str_list(value, label)? {
                if !(item.starts_with('/') || item == "~" || item.starts_with("~/")) {
                    return Err(format!(
                        "{label} entries must be absolute or ~/-relative paths: {item:?}"
                    ));
                }
                if !result.contains(&item) {
                    result.push(item);
                }
            }
            result
        }
    };
    Ok(json!({
        "coordinator_writable": scope,
        "env": env,
        "home": home,
        "home_expose": relative_list(&section, "home_expose", &["bin", ".local/bin"])?,
        "home_hidden": relative_list(&section, "home_hidden", &[".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker/config.json", ".netrc", ".git-credentials", ".pgpass", ".arcrc", ".config/gh/hosts.yml", ".config/gcloud"])?,
        "home_private": relative_list(&section, "home_private", &[".cache", ".buck"])?,
        "home_private_files": private_files,
        "home_shared": relative_list(&section, "home_shared", &[".claude", ".codex", ".muse", ".config/muse", ".config/opencode", ".local/share/opencode", ".local/state/herdr", ".local/share/muse", ".cargo/registry", ".cargo/git"])?,
        "isolation": isolation,
        "limits": limits,
        "outputs": relative_list(&section, "outputs", &["ai_docs", "experiments"])?,
        "protect_system": protect,
        "read_write": read_write,
        "tmp_size": tmp_size,
    }))
}

fn cache_glob_groups(value: &Value, label: &str) -> Result<Vec<(String, Vec<String>)>> {
    let raw = value
        .as_object()
        .ok_or_else(|| format!("{label} must be a JSON object"))?;
    let mut names: Vec<&String> = raw.keys().collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        validate_name(name, &format!("{label} repository name"))?;
        let globs = string_tuple(
            &raw[name],
            &format!("{label}.{name}"),
            validate_cache_glob,
            true,
        )?;
        if globs.is_empty() {
            return Err(format!(
                "{label}.{name} must contain at least one cache glob"
            ));
        }
        result.push((name.clone(), globs));
    }
    Ok(result)
}

fn authoritative_candidate(root: &Path, config_path: &Path) -> bool {
    if config_path.is_symlink() {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return true;
    };
    let Ok((document, _format)) = yamlconfig::load_document(&text) else {
        return true;
    };
    let Some(raw) = document.as_object() else {
        return true;
    };
    let Some(worktrees_value) = raw.get("worktrees_dir").and_then(Value::as_str) else {
        return true;
    };
    let layout = raw
        .get("layout")
        .and_then(Value::as_str)
        .unwrap_or("nested");
    if !matches!(layout, "nested" | "flat") {
        return true;
    }
    if worktrees_value.starts_with('/')
        || worktrees_value.is_empty()
        || worktrees_value.split('/').any(|part| part == "..")
    {
        return true;
    }
    let worktrees = root.join(normpath(worktrees_value));
    let control = if layout == "nested" {
        worktrees.clone()
    } else {
        worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    };
    if control.join("wrkslots").is_symlink() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(&control) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        (name.starts_with("ACTIVE.") && (name.ends_with(".json") || name.ends_with(".journal")))
            || (name.starts_with("ARCHIVED.") && name.ends_with(".json"))
            || (name.starts_with("CREATE.") && name.ends_with(".journal"))
            || (name.starts_with("FINISH.") && name.ends_with(".journal"))
            || (name.starts_with("VALIDATE-BATCH-SEAL.") && name.ends_with(".journal"))
            || name.starts_with("EVENTS.")
    })
}

fn discover_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        let candidate = if explicit.is_absolute() {
            explicit.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| format!("cannot resolve the working directory: {error}"))?
                .join(explicit)
        };
        if candidate.is_symlink() || !candidate.is_dir() {
            return Err(format!(
                "project root must be an existing real directory: {}",
                candidate.display()
            ));
        }
        return candidate
            .canonicalize()
            .map_err(|error| format!("cannot resolve project root: {error}"));
    }
    let current = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot resolve the working directory: {error}"))?;
    let mut fallback: Option<PathBuf> = None;
    let mut candidate: Option<PathBuf> = Some(current.clone());
    while let Some(root) = candidate {
        let config_path = root.join(".wrkslots.yml");
        if config_path.exists() || config_path.is_symlink() {
            fallback = Some(root.clone());
            if authoritative_candidate(&root, &config_path) {
                return Ok(root);
            }
        }
        candidate = root.parent().map(Path::to_path_buf);
    }
    if let Some(root) = fallback {
        return Ok(root);
    }
    Err("no .wrkslots.yml found; run 'wrkslots init' from the project root".to_owned())
}

fn runnable_command(root: &Path, raw: &Value, label: &str, field: &str) -> Result<PathBuf> {
    let text = as_str(raw, &format!("configuration.{field}"))?;
    let (_relative, absolute) = relative_inside(root, text, label)?;
    let runnable = absolute.is_file()
        && !absolute.is_symlink()
        && (absolute.extension().is_some_and(|ext| ext == "py") || is_executable(&absolute));
    if !runnable {
        return Err(format!(
            "configured {label} is missing, not runnable, or unsafe: {}",
            absolute.display()
        ));
    }
    Ok(absolute)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// Validate the project at `root` (or discovered from the cwd) and return
/// the normalized configuration document.
pub(crate) fn load_config(root: Option<&Path>, machine_override: Option<&str>) -> Result<Value> {
    let root = discover_root(root)?;
    let path = root.join(".wrkslots.yml");
    refuse_symlink(&path, "configuration")?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing configuration: {}", path.display())
        } else {
            format!("cannot read configuration {}: {error}", path.display())
        }
    })?;
    let (document, _format) = yamlconfig::load_document(&text)
        .map_err(|error| format!("configuration is malformed: {}: {error}", path.display()))?;
    let raw = document
        .as_object()
        .ok_or_else(|| format!("configuration must be a mapping: {}", path.display()))?;
    exact_keys(raw, "configuration").map_err(|error| {
        format!(
            "{error} in {}. Migrate it with `wrkslots init --repair` (it refuses to change any field that already has a conflicting value).",
            path.display()
        )
    })?;
    let schema = as_int(&raw["schema"], "configuration.schema", 0)?;
    if schema != SCHEMA {
        return Err(format!(
            "unsupported configuration schema in {}: found {}, this build speaks {SCHEMA}. Migrate it with `wrkslots init --repair` (it refuses to change any field that already has a conflicting value).",
            path.display(),
            raw["schema"]
        ));
    }
    let machine = match machine_override {
        Some(machine) => machine.to_owned(),
        None => match std::env::var("WRKSLOTS_MACHINE") {
            Ok(machine) => machine,
            Err(_) => as_str(&raw["machine"], "configuration.machine")?.to_owned(),
        },
    };
    validate_name(&machine, "machine")?;
    let (_rel, worktrees) = relative_inside(
        &root,
        as_str(&raw["worktrees_dir"], "configuration.worktrees_dir")?,
        "worktrees_dir",
    )?;
    if !worktrees.is_dir() || worktrees.is_symlink() {
        return Err(format!(
            "configured worktrees directory is missing or unsafe: {}",
            worktrees.display()
        ));
    }
    let layout = validate_layout(
        raw.get("layout")
            .map(|value| as_str(value, "configuration.layout"))
            .transpose()?
            .unwrap_or("nested"),
    )?;
    let control = control_directory_for(&worktrees, &layout);
    if !control.is_dir() || control.is_symlink() {
        return Err(format!(
            "configured control directory is missing or unsafe: {}",
            control.display()
        ));
    }
    let remote = validate_remote(as_str(
        &raw["default_remote"],
        "configuration.default_remote",
    )?)?;
    let landed_ref = validate_full_ref(
        as_str(
            &raw["default_landed_ref"],
            "configuration.default_landed_ref",
        )?,
        "default landed ref",
    )?;
    let liveness = runnable_command(
        &root,
        &raw["liveness_command"],
        "liveness command",
        "liveness_command",
    )?;
    let liveness_batch = match raw.get("liveness_batch_command") {
        None => Value::Null,
        Some(value) => {
            let absolute = runnable_command(
                &root,
                value,
                "batch liveness command",
                "liveness_batch_command",
            )?;
            json!(absolute.to_string_lossy())
        }
    };
    let cap = match raw.get("max_active_slots") {
        None | Some(Value::Null) => Value::Null,
        Some(value) => json!(as_int(value, "configuration.max_active_slots", 0)?),
    };
    let cache_globs = match raw.get("cache_globs") {
        None => Vec::new(),
        Some(value) => string_tuple(
            value,
            "configuration.cache_globs",
            validate_cache_glob,
            true,
        )?,
    };
    let repo_cache_globs = match raw.get("repo_cache_globs") {
        None => Vec::new(),
        Some(value) => cache_glob_groups(value, "configuration.repo_cache_globs")?,
    };
    let hooks = match raw.get("post_provision_hooks") {
        None => Vec::new(),
        Some(value) => string_tuple(
            value,
            "configuration.post_provision_hooks",
            validate_hook,
            false,
        )?,
    };
    let salvage = match raw.get("salvage_push_remotes") {
        None => Vec::new(),
        Some(value) => string_tuple(
            value,
            "configuration.salvage_push_remotes",
            validate_salvage_remote,
            true,
        )?,
    };
    let disk_keys = [
        "disk_advisory_bytes",
        "disk_provisioning_floor_bytes",
        "disk_emergency_bytes",
    ];
    let present = disk_keys
        .iter()
        .filter(|key| raw.contains_key(**key))
        .count();
    let (advisory, provisioning, emergency) = if present == 0 {
        (Value::Null, Value::Null, Value::Null)
    } else if present != 3 {
        return Err(
            "configuration disk thresholds must supply advisory, provisioning floor, and emergency bytes together"
                .to_owned(),
        );
    } else {
        let advisory = as_int(&raw[disk_keys[0]], "configuration.disk_advisory_bytes", 1)?;
        let provisioning = as_int(
            &raw[disk_keys[1]],
            "configuration.disk_provisioning_floor_bytes",
            1,
        )?;
        let emergency = as_int(&raw[disk_keys[2]], "configuration.disk_emergency_bytes", 1)?;
        if !(emergency < provisioning && provisioning < advisory) {
            return Err(
                "configuration disk thresholds must satisfy emergency < provisioning floor < advisory"
                    .to_owned(),
            );
        }
        (json!(advisory), json!(provisioning), json!(emergency))
    };
    let representation = match raw.get("slot_representation") {
        None => "worktree".to_owned(),
        Some(value) => {
            let text = value.as_str().unwrap_or_default();
            if !matches!(text, "worktree" | "image") {
                return Err(format!(
                    "configuration.slot_representation must be one of worktree, image, not {value}"
                ));
            }
            text.to_owned()
        }
    };
    Ok(json!({
        "cache_globs": cache_globs,
        "config_path": path.to_string_lossy(),
        "control": control.to_string_lossy(),
        "default_landed_ref": landed_ref,
        "default_remote": remote,
        "disk_advisory_bytes": advisory,
        "disk_emergency_bytes": emergency,
        "disk_provisioning_floor_bytes": provisioning,
        "heartbeat_ttl_seconds": as_int(&raw["heartbeat_ttl_seconds"], "configuration.heartbeat_ttl_seconds", 1)?,
        "image": image_settings(raw)?,
        "layout": layout,
        "liveness_batch_command": liveness_batch,
        "liveness_command": liveness.to_string_lossy(),
        "machine": machine,
        "max_active_slots": cap,
        "post_provision_hooks": hooks,
        "repo_cache_globs": repo_cache_globs.iter().map(|(name, globs)| json!([name, globs])).collect::<Vec<_>>(),
        "root": root.to_string_lossy(),
        "salvage_push_remotes": salvage,
        "sandbox": sandbox_settings(raw)?,
        "slot_representation": representation,
        "worktrees": worktrees.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_globs_stay_literal_first() {
        assert_eq!(validate_cache_glob("target/**").expect("glob"), "target/**");
        assert!(validate_cache_glob("*/x").is_err());
        assert!(validate_cache_glob(".git/x").is_err());
    }
}
