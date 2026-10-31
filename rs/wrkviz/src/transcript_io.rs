//! Archive file IO at parity with `py/wrkviz/archive.py`:
//! atomic replace-if-changed writes and line-delimited JSON.

use std::io::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::json::{as_object, os_error, read_json};

#[cfg(unix)]
fn create_private(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| os_error(&error, path))
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::create(path).map_err(|error| os_error(&error, path))
}

/// Atomically write bytes to `path` (temp file, fsync, rename,
/// directory fsync), returning nothing; the caller decides
/// whether the write was a change.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| os_error(&error, parent))?;
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let write = (|| -> Result<(), String> {
        let mut file = create_private(&tmp)?;
        file.write_all(bytes)
            .map_err(|error| os_error(&error, &tmp))?;
        file.sync_all().map_err(|error| os_error(&error, &tmp))?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|error| os_error(&error, path))?;
        Ok(())
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Replace `path` only when its bytes differ; returns whether
/// a write happened. Mirrors `write_text_if_changed`.
pub fn write_text_if_changed(path: &Path, text: &str) -> Result<bool, String> {
    write_text_mode(path, text, false)
}

/// Same, marking the result executable.
pub fn write_text_if_changed_executable(path: &Path, text: &str) -> Result<bool, String> {
    write_text_mode(path, text, true)
}

fn write_text_mode(path: &Path, text: &str, executable: bool) -> Result<bool, String> {
    if path.is_file() {
        if let Ok(bytes) = std::fs::read(path) {
            let existing = std::str::from_utf8(&bytes)
                .map_err(|error| utf8_error_message(&bytes, error))?
                .to_string();
            if existing == text {
                if executable {
                    set_executable(path)?;
                }
                return Ok(false);
            }
        }
    }
    atomic_write(path, text.as_bytes())?;
    if executable {
        set_executable(path)?;
    }
    Ok(true)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| os_error(&error, path))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(path, permissions).map_err(|error| os_error(&error, path))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Write canonical JSON through the replace-if-changed rule.
pub fn write_json_if_changed(path: &Path, value: &Value) -> Result<bool, String> {
    write_text_if_changed(path, &crate::json::dump_canonical(value))
}

fn utf8_error_message(bytes: &[u8], error: std::str::Utf8Error) -> String {
    let position = error.valid_up_to();
    let byte = bytes.get(position).copied().unwrap_or(0);
    let reason = if error.error_len().is_none() {
        "unexpected end of data"
    } else if !(0xC2..=0xF4).contains(&byte) {
        "invalid start byte"
    } else {
        "invalid continuation byte"
    };
    format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}")
}

/// Read a line-delimited JSON record file; absent means empty.
pub fn read_jsonl(path: &Path) -> Result<Vec<Value>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let not_regular = std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
        .unwrap_or(false);
    if not_regular {
        return Err(format!(
            "line-delimited JSON record file is not a regular file: {}",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| os_error(&error, path))?;
    let mut records = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let where_ = format!("{}:{}", path.display(), index + 1);
        let value =
            crate::json::parse_json_lenient(line).map_err(|error| format!("{where_}: {error}"))?;
        let object = as_object(&value, &where_)?;
        records.push(Value::Object(object.clone()));
    }
    Ok(records)
}

/// `read_json` for documents the export validates itself.
pub fn read_document(path: &Path) -> Result<Value, String> {
    read_json(path)
}
