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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::yamlconfig;

type Result<T> = std::result::Result<T, String>;

const SCHEMA: i64 = 2;
/// The wrkslots version this build is, as `min_wrkslots_version` names it;
/// always the Python edition's `wrkslots.__version__` (#1109).
pub(crate) const WRKSLOTS_VERSION: &str = "0.10.0";
const REQUIRED: [&str; 6] = [
    "schema",
    "worktrees_dir",
    "default_remote",
    "default_landed_ref",
    "heartbeat_ttl_seconds",
    "liveness_command",
];
const OPTIONAL: [&str; 23] = [
    "machine",
    "liveness_batch_command",
    "max_active_slots",
    "eden_host_cap",
    "layout",
    "cache_globs",
    "repo_cache_globs",
    "post_provision_hooks",
    "identity",
    "salvage_push_remotes",
    "ttl_rescue_ref_root",
    "reclaimable_ignored_paths",
    "disk_advisory_bytes",
    "disk_provisioning_floor_bytes",
    "disk_emergency_bytes",
    "slot_representation",
    "retire_freeze",
    "proc_scan_bound_seconds",
    "image",
    "sandbox",
    "worktrees_image",
    "min_wrkslots_version",
    "ignorable_keys",
];

/// Unknown keys this process has already warned it ignores (#1109).
static WARNED_IGNORED_KEYS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub(crate) fn version_tuple(text: &str, label: &str) -> Result<(u64, u64, u64)> {
    let invalid = || {
        format!(
            "{label} must be a version MAJOR.MINOR.PATCH, such as {WRKSLOTS_VERSION}; found {}",
            crate::storage_state::py_repr(text)
        )
    };
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 3 {
        return Err(invalid());
    }
    let mut numbers = [0u64; 3];
    for (slot, part) in numbers.iter_mut().zip(&parts) {
        let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        if !digits || (part.len() > 1 && part.starts_with('0')) {
            return Err(invalid());
        }
        *slot = part.parse().map_err(|_| invalid())?;
    }
    Ok((numbers[0], numbers[1], numbers[2]))
}

/// The key names a configuration's `ignorable_keys` lists, validated.
pub(crate) fn ignorable_keys(raw: &Map<String, Value>) -> Result<BTreeSet<String>> {
    let Some(listed) = raw.get("ignorable_keys") else {
        return Ok(BTreeSet::new());
    };
    let not_names = || "configuration.ignorable_keys must be a list of key names".to_string();
    let items = listed.as_array().ok_or_else(not_names)?;
    let mut names = BTreeSet::new();
    for item in items {
        let name = item.as_str().ok_or_else(not_names)?;
        let shaped = !name.is_empty()
            && name.len() <= 64
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if !shaped {
            return Err(format!(
                "configuration.ignorable_keys: invalid key name {}; use lowercase letters, digits and '_', starting with a letter",
                crate::storage_state::py_repr(name)
            ));
        }
        if !names.insert(name.to_string()) {
            return Err("configuration.ignorable_keys must not contain duplicates".to_string());
        }
    }
    Ok(names)
}

