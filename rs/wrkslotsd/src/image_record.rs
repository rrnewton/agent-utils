//! Typed slot-image records and the representation hooks (P8/P7, mu-rsimg).
//!
//! Mirrors the record half of Python `slotimage.py`: the `IMAGE.json`
//! document (`wrkslots-slot-image/v1`), its validation messages, the
//! `slot-images/<slot_type>/<slot>/` layout, and the atomic record
//! write. Mount mechanics live in `image_mounts`; lifecycle writers
//! stay with the shared core, which calls the decision hooks below
//! and renders their outcomes. All functions are pure or operate
//! only on paths the caller supplies, so tests use temp fixtures
//! and temp images exclusively.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub(crate) const SCHEMA: &str = "wrkslots-slot-image/v1";
pub(crate) const IMAGES_DIRECTORY: &str = "slot-images";
pub(crate) const RECORD_NAME: &str = "IMAGE.json";
pub(crate) const IMAGE_FILE_NAME: &str = "slot.img";
pub(crate) const STATE_IMAGE_NAME: &str = "state.img";
pub(crate) const STATE_MOUNT_NAME: &str = "state";
pub(crate) const DISCARD_PHASE: &str = "discarding-stale-copy";

/// One validated `IMAGE.json`, mirroring Python `slotimage.SlotImage`'s
/// record fields (mount-derived fields are computed in `image_mounts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageRecord {
    pub(crate) directory: PathBuf,
    pub(crate) slot: String,
    pub(crate) slot_type: String,
    pub(crate) location: PathBuf,
    pub(crate) ceiling_bytes: i64,
    pub(crate) state_ceiling_bytes: i64,
    pub(crate) backend: String,
    pub(crate) created_at: String,
    pub(crate) phase: String,
}

impl ImageRecord {
    pub(crate) fn record_path(&self) -> PathBuf {
        self.directory.join(RECORD_NAME)
    }

    pub(crate) fn image_file(&self) -> PathBuf {
        self.directory.join(IMAGE_FILE_NAME)
    }

    /// The sparse state image, mirroring `SlotImage.state_image`.
    pub(crate) fn state_image(&self) -> PathBuf {
        self.directory.join(STATE_IMAGE_NAME)
    }

    /// Where the state image mounts, mirroring `SlotImage.state_mount`.
    pub(crate) fn state_mount(&self) -> PathBuf {
        self.directory.join(STATE_MOUNT_NAME)
    }

    /// The `IMAGE.json` document, mirroring `slotimage._record_to_obj`.
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "slot": self.slot,
            "slot_type": self.slot_type,
            "location": self.location.to_string_lossy(),
            "ceiling_bytes": self.ceiling_bytes,
            "state_ceiling_bytes": self.state_ceiling_bytes,
            "backend": self.backend,
            "created_at": self.created_at,
            "phase": self.phase,
        })
    }
}

pub(crate) fn images_root(control: &Path) -> PathBuf {
    control.join(IMAGES_DIRECTORY)
}

pub(crate) fn image_directory(control: &Path, slot_type: &str, slot: &str) -> PathBuf {
    images_root(control).join(slot_type).join(slot)
}

/// Write `IMAGE.json` atomically, mirroring `slotimage._write_record`:
/// temp file, fsync, replace, then fsync the containing directory.
pub(crate) fn write_record(record: &ImageRecord) -> Result<(), String> {
    let path = record.record_path();
    let temporary = record.directory.join(format!("{RECORD_NAME}.tmp"));
    // Python writes with sort_keys=True and indent=2, so render the
    // document through a sorted pretty printer to match its bytes.
    let data = sorted_pretty(&record.to_value());
    std::fs::write(&temporary, data)
        .map_err(|error| format!("cannot write slot image record {temporary:?}: {error}"))?;
    sync_file(&temporary)?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot replace slot image record {path:?}: {error}"))?;
    sync_directory(&record.directory)
}

fn sorted_pretty(value: &Value) -> String {
    fn render(value: &Value, depth: usize) -> String {
        let pad = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    return "{}".to_string();
                }
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let parts: Vec<String> = keys
                    .iter()
                    .map(|key| {
                        format!(
                            "{inner}{}: {}",
                            serde_json::to_string(key).unwrap_or_default(),
                            render(&map[*key], depth + 1)
                        )
                    })
                    .collect();
                format!("{{\n{}\n{pad}}}", parts.join(",\n"))
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let parts: Vec<String> = items
                    .iter()
                    .map(|item| format!("{inner}{}", render(item, depth + 1)))
                    .collect();
                format!("[\n{}\n{pad}]", parts.join(",\n"))
            }
            other => other.to_string(),
        }
    }
    format!("{}\n", render(value, 0))
}

fn sync_file(path: &Path) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot sync slot image record {path:?}: {error}"))
}

fn sync_directory(path: &Path) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot sync slot image directory {path:?}: {error}"))
}

