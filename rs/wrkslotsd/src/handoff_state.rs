//! Generation-bound handoff sidecar validation parity.
//!
//! Reproduces the existing sidecar loader bound to its active record:
//! bounded regular-file reading with identity stability checks, strict
//! JSON with duplicate-key refusal at every depth, schema 1/2 field and
//! provenance rules, and content digest binding, printing the artifact in
//! the writer's serialized shape. Read-only; it never enqueues retirement,
//! appends history, or touches the slot.

use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest as _, Sha256};

use crate::active_state::{
    as_int, as_mapping, as_str, exact_keys, load_cfg, parse_state, parse_timestamp, slot_directory,
    Cfg,
};
use crate::config_validate::validate_name;

type Result<T> = std::result::Result<T, String>;

const SIDECAR_SCHEMA: i64 = 2;
const LEGACY_SCHEMA: i64 = 1;
const HANDOFF_BYTES_LIMIT: usize = 1024 * 1024;
pub(crate) const SIDECAR_BYTES_LIMIT: usize = HANDOFF_BYTES_LIMIT * 6 + 64 * 1024;

pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

fn bound_text(limit: usize) -> String {
    if limit == HANDOFF_BYTES_LIMIT {
        "1 MiB".to_owned()
    } else {
        format!("{limit}-byte")
    }
}