/// Apply `min_wrkslots_version` and `ignorable_keys` (#1109); see the
/// Python edition's `_forward_compatible_config`.
fn forward_compatible(raw: &Map<String, Value>, path: &Path) -> Result<Map<String, Value>> {
    if let Some(value) = raw.get("min_wrkslots_version") {
        let required = as_str(value, "configuration.min_wrkslots_version")?;
        if version_tuple(required, "configuration.min_wrkslots_version")?
            > version_tuple(WRKSLOTS_VERSION, "wrkslots version")?
        {
            return Err(format!(
                "configuration {} requires wrkslots {required} or later (min_wrkslots_version); this is wrkslots {WRKSLOTS_VERSION}. Run a newer wrkslots for this project",
                path.display()
            ));
        }
    }
    let ignorable = ignorable_keys(raw)?;
    let ignored: Vec<&String> = raw
        .keys()
        .filter(|key| {
            !REQUIRED.contains(&key.as_str())
                && !OPTIONAL.contains(&key.as_str())
                && ignorable.contains(*key)
        })
        .collect();
    if !ignored.is_empty() {
        let mut warned = WARNED_IGNORED_KEYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in &ignored {
            if warned.insert((*key).clone()) {
                eprintln!(
                    "WARNING: wrkslots {WRKSLOTS_VERSION} ignores configuration key {key} in {}: it does not know the key, and ignorable_keys lists it",
                    path.display()
                );
            }
        }
    }
    Ok(raw
        .iter()
        .filter(|(key, _)| !ignored.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

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

pub(crate) fn short_hostname() -> Result<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("cannot read the machine hostname: {error}"))?;
    let name = raw.trim().split('.').next().unwrap_or_default().to_string();
    validate_name(&name, "machine")?;
    Ok(name)
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

pub(crate) fn relative_inside(root: &Path, raw: &str, label: &str) -> Result<(String, PathBuf)> {
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

/// Reclaimable build-output paths follow the cache-glob rules.
fn validate_reclaimable_ignored_path(value: &str) -> Result<String> {
    validate_cache_glob(value)
        .map_err(|error| format!("configuration.reclaimable_ignored_paths: {error}"))
}

pub(crate) fn validate_hook(value: &str) -> Result<String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err("post-provision hooks must be non-empty shell commands".to_owned());
    }
    Ok(value.to_owned())
}

type HookEntries = (Vec<String>, Vec<Option<Vec<String>>>);

/// Parse `post_provision_hooks`: each entry a command, or
/// `{command, slot_types}` (#1122), mirroring the Python oracle.
/// Returns the commands and, in parallel, each one's slot types
/// (`None` for a plain command or a mapping without `slot_types`).
fn post_provision_hook_entries(value: &Value, label: &str) -> Result<HookEntries> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{label} must be a JSON array"))?;
    let mut commands: Vec<String> = Vec::new();
    let mut selections: Vec<Option<Vec<String>>> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item_label = format!("{label}[{index}]");
        if let Some(text) = item.as_str() {
            commands.push(validate_hook(text)?);
            selections.push(None);
            continue;
        }
        let Some(entry) = item.as_object() else {
            return Err(format!("{item_label} must be a JSON object"));
        };
        let mut details: Vec<String> = Vec::new();
        if !entry.contains_key("command") {
            details.push("missing command".to_owned());
        }
        let mut unknown: Vec<&str> = entry
            .keys()
            .map(String::as_str)
            .filter(|key| !matches!(*key, "command" | "slot_types"))
            .collect();
        unknown.sort_unstable();
        if !unknown.is_empty() {
            details.push(format!("unknown {}", unknown.join(", ")));
        }
        if !details.is_empty() {
            return Err(format!(
                "{item_label} has invalid fields: {}",
                details.join("; ")
            ));
        }
        commands.push(validate_hook(as_str(
            entry.get("command").unwrap_or(&Value::Null),
            &format!("{item_label}.command"),
        )?)?);
        let Some(raw_types) = entry.get("slot_types") else {
            selections.push(None);
            continue;
        };
        let Some(types) = raw_types.as_array() else {
            return Err(format!("{item_label}.slot_types must be a JSON array"));
        };
        let mut selected: Vec<String> = Vec::new();
        for (type_index, raw_type) in types.iter().enumerate() {
            let slot_type = as_str(raw_type, &format!("{item_label}.slot_types[{type_index}]"))?;
            if !matches!(slot_type, "agent" | "validate") {
                return Err(format!(
                    "{item_label}.slot_types names unknown slot type {}; expected agent or validate",
                    crate::storage_state::py_repr(slot_type)
                ));
            }
            if selected.iter().any(|chosen| chosen == slot_type) {
                return Err(format!(
                    "{item_label}.slot_types repeats {}",
                    crate::storage_state::py_repr(slot_type)
                ));
            }
            selected.push(slot_type.to_owned());
        }
        if selected.is_empty() {
            return Err(format!(
                "{item_label}.slot_types must name at least one slot type"
            ));
        }
        selections.push(Some(selected));
    }
    Ok((commands, selections))
}

