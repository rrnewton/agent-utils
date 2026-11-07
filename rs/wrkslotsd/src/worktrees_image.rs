//! Pure worktrees-image core (P8, mu-rsimg, unwired until dispatch).
//!
//! Mirrors the testable half of Python `worktreesimage.py`: settings
//! validation (`settings_from_obj`), mount classification
//! (`classify_mount`), the migration `listing_signature`, and the
//! `.worktrees-image.json` bind records. Privileged btrfs/mount
//! operations stay behind a runner seam in the writer step; nothing
//! here runs sudo, mounts, or touches a live image.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::image_mounts::MountEntry;

pub(crate) const SCHEMA: &str = "wrkslots-worktrees-image/v1";
pub(crate) const MIN_SIZE_BYTES: i64 = 1_073_741_824;
pub(crate) const IMAGE_MOUNT_NAME: &str = ".slots-image";
pub(crate) const RECORD_NAME: &str = ".worktrees-image.json";

pub(crate) const MOUNTED: &str = "mounted";
pub(crate) const NOT_MOUNTED: &str = "not-mounted";
pub(crate) const WRONG_MOUNT: &str = "wrong-mount";
pub(crate) const NOT_EMPTY_UNMOUNTED: &str = "not-empty-unmounted";
pub(crate) const MISSING: &str = "missing";

/// Configuration for the project worktrees image, mirroring
/// Python `WorktreesImageSettings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorktreesImageSettings {
    pub(crate) relative_path: String,
    pub(crate) size_bytes: i64,
    pub(crate) filesystem: String,
    pub(crate) mount: String,
    pub(crate) slot_quota_bytes: Option<i64>,
}

impl WorktreesImageSettings {
    pub(crate) fn image_file(&self, root: &Path) -> PathBuf {
        root.join(&self.relative_path)
    }
}

fn py_repr(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
        other => other.to_string(),
    }
}

/// Validate one `worktrees_image` section, mirroring Python
/// `settings_from_obj` with its error strings.
pub(crate) fn settings_from_obj(value: &Value) -> Result<WorktreesImageSettings, String> {
    let Some(map) = value.as_object() else {
        return Err("configuration.worktrees_image must be a mapping".to_string());
    };
    let known = ["path", "size_bytes", "fs", "mount", "slot_quota_bytes"];
    let mut unknown: Vec<&String> = map
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|key| key.as_str()).collect();
        return Err(format!(
            "configuration.worktrees_image has unknown keys: {}",
            names.join(", ")
        ));
    }
    let raw_path = map.get("path").and_then(Value::as_str).unwrap_or_default();
    if raw_path.trim().is_empty() {
        return Err("configuration.worktrees_image.path must be a non-empty string".to_string());
    }
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() || candidate.components().any(|part| part.as_os_str() == "..") {
        return Err(
            "configuration.worktrees_image.path must be a project-relative path without '..'"
                .to_string(),
        );
    }
    let size = map.get("size_bytes").and_then(Value::as_i64);
    if size.map(|value| value < MIN_SIZE_BYTES).unwrap_or(true) {
        return Err(format!(
            "configuration.worktrees_image.size_bytes must be an integer of at least {MIN_SIZE_BYTES}"
        ));
    }
    let filesystem = map.get("fs").and_then(Value::as_str).unwrap_or("btrfs");
    if filesystem != "btrfs" {
        return Err(format!(
            "configuration.worktrees_image.fs must be one of btrfs, not {}",
            py_repr(&json!(filesystem))
        ));
    }
    let mount = map.get("mount").and_then(Value::as_str).unwrap_or("kernel");
    if mount == "fuse" {
        return Err(
            "configuration.worktrees_image.mount 'fuse' is unavailable: btrfs has no unprivileged FUSE driver (no fuse2fs equivalent); use 'kernel' with passwordless sudo, or do not enable worktrees_image on this host"
                .to_string(),
        );
    }
    if !matches!(mount, "kernel" | "auto") {
        return Err(format!(
            "configuration.worktrees_image.mount must be one of kernel, auto, not {}",
            py_repr(&json!(mount))
        ));
    }
    let quota = map.get("slot_quota_bytes");
    if let Some(quota) = quota {
        if quota.as_i64().map(|value| value < 1).unwrap_or(true) {
            return Err(
                "configuration.worktrees_image.slot_quota_bytes must be a positive integer"
                    .to_string(),
            );
        }
    }
    Ok(WorktreesImageSettings {
        relative_path: raw_path.replace('\\', "/"),
        size_bytes: size.unwrap_or_default(),
        filesystem: filesystem.to_string(),
        mount: mount.to_string(),
        slot_quota_bytes: quota.and_then(Value::as_i64),
    })
}

