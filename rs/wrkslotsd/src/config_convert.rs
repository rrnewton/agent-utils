//! Writer: rewrite `.wrkslots.yml` as literate YAML or as JSON.
//!
//! Reproduces `config convert`: the whole configuration is validated
//! first (an invalid file is not rewritten), then the raw document is
//! rendered in the target format under the configuration lock, keeping
//! the previous bytes in `<name>.bak`. Fixture registries only until
//! the edition cutover (see `ensure_states::require_fixture`).

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::active_state::load_cfg;
use crate::config_docs::{
    HEADER_TEMPLATE, IDENTITY_DOCS, IMAGE_DOCS, LIMIT_DOCS, OPTIONAL_KEYS, SANDBOX_DOCS, TOP_DOCS,
    WORKTREES_IMAGE_DOCS,
};
use crate::config_validate::load_config;
use crate::ensure_states::{atomic_write_bytes, require_fixture};
use crate::yamlconfig::{emit_document, load_document};

type Result<T> = std::result::Result<T, String>;

fn comments() -> HashMap<Vec<String>, String> {
    let mut out: HashMap<Vec<String>, String> = HashMap::new();
    for (key, text) in TOP_DOCS {
        out.insert(vec![key.to_string()], text.to_string());
    }
    for (key, text) in SANDBOX_DOCS {
        out.insert(
            vec!["sandbox".to_string(), key.to_string()],
            text.to_string(),
        );
    }
    for (key, text) in LIMIT_DOCS {
        out.insert(
            vec!["sandbox".to_string(), "limits".to_string(), key.to_string()],
            text.to_string(),
        );
    }
    for (key, text) in IMAGE_DOCS {
        out.insert(vec!["image".to_string(), key.to_string()], text.to_string());
    }
    for (key, text) in IDENTITY_DOCS {
        out.insert(
            vec!["identity".to_string(), key.to_string()],
            text.to_string(),
        );
    }
    for (key, text) in WORKTREES_IMAGE_DOCS {
        out.insert(
            vec!["worktrees_image".to_string(), key.to_string()],
            text.to_string(),
        );
    }
    out
}

fn order_for(path: &[String]) -> Option<&'static [(&'static str, &'static str)]> {
    match path {
        [] => Some(TOP_DOCS),
        [sandbox] if sandbox == "sandbox" => Some(SANDBOX_DOCS),
        [sandbox, limits] if sandbox == "sandbox" && limits == "limits" => Some(LIMIT_DOCS),
        [image] if image == "image" => Some(IMAGE_DOCS),
        [image] if image == "worktrees_image" => Some(WORKTREES_IMAGE_DOCS),
        [identity] if identity == "identity" => Some(IDENTITY_DOCS),
        _ => None,
    }
}

fn key_order(path: &[String], keys: &[String]) -> Vec<String> {
    let Some(known) = order_for(path) else {
        return keys.to_vec();
    };
    let known_names: Vec<&str> = known.iter().map(|(key, _)| *key).collect();
    let mut out: Vec<String> = known_names
        .iter()
        .filter(|key| keys.iter().any(|item| item == *key))
        .map(|key| key.to_string())
        .collect();
    let mut unknown: Vec<String> = keys
        .iter()
        .filter(|key| !known_names.contains(&key.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    out.append(&mut unknown);
    out
}

fn sorted_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sorted_value(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_value).collect()),
        other => other.clone(),
    }
}

/// Serialize a configuration: canonical literate YAML, or indented JSON.
pub(crate) fn render_config(payload: &Value, fmt: &str) -> Result<String> {
    if fmt == "json" {
        let pretty = serde_json::to_string_pretty(&sorted_value(payload))
            .map_err(|error| error.to_string())?;
        let mut text = crate::ensure_states::escape_ascii_json(&pretty);
        text.push('\n');
        return Ok(text);
    }
    if fmt != "yaml" {
        return Err(format!("unknown configuration format {fmt:?}"));
    }
    let absent: Vec<&str> = TOP_DOCS
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| {
            OPTIONAL_KEYS.contains(key)
                && payload
                    .as_object()
                    .map(|map| !map.contains_key(*key))
                    .unwrap_or(false)
        })
        .collect();
    let footer = if absent.is_empty() {
        String::new()
    } else {
        format!(
            "Optional keys not set here (absent keeps their documented meaning): {}.",
            absent.join(", ")
        )
    };
    let header = HEADER_TEMPLATE
        .replace("{name}", ".wrkslots.yml")
        .replace("{{", "{")
        .replace("}}", "}");
    emit_document(payload, &header, &footer, &comments(), &key_order)
}

fn refuse_symlink(path: &Path, label: &str) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("{label} must not be a symlink: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect {label} {}: {error}",
            path.display()
        )),
    }
}

pub(crate) fn write_config(path: &Path, payload: &Value, fmt: &str) -> Result<()> {
    let mut previous: Option<Vec<u8>> = None;
    if path.exists() || path.is_symlink() {
        refuse_symlink(path, "configuration")?;
        previous =
            Some(std::fs::read(path).map_err(|error| {
                format!("cannot read configuration {}: {error}", path.display())
            })?);
    }
    let text = render_config(payload, fmt)?;
    if let Some(previous) = &previous {
        if *previous == text.as_bytes() {
            return Ok(());
        }
        let backup = path.with_file_name(format!(
            "{}.bak",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        refuse_symlink(&backup, "configuration backup")?;
        if let Some(parent) = backup.parent() {
            if let Ok(entries) = std::fs::read_dir(parent) {
                let prefix = format!(
                    "{}.tmp.",
                    backup
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with(&prefix) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        atomic_write_bytes(&backup, previous)?;
    }
    atomic_write_bytes(path, text.as_bytes())
}

/// Rewrite the configuration in `target` format, printing the outcome.
pub(crate) fn config_convert(
    root: Option<&Path>,
    machine_override: Option<&str>,
    target: &str,
) -> Result<String> {
    require_fixture()?;
    let _validated = load_config(root, machine_override)?;
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let config_path = cfg.root.join(".wrkslots.yml");
    let _lock = crate::hold_write::acquire_config_lock(&cfg)?;
    refuse_symlink(&config_path, "configuration")?;
    let text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("missing configuration: {}", config_path.display()))
        }
        Err(error) => {
            return Err(format!(
                "cannot read configuration {}: {error}",
                config_path.display()
            ))
        }
    };
    let (raw, current) =
        load_document(&text).map_err(|error| format!("configuration is malformed: {error}"))?;
    if !raw.is_object() {
        return Err(format!(
            "configuration must be a mapping: {}",
            config_path.display()
        ));
    }
    write_config(&config_path, &raw, target)?;
    let backup = config_path.with_file_name(".wrkslots.yml.bak");
    Ok(format!(
        "wrote {} as {target} (was {current}); previous bytes in {}\n",
        config_path.display(),
        backup.display()
    ))
}