/// The configured hooks that run in a new slot of `slot_type`, in order.
pub(crate) fn post_provision_hooks_for(config: &Value, slot_type: &str) -> Vec<String> {
    let hooks: Vec<String> = config
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let selections: Vec<Value> = config
        .get("post_provision_hook_slot_types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if selections.is_empty() {
        return hooks;
    }
    hooks
        .into_iter()
        .zip(selections)
        .filter_map(|(command, types)| match &types {
            Value::Null => Some(command),
            Value::Array(selected)
                if selected
                    .iter()
                    .any(|value| value.as_str() == Some(slot_type)) =>
            {
                Some(command)
            }
            _ => None,
        })
        .collect()
}

const IDENTITY_MAX_CHARS: usize = 256;

fn identity_control(character: char) -> bool {
    (character as u32) < 0x20 || character as u32 == 0x7f
}

/// Validate the `identity` section (#1096): `{name, email}`, refused
/// exactly where the existing loader refuses it. Null when absent.
fn identity_settings(raw: &Map<String, Value>) -> Result<Value> {
    let label = "configuration.identity";
    let Some(value) = raw.get("identity") else {
        return Ok(Value::Null);
    };
    let Some(section) = value.as_object() else {
        return Err(format!("{label} must be a JSON object"));
    };
    let mut missing: Vec<&str> = ["email", "name"]
        .into_iter()
        .filter(|key| !section.contains_key(*key))
        .collect();
    missing.sort_unstable();
    let mut unknown: Vec<&str> = section
        .keys()
        .map(String::as_str)
        .filter(|key| !matches!(*key, "name" | "email"))
        .collect();
    unknown.sort_unstable();
    if !missing.is_empty() || !unknown.is_empty() {
        let mut details = Vec::new();
        if !missing.is_empty() {
            details.push(format!("missing {}", missing.join(", ")));
        }
        if !unknown.is_empty() {
            details.push(format!("unknown {}", unknown.join(", ")));
        }
        return Err(format!(
            "{label} has invalid fields: {}",
            details.join("; ")
        ));
    }
    let name = as_str(&section["name"], &format!("{label}.name"))?;
    let email = as_str(&section["email"], &format!("{label}.email"))?;
    if name.trim().is_empty()
        || name != name.trim()
        || name.chars().count() > IDENTITY_MAX_CHARS
        || name.contains(['<', '>'])
        || name.chars().any(identity_control)
    {
        return Err(format!(
            "{label}.name must be a non-empty name of at most {IDENTITY_MAX_CHARS} characters, without surrounding spaces, angle brackets, or control characters"
        ));
    }
    let shaped = match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.is_empty()
                && email.chars().filter(|character| *character == '@').count() == 1
                && !email
                    .chars()
                    .any(|character| character.is_whitespace() || matches!(character, '<' | '>'))
        }
        None => false,
    };
    if !shaped || email.chars().count() > IDENTITY_MAX_CHARS || email.chars().any(identity_control)
    {
        return Err(format!(
            "{label}.email must be one address such as agent@example.com: no spaces, angle brackets, or control characters, at most {IDENTITY_MAX_CHARS} characters"
        ));
    }
    Ok(json!({"email": email, "name": name}))
}