pub(crate) fn read_regular_file_identity(
    path: &Path,
    label: &str,
    limit: usize,
) -> Result<(Vec<u8>, FileIdentity)> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    if metadata.len() > limit as u64 {
        return Err(format!(
            "{label} exceeds the {} safety bound: {}",
            bound_text(limit),
            path.display()
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    let before = file
        .metadata()
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    let mut contents = Vec::new();
    (&file)
        .take((limit + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    let path_after = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot recheck {label} path {}: {error}", path.display()))?;
    if contents.len() > limit {
        return Err(format!(
            "{label} exceeds the {} safety bound: {}",
            bound_text(limit),
            path.display()
        ));
    }
    let stable = |left: &std::fs::Metadata, right: &std::fs::Metadata| {
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.mode() == right.mode()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.ctime() == right.ctime()
    };
    if !stable(&before, &after) {
        return Err(format!(
            "{label} changed while it was read: {}",
            path.display()
        ));
    }
    if !stable(&after, &path_after) {
        return Err(format!(
            "{label} path changed while it was read: {}",
            path.display()
        ));
    }
    if contents.len() as u64 != after.len() {
        return Err(format!(
            "{label} size changed while it was read: {}",
            path.display()
        ));
    }
    Ok((
        contents.clone(),
        FileIdentity {
            device: after.dev(),
            inode: after.ino(),
            size: after.len(),
            sha256: format!("{:x}", Sha256::digest(&contents)),
        },
    ))
}

/// Strict JSON value with duplicate keys refused at every depth, mirroring
/// the existing loader's `object_pairs_hook` refusal.
#[derive(Debug)]
enum Strict {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Strict>),
    Object(Vec<(String, Strict)>),
}

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Strict;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_none<E>(self) -> std::result::Result<Strict, E> {
                Ok(Strict::Null)
            }

            fn visit_unit<E>(self) -> std::result::Result<Strict, E> {
                Ok(Strict::Null)
            }

            fn visit_bool<E>(self, value: bool) -> std::result::Result<Strict, E> {
                Ok(Strict::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Strict, E> {
                Ok(Strict::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Strict, E> {
                Ok(Strict::Number(value.into()))
            }

            fn visit_f64<E>(self, value: f64) -> std::result::Result<Strict, E> {
                let number = serde_json::Number::from_f64(value)
                    .unwrap_or_else(|| serde_json::Number::from(0));
                Ok(Strict::Number(number))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Strict, E> {
                Ok(Strict::String(value.to_owned()))
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Strict, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Strict>()? {
                    items.push(item);
                }
                Ok(Strict::Array(items))
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Strict, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut pairs: Vec<(String, Strict)> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if pairs.iter().any(|(seen, _)| *seen == key) {
                        return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                    }
                    let value = map.next_value::<Strict>()?;
                    pairs.push((key, value));
                }
                Ok(Strict::Object(pairs))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn strict_to_value(value: &Strict) -> Value {
    match value {
        Strict::Null => Value::Null,
        Strict::Bool(value) => Value::Bool(*value),
        Strict::Number(value) => Value::Number(value.clone()),
        Strict::String(value) => Value::String(value.clone()),
        Strict::Array(items) => Value::Array(items.iter().map(strict_to_value).collect()),
        Strict::Object(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), strict_to_value(value)))
                .collect(),
        ),
    }
}

fn strict_json_object(contents: &[u8], label: &str) -> Result<Map<String, Value>> {
    let strict: Strict = serde_json::from_slice(contents)
        .map_err(|error| format!("{label} is malformed JSON: {error}"))?;
    match strict_to_value(&strict) {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{label} must be a JSON object")),
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn identity_from_obj(value: &Value, label: &str) -> Result<Option<FileIdentity>> {
    if value.is_null() {
        return Ok(None);
    }
    let raw = as_mapping(value, label)?;
    let identity = raw
        .get("identity")
        .ok_or_else(|| format!("{label}.identity must be a JSON array"))?;
    let parts = identity
        .as_array()
        .ok_or_else(|| format!("{label}.identity must be a JSON array"))?;
    if parts.len() != 2 {
        return Err(format!("{label}.identity must contain device and inode"));
    }
    let digest = as_str(
        raw.get("sha256").unwrap_or(&Value::Null),
        &format!("{label}.sha256"),
    )?;
    if !is_digest(digest) {
        return Err(format!(
            "{label}.sha256 must be 64 lowercase hexadecimal digits"
        ));
    }
    Ok(Some(FileIdentity {
        device: as_int(&parts[0], &format!("{label}.identity[0]"), 0)? as u64,
        inode: as_int(&parts[1], &format!("{label}.identity[1]"), 1)? as u64,
        size: as_int(
            raw.get("size").unwrap_or(&Value::Null),
            &format!("{label}.size"),
            0,
        )? as u64,
        sha256: digest.to_owned(),
    }))
}

pub(crate) fn identity_to_obj(identity: &FileIdentity) -> Value {
    json!({
        "identity": [identity.device, identity.inode],
        "size": identity.size,
        "sha256": identity.sha256,
    })
}

fn resolve_loose(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolve_loose(parent).join(name),
        _ => path.to_path_buf(),
    }
}

pub(crate) fn sidecar_path(control: &Path, machine: &str, slot: &str, generation: i64) -> PathBuf {
    control.join(format!(
        "HANDOFF.{}.{}.{}.{}.{}.json",
        machine.chars().count(),
        machine,
        slot.chars().count(),
        slot,
        generation
    ))
}

/// Read and validate the handoff sidecar bound to one active slot,
/// printing the artifact in the writer's serialized shape (or null).
pub(crate) fn parse_handoff(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
) -> Result<Value> {
    validate_name(slot, "slot")?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let state = parse_state(root, machine_override)?;
    let record = state
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .ok_or_else(|| format!("slot {slot} is not active on {selected}"))?;
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("slot {slot} has no generation"))?;
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_owned();
    match load_sidecar_artifact(&cfg, &selected, slot, &slot_type, generation)? {
        Some(artifact) => Ok(artifact.value),
        None => Ok(Value::Null),
    }
}

/// One validated handoff sidecar: its printed shape plus the storage
/// identity of the file it was read from.
pub(crate) struct SidecarArtifact {
    pub(crate) value: Value,
    pub(crate) storage_identity: FileIdentity,
}

/// Load and validate the generation-bound sidecar for one slot, or None
/// when no sidecar pathname exists.
pub(crate) fn load_sidecar_artifact(
    cfg: &Cfg,
    selected: &str,
    slot: &str,
    slot_type: &str,
    generation: i64,
) -> Result<Option<SidecarArtifact>> {
    let path = sidecar_path(&cfg.control, selected, slot, generation);
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    let (contents, storage_identity) =
        read_regular_file_identity(&path, "handoff sidecar", SIDECAR_BYTES_LIMIT)?;
    validate_sidecar(
        cfg,
        selected,
        slot,
        slot_type,
        generation,
        &path,
        &contents,
        storage_identity,
    )
    .map(Some)
}

/// Validate sidecar contents read from `path` (the canonical sidecar
/// path, or its retired-storage successor) against one slot generation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_sidecar(
    cfg: &Cfg,
    selected: &str,
    slot: &str,
    slot_type: &str,
    generation: i64,
    path: &Path,
    contents: &[u8],
    storage_identity: FileIdentity,
) -> Result<SidecarArtifact> {
    let label = format!("handoff sidecar {}", path.display());
    let raw = strict_json_object(contents, "handoff sidecar")?;
    let schema = as_int(
        raw.get("schema").unwrap_or(&Value::Null),
        &format!("{label}.schema"),
        0,
    )?;
    let mut fields = vec![
        "schema",
        "machine",
        "slot",
        "generation",
        "recorded_at",
        "source",
        "source_identity",
        "contents_utf8",
        "sha256",
    ];
    if schema == SIDECAR_SCHEMA {
        fields.push("source_path");
    } else if schema != LEGACY_SCHEMA {
        return Err(format!(
            "unsupported handoff sidecar schema in {}",
            path.display()
        ));
    }
    exact_keys(&raw, &fields, &[], &label)?;
    let machine = as_str(&raw["machine"], &format!("{label}.machine"))?;
    let sidecar_slot = as_str(&raw["slot"], &format!("{label}.slot"))?;
    let sidecar_generation = as_int(&raw["generation"], &format!("{label}.generation"), 1)?;
    if machine != selected || sidecar_slot != slot || sidecar_generation != generation {
        return Err(format!(
            "handoff sidecar identity does not match slot generation: {}",
            path.display()
        ));
    }
    let recorded_at = as_str(&raw["recorded_at"], &format!("{label}.recorded_at"))?;
    parse_timestamp(recorded_at, &format!("{label}.recorded_at"))?;
    let source = as_str(&raw["source"], &format!("{label}.source"))?;
    if !matches!(source, "write-handoff" | "legacy-slot-file") {
        return Err(format!("{label}.source is invalid"));
    }
    let expected_legacy = resolve_loose(&slot_directory(cfg, slot, slot_type)?.join("HANDOFF.md"));
    let source_path: Option<PathBuf> = if schema == LEGACY_SCHEMA {
        if source == "legacy-slot-file" {
            Some(expected_legacy.clone())
        } else {
            None
        }
    } else {
        let raw_path = as_str(&raw["source_path"], &format!("{label}.source_path"))?;
        if !raw_path.starts_with('/') || raw_path.split('/').any(|part| part == "..") {
            return Err(format!(
                "{label}.source_path must be a normalized absolute path"
            ));
        }
        Some(PathBuf::from(raw_path))
    };
    let source_identity =
        identity_from_obj(&raw["source_identity"], &format!("{label}.source_identity"))?;
    if schema == SIDECAR_SCHEMA && source_identity.is_none() {
        return Err(format!("{label} lacks source file identity"));
    }
    if schema == LEGACY_SCHEMA && source == "legacy-slot-file" && source_identity.is_none() {
        return Err(format!("{label} lacks its historical legacy file identity"));
    }
    if source == "legacy-slot-file" && source_path.as_deref() != Some(expected_legacy.as_path()) {
        return Err(format!(
            "{label} legacy source path does not match its slot"
        ));
    }
    let contents_text = as_str(&raw["contents_utf8"], &format!("{label}.contents_utf8"))?;
    if contents_text.len() > HANDOFF_BYTES_LIMIT {
        return Err(format!("{label} exceeds the 1 MiB safety bound"));
    }
    let digest = as_str(&raw["sha256"], &format!("{label}.sha256"))?;
    let actual = format!("{:x}", Sha256::digest(contents_text.as_bytes()));
    if !is_digest(digest) || actual != digest {
        return Err(format!("{label} digest does not match its content"));
    }
    if let Some(identity) = &source_identity {
        if identity.sha256 != digest {
            return Err(format!("{label} source digest does not match its content"));
        }
    }
    if schema != SIDECAR_SCHEMA || source_path.is_none() || source_identity.is_none() {
        return Err(
            "legacy handoff sidecar must be provenance-upgraded before it is written".to_owned(),
        );
    }
    Ok(SidecarArtifact {
        value: json!({
            "schema": SIDECAR_SCHEMA,
            "machine": selected,
            "slot": slot,
            "generation": generation,
            "recorded_at": recorded_at,
            "source": source,
            "source_path": source_path.expect("checked above").to_string_lossy(),
            "source_identity": identity_to_obj(source_identity.as_ref().expect("checked above")),
            "contents_utf8": contents_text,
            "sha256": digest,
        }),
        storage_identity,
    })
}
