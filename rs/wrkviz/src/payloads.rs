//! Content-addressed storage for the bulk text a tool call
//! produced: `py/wrkviz/payloads.py` at parity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::json::{
    as_array, as_int, as_object, as_string, dump_canonical, dump_compact, read_json,
};
use crate::team_data::PayloadRef;
use crate::transcript_io::write_text_if_changed;

const MANIFEST_NAME: &str = "manifest.json";
const SHARD_SUFFIX: &str = ".jsonl";
const LINE_PREFIX: &str = "{\"sha256\":\"";
const DIGEST_START: usize = 11;
const DIGEST_END: usize = DIGEST_START + 64;
const AFTER_DIGEST: &str = "\",\"text\":";

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The content address of one payload.
pub fn payload_digest(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// The reference `raw/team.json` keeps in place of *text*.
pub fn payload_ref(text: &str) -> PayloadRef {
    PayloadRef {
        sha256: payload_digest(text),
        byte_length: text.len() as i64,
    }
}

fn validate_digest(value: &str, where_: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "{where_}: not a lowercase sha-256 hex digest: {}",
            crate::json::py_repr_str(value)
        ));
    }
    Ok(())
}

/// The name of the one shard file that can hold this digest.
pub fn shard_name(sha256: &str) -> Result<String, String> {
    validate_digest(sha256, "payload digest")?;
    Ok(format!("{}{SHARD_SUFFIX}", &sha256[..2]))
}

fn encode(digest: &str, text: &str) -> Result<String, String> {
    let mut record = Map::new();
    record.insert("sha256".into(), Value::String(digest.to_string()));
    record.insert("text".into(), Value::String(text.to_string()));
    let line = dump_compact(&Value::Object(record));
    if !line.starts_with(LINE_PREFIX) || !line[DIGEST_END..].starts_with(AFTER_DIGEST) {
        return Err(
            "the canonical payload encoding no longer places the digest at a fixed offset; \
             the fixed-offset index in this module is invalid"
                .to_string(),
        );
    }
    Ok(line)
}

fn digest_of_line(line: &str, where_: &str) -> Result<String, String> {
    if !line.starts_with(LINE_PREFIX)
        || line.len() < DIGEST_END
        || !line[DIGEST_END..].starts_with(AFTER_DIGEST)
    {
        return Err(format!(
            "{where_}: payload record does not have the canonical shape"
        ));
    }
    let digest = &line[DIGEST_START..DIGEST_END];
    validate_digest(digest, where_)?;
    Ok(digest.to_string())
}

fn text_of_line(line: &str, where_: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(line).map_err(|_| format!("{where_}: expected an object"))?;
    let object = as_object(&value, where_)?;
    Ok(as_string(
        object.get("text").unwrap_or(&Value::Null),
        &format!("{where_}.text"),
    )?
    .to_string())
}

fn shard_paths(root: &Path) -> Vec<PathBuf> {
    if root.is_symlink() || !root.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            path.is_file()
                && !path.is_symlink()
                && name.ends_with(SHARD_SUFFIX)
                && name.len() == SHARD_SUFFIX.len() + 2
                && name[..2]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .collect();
    paths.sort();
    paths
}

fn index_shard_lines(text: &str, path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut lines: BTreeMap<String, String> = BTreeMap::new();
    for (index, line) in text.split('\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let where_ = format!("{}:{}", path.display(), index + 1);
        let digest = digest_of_line(line, &where_)?;
        if let Some(previous) = lines.get(&digest) {
            if previous != line {
                return Err(format!(
                    "{where_}: two different payloads are stored under digest {digest}"
                ));
            }
        }
        lines.insert(digest, line.to_string());
    }
    Ok(lines)
}

/// One shard's contribution to the manifest.
#[derive(Clone, Debug)]
pub struct PayloadShardRecord {
    /// Shard file name.
    pub name: String,
    /// Record count.
    pub records: i64,
    /// Total text bytes.
    pub text_bytes: i64,
    /// Shard content digest.
    pub sha256: String,
}

impl PayloadShardRecord {
    fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("name".into(), Value::String(self.name.clone()));
        out.insert("records".into(), self.records.into());
        out.insert("text_bytes".into(), self.text_bytes.into());
        out.insert("sha256".into(), Value::String(self.sha256.clone()));
        Value::Object(out)
    }
}