/// Validate one `IMAGE.json` the way Python `slotimage.read_record`
/// does, with the shared core's differential-proven messages (first
/// ported in `storage_state::load_image_record`). `slot_path` is the
/// slot's canonical directory from the caller's layout.
pub(crate) fn load_image_record(
    record_path: &Path,
    slot: &str,
    slot_type: &str,
    slot_path: &Path,
) -> Result<ImageRecord, String> {
    let text = std::fs::read_to_string(record_path)
        .map_err(|error| format!("cannot read slot image record {record_path:?}: {error}"))?;
    let image: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read slot image record {record_path:?}: {error}"))?;
    if !image.is_object() {
        return Err(format!(
            "slot image record {} is not an object",
            record_path.display()
        ));
    }
    let schema = image.get("schema").unwrap_or(&Value::Null);
    if schema.as_str() != Some(SCHEMA) {
        let shown = match schema {
            Value::String(text) => format!("'{text}'"),
            Value::Null => "None".to_string(),
            other => other.to_string(),
        };
        return Err(format!(
            "slot image record {} has unsupported schema {shown}",
            record_path.display()
        ));
    }
    let field_str = |key: &str| -> Result<String, String> {
        image
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "slot image record {} has an invalid '{key}'",
                    record_path.display()
                )
            })
    };
    let field_int = |key: &str| -> Result<i64, String> {
        image.get(key).and_then(Value::as_i64).ok_or_else(|| {
            format!(
                "slot image record {} has an invalid '{key}'",
                record_path.display()
            )
        })
    };
    let record_slot = field_str("slot")?;
    let record_type = field_str("slot_type")?;
    if record_slot != slot || record_type != slot_type {
        return Err(format!(
            "slot image record {} names another slot ({record_type}/{record_slot})",
            record_path.display()
        ));
    }
    let ceiling_bytes = field_int("ceiling_bytes")?;
    let state_ceiling_bytes = field_int("state_ceiling_bytes")?;
    let backend = field_str("backend")?;
    let created_at = field_str("created_at")?;
    let phase = field_str("phase")?;
    let location = field_str("location")?;
    if !Path::new(&location).is_absolute() {
        return Err(format!(
            "slot image record {} has a relative location",
            record_path.display()
        ));
    }
    if location.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "slot image record {} has a non-canonical location {location}",
            record_path.display()
        ));
    }
    let location_path = PathBuf::from(&location);
    if location_path != slot_path {
        let name = location_path
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fence_suffix = name.strip_prefix(&format!(".{slot}."));
        let fence = fence_suffix
            .map(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            })
            .unwrap_or(false)
            && location_path.parent() == slot_path.parent();
        let staging_prefix = format!(".{slot}.convert-");
        let staging = name.starts_with(&staging_prefix)
            && name[staging_prefix.len()..].len() == 8
            && name[staging_prefix.len()..]
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
            && location_path.parent() == record_path.parent().and_then(Path::parent);
        if !fence && !staging {
            return Err(format!(
                "slot image record {} places slot {slot_type}/{slot} at {location}, not at {}; refusing to mount or remove anything there",
                record_path.display(),
                slot_path.display()
            ));
        }
    }
    let directory = record_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Ok(ImageRecord {
        directory,
        slot: record_slot,
        slot_type: record_type,
        location: location_path,
        ceiling_bytes,
        state_ceiling_bytes,
        backend,
        created_at,
        phase,
    })
}

/// The slot's image record, or `None` when it is not image-backed,
/// mirroring Python `slotimage.load`.
pub(crate) fn load_slot_image(
    control: &Path,
    slot_type: &str,
    slot: &str,
    slot_path: &Path,
) -> Result<Option<ImageRecord>, String> {
    let record_path = image_directory(control, slot_type, slot).join(RECORD_NAME);
    if record_path.symlink_metadata().is_err() {
        return Ok(None);
    }
    load_image_record(&record_path, slot, slot_type, slot_path).map(Some)
}

/// Every image record under the control directory, mirroring Python
/// `slotimage.all_images`. `slot_path` resolves a slot's canonical
/// directory from its type and name.
pub(crate) fn all_images(
    control: &Path,
    slot_path: impl Fn(&str, &str) -> PathBuf,
) -> Result<Vec<ImageRecord>, String> {
    let root = images_root(control);
    let mut found = Vec::new();
    let Ok(type_entries) = std::fs::read_dir(&root) else {
        return Ok(found);
    };
    let mut type_dirs: Vec<PathBuf> = type_entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path
                    .symlink_metadata()
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false)
        })
        .collect();
    type_dirs.sort();
    for type_dir in &type_dirs {
        let slot_type = type_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(entries) = std::fs::read_dir(type_dir) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        dirs.sort();
        for dir in &dirs {
            let record_path = dir.join(RECORD_NAME);
            if record_path.exists() {
                let slot = dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let expected = slot_path(&slot_type, &slot);
                found.push(load_image_record(
                    &record_path,
                    &slot,
                    &slot_type,
                    &expected,
                )?);
            }
        }
    }
    Ok(found)
}