/// Classify the slots directory's mount state from an injected
/// mount table, mirroring Python `classify_mount`.
pub(crate) fn classify_mount(
    slots_dir: &Path,
    entries: &[MountEntry],
    slots_dir_exists: bool,
    slots_dir_empty: bool,
) -> &'static str {
    if !slots_dir_exists {
        return MISSING;
    }
    let target = crate::image_mounts::normalize_absolute(slots_dir);
    let top = entries
        .iter()
        .rev()
        .find(|entry| crate::image_mounts::normalize_absolute(&entry.mount_point) == target);
    let Some(top) = top else {
        return if slots_dir_empty {
            NOT_MOUNTED
        } else {
            NOT_EMPTY_UNMOUNTED
        };
    };
    if top.fstype != "btrfs" {
        return WRONG_MOUNT;
    }
    MOUNTED
}

pub(crate) fn image_mount_dir(control: &Path) -> PathBuf {
    control.join(IMAGE_MOUNT_NAME)
}

pub(crate) fn record_path(control: &Path) -> PathBuf {
    control.join(RECORD_NAME)
}

/// One slot's recorded bind, mirroring Python `BindRecord`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindRecord {
    pub(crate) subvolume: String,
    pub(crate) mount_id: Option<i64>,
    pub(crate) target: Option<String>,
}

/// Load `.worktrees-image.json`, accepting the earlier string form,
/// mirroring Python `load_bind_records`.
pub(crate) fn load_bind_records(control: &Path) -> Result<BTreeMap<String, BindRecord>, String> {
    let path = record_path(control);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read worktrees image record {path:?}: {error}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot read worktrees image record {path:?}: {error}"))?;
    let Some(binds) = value.get("binds").and_then(Value::as_object) else {
        return Err(format!(
            "malformed worktrees image record: {}",
            path.display()
        ));
    };
    let mut records = BTreeMap::new();
    for (slot, item) in binds {
        if let Some(subvolume) = item.as_str() {
            records.insert(
                slot.clone(),
                BindRecord {
                    subvolume: subvolume.to_string(),
                    mount_id: None,
                    target: None,
                },
            );
        } else if let Some(subvolume) = item.get("subvolume").and_then(Value::as_str) {
            records.insert(
                slot.clone(),
                BindRecord {
                    subvolume: subvolume.to_string(),
                    mount_id: item.get("mount_id").and_then(Value::as_i64),
                    target: item
                        .get("target")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            );
        } else {
            return Err(format!(
                "malformed worktrees image record: {}",
                path.display()
            ));
        }
    }
    Ok(records)
}

/// Persist bind records atomically, mirroring `save_bind_records`
/// (sorted keys, indent 2, trailing newline, temp-then-replace).
pub(crate) fn save_bind_records(
    control: &Path,
    records: &BTreeMap<String, BindRecord>,
) -> Result<(), String> {
    let path = record_path(control);
    let temporary = control.join(format!("{RECORD_NAME}.tmp"));
    let binds: serde_json::Map<String, Value> = records
        .iter()
        .map(|(slot, record)| {
            (
                slot.clone(),
                json!({
                    "subvolume": record.subvolume,
                    "mount_id": record.mount_id,
                    "target": record.target,
                }),
            )
        })
        .collect();
    let payload = json!({ "binds": binds });
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );
    std::fs::write(&temporary, text)
        .map_err(|error| format!("cannot write worktrees image record {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot replace worktrees image record {path:?}: {error}"))
}