/// The complete inventory of one team's payload tree.
#[derive(Clone, Debug)]
pub struct PayloadManifest {
    /// Total records.
    pub records: i64,
    /// Total text bytes.
    pub text_bytes: i64,
    /// Shards.
    pub shards: Vec<PayloadShardRecord>,
}

impl PayloadManifest {
    fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("schema_version".into(), 1.into());
        out.insert("tool".into(), Value::String("wrkviz".into()));
        out.insert("records".into(), self.records.into());
        out.insert("text_bytes".into(), self.text_bytes.into());
        out.insert(
            "shards".into(),
            Value::Array(
                self.shards
                    .iter()
                    .map(PayloadShardRecord::to_json)
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

/// What one merge holds afterwards and what it added/found.
#[derive(Clone, Debug)]
pub struct PayloadWriteReport {
    /// Total stored payloads.
    pub stored: i64,
    /// Total stored bytes.
    pub stored_bytes: i64,
    /// Newly stored payloads.
    pub newly_stored: i64,
    /// Newly stored bytes.
    pub newly_stored_bytes: i64,
    /// Files changed.
    pub files_changed: i64,
    /// Resulting manifest.
    pub manifest: PayloadManifest,
    /// Shards recorded before and absent now.
    pub pruned_shards: Vec<String>,
    /// Shards whose bytes disagreed with their record.
    pub damaged_shards: Vec<String>,
}

/// Read the recorded inventory, or `None` for a team that has
/// never stored a payload.
pub fn load_payload_manifest(root: &Path) -> Result<Option<PayloadManifest>, String> {
    let path = root.join(MANIFEST_NAME);
    if path.is_symlink() || !path.is_file() {
        return Ok(None);
    }
    let document = read_json(&path)?;
    let object = as_object(&document, &path.display().to_string())?;
    let tool_ok = matches!(
        object.get("tool").and_then(Value::as_str),
        Some("wrkviz") | Some("agent-team-timeline")
    );
    if !crate::json::json_eq_one(object.get("schema_version")) || !tool_ok {
        return Err(format!("invalid payload manifest at {}", path.display()));
    }
    let raw_shards = as_array(
        object.get("shards").unwrap_or(&Value::Null),
        &format!(
            "{}: payload manifest shards is not an array",
            path.display()
        ),
    )?;
    let mut shards = Vec::new();
    for (index, item) in raw_shards.iter().enumerate() {
        let where_ = format!("{}: shards[{index}]", path.display());
        let entry = as_object(item, &where_)?;
        shards.push(PayloadShardRecord {
            name: as_string(
                entry.get("name").unwrap_or(&Value::Null),
                &format!("{where_}.name"),
            )?
            .to_string(),
            records: as_int(
                entry.get("records").unwrap_or(&Value::Null),
                &format!("{where_}.records"),
            )?,
            text_bytes: as_int(
                entry.get("text_bytes").unwrap_or(&Value::Null),
                &format!("{where_}.text_bytes"),
            )?,
            sha256: as_string(
                entry.get("sha256").unwrap_or(&Value::Null),
                &format!("{where_}.sha256"),
            )?
            .to_string(),
        });
    }
    Ok(Some(PayloadManifest {
        records: as_int(
            object.get("records").unwrap_or(&Value::Null),
            &format!("{}.records", path.display()),
        )?,
        text_bytes: as_int(
            object.get("text_bytes").unwrap_or(&Value::Null),
            &format!("{}.text_bytes", path.display()),
        )?,
        shards,
    }))
}

/// Union the stored payloads with the observed ones and
/// rewrite only what changed.
pub fn merge_payloads(root: &Path, texts: &[String]) -> Result<PayloadWriteReport, String> {
    let mut incoming: BTreeMap<String, String> = BTreeMap::new();
    let mut incoming_bytes: BTreeMap<String, i64> = BTreeMap::new();
    for text in texts {
        let digest = payload_digest(text);
        if !incoming.contains_key(&digest) {
            incoming.insert(digest.clone(), encode(&digest, text)?);
            incoming_bytes.insert(digest, text.len() as i64);
        }
    }
    let mut by_shard: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (digest, line) in &incoming {
        by_shard
            .entry(shard_name(digest)?)
            .or_default()
            .insert(digest.clone(), line.clone());
    }
    let previous = load_payload_manifest(root)?;
    let previous_shards: BTreeMap<String, PayloadShardRecord> = previous
        .as_ref()
        .map(|manifest| {
            manifest
                .shards
                .iter()
                .map(|shard| (shard.name.clone(), shard.clone()))
                .collect()
        })
        .unwrap_or_default();
    let present: BTreeSet<String> = shard_paths(root)
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    let pruned: Vec<String> = previous_shards
        .keys()
        .filter(|name| !present.contains(*name))
        .cloned()
        .collect();
    if present.is_empty() && by_shard.is_empty() && previous.is_none() {
        return Ok(PayloadWriteReport {
            stored: 0,
            stored_bytes: 0,
            newly_stored: 0,
            newly_stored_bytes: 0,
            files_changed: 0,
            manifest: PayloadManifest {
                records: 0,
                text_bytes: 0,
                shards: Vec::new(),
            },
            pruned_shards: Vec::new(),
            damaged_shards: Vec::new(),
        });
    }
    let mut files_changed = 0i64;
    let mut newly_stored = 0i64;
    let mut newly_stored_bytes = 0i64;
    let mut damaged: Vec<String> = Vec::new();
    let mut shard_records: Vec<PayloadShardRecord> = Vec::new();
    let mut names: BTreeSet<String> = present.clone();
    names.extend(by_shard.keys().cloned());
    for name in names {
        let carried = if present.contains(&name) {
            previous_shards.get(&name)
        } else {
            None
        };
        let additions = by_shard.get(&name);
        let path = root.join(&name);
        if additions.is_none() {
            if let Some(carried) = carried {
                shard_records.push(carried.clone());
                continue;
            }
        }
        let raw = if present.contains(&name) {
            std::fs::read_to_string(&path).map_err(|error| crate::json::os_error(&error, &path))?
        } else {
            String::new()
        };
        let mut carried = carried;
        if let Some(record) = &carried {
            if sha256_hex(raw.as_bytes()) != record.sha256 {
                damaged.push(format!(
                    "{name}: shard bytes did not match the digest recorded for them; \
                     the shard was re-measured from what is on disk"
                ));
                carried = None;
            }
        }
        let mut stored = index_shard_lines(&raw, &path)?;
        let base_bytes = match &carried {
            Some(record) => record.text_bytes,
            None => {
                let mut total = 0i64;
                for (digest, line) in &stored {
                    total +=
                        text_of_line(line, &format!("{}#{digest}", path.display()))?.len() as i64;
                }
                total
            }
        };
        let mut added = 0i64;
        let mut added_bytes = 0i64;
        if let Some(additions) = additions {
            for (digest, line) in additions {
                match stored.get(digest) {
                    None => {
                        stored.insert(digest.clone(), line.clone());
                        added += 1;
                        added_bytes += incoming_bytes[digest];
                    }
                    Some(held) if held != line => {
                        return Err(format!(
                            "{}: two different payloads are stored under digest {digest}",
                            path.display()
                        ));
                    }
                    _ => {}
                }
            }
        }
        let text: String = stored
            .keys()
            .map(|digest| format!("{}\n", stored[digest]))
            .collect();
        if write_text_if_changed(&path, &text)? {
            files_changed += 1;
        }
        newly_stored += added;
        newly_stored_bytes += added_bytes;
        shard_records.push(PayloadShardRecord {
            name,
            records: stored.len() as i64,
            text_bytes: base_bytes + added_bytes,
            sha256: sha256_hex(text.as_bytes()),
        });
    }
    let manifest = PayloadManifest {
        records: shard_records.iter().map(|s| s.records).sum(),
        text_bytes: shard_records.iter().map(|s| s.text_bytes).sum(),
        shards: shard_records,
    };
    if write_text_if_changed(
        &root.join(MANIFEST_NAME),
        &dump_canonical(&manifest.to_json()),
    )? {
        files_changed += 1;
    }
    Ok(PayloadWriteReport {
        stored: manifest.records,
        stored_bytes: manifest.text_bytes,
        newly_stored,
        newly_stored_bytes,
        files_changed,
        manifest,
        pruned_shards: pruned,
        damaged_shards: damaged,
    })
}
