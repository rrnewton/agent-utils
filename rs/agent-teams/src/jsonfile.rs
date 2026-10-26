//! JSON file handling per design section 6.1: versioned `schema`, `comment`/`notes`
//! preserved by machine rewrites, atomic writes, and permission checks.

use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// An error reading or validating a data file, with a stable machine-readable code.
#[derive(Debug)]
pub struct DataError {
    /// Stable code such as `schema`, `permissions`, `parse`, `io`.
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl DataError {
    /// Build an error.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        DataError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DataError {}

impl From<serde_json::Error> for DataError {
    fn from(err: serde_json::Error) -> Self {
        DataError::new("parse", err.to_string())
    }
}

impl From<io::Error> for DataError {
    fn from(err: io::Error) -> Self {
        DataError::new("io", err.to_string())
    }
}

/// Result alias for data-file operations.
pub type DataResult<T> = Result<T, DataError>;

/// Refuse a file that group or others may write (section 6.1).
pub fn check_permissions(path: &Path) -> DataResult<()> {
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o022 != 0 {
        return Err(DataError::new(
            "permissions",
            format!(
                "{} is writable by group or others (mode {:o}); routing data must not be",
                path.display(),
                mode & 0o777
            ),
        ));
    }
    Ok(())
}

/// Read and parse a JSON file, checking permissions first.
pub fn read_json(path: &Path) -> DataResult<Value> {
    check_permissions(path)?;
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|e| DataError::new("parse", format!("{}: {e}", path.display())))
}

/// Check the `schema` field: `agent-teams-<kind>/v<major>`; an unknown major is refused.
pub fn check_schema(value: &Value, kind: &str, path: &Path) -> DataResult<()> {
    let expected = format!("agent-teams-{kind}/v1");
    match value.get("schema").and_then(Value::as_str) {
        Some(s) if s == expected => Ok(()),
        Some(s) => Err(DataError::new(
            "schema",
            format!("{}: schema {s:?}, expected {expected:?}", path.display()),
        )),
        None => Err(DataError::new(
            "schema",
            format!("{}: no schema field, expected {expected:?}", path.display()),
        )),
    }
}

/// Keys an object of this kind may carry; used for unknown-key warnings (section 6.1).
pub fn unknown_keys<'a>(obj: &'a Map<String, Value>, known: &[&str]) -> Vec<&'a str> {
    obj.keys()
        .map(String::as_str)
        .filter(|k| !known.contains(k) && *k != "comment" && *k != "notes")
        .collect()
}

/// Write JSON atomically: two-space indent, trailing newline, previous file kept as
/// `<name>.<timestamp>` when it exists. Key order is preserved by `preserve_order`.
pub fn write_json_atomic(path: &Path, value: &Value) -> DataResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let stamp = crate::timeutil::now_unix();
        let backup = backup_path(path, stamp);
        fs::copy(path, &backup)?;
    }
    let mut text =
        serde_json::to_string_pretty(value).map_err(|e| DataError::new("io", e.to_string()))?;
    text.push('\n');
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, text)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// The backup path used by [`write_json_atomic`].
pub fn backup_path(path: &Path, stamp: i64) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.{stamp}"))
}

/// Create `path` and any missing parents as directories private to the caller (mode 0700),
/// whatever the umask, and make `path` itself private if it already exists. A queue lane or
/// hook inbox made with the usual 022 umask would be readable by everyone, and the queue
/// library refuses to read a queue directory that is not private.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "not a directory: {}",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write a file, mode 0600 whatever the umask, only if it does not exist (`O_EXCL`); returns
/// false when it already did.
pub fn write_new(path: &Path, contents: &[u8]) -> io::Result<bool> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(contents)?;
            file.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Read a JSON file if present; `None` when absent.
pub fn read_json_opt(path: &Path) -> DataResult<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_json(path)?))
}

/// Expand a leading `~/` against `$HOME`.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Create a file with complete contents or not at all: write a temp file,
/// then `link()` it into place. A crash can never leave a truncated file at
/// the final path (a truncated ledger claim would brick its ID).
pub fn write_new_atomic(path: &Path, contents: &[u8]) -> io::Result<bool> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), contents.len()));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    match fs::hard_link(&tmp, path) {
        Ok(()) => {
            let _ = fs::remove_file(&tmp);
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            Ok(false)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}