/// Hook 1 (P7): resolve the requested representation the way Python
/// `_requested_representation` (`cli.py:4633`) does. `requested` is
/// the CLI value (`config`, `worktree`, or `image`); the
/// configuration's `slot_representation` decides for `config`.
pub(crate) fn resolve_representation<'a>(
    requested: &str,
    config_slot_representation: &'a str,
) -> Result<&'a str, String> {
    match requested {
        "config" => Ok(config_slot_representation),
        "worktree" => Ok("worktree"),
        "image" => Ok("image"),
        other => Err(format!(
            "--representation must be config, worktree, or image, not '{}'",
            other.replace('\\', "\\\\").replace('\'', "\\'")
        )),
    }
}

/// Hook 2 (P7, until P8 provisions): the create-path decision for a
/// resolved representation. `Some` is the refusal the create writer
/// returns verbatim, with no slot, journal, or ACTIVE change made;
/// `None` means proceed on the worktree path. This is the hook that
/// closes the create `config`→`image` fail-open: resolution happens
/// before any provisioning side effect, so an image-resolved create
/// can never fall through to a plain worktree.
pub(crate) fn provision_decision(resolved_representation: &str) -> Option<&'static str> {
    if resolved_representation == "image" {
        Some("slot-image provisioning stays with the Python edition in this slice")
    } else {
        None
    }
}

/// Hook 4 decision (P7, until P8 destroys): whether fence-relocate
/// and destroy/unbind for an image-backed slot may proceed in Rust.
/// Typed, not a user string: the owning writer renders its existing
/// Python-identical refusal for `StayPython`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TeardownDecision {
    /// No image record: the worktree teardown path applies.
    Proceed,
    /// Image-backed: teardown mechanics stay with Python until P8.
    StayPython,
}

pub(crate) fn teardown_decision(record: Option<&ImageRecord>) -> TeardownDecision {
    if record.is_some() {
        TeardownDecision::StayPython
    } else {
        TeardownDecision::Proceed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wrkslotsd-image-record-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn fixture_record(control: &Path, location: &Path) -> ImageRecord {
        ImageRecord {
            directory: image_directory(control, "agent", "slot-a"),
            slot: "slot-a".to_string(),
            slot_type: "agent".to_string(),
            location: location.to_path_buf(),
            ceiling_bytes: 8 * 1024 * 1024 * 1024,
            state_ceiling_bytes: 2 * 1024 * 1024 * 1024,
            backend: "kernel".to_string(),
            created_at: "2026-11-07T00:00:00+00:00".to_string(),
            phase: "ready".to_string(),
        }
    }

    #[test]
    fn record_round_trips_through_temp_file() {
        let root = temp_root("roundtrip");
        let control = root.join("control");
        let slot_path = root.join("slots").join("slot-a");
        let record = fixture_record(&control, &slot_path);
        std::fs::create_dir_all(&record.directory).expect("image dir");
        write_record(&record).expect("write");
        let text = std::fs::read_to_string(record.record_path()).expect("read");
        assert!(
            text.contains("\"schema\": \"wrkslots-slot-image/v1\""),
            "{text}"
        );
        // Python writes sort_keys=True: backend precedes ceiling_bytes.
        assert!(
            text.find("\"backend\"").expect("backend")
                < text.find("\"ceiling_bytes\"").expect("ceiling"),
            "{text}"
        );
        let loaded = load_slot_image(&control, "agent", "slot-a", &slot_path)
            .expect("load")
            .expect("some");
        assert_eq!(loaded, record);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn absent_record_is_none_and_bad_schema_refuses() {
        let root = temp_root("absent");
        let control = root.join("control");
        let slot_path = root.join("slots").join("slot-a");
        assert_eq!(
            load_slot_image(&control, "agent", "slot-a", &slot_path).expect("load"),
            None
        );
        let record = fixture_record(&control, &slot_path);
        std::fs::create_dir_all(&record.directory).expect("image dir");
        std::fs::write(record.record_path(), "{\"schema\": \"other/v9\"}\n").expect("write");
        let error = load_slot_image(&control, "agent", "slot-a", &slot_path)
            .expect_err("bad schema must refuse");
        assert!(
            error.contains("has unsupported schema 'other/v9'"),
            "{error}"
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn representation_resolution_matches_python() {
        assert_eq!(
            resolve_representation("config", "image").expect("ok"),
            "image"
        );
        assert_eq!(
            resolve_representation("config", "worktree").expect("ok"),
            "worktree"
        );
        assert_eq!(
            resolve_representation("image", "worktree").expect("ok"),
            "image"
        );
        assert!(resolve_representation("bogus", "worktree").is_err());
        // The fail-open regression guard: config resolving to image
        // must hit the provision refusal, never proceed.
        let resolved = resolve_representation("config", "image").expect("ok");
        assert_eq!(
            provision_decision(resolved),
            Some("slot-image provisioning stays with the Python edition in this slice")
        );
        assert_eq!(provision_decision("worktree"), None);
        assert_eq!(teardown_decision(None), TeardownDecision::Proceed);
    }
}