fn validate_rescue_ref_root(value: &Value) -> Result<String> {
    let text = as_str(value, "configuration.ttl_rescue_ref_root")?;
    let root = text.trim_end_matches('/');
    if !root.starts_with("refs/") || root == "refs" {
        return Err(format!(
            "invalid ttl rescue ref root {}: it must name a namespace under refs/, such as refs/rescue or refs/heads/rescue",
            crate::storage_state::py_repr(text)
        ));
    }
    Ok(root.to_owned())
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

fn worktrees_image_settings(raw: &Map<String, Value>) -> Result<Value> {
    let Some(section) = raw.get("worktrees_image") else {
        return Ok(Value::Null);
    };
    let section = section
        .as_object()
        .ok_or_else(|| "configuration.worktrees_image must be a JSON object".to_owned())?;
    let unknown: Vec<&str> = section
        .keys()
        .map(String::as_str)
        .filter(|key| {
            !matches!(
                *key,
                "path"
                    | "size_bytes"
                    | "fs"
                    | "mount"
                    | "slot_quota_bytes"
                    | "machines"
                    | "mount_at"
                    | "quota_mode",
            )
        })
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "configuration.worktrees_image has invalid fields: unknown {}",
            unknown.join(", ")
        ));
    }
    let path = section
        .get("path")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            "configuration.worktrees_image.path must be a non-empty string".to_owned()
        })?;
    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
        return Err(
            "configuration.worktrees_image.path must be a project-relative path without '..'"
                .to_owned(),
        );
    }
    let size = section
        .get("size_bytes")
        .ok_or_else(|| "configuration.worktrees_image.size_bytes is required".to_owned())?;
    let size = as_int(
        size,
        "configuration.worktrees_image.size_bytes",
        1024_i64.pow(3),
    )?;
    let filesystem = section.get("fs").and_then(Value::as_str).unwrap_or("btrfs");
    if filesystem != "btrfs" {
        return Err(format!(
            "configuration.worktrees_image.fs must be one of btrfs, not {filesystem:?}"
        ));
    }
    let mount = section
        .get("mount")
        .and_then(Value::as_str)
        .unwrap_or("kernel");
    if mount == "fuse" {
        return Err(
            "configuration.worktrees_image.mount 'fuse' is unavailable: btrfs has no \
             unprivileged FUSE driver (no fuse2fs equivalent); use 'kernel' with \
             passwordless sudo, or do not enable worktrees_image on this host"
                .to_owned(),
        );
    }
    if !matches!(mount, "kernel" | "auto") {
        return Err(format!(
            "configuration.worktrees_image.mount must be one of kernel, auto, not {mount:?}"
        ));
    }
    let quota = match section.get("slot_quota_bytes") {
        None => Value::Null,
        Some(value) => json!(as_int(
            value,
            "configuration.worktrees_image.slot_quota_bytes",
            1
        )?),
    };
    let mount_at = section
        .get("mount_at")
        .and_then(Value::as_str)
        .unwrap_or("binds");
    if !matches!(mount_at, "binds" | "worktrees_dir") {
        return Err(format!(
            "configuration.worktrees_image.mount_at must be one of binds, worktrees_dir, not {mount_at:?}"
        ));
    }
    let quota_mode = section
        .get("quota_mode")
        .and_then(Value::as_str)
        .unwrap_or("simple");
    if !matches!(quota_mode, "simple" | "full") {
        return Err(format!(
            "configuration.worktrees_image.quota_mode must be one of simple, full, not {quota_mode:?}"
        ));
    }
    // Validated on every host, named or not, with the Python
    // edition's exact refusal.
    let machines = match section.get("machines") {
        None => Value::Null,
        Some(value) => {
            let invalid = || {
                "configuration.worktrees_image.machines must be a non-empty list of unique non-empty machine names"
                    .to_owned()
            };
            let Some(names) = value.as_array() else {
                return Err(invalid());
            };
            let mut parsed: Vec<String> = Vec::new();
            for name in names {
                let Some(name) = name.as_str() else {
                    return Err(invalid());
                };
                if name.trim().is_empty() || parsed.iter().any(|seen| seen == name) {
                    return Err(invalid());
                }
                parsed.push(name.to_string());
            }
            if parsed.is_empty() {
                return Err(invalid());
            }
            json!(parsed)
        }
    };
    Ok(json!({
        "fs": filesystem,
        "machines": machines,
        "mount": mount,
        "mount_at": mount_at,
        "path": path,
        "quota_mode": quota_mode,
        "size_bytes": size,
        "slot_quota_bytes": quota,
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
const LIMIT_KEYS: [&str; 8] = [
    "memory_max",
    "memory_high",
    "cpu_quota",
    "tasks_max",
    "io_weight",
    "all_slots_memory_max",
    "all_slots_cpu_quota",
    "slot_cpu_quotas",
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

fn slot_cpu_quotas(value: &Value) -> Result<Value> {
    const LABEL: &str = "configuration.sandbox.limits.slot_cpu_quotas";
    if value.is_null() {
        return Ok(json!([]));
    }
    let Some(items) = value.as_array() else {
        return Err(format!(
            "{LABEL} must be a list of {{slot_prefix, cpu_quota}} objects"
        ));
    };
    let mut out = Vec::new();
    for item in items {
        let Some(entry) = item.as_object() else {
            return Err(format!(
                "{LABEL} entries must be {{slot_prefix, cpu_quota}} objects"
            ));
        };
        let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        keys.sort();
        if keys != ["cpu_quota", "slot_prefix"] {
            return Err(format!(
                "{LABEL} entries must be {{slot_prefix, cpu_quota}} objects"
            ));
        }
        let prefix = entry.get("slot_prefix").unwrap_or(&Value::Null);
        let Some(prefix) = prefix.as_str().filter(|text| !text.is_empty()) else {
            return Err(format!("{LABEL} slot_prefix must be a non-empty string"));
        };
        let quota = entry.get("cpu_quota").unwrap_or(&Value::Null);
        if quota.is_null() {
            return Err(format!("{LABEL} cpu_quota must not be null"));
        }
        let quota = optional_limit(
            quota,
            "configuration.sandbox.limits.slot_cpu_quotas.cpu_quota",
        )?;
        out.push(json!({"slot_prefix": prefix, "cpu_quota": quota}));
    }
    Ok(Value::Array(out))
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
            "memory_max": null, "tasks_max": 8192, "slot_cpu_quotas": []
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
                "slot_cpu_quotas": slot_cpu_quotas(&get("slot_cpu_quotas"))?,
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
    let raw = &forward_compatible(raw, &path)?;
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
    let env_machine = std::env::var("WRKSLOTS_MACHINE")
        .ok()
        .filter(|machine| !machine.is_empty());
    let (machine, machine_source) =
        if let Some(machine) = machine_override.filter(|machine| !machine.is_empty()) {
            (machine.to_owned(), "--machine")
        } else if let Some(machine) = env_machine {
            (machine, "WRKSLOTS_MACHINE")
        } else {
            match raw.get("machine") {
                None | Some(Value::Null) => (short_hostname()?, "host name (machine absent)"),
                Some(value) => {
                    let configured = as_str(value, "configuration.machine")?;
                    if configured == "auto" {
                        (short_hostname()?, "host name (machine: auto)")
                    } else {
                        (configured.to_owned(), "configuration.machine")
                    }
                }
            }
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
    let eden_cap = match raw.get("eden_host_cap") {
        None | Some(Value::Null) => None,
        Some(value) => Some(json!(as_int(value, "configuration.eden_host_cap", 1)?)),
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
    let (hooks, hook_slot_types) = match raw.get("post_provision_hooks") {
        None => (Vec::new(), Vec::new()),
        Some(value) => post_provision_hook_entries(value, "configuration.post_provision_hooks")?,
    };
    let identity = identity_settings(raw)?;
    let ttl_rescue_ref_root = match raw.get("ttl_rescue_ref_root") {
        None => "refs/rescue".to_owned(),
        Some(value) => validate_rescue_ref_root(value)?,
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
    let reclaimable = match raw.get("reclaimable_ignored_paths") {
        None => Vec::new(),
        Some(value) => string_tuple(
            value,
            "configuration.reclaimable_ignored_paths",
            validate_reclaimable_ignored_path,
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
    // The one host-scope decision: an unlisted host sees the
    // section as absent, plus why it was skipped.
    let wt_validated = worktrees_image_settings(raw)?;
    let (wt_value, wt_skipped) = if wt_validated.is_null() {
        (Value::Null, Value::Null)
    } else {
        let machines: Vec<String> = wt_validated
            .get("machines")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if machines.is_empty() || machines.iter().any(|name| name == &machine) {
            (wt_validated, Value::Null)
        } else {
            (
                Value::Null,
                json!(format!(
                    "configuration.worktrees_image applies on machines [{}] only; this host is {machine}",
                    machines.join(", ")
                )),
            )
        }
    };
    // #1279: when the worktrees image applies but is not mounted,
    // warn once per project root per process, on stderr only, as
    // the Python loader does after its mount attempt. The Rust
    // loader never mounts, so the reason is the classified state.
    if wt_value.is_object() {
        let direct = wt_value.get("mount_at").and_then(Value::as_str) == Some("worktrees_dir");
        let mount_dir = if direct {
            worktrees.clone()
        } else {
            worktrees
                .parent()
                .map(|parent| parent.join(crate::worktrees_image::IMAGE_MOUNT_NAME))
                .unwrap_or_default()
        };
        let entries = std::fs::read_to_string("/proc/self/mountinfo")
            .map(|text| crate::image_mounts::parse_mountinfo(&text))
            .unwrap_or_default();
        let exists = mount_dir.is_dir();
        let empty = if exists {
            std::fs::read_dir(&mount_dir)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
        } else {
            true
        };
        let state = crate::worktrees_image::classify_mount(&mount_dir, &entries, exists, empty);
        if state != crate::worktrees_image::MOUNTED {
            use std::sync::OnceLock;
            static WARNED: OnceLock<Mutex<std::collections::BTreeSet<PathBuf>>> = OnceLock::new();
            let warned = WARNED.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()));
            let first = warned
                .lock()
                .map(|mut set| set.insert(root.clone()))
                .unwrap_or(false);
            if first {
                eprintln!("wrkslots: WARNING: worktrees image not mounted: {state}");
            }
        }
    }
    let mut payload = json!({
        "cache_globs": cache_globs,
        "config_path": path.to_string_lossy(),
        "control": control.to_string_lossy(),
        "default_landed_ref": landed_ref,
        "default_remote": remote,
        "disk_advisory_bytes": advisory,
        "disk_emergency_bytes": emergency,
        "disk_provisioning_floor_bytes": provisioning,
        "heartbeat_ttl_seconds": as_int(&raw["heartbeat_ttl_seconds"], "configuration.heartbeat_ttl_seconds", 1)?,
        "identity": identity,
        "image": image_settings(raw)?,
        "layout": layout,
        "liveness_batch_command": liveness_batch,
        "liveness_command": liveness.to_string_lossy(),
        "machine": machine,
        "machine_source": machine_source,
        "max_active_slots": cap,
        "post_provision_hooks": hooks,
        "post_provision_hook_slot_types": hook_slot_types,
        "reclaimable_ignored_paths": reclaimable,
        "repo_cache_globs": repo_cache_globs.iter().map(|(name, globs)| json!([name, globs])).collect::<Vec<_>>(),
        "retire_freeze": raw.get("retire_freeze").cloned().unwrap_or(Value::Null),
        "root": root.to_string_lossy(),
        "salvage_push_remotes": salvage,
        "ttl_rescue_ref_root": ttl_rescue_ref_root,
        "sandbox": sandbox_settings(raw)?,
        "slot_representation": representation,
        "worktrees": worktrees.to_string_lossy(),
        "worktrees_image": wt_value,
        "worktrees_image_skipped": wt_skipped,
    });
    if let Some(value) = eden_cap {
        payload
            .as_object_mut()
            .expect("config payload object")
            .insert("eden_host_cap".to_owned(), value);
    }
    Ok(payload)
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

    #[test]
    fn hook_entries_and_selection_mirror_python() {
        let value = json!([
            "everywhere",
            {"command": "agent-only", "slot_types": ["agent"]},
            {"command": "both", "slot_types": ["validate", "agent"]},
            {"command": "plain-mapping"},
        ]);
        let (commands, selections) =
            post_provision_hook_entries(&value, "configuration.post_provision_hooks")
                .expect("entries");
        assert_eq!(
            commands,
            ["everywhere", "agent-only", "both", "plain-mapping"]
        );
        assert_eq!(
            selections,
            [
                None,
                Some(vec!["agent".to_owned()]),
                Some(vec!["validate".to_owned(), "agent".to_owned()]),
                None,
            ]
        );
        let config = json!({
            "post_provision_hooks": commands,
            "post_provision_hook_slot_types": selections,
        });
        assert_eq!(
            post_provision_hooks_for(&config, "validate"),
            ["everywhere", "both", "plain-mapping"]
        );
        let invalid = json!([{"command": "true", "slot_types": ["agnet"]}]);
        let error = post_provision_hook_entries(&invalid, "configuration.post_provision_hooks")
            .expect_err("unknown type");
        assert!(error.contains("unknown slot type 'agnet'"), "{error}");
    }
}