/// Sorted `(relative path, kind, size)` for every migratable entry
/// under a temp/fixture tree, mirroring Python `listing_signature`.
/// Special files are skipped, as in the oracle.
pub(crate) fn listing_signature(root: &Path) -> Vec<(String, String, u64)> {
    let mut signature = Vec::new();
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, String, u64)>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for entry in paths {
            let relative = entry
                .strip_prefix(root)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            let file_type = entry.symlink_metadata().map(|meta| meta.file_type());
            match file_type {
                Ok(kind) if kind.is_symlink() => {
                    let size = std::fs::read_link(&entry)
                        .map(|target| target.as_os_str().len() as u64)
                        .unwrap_or(0);
                    out.push((relative, "link".to_string(), size));
                }
                Ok(kind) if kind.is_dir() => {
                    out.push((relative.clone(), "dir".to_string(), 0));
                    walk(root, &entry, out);
                }
                Ok(kind) if kind.is_file() => {
                    let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                    out.push((relative, "file".to_string(), size));
                }
                _ => {}
            }
        }
    }
    walk(root, root, &mut signature);
    signature.sort();
    signature
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wrkslotsd-wt-image-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn settings_validation_matches_python() {
        let ok =
            settings_from_obj(&json!({"path": "wt.img", "size_bytes": 1073741824})).expect("ok");
        assert_eq!(ok.filesystem, "btrfs");
        assert_eq!(ok.mount, "kernel");
        assert!(
            settings_from_obj(&json!({"path": "../wt.img", "size_bytes": 1073741824})).is_err()
        );
        assert!(settings_from_obj(&json!({"path": "wt.img", "size_bytes": 5})).is_err());
        let fuse = settings_from_obj(
            &json!({"path": "wt.img", "size_bytes": 1073741824, "mount": "fuse"}),
        )
        .expect_err("fuse refused");
        assert!(fuse.contains("no unprivileged FUSE driver"), "{fuse}");
    }

    #[test]
    fn classify_and_records_and_signature() {
        let root = temp_root("classify");
        let slots = root.join("slots");
        assert_eq!(classify_mount(&slots, &[], false, true), MISSING);
        assert_eq!(classify_mount(&slots, &[], true, true), NOT_MOUNTED);
        assert_eq!(
            classify_mount(&slots, &[], true, false),
            NOT_EMPTY_UNMOUNTED
        );
        let btrfs = [MountEntry {
            mount_point: slots.clone(),
            fstype: "btrfs".to_string(),
            source: "/dev/loop0".to_string(),
            device: "0:7".to_string(),
            root: "/".to_string(),
        }];
        assert_eq!(classify_mount(&slots, &btrfs, true, false), MOUNTED);

        let control = root.join("control");
        std::fs::create_dir_all(&control).expect("control");
        let mut records = BTreeMap::new();
        records.insert(
            "slot-a".to_string(),
            BindRecord {
                subvolume: "slot-a".to_string(),
                mount_id: Some(42),
                target: None,
            },
        );
        save_bind_records(&control, &records).expect("save");
        assert_eq!(load_bind_records(&control).expect("load"), records);

        std::fs::create_dir_all(root.join("tree/sub")).expect("tree");
        std::fs::write(root.join("tree/sub/file.txt"), "abc").expect("file");
        let signature = listing_signature(&root.join("tree"));
        assert!(signature.contains(&("sub".to_string(), "dir".to_string(), 0)));
        assert!(signature.contains(&("sub/file.txt".to_string(), "file".to_string(), 3)));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
