//! The snapshot pointer (`snapshot-root.json`) and the legacy
//! in-archive snapshot layout: the Rust mirror of
//! `wrkviz.snapshot_store`'s `pointer_summary`, `read_pointer`,
//! `legacy_team_roots`, and `default_store_root`.

use crate::json::{as_object, as_string, read_json, resolve_path};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const POINTER_FILE: &str = "snapshot-root.json";
const LEGACY_DIRECTORY: &str = "source_snapshots";
const DEFAULT_STORE_SUFFIX: &str = ".sources";
const POINTER_KIND: &str = "source-snapshot-location";

struct Pointer {
    layout: String,
    root: Option<String>,
}

fn read_pointer(archive: &Path) -> Result<Option<Pointer>, String> {
    let path = archive.join(POINTER_FILE);
    if path.is_symlink() {
        return Err(format!(
            "{}: snapshot location pointer is a symlink",
            path.display()
        ));
    }
    if !path.is_file() {
        return Ok(None);
    }
    let value = read_json(&path)?;
    let obj = as_object(&value, &path.display().to_string())?.clone();
    let kind = as_string(
        obj.get("kind").unwrap_or(&Value::Null),
        &format!("{}: kind", path.display()),
    )?;
    let version = obj.get("schema_version").unwrap_or(&Value::Null);
    let version_ok = version.as_i64() == Some(1)
        || version.as_f64().is_some_and(|number| number == 1.0)
        || version.as_bool() == Some(true);
    if kind != POINTER_KIND || !version_ok {
        return Err(format!(
            "{}: not a schema-1 {POINTER_KIND} document",
            path.display()
        ));
    }
    let layout = as_string(
        obj.get("layout").unwrap_or(&Value::Null),
        &format!("{}: layout", path.display()),
    )?
    .to_string();
    let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    if layout == "in-archive-legacy" {
        if keys != std::collections::BTreeSet::from(["schema_version", "kind", "layout"]) {
            return Err(format!(
                "{}: unexpected fields for an in-archive layout",
                path.display()
            ));
        }
        return Ok(Some(Pointer { layout, root: None }));
    }
    if layout != "external" {
        return Err(format!(
            "{}: unknown layout {}",
            path.display(),
            crate::json::py_repr_str(&layout)
        ));
    }
    if keys != std::collections::BTreeSet::from(["schema_version", "kind", "layout", "root"]) {
        return Err(format!(
            "{}: unexpected fields for an external layout",
            path.display()
        ));
    }
    let root = as_string(
        obj.get("root").unwrap_or(&Value::Null),
        &format!("{}: root", path.display()),
    )?
    .to_string();
    if root.is_empty() || Path::new(&root).is_absolute() {
        return Err(format!(
            "{}: root must be a non-empty path relative to the archive, so that an archive \
             moved together with its store keeps working",
            path.display()
        ));
    }
    Ok(Some(Pointer {
        layout,
        root: Some(root),
    }))
}

fn populated(path: &Path) -> bool {
    if path.is_symlink() || !path.is_dir() {
        return false;
    }
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

fn legacy_team_roots(archive: &Path) -> Result<Vec<PathBuf>, String> {
    let teams = archive.join("teams");
    if !teams.is_dir() {
        return Ok(Vec::new());
    }
    let read = std::fs::read_dir(&teams).map_err(|error| crate::json::os_error(&error, &teams))?;
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| crate::json::os_error(&error, &teams))?;
        entries.push(entry.path());
    }
    entries.sort();
    let mut found = Vec::new();
    for team in entries {
        let meta = std::fs::symlink_metadata(&team);
        let is_real_dir = meta.is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
        if !is_real_dir {
            continue;
        }
        let candidate = team.join(LEGACY_DIRECTORY);
        if populated(&candidate) {
            found.push(candidate);
        }
    }
    Ok(found)
}

fn default_store_root(archive: &Path) -> PathBuf {
    let resolved = resolve_path(archive);
    let name = resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    resolved
        .parent()
        .map(|parent| parent.join(format!("{name}{DEFAULT_STORE_SUFFIX}")))
        .unwrap_or_else(|| PathBuf::from(format!("{name}{DEFAULT_STORE_SUFFIX}")))
}

fn tree_size(root: &Path) -> (i64, i64) {
    let mut files = 0i64;
    let mut total = 0i64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                // os.walk lists a symlink-to-dir among dirnames:
                // neither counted nor descended. A symlink to a
                // file (or a broken link) is a filename, sized by
                // lstat.
                match std::fs::metadata(&path) {
                    Ok(target) if target.is_dir() => {}
                    _ => {
                        files += 1;
                        total += meta.len() as i64;
                    }
                }
            } else if meta.is_dir() {
                pending.push(path);
            } else {
                files += 1;
                total += meta.len() as i64;
            }
        }
    }
    (files, total)
}

/// Describe an archive's snapshot layout for `inspect`, without
/// resolving a team.
pub fn pointer_summary(archive: &Path) -> Result<Value, String> {
    let recorded = read_pointer(archive)?;
    let legacy = legacy_team_roots(archive)?;
    let layout: String = match &recorded {
        Some(pointer) => pointer.layout.clone(),
        None if !legacy.is_empty() => "in-archive-legacy".to_string(),
        None => "external".to_string(),
    };
    let store_root: Value = if layout == "external" {
        let root = match &recorded {
            Some(Pointer {
                root: Some(root), ..
            }) => resolve_path(&archive.join(root)),
            _ => default_store_root(archive),
        };
        Value::String(root.display().to_string())
    } else {
        Value::Null
    };
    let mut files = 0i64;
    let mut total = 0i64;
    for path in &legacy {
        let (count, size) = tree_size(path);
        files += count;
        total += size;
    }
    let teams: Vec<Value> = legacy
        .iter()
        .filter_map(|path| {
            path.parent()
                .and_then(Path::file_name)
                .map(|name| Value::String(name.to_string_lossy().into_owned()))
        })
        .collect();
    let mut out = Map::new();
    out.insert("layout".to_string(), Value::String(layout));
    out.insert("store_root".to_string(), store_root);
    out.insert("recorded".to_string(), Value::Bool(recorded.is_some()));
    out.insert("in_archive_teams".to_string(), Value::Array(teams));
    out.insert("in_archive_files".to_string(), json!(files));
    out.insert("in_archive_bytes".to_string(), json!(total));
    let _ = out;
    Ok(Value::Object(out))
}
