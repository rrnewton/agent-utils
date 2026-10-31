//! Import a Codex coordinator lineage from append-only
//! rollout JSONL files: `py/wrkviz/codex.py` at parity.
//! Record-level format facts come from
//! `agent_harness::rollout`; lineage, snapshotting, and
//! team assembly live here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_harness::rollout::{
    complete_prefix, content_parts, iso_ms, numeric_epoch_ms, parse_rollout_records, RolloutError,
    RolloutRecord,
};
use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::compressed::{
    iter_logical_glob_rollouts, logical_path, read_first_line, read_source_bytes, resolve_source,
};
use crate::history::SourceCopy;
use crate::json::{as_object, py_repr_str};
use crate::team_data::{Agent, Edge, Event, SourceSnapshot, TeamData, ToolCall, Turn};

const CLASSIFICATION_VERSION: &str = "authorship-v1";

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn str_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

/// The Python `_string`: a stripped string, with empty (and
/// non-strings) reading as absent.
fn sval(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|held| held != 0.0),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    }
}

fn ascii_dump(value: &Value) -> String {
    // CPython `json.dumps` default: non-ASCII is \u-escaped.
    let compact = crate::json::dump_compact(value);
    let mut out = String::with_capacity(compact.len());
    for c in compact.chars() {
        if (c as u32) < 0x80 {
            out.push(c);
        } else if (c as u32) <= 0xffff {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            let value = (c as u32) - 0x1_0000;
            out.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xd800 + (value >> 10),
                0xdc00 + (value & 0x3ff)
            ));
        }
    }
    out
}

fn int_of(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(value) if !value.is_boolean() => value.as_i64(),
        _ => None,
    }
}

fn textual(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(other) => Some(ascii_dump(other)),
    }
}

fn map_of(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// One continuation boundary between coordinator sessions.
#[derive(Clone, Debug, PartialEq)]
pub struct ContinuationLink {
    /// Predecessor thread.
    pub predecessor_thread_id: String,
    /// Successor thread.
    pub thread_id: String,
    /// Predecessor source path.
    pub predecessor_source_path: String,
    /// Predecessor boundary line.
    pub predecessor_source_line: i64,
    /// Predecessor boundary instant (ms).
    pub predecessor_at_ms: i64,
    /// Successor source path.
    pub source_path: String,
    /// Successor start instant (ms).
    pub started_at_ms: i64,
    /// Gap between the two (ms).
    pub gap_ms: i64,
}

impl ContinuationLink {
    /// The durable JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "predecessor_thread_id".into(),
            Value::String(self.predecessor_thread_id.clone()),
        );
        out.insert("thread_id".into(), Value::String(self.thread_id.clone()));
        out.insert(
            "predecessor_source_path".into(),
            Value::String(self.predecessor_source_path.clone()),
        );
        out.insert(
            "predecessor_source_line".into(),
            self.predecessor_source_line.into(),
        );
        out.insert("predecessor_at_ms".into(), self.predecessor_at_ms.into());
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert("started_at_ms".into(), self.started_at_ms.into());
        out.insert("gap_ms".into(), self.gap_ms.into());
        Value::Object(out)
    }

    /// Validate one continuation record from a manifest.
    pub fn from_json(raw: &Value, where_: &str) -> Result<ContinuationLink, String> {
        let item = as_object(raw, where_)?;
        let expected = [
            "predecessor_thread_id",
            "thread_id",
            "predecessor_source_path",
            "predecessor_source_line",
            "predecessor_at_ms",
            "source_path",
            "started_at_ms",
            "gap_ms",
        ];
        let held: Vec<&str> = item.keys().map(String::as_str).collect();
        if held.len() != expected.len() || !expected.iter().all(|key| item.contains_key(*key)) {
            let missing: Vec<String> = expected
                .iter()
                .filter(|key| !item.contains_key(**key))
                .map(|key| format!("'{key}'"))
                .collect();
            let mut unknown: Vec<String> = held
                .iter()
                .filter(|key| !expected.contains(key))
                .map(|key| format!("'{key}'"))
                .collect();
            unknown.sort();
            return Err(format!(
                "{where_}: invalid continuation fields; missing=[{}], unknown=[{}]",
                missing.join(", "),
                unknown.join(", ")
            ));
        }
        let predecessor_thread_id = str_of(item.get("predecessor_thread_id"));
        let thread_id = str_of(item.get("thread_id"));
        let predecessor_source_path = str_of(item.get("predecessor_source_path"));
        let predecessor_source_line = int_of(item.get("predecessor_source_line"));
        let predecessor_at_ms = int_of(item.get("predecessor_at_ms"));
        let source_path = str_of(item.get("source_path"));
        let started_at_ms = int_of(item.get("started_at_ms"));
        let gap_ms = int_of(item.get("gap_ms"));
        let malformed = || format!("{where_}: malformed continuation record");
        let (Some(predecessor_thread_id), Some(thread_id)) = (predecessor_thread_id, thread_id)
        else {
            return Err(malformed());
        };
        if predecessor_thread_id.is_empty()
            || thread_id.is_empty()
            || predecessor_thread_id == thread_id
        {
            return Err(malformed());
        }
        let (Some(predecessor_source_path), Some(source_path)) =
            (predecessor_source_path, source_path)
        else {
            return Err(malformed());
        };
        let (Some(predecessor_source_line), Some(predecessor_at_ms)) =
            (predecessor_source_line, predecessor_at_ms)
        else {
            return Err(malformed());
        };
        let (Some(started_at_ms), Some(gap_ms)) = (started_at_ms, gap_ms) else {
            return Err(malformed());
        };
        if predecessor_source_line < 1
            || predecessor_at_ms < 0
            || started_at_ms < 0
            || gap_ms <= 0
            || gap_ms != started_at_ms - predecessor_at_ms
        {
            return Err(malformed());
        }
        safe_snapshot_relative(&predecessor_source_path)?;
        safe_snapshot_relative(&source_path)?;
        Ok(ContinuationLink {
            predecessor_thread_id,
            thread_id,
            predecessor_source_path,
            predecessor_source_line,
            predecessor_at_ms,
            source_path,
            started_at_ms,
            gap_ms,
        })
    }
}

/// Result of copying a lineage into the snapshot store.
pub struct SnapshotResult {
    /// Source copies, in path order.
    pub sources: Vec<SourceCopy>,
    /// Files changed.
    pub files_changed: i64,
    /// Continuation links.
    pub continuations: Vec<ContinuationLink>,
    /// Upstream-retired source paths.
    pub upstream_retired: Vec<String>,
}

fn safe_snapshot_relative(source_path: &str) -> Result<(), String> {
    let parts: Vec<&str> = source_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if source_path.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(format!(
            "unsafe source snapshot path {}",
            py_repr_str(source_path)
        ));
    }
    Ok(())
}

fn snapshot_target(snapshot_root: &Path, source_path: &str) -> Result<PathBuf, String> {
    safe_snapshot_relative(source_path)?;
    let mut target = snapshot_root.to_path_buf();
    for part in source_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        target.push(part);
    }
    Ok(target)
}

fn parse_json_object(raw: &[u8], where_: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_slice::<Value>(raw) {
        Ok(Value::Object(map)) if !map.is_empty() => Ok(map),
        Ok(_) => Err(format!("{where_}: expected a non-empty JSON object")),
        Err(error) => Err(format!("{where_}: invalid JSON: {error}")),
    }
}

fn complete_records(complete: &[u8], where_: &str) -> Result<Vec<RolloutRecord>, String> {
    parse_rollout_records(complete).map_err(|error| match error {
        RolloutError::InvalidJson(line) => {
            let lines: Vec<&[u8]> = complete.split(|byte| *byte == b'\n').collect();
            let raw = lines.get((line - 1) as usize).copied().unwrap_or(b"");
            let detail = serde_json::from_slice::<Value>(raw)
                .err()
                .map(|err| err.to_string())
                .unwrap_or_default();
            format!("{where_}:{line}: invalid JSON: {detail}")
        }
        RolloutError::NotAnObject(line) => {
            format!("{where_}:{line}: expected a non-empty JSON object")
        }
    })
}

fn record_payload(record: &RolloutRecord) -> Map<String, Value> {
    record.payload()
}

fn fallback_id(thread_id: &str, record: &RolloutRecord, kind: &str) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(thread_id.as_bytes());
    material.push(0);
    material.extend_from_slice(kind.as_bytes());
    material.push(0);
    material.extend_from_slice(record.line.to_string().as_bytes());
    material.push(0);
    material.extend_from_slice(&record.raw);
    // Python hashes the four parts with explicit NUL separators
    // between (and only between) them; the concatenation above
    // already has that shape.
    let digest = sha256_hex(&material);
    format!("evt_{}", &digest[..24])
}

// ---------------------------------------------------------------- fd snapshot IO

fn py_os(error: &std::io::Error, name: &str) -> String {
    let errno = error.raw_os_error().unwrap_or(0);
    let raw = error.to_string();
    let message = raw
        .strip_suffix(&format!(" (os error {errno})"))
        .unwrap_or(&raw)
        .to_string();
    format!("[Errno {errno}] {message}: '{name}'")
}

fn lexical_absolute(path: &Path) -> PathBuf {
    // `Path.absolute()`: lexical absolutization only, with no
    // symlink substitution (unlike `Path.resolve()`).
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| Path::new("/").to_path_buf())
            .join(path)
    };
    let mut out = PathBuf::from("/");
    for part in joined.components() {
        match part {
            std::path::Component::Normal(name) => out.push(name),
            std::path::Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

fn open_directory_no_symlinks(path: &Path, create: bool) -> Result<Option<PathBuf>, String> {
    // Component-by-component walk that never follows a symlink
    // component (the std-only spelling of the Python edition's
    // O_NOFOLLOW directory walk).
    let absolute = lexical_absolute(path);
    let mut current = PathBuf::from("/");
    let components: Vec<String> = absolute
        .components()
        .skip(1)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if components.is_empty() && absolute != Path::new("/") {
        return Err(format!(
            "invalid empty snapshot directory path: {}",
            path.display()
        ));
    }
    for part in &components {
        let candidate = current.join(part);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    // O_DIRECTORY|O_NOFOLLOW on a symlink
                    // reports ENOTDIR from CPython's os.open.
                    let kind = (libc::ENOTDIR, "Not a directory");
                    return Err(format!(
                        "symlink or non-directory snapshot directory component {} in {}: [Errno {}] {}: '{}'",
                        py_repr_str(part),
                        absolute.display(),
                        kind.0,
                        kind.1,
                        part
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&candidate)
                    .map_err(|error| {
                        format!(
                            "cannot create snapshot directory component {}: {}",
                            py_repr_str(part),
                            py_os(&error, part)
                        )
                    })?;
            }
            Err(error) => {
                return Err(format!(
                    "invalid snapshot directory component {} in {}: {}",
                    py_repr_str(part),
                    absolute.display(),
                    py_os(&error, part)
                ));
            }
        }
        current = candidate;
    }
    Ok(Some(current))
}

fn file_name_of(relative: &str) -> String {
    relative
        .rsplit('/')
        .find(|part| !part.is_empty() && *part != ".")
        .unwrap_or_default()
        .to_string()
}

fn parent_of(snapshot_root: &Path, source_path: &str) -> PathBuf {
    let parts: Vec<&str> = source_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let mut parent = snapshot_root.to_path_buf();
    for part in &parts[..parts.len().saturating_sub(1)] {
        parent.push(part);
    }
    parent
}

struct OpenedSnapshot {
    data: Vec<u8>,
    mtime_ns: i64,
}

fn checked_file(path: &Path, display: &Path) -> Result<Option<std::fs::File>, String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(py_os(&error, &path.display().to_string())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "source snapshot target must not be a symlink: {}",
            display.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "source snapshot target is not a regular file: {}",
            display.display()
        ));
    }
    let file = std::fs::File::open(path).map_err(|error| {
        format!(
            "cannot safely open source snapshot {}: {}",
            display.display(),
            py_os(&error, &display.display().to_string())
        )
    })?;
    let opened = file
        .metadata()
        .map_err(|error| py_os(&error, &display.display().to_string()))?;
    if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
        return Err(format!(
            "source snapshot changed while opening: {}",
            display.display()
        ));
    }
    Ok(Some(file))
}

fn open_snapshot_file(
    snapshot_root: &Path,
    source_path: &str,
) -> Result<Option<OpenedSnapshot>, String> {
    safe_snapshot_relative(source_path)?;
    let parent = parent_of(snapshot_root, source_path);
    let Some(parent) = open_directory_no_symlinks(&parent, false)? else {
        return Ok(None);
    };
    let display = snapshot_target(snapshot_root, source_path)?;
    let candidate = parent.join(file_name_of(source_path));
    let Some(mut file) = checked_file(&candidate, &display)? else {
        return Ok(None);
    };
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;
    let meta = file
        .metadata()
        .map_err(|error| py_os(&error, &display.display().to_string()))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| py_os(&error, &display.display().to_string()))?;
    Ok(Some(OpenedSnapshot {
        data,
        mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
    }))
}

fn read_snapshot_file(
    snapshot_root: &Path,
    source_path: &str,
) -> Result<Option<OpenedSnapshot>, String> {
    open_snapshot_file(snapshot_root, source_path)
}

fn first_snapshot_metadata(
    snapshot_root: &Path,
    source_path: &str,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(opened) = open_snapshot_file(snapshot_root, source_path)? else {
        return Ok(None);
    };
    let first = match opened.data.iter().position(|byte| *byte == b'\n') {
        Some(index) => &opened.data[..index],
        None => &opened.data[..],
    };
    if first.is_empty() {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_slice::<Value>(first) else {
        return Ok(None);
    };
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Ok(None);
    }
    Ok(Some(map_of(value.get("payload"))))
}

fn random_hex8() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read as _;
        let _ = file.read_exact(&mut bytes);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_snapshot_file(
    snapshot_root: &Path,
    source_path: &str,
    data: &[u8],
    expected: Option<&[u8]>,
) -> Result<bool, String> {
    safe_snapshot_relative(source_path)?;
    let parent = parent_of(snapshot_root, source_path);
    let Some(parent) = open_directory_no_symlinks(&parent, true)? else {
        return Err(format!(
            "cannot create source snapshot parent for {}",
            py_repr_str(source_path)
        ));
    };
    let name = file_name_of(source_path);
    let display = snapshot_target(snapshot_root, source_path)?;
    let candidate = parent.join(&name);
    let current = read_at_path(&candidate, &display)?;
    if current.as_deref() != expected {
        return Err(format!(
            "source snapshot changed after validation: {}",
            display.display()
        ));
    }
    if current.as_deref() == Some(data) {
        return Ok(false);
    }
    let temporary = parent.join(format!(".{name}.{}.{}", std::process::id(), random_hex8()));
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| py_os(&error, &temporary.display().to_string()))?;
        let written = file
            .write_all(data)
            .and_then(|()| file.sync_all())
            .map_err(|error| py_os(&error, &temporary.display().to_string()));
        if let Err(message) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(message);
        }
    }
    if let Err(error) = std::fs::rename(&temporary, &candidate) {
        let _ = std::fs::remove_file(&temporary);
        return Err(py_os(&error, &name));
    }
    if let Ok(dir) = std::fs::File::open(&parent) {
        let _ = dir.sync_all();
    }
    Ok(true)
}

fn read_at_path(candidate: &Path, display: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read as _;
    let Some(mut file) = checked_file(candidate, display)? else {
        return Ok(None);
    };
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| py_os(&error, &display.display().to_string()))?;
    Ok(Some(data))
}

// ---------------------------------------------------------------- lineage

fn lineage_roots(
    root_thread_id: &str,
    continuation_thread_ids: &[String],
) -> Result<Vec<String>, String> {
    let mut roots = vec![root_thread_id.to_string()];
    roots.extend(continuation_thread_ids.iter().cloned());
    if roots.iter().any(|item| item.is_empty()) {
        return Err("Codex root and continuation session IDs must not be empty".to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    if roots.iter().any(|item| !seen.insert(item.clone())) {
        return Err("Codex root and continuation session IDs must be unique".to_string());
    }
    Ok(roots)
}

fn metadata_lineage_root(
    metadata: &Map<String, Value>,
    lineage_roots: &[String],
    where_: &str,
) -> Result<String, String> {
    let thread_id = str_of(metadata.get("id"));
    let session_id = str_of(metadata.get("session_id"));
    let matches: Vec<&String> = lineage_roots
        .iter()
        .filter(|root| {
            thread_id.as_deref() == Some(root.as_str())
                || session_id.as_deref() == Some(root.as_str())
        })
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "{where_}: source belongs to {} configured Codex lineages",
            matches.len()
        ));
    }
    Ok(matches[0].clone())
}

fn first_metadata(path: &Path) -> Option<Map<String, Value>> {
    let line = read_first_line(path).ok()?;
    if line.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_slice(&line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    Some(map_of(value.get("payload")))
}

#[allow(clippy::type_complexity)]
fn discover_codex_lineage(
    sessions_root: &Path,
    root_thread_id: &str,
    exclude_root: Option<&Path>,
    continuation_thread_ids: &[String],
) -> Result<Vec<(PathBuf, Map<String, Value>)>, String> {
    let roots = lineage_roots(root_thread_id, continuation_thread_ids)?;
    let excluded = exclude_root.map(crate::json::resolve_path);
    let mut candidates = Vec::new();
    for path in iter_logical_glob_rollouts(sessions_root) {
        if let Some(excluded) = &excluded {
            if crate::json::resolve_path(&path).starts_with(excluded) {
                continue;
            }
        }
        let Some(metadata) = first_metadata(&path) else {
            continue;
        };
        let thread_id = str_of(metadata.get("id"));
        let session_id = str_of(metadata.get("session_id"));
        if thread_id.is_some_and(|id| roots.contains(&id))
            || session_id.is_some_and(|id| roots.contains(&id))
        {
            candidates.push((path, metadata));
        }
    }
    Ok(candidates)
}

/// Structured session metadata, root first, for identity.
pub fn codex_identity_metadata(
    snapshot_root: &Path,
    source_paths: &[String],
    root_thread_id: &str,
    continuation_thread_ids: &[String],
) -> Result<Vec<Map<String, Value>>, String> {
    let roots = lineage_roots(root_thread_id, continuation_thread_ids)?;
    let root_order: BTreeMap<&str, usize> = roots
        .iter()
        .enumerate()
        .map(|(index, root)| (root.as_str(), index))
        .collect();
    let mut records: Vec<(Map<String, Value>, String)> = Vec::new();
    for source_path in source_paths {
        let Some(metadata) = first_snapshot_metadata(snapshot_root, source_path)? else {
            return Err(format!(
                "source snapshot lacks valid session metadata: {}",
                py_repr_str(source_path)
            ));
        };
        let lineage = metadata_lineage_root(&metadata, &roots, source_path)?;
        records.push((metadata, lineage));
    }
    records.sort_by(|(left_meta, left_root), (right_meta, right_root)| {
        let left_id = str_of(left_meta.get("id")).unwrap_or_default();
        let right_id = str_of(right_meta.get("id")).unwrap_or_default();
        (
            root_order[left_root.as_str()],
            i64::from(left_id != *left_root),
            left_id,
        )
            .cmp(&(
                root_order[right_root.as_str()],
                i64::from(right_id != *right_root),
                right_id,
            ))
    });
    Ok(records.into_iter().map(|(metadata, _)| metadata).collect())
}

fn continuation_links(
    roots: &[String],
    root_sources: &BTreeMap<String, String>,
    complete_by_path: &BTreeMap<String, Vec<u8>>,
    metadata_by_path: &BTreeMap<String, Map<String, Value>>,
    previous: &[ContinuationLink],
) -> Result<Vec<ContinuationLink>, String> {
    let expected_pairs: Vec<(&String, &String)> = roots.iter().zip(roots.iter().skip(1)).collect();
    if previous.len() > expected_pairs.len() {
        return Err("source manifest records more continuations than were configured".to_string());
    }
    let mut result = Vec::new();
    for (index, (predecessor_id, successor_id)) in expected_pairs.iter().enumerate() {
        let predecessor_path = &root_sources[*predecessor_id];
        let successor_path = &root_sources[*successor_id];
        let successor_started_ms = iso_ms(
            metadata_by_path[successor_path]
                .get("timestamp")
                .unwrap_or(&Value::Null),
        );
        if successor_started_ms <= 0 {
            return Err(format!(
                "continuation root {} lacks a valid start timestamp",
                py_repr_str(successor_id)
            ));
        }
        let predecessor_records =
            complete_records(&complete_by_path[predecessor_path], predecessor_path)?;
        if index < previous.len() {
            let link = previous[index].clone();
            if link.predecessor_thread_id != **predecessor_id
                || link.thread_id != **successor_id
                || link.predecessor_source_path != *predecessor_path
                || link.source_path != *successor_path
                || link.started_at_ms != successor_started_ms
                || link.predecessor_source_line > predecessor_records.len() as i64
            {
                return Err(format!(
                    "recorded continuation {index} no longer matches its configured sessions"
                ));
            }
            let recorded = &predecessor_records[(link.predecessor_source_line - 1) as usize];
            if recorded.timestamp_ms() != link.predecessor_at_ms {
                return Err(format!(
                    "recorded continuation {index} boundary evidence changed"
                ));
            }
            result.push(link);
            continue;
        }
        let eligible: Vec<&RolloutRecord> = predecessor_records
            .iter()
            .filter(|record| {
                let stamp = record.timestamp_ms();
                stamp > 0 && stamp < successor_started_ms
            })
            .collect();
        if eligible.is_empty() {
            return Err(format!(
                "continuation {} has no predecessor record before its start",
                py_repr_str(successor_id)
            ));
        }
        let boundary = eligible
            .iter()
            .max_by(|a, b| (a.timestamp_ms(), a.line).cmp(&(b.timestamp_ms(), b.line)))
            .unwrap();
        let predecessor_at_ms = boundary.timestamp_ms();
        result.push(ContinuationLink {
            predecessor_thread_id: (*predecessor_id).clone(),
            thread_id: (*successor_id).clone(),
            predecessor_source_path: predecessor_path.clone(),
            predecessor_source_line: boundary.line,
            predecessor_at_ms,
            source_path: successor_path.clone(),
            started_at_ms: successor_started_ms,
            gap_ms: successor_started_ms - predecessor_at_ms,
        });
    }
    Ok(result)
}

struct PendingCopy {
    source: SourceCopy,
    complete: Vec<u8>,
    baseline: Option<Vec<u8>>,
}

/// Copy the newline-complete lineage into *snapshot_root*
/// after monotonic checks.
#[allow(clippy::too_many_arguments)]
pub fn snapshot_codex_lineage(
    sessions_root: &Path,
    root_thread_id: &str,
    snapshot_root: &Path,
    previous_sources: &[SourceCopy],
    updated_at: &str,
    continuation_thread_ids: &[String],
    previous_continuations: &[ContinuationLink],
) -> Result<SnapshotResult, String> {
    let mut previous_by_path: BTreeMap<String, SourceCopy> = BTreeMap::new();
    for source in previous_sources {
        if previous_by_path.contains_key(&source.source_path) {
            return Err(format!(
                "source manifest contains duplicate path {}",
                py_repr_str(&source.source_path)
            ));
        }
        previous_by_path.insert(source.source_path.clone(), source.clone());
    }
    let roots = lineage_roots(root_thread_id, continuation_thread_ids)?;
    let candidates = discover_codex_lineage(
        sessions_root,
        root_thread_id,
        Some(snapshot_root),
        continuation_thread_ids,
    )?;
    let mut candidate_paths: BTreeMap<String, (PathBuf, Map<String, Value>)> = BTreeMap::new();
    let mut lineage_by_thread: BTreeMap<String, String> = BTreeMap::new();
    let mut root_sources: BTreeMap<String, String> = BTreeMap::new();
    for (path, metadata) in candidates {
        let relative = path
            .strip_prefix(sessions_root)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .map_err(|_| {
                format!(
                    "rollout {} is outside sessions root {}",
                    path.display(),
                    sessions_root.display()
                )
            })?;
        safe_snapshot_relative(&relative)?;
        if candidate_paths.contains_key(&relative) {
            return Err(format!(
                "duplicate Codex source path {}",
                py_repr_str(&relative)
            ));
        }
        let lineage_root_id = metadata_lineage_root(&metadata, &roots, &relative)?;
        let Some(thread_id) = str_of(metadata.get("id")) else {
            return Err(format!(
                "rollout {} metadata lacks a thread id",
                path.display()
            ));
        };
        if let Some(prior) = lineage_by_thread.get(&thread_id) {
            if *prior != lineage_root_id {
                return Err(format!(
                    "Codex thread {} occurs in multiple configured lineages",
                    py_repr_str(&thread_id)
                ));
            }
        }
        lineage_by_thread.insert(thread_id.clone(), lineage_root_id.clone());
        if thread_id == lineage_root_id {
            if lineage_root_id != root_thread_id
                && str_of(metadata.get("session_id")).as_deref() != Some(lineage_root_id.as_str())
            {
                return Err(format!(
                    "configured continuation {} is not a root session",
                    py_repr_str(&lineage_root_id)
                ));
            }
            if root_sources.contains_key(&lineage_root_id) {
                return Err(format!(
                    "configured Codex root {} has multiple rollouts",
                    py_repr_str(&lineage_root_id)
                ));
            }
            root_sources.insert(lineage_root_id.clone(), relative.clone());
        }
        candidate_paths.insert(relative, (path, metadata));
    }
    // Upstream-retired sources stand in from the archive's
    // own snapshot, verified against their manifest record.
    let mut retired_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for source_path in previous_by_path.keys() {
        if candidate_paths.contains_key(source_path) {
            continue;
        }
        let recorded = &previous_by_path[source_path];
        let plain = sessions_root.join(source_path);
        if resolve_source(&plain).is_some() {
            return Err(format!(
                "append-only source violation: previously observed rollout is still present \
                 but no longer readable as this lineage: {}",
                py_repr_str(source_path)
            ));
        }
        let target = snapshot_target(snapshot_root, source_path)?;
        let Some(held) = read_snapshot_file(snapshot_root, source_path)? else {
            return Err(format!(
                "previously observed rollout is gone upstream and the archive's own snapshot \
                 is missing too: {} (expected {})",
                py_repr_str(source_path),
                target.display()
            ));
        };
        let data = held.data;
        if (data.len() as i64) < recorded.copied_bytes
            || sha256_hex(&data[..recorded.copied_bytes as usize]) != recorded.sha256
        {
            return Err(format!(
                "previously observed rollout is gone upstream and the archive's own snapshot \
                 does not match its manifest record: {}",
                target.display()
            ));
        }
        let complete = data[..recorded.copied_bytes as usize].to_vec();
        let metadata_line = complete.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
        let record = parse_json_object(
            metadata_line,
            &format!("{}: archived metadata", target.display()),
        )?;
        if record.get("type").and_then(Value::as_str) != Some("session_meta") {
            return Err(format!(
                "archived rollout {} lacks session metadata",
                target.display()
            ));
        }
        let metadata = map_of(record.get("payload"));
        let lineage_root_id = metadata_lineage_root(&metadata, &roots, source_path)?;
        let thread_id = str_of(metadata.get("id"));
        if thread_id.as_deref() != Some(recorded.thread_id.as_str()) || thread_id.is_none() {
            return Err(format!(
                "archived rollout {} thread identity disagrees with its manifest record",
                target.display()
            ));
        }
        let thread_id = thread_id.unwrap();
        if let Some(prior) = lineage_by_thread.get(&thread_id) {
            if *prior != lineage_root_id {
                return Err(format!(
                    "Codex thread {} occurs in multiple configured lineages",
                    py_repr_str(&thread_id)
                ));
            }
        }
        lineage_by_thread.insert(thread_id.clone(), lineage_root_id.clone());
        if thread_id == lineage_root_id {
            if root_sources.contains_key(&lineage_root_id) {
                return Err(format!(
                    "configured Codex root {} has multiple rollouts",
                    py_repr_str(&lineage_root_id)
                ));
            }
            root_sources.insert(lineage_root_id.clone(), source_path.clone());
        }
        candidate_paths.insert(
            source_path.clone(),
            (PathBuf::from(&recorded.original_path), metadata),
        );
        retired_bytes.insert(source_path.clone(), complete);
    }
    let missing_roots: Vec<&String> = roots
        .iter()
        .filter(|root| !root_sources.contains_key(*root))
        .collect();
    if !missing_roots.is_empty() {
        let names: Vec<String> = missing_roots.iter().map(|root| py_repr_str(root)).collect();
        return Err(format!(
            "Codex configured root sessions were not found: {}",
            names.join(", ")
        ));
    }
    if candidate_paths.is_empty() {
        return Err(format!(
            "Codex root thread {} was not found",
            py_repr_str(root_thread_id)
        ));
    }
    let mut pending: Vec<PendingCopy> = Vec::new();
    let mut complete_by_path: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut metadata_by_path: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (source_path, (path, metadata)) in &candidate_paths {
        let retired = retired_bytes.contains_key(source_path);
        let data = if retired {
            retired_bytes[source_path].clone()
        } else {
            read_source_bytes(path).map_err(|error| {
                if error.contains("no such file (plain or") {
                    format!("cannot read rollout {}: {error}", path.display())
                } else {
                    error
                }
            })?
        };
        let complete = complete_prefix(&data).to_vec();
        if complete.is_empty() {
            return Err(format!(
                "rollout {} has no newline-complete metadata record",
                path.display()
            ));
        }
        let first_line = complete.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
        let copied_record =
            parse_json_object(first_line, &format!("{}: copied metadata", path.display()))?;
        if copied_record.get("type").and_then(Value::as_str) != Some("session_meta") {
            return Err(format!(
                "rollout {} copied prefix lacks session metadata",
                path.display()
            ));
        }
        let copied_metadata = map_of(copied_record.get("payload"));
        if copied_metadata != *metadata {
            return Err(format!(
                "rollout {} metadata changed between discovery and snapshot read",
                path.display()
            ));
        }
        let Some(thread_id) = str_of(copied_metadata.get("id")) else {
            return Err(format!(
                "rollout {} metadata lacks a thread id",
                path.display()
            ));
        };
        metadata_lineage_root(&copied_metadata, &roots, source_path)?;
        complete_by_path.insert(source_path.clone(), complete.clone());
        metadata_by_path.insert(source_path.clone(), copied_metadata);
        let target = snapshot_target(snapshot_root, source_path)?;
        let previous = previous_by_path.get(source_path);
        let opened_snapshot = read_snapshot_file(snapshot_root, source_path)?;
        let existing: Option<Vec<u8>> = opened_snapshot.map(|opened| opened.data);
        if let Some(previous) = previous {
            if previous.thread_id != thread_id {
                return Err(format!(
                    "append-only source violation: thread identity changed for {}",
                    py_repr_str(source_path)
                ));
            }
            let Some(existing_bytes) = &existing else {
                return Err(format!(
                    "source snapshot for previously observed rollout is missing: {}",
                    target.display()
                ));
            };
            if (existing_bytes.len() as i64) < previous.copied_bytes {
                return Err(format!(
                    "source snapshot is shorter than its manifest record: {}",
                    target.display()
                ));
            }
            if sha256_hex(&existing_bytes[..previous.copied_bytes as usize]) != previous.sha256 {
                return Err(format!(
                    "source snapshot does not match its manifest record: {}",
                    target.display()
                ));
            }
        }
        if retired {
            let previous = previous.unwrap();
            let mut source = previous.clone();
            if source
                .upstream_retired_at
                .as_deref()
                .is_none_or(|held| held.is_empty())
            {
                source.upstream_retired_at = Some(updated_at.to_string());
            }
            pending.push(PendingCopy {
                source,
                complete: existing.clone().unwrap(),
                baseline: existing,
            });
            continue;
        }
        let baseline = existing.clone().unwrap_or_default();
        if complete.len() < baseline.len() {
            return Err(format!(
                "append-only source violation: newline-complete prefix shrank for {} ({} -> {} bytes)",
                py_repr_str(source_path),
                baseline.len(),
                complete.len()
            ));
        }
        if complete[..baseline.len()] != baseline[..] {
            return Err(format!(
                "append-only source violation: existing prefix was rewritten for {}",
                py_repr_str(source_path)
            ));
        }
        let content_changed = complete != baseline;
        let manifest_advanced = previous.is_none()
            || complete.len() as i64 != previous.map(|p| p.copied_bytes).unwrap_or(0);
        let source_updated_at = if content_changed || manifest_advanced || previous.is_none() {
            updated_at.to_string()
        } else {
            previous
                .map(|held| held.updated_at.clone())
                .unwrap_or_default()
        };
        pending.push(PendingCopy {
            source: SourceCopy {
                source_path: source_path.clone(),
                original_path: crate::json::resolve_path(&logical_path(path))
                    .display()
                    .to_string(),
                snapshot_path: source_path.clone(),
                thread_id,
                copied_bytes: complete.len() as i64,
                line_count: complete.iter().filter(|byte| **byte == b'\n').count() as i64,
                sha256: sha256_hex(&complete),
                updated_at: source_updated_at,
                upstream_retired_at: None,
            },
            complete,
            baseline: existing,
        });
    }
    let continuations = continuation_links(
        &roots,
        &root_sources,
        &complete_by_path,
        &metadata_by_path,
        previous_continuations,
    )?;
    let mut changed = 0i64;
    for item in &pending {
        if write_snapshot_file(
            snapshot_root,
            &item.source.snapshot_path,
            &item.complete,
            item.baseline.as_deref(),
        )? {
            changed += 1;
        }
    }
    Ok(SnapshotResult {
        sources: pending.into_iter().map(|item| item.source).collect(),
        files_changed: changed,
        continuations,
        upstream_retired: retired_bytes.keys().cloned().collect(),
    })
}

// ---------------------------------------------------------------- team load

struct Rollout {
    #[allow(dead_code)]
    path: PathBuf,
    source_path: String,
    snapshot: SourceSnapshot,
    metadata: Map<String, Value>,
    canonical_records: Vec<RolloutRecord>,
    lineage_root_id: String,
}

struct AgentSeed {
    thread_id: String,
    parent_thread_id: Option<String>,
    agent_path: String,
    nickname: Option<String>,
    role: Option<String>,
    depth: i64,
    started_at_ms: i64,
    source_path: String,
    lineage_root_id: String,
    raw_agent_path: String,
}

fn canonical_records(
    records: &[RolloutRecord],
    metadata: &Map<String, Value>,
    root_thread_id: &str,
) -> Vec<RolloutRecord> {
    let thread_id = str_of(metadata.get("id")).unwrap_or_default();
    if thread_id == root_thread_id {
        return records.iter().skip(1).cloned().collect();
    }
    let agent_path = str_of(metadata.get("agent_path"));
    let mut initial_turn_id: Option<String> = None;
    let mut incoming_line = 0i64;
    if let Some(agent_path) = &agent_path {
        for record in records.iter().skip(1) {
            if record.kind() != Some("response_item") {
                continue;
            }
            let payload = record_payload(record);
            if payload.get("type").and_then(Value::as_str) != Some("agent_message") {
                continue;
            }
            if str_of(payload.get("recipient")).as_deref() != Some(agent_path.as_str()) {
                continue;
            }
            let passthrough = map_of(payload.get("internal_chat_message_metadata_passthrough"));
            initial_turn_id = str_of(passthrough.get("turn_id"));
            incoming_line = record.line;
            if initial_turn_id.is_some() {
                break;
            }
        }
    }
    if let Some(initial_turn_id) = &initial_turn_id {
        for (index, record) in records.iter().enumerate() {
            if record.line > incoming_line || record.kind() != Some("event_msg") {
                continue;
            }
            let payload = record_payload(record);
            if payload.get("type").and_then(Value::as_str) == Some("task_started")
                && str_of(payload.get("turn_id")).as_deref() == Some(initial_turn_id.as_str())
            {
                return records[index..].to_vec();
            }
        }
    }
    let session_started_ms = iso_ms(metadata.get("timestamp").unwrap_or(&Value::Null));
    for (index, record) in records.iter().enumerate().skip(1) {
        if record.kind() != Some("event_msg") {
            continue;
        }
        let payload = record_payload(record);
        if payload.get("type").and_then(Value::as_str) != Some("task_started") {
            continue;
        }
        let started_ms = numeric_epoch_ms(
            payload.get("started_at").unwrap_or(&Value::Null),
            record.timestamp_ms(),
        );
        if started_ms >= session_started_ms {
            return records[index..].to_vec();
        }
    }
    Vec::new()
}

fn read_rollout(
    path: &Path,
    sessions_root: &Path,
    metadata: &Map<String, Value>,
    lineage_root_id: &str,
    secure_source_path: Option<&str>,
) -> Result<Rollout, String> {
    let (data, mtime_ns, size) = if let Some(secure) = secure_source_path {
        let Some(opened) = read_snapshot_file(sessions_root, secure)? else {
            return Err(format!(
                "source snapshot disappeared before parsing: {}",
                path.display()
            ));
        };
        (
            opened.data.clone(),
            opened.mtime_ns,
            opened.data.len() as i64,
        )
    } else {
        let data = read_source_bytes(path).map_err(|error| {
            if error.contains("no such file (plain or") {
                format!("cannot read rollout {}: {error}", path.display())
            } else {
                error
            }
        })?;
        let physical = resolve_source(path).unwrap_or_else(|| path.to_path_buf());
        let meta = std::fs::metadata(&physical)
            .map_err(|error| crate::json::os_error(&error, &physical))?;
        use std::os::unix::fs::MetadataExt as _;
        (
            data,
            meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            meta.size() as i64,
        )
    };
    let complete = complete_prefix(&data);
    let complete_bytes = complete.len();
    let records = complete_records(complete, &path.display().to_string())?;
    let thread_id = str_of(metadata.get("id")).unwrap_or_default();
    let source_path = path
        .strip_prefix(sessions_root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string());
    let snapshot = SourceSnapshot {
        path: source_path.clone(),
        thread_id: thread_id.clone(),
        size_bytes: data.len() as i64,
        mtime_ns,
        sha256: sha256_hex(&data),
        complete_bytes: complete_bytes as i64,
        line_count: records.len() as i64,
        working_directory: str_of(metadata.get("cwd")),
        repository_url: str_of(map_of(metadata.get("git")).get("repository_url")),
        semantic_sha256: None,
        semantic_complete_bytes: None,
    };
    let _ = size;
    Ok(Rollout {
        path: path.to_path_buf(),
        source_path,
        snapshot,
        metadata: metadata.clone(),
        canonical_records: canonical_records(&records, metadata, lineage_root_id),
        lineage_root_id: lineage_root_id.to_string(),
    })
}

fn agent_seed(rollout: &Rollout) -> AgentSeed {
    let metadata = &rollout.metadata;
    let thread_id = str_of(metadata.get("id")).unwrap_or_default();
    let source = map_of(metadata.get("source"));
    let subagent = map_of(source.get("subagent"));
    let spawn = map_of(subagent.get("thread_spawn"));
    let parent =
        str_of(metadata.get("parent_thread_id")).or_else(|| str_of(spawn.get("parent_thread_id")));
    let nickname =
        str_of(metadata.get("agent_nickname")).or_else(|| str_of(spawn.get("agent_nickname")));
    let role = str_of(metadata.get("agent_role")).or_else(|| str_of(spawn.get("agent_role")));
    let depth = int_of(spawn.get("depth")).unwrap_or(if thread_id == rollout.lineage_root_id {
        0
    } else {
        1
    });
    let path = str_of(metadata.get("agent_path"))
        .or_else(|| str_of(spawn.get("agent_path")))
        .unwrap_or_else(|| {
            if thread_id == rollout.lineage_root_id {
                "/root".to_string()
            } else {
                format!("/root/{thread_id}")
            }
        });
    AgentSeed {
        thread_id: thread_id.clone(),
        parent_thread_id: parent,
        agent_path: path.clone(),
        nickname,
        role,
        depth,
        started_at_ms: iso_ms(metadata.get("timestamp").unwrap_or(&Value::Null)),
        source_path: rollout.source_path.clone(),
        lineage_root_id: rollout.lineage_root_id.clone(),
        raw_agent_path: path,
    }
}

fn scoped_id(lineage_root_id: &str, root_thread_id: &str, raw_id: &str) -> String {
    if lineage_root_id == root_thread_id {
        raw_id.to_string()
    } else {
        format!(
            "codex-continuation-{}-{}-{}-{}",
            lineage_root_id.chars().count(),
            lineage_root_id,
            raw_id.chars().count(),
            raw_id
        )
    }
}

fn continuation_path(lineage_root_id: &str) -> String {
    let mut quoted = String::new();
    for byte in lineage_root_id.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~') {
            quoted.push(c);
        } else {
            quoted.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("/root/continuation-{quoted}")
}

fn normalize_seed(
    seed: &AgentSeed,
    root_thread_id: &str,
    continuation_thread_ids: &[String],
) -> Result<AgentSeed, String> {
    if seed.lineage_root_id == root_thread_id {
        return Ok(seed.clone_seed());
    }
    let Some(continuation_index) = continuation_thread_ids
        .iter()
        .position(|id| *id == seed.lineage_root_id)
    else {
        return Err(format!(
            "agent {} belongs to an unknown continuation",
            py_repr_str(&seed.thread_id)
        ));
    };
    let predecessor = if continuation_index == 0 {
        root_thread_id.to_string()
    } else {
        continuation_thread_ids[continuation_index - 1].clone()
    };
    let prefix = continuation_path(&seed.lineage_root_id);
    let raw_path = {
        let trimmed = seed.raw_agent_path.trim_end_matches('/');
        if trimmed.is_empty() {
            "/root"
        } else {
            trimmed
        }
    };
    let normalized_path = if raw_path == "/root" {
        prefix.clone()
    } else if let Some(rest) = raw_path.strip_prefix("/root/") {
        format!("{prefix}/{rest}")
    } else {
        format!("{prefix}/{}", raw_path.trim_matches('/'))
    };
    let is_lineage_root = seed.thread_id == seed.lineage_root_id;
    Ok(AgentSeed {
        thread_id: seed.thread_id.clone(),
        parent_thread_id: if is_lineage_root {
            Some(predecessor)
        } else {
            seed.parent_thread_id.clone()
        },
        agent_path: normalized_path,
        nickname: seed.nickname.clone(),
        role: if is_lineage_root {
            Some("coordinator".to_string())
        } else {
            seed.role.clone()
        },
        depth: if is_lineage_root {
            continuation_index as i64 + 1
        } else {
            seed.depth + continuation_index as i64 + 1
        },
        started_at_ms: seed.started_at_ms,
        source_path: seed.source_path.clone(),
        lineage_root_id: seed.lineage_root_id.clone(),
        raw_agent_path: seed.raw_agent_path.clone(),
    })
}

impl AgentSeed {
    fn clone_seed(&self) -> AgentSeed {
        AgentSeed {
            thread_id: self.thread_id.clone(),
            parent_thread_id: self.parent_thread_id.clone(),
            agent_path: self.agent_path.clone(),
            nickname: self.nickname.clone(),
            role: self.role.clone(),
            depth: self.depth,
            started_at_ms: self.started_at_ms,
            source_path: self.source_path.clone(),
            lineage_root_id: self.lineage_root_id.clone(),
            raw_agent_path: self.raw_agent_path.clone(),
        }
    }
}

type PromptPairs = Vec<((i64, String), Vec<String>)>;

fn prompt_pairs(records: &[RolloutRecord]) -> PromptPairs {
    let mut pairs: PromptPairs = Vec::new();
    for record in records {
        if record.kind() != Some("response_item") {
            continue;
        }
        let payload = record_payload(record);
        if payload.get("type").and_then(Value::as_str) != Some("message")
            || payload.get("role").and_then(Value::as_str) != Some("user")
        {
            continue;
        }
        let (text, _, _) = content_parts(&payload);
        if let (Some(text), Some(item_id)) = (text, sval(payload.get("id"))) {
            let key = (record.timestamp_ms(), text);
            match pairs.iter_mut().find(|(held, _)| *held == key) {
                Some((_, ids)) => ids.push(item_id),
                None => pairs.push((key, vec![item_id])),
            }
        }
    }
    pairs
}

fn claim_prompt_id(pairs: &mut PromptPairs, timestamp_ms: i64, text: &str) -> Option<String> {
    for ((candidate_ms, candidate_text), ids) in pairs.iter_mut() {
        if *candidate_ms == timestamp_ms && candidate_text == text && !ids.is_empty() {
            return Some(ids.remove(0));
        }
    }
    let mut best: Option<(i64, usize)> = None;
    for (index, ((candidate_ms, candidate_text), ids)) in pairs.iter().enumerate() {
        if candidate_text == text && !ids.is_empty() {
            let delta = (candidate_ms - timestamp_ms).abs();
            if delta <= 10 && best.as_ref().is_none_or(|(held, _)| delta < *held) {
                best = Some((delta, index));
            }
        }
    }
    let (_, index) = best?;
    Some(pairs[index].1.remove(0))
}

fn nested_tools(name: &str, input_text: Option<&str>) -> Vec<(String, i64)> {
    if name != "exec" {
        return Vec::new();
    }
    let Some(input_text) = input_text else {
        return Vec::new();
    };
    let regex = regex::Regex::new(r"\btools\.([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for captured in regex.captures_iter(input_text) {
        *counts.entry(captured[1].to_string()).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

fn turn_id_from_payload(
    payload: &Map<String, Value>,
    current: Option<String>,
    lineage_root_id: &str,
    root_thread_id: &str,
) -> Option<String> {
    let passthrough = map_of(payload.get("internal_chat_message_metadata_passthrough"));
    match str_of(passthrough.get("turn_id")) {
        Some(raw) => Some(scoped_id(lineage_root_id, root_thread_id, &raw)),
        None => current,
    }
}

#[derive(Clone)]
struct TurnBuilder {
    turn_id: String,
    thread_id: String,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    status: String,
    first_token_ms: Option<i64>,
    error: Option<String>,
    last_agent_message: Option<String>,
}

#[derive(Clone)]
struct ToolBuilder {
    call_id: String,
    item_id: Option<String>,
    thread_id: String,
    turn_id: Option<String>,
    name: String,
    namespace: Option<String>,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    status: String,
    input_text: Option<String>,
    output_text: Option<String>,
    nested: Vec<(String, i64)>,
    source_line: i64,
    lineage_root_id: String,
}

struct Activity {
    owner_thread_id: String,
    timestamp_ms: i64,
    payload: Map<String, Value>,
    source_line: i64,
    lineage_root_id: String,
}

/// Load one Codex root and every rollout in its lineage.
#[allow(clippy::too_many_arguments)]
pub fn load_codex_team(
    sessions_root: &Path,
    root_thread_id: &str,
    team_slug: &str,
    display_timezone: &str,
    source_paths: Option<&[String]>,
    continuation_links: &[ContinuationLink],
) -> Result<TeamData, String> {
    if display_timezone.parse::<chrono_tz::Tz>().is_err() {
        if let Some(refusal) = crate::window::tz_refusal(display_timezone) {
            return Err(refusal);
        }
        return Err(format!(
            "unknown display timezone {}",
            py_repr_str(display_timezone)
        ));
    }
    let continuation_thread_ids: Vec<String> = continuation_links
        .iter()
        .map(|link| link.thread_id.clone())
        .collect();
    let roots = lineage_roots(root_thread_id, &continuation_thread_ids)?;
    for (index, link) in continuation_links.iter().enumerate() {
        let expected = if index == 0 {
            root_thread_id
        } else {
            continuation_links[index - 1].thread_id.as_str()
        };
        if link.predecessor_thread_id != expected {
            return Err(format!(
                "continuation {index} does not follow the configured predecessor"
            ));
        }
    }
    let mut secure_paths: BTreeMap<PathBuf, String> = BTreeMap::new();
    let candidates: Vec<(PathBuf, Map<String, Value>)> = match source_paths {
        None => discover_codex_lineage(
            sessions_root,
            root_thread_id,
            None,
            &continuation_thread_ids,
        )?,
        Some(paths) => {
            let mut seen = std::collections::BTreeSet::new();
            let mut explicit = Vec::new();
            for source_path in paths {
                if !seen.insert(source_path.clone()) {
                    return Err(format!(
                        "duplicate explicit source path {}",
                        py_repr_str(source_path)
                    ));
                }
                let path = snapshot_target(sessions_root, source_path)?;
                let Some(metadata) = first_snapshot_metadata(sessions_root, source_path)? else {
                    return Err(format!(
                        "explicit source snapshot lacks valid session metadata: {}",
                        py_repr_str(source_path)
                    ));
                };
                metadata_lineage_root(&metadata, &roots, source_path)?;
                secure_paths.insert(path.clone(), source_path.clone());
                explicit.push((path, metadata));
            }
            explicit
        }
    };
    if candidates.is_empty() {
        return Err(format!(
            "Codex root thread {} was not found",
            py_repr_str(root_thread_id)
        ));
    }
    let mut rollouts: Vec<Rollout> = Vec::new();
    for (path, metadata) in &candidates {
        let lineage = metadata_lineage_root(metadata, &roots, &path.display().to_string())?;
        rollouts.push(read_rollout(
            path,
            sessions_root,
            metadata,
            &lineage,
            secure_paths.get(path).map(String::as_str),
        )?);
    }
    if let Some(paths) = source_paths {
        let expected: std::collections::BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        let parsed: std::collections::BTreeSet<&str> = rollouts
            .iter()
            .map(|rollout| rollout.source_path.as_str())
            .collect();
        if parsed != expected {
            let expected_list: Vec<String> =
                expected.iter().map(|item| py_repr_str(item)).collect();
            let parsed_list: Vec<String> = parsed.iter().map(|item| py_repr_str(item)).collect();
            return Err(format!(
                "parsed source set does not match the validated source manifest: expected [{}], parsed [{}]",
                expected_list.join(", "),
                parsed_list.join(", ")
            ));
        }
    }
    let mut seeds: BTreeMap<String, AgentSeed> = BTreeMap::new();
    let mut lineage_by_thread: BTreeMap<String, String> = BTreeMap::new();
    for rollout in &rollouts {
        let seed = normalize_seed(
            &agent_seed(rollout),
            root_thread_id,
            &continuation_thread_ids,
        )?;
        if let Some(prior) = lineage_by_thread.get(&seed.thread_id) {
            if *prior != seed.lineage_root_id {
                return Err(format!(
                    "Codex thread {} occurs in multiple configured lineages",
                    py_repr_str(&seed.thread_id)
                ));
            }
        }
        lineage_by_thread.insert(seed.thread_id.clone(), seed.lineage_root_id.clone());
        let replace = match seeds.get(&seed.thread_id) {
            None => true,
            Some(previous) => {
                (seed.started_at_ms, seed.source_path.clone())
                    < (previous.started_at_ms, previous.source_path.clone())
            }
        };
        if replace {
            seeds.insert(seed.thread_id.clone(), seed);
        }
    }
    if !seeds.contains_key(root_thread_id) {
        return Err(format!(
            "lineage lacks root metadata for {}",
            py_repr_str(root_thread_id)
        ));
    }
    let missing: Vec<&String> = continuation_thread_ids
        .iter()
        .filter(|id| !seeds.contains_key(*id))
        .collect();
    if !missing.is_empty() {
        let names: Vec<String> = missing.iter().map(|id| py_repr_str(id)).collect();
        return Err(format!(
            "lineage lacks continuation root metadata for {}",
            names.join(", ")
        ));
    }
    let mut turns: BTreeMap<(String, String), TurnBuilder> = BTreeMap::new();
    let mut tools: BTreeMap<String, ToolBuilder> = BTreeMap::new();
    let mut events: BTreeMap<(String, String), Event> = BTreeMap::new();
    let mut activities: Vec<Activity> = Vec::new();
    let mut sorted_rollouts: Vec<&Rollout> = rollouts.iter().collect();
    sorted_rollouts.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    for rollout in sorted_rollouts {
        let thread_id = str_of(rollout.metadata.get("id")).unwrap_or_default();
        let lineage_root_id = rollout.lineage_root_id.clone();
        let mut pairs = prompt_pairs(&rollout.canonical_records);
        let mut current_turn: Option<String> = None;
        for record in &rollout.canonical_records {
            let top_type = record.kind();
            let payload = record_payload(record);
            let payload_type = payload.get("type").and_then(Value::as_str);
            let timestamp_ms = record.timestamp_ms();
            if top_type == Some("turn_context") {
                if let Some(raw) = str_of(payload.get("turn_id")) {
                    current_turn = Some(scoped_id(&lineage_root_id, root_thread_id, &raw));
                }
                continue;
            }
            if top_type == Some("event_msg") && payload_type == Some("task_started") {
                let Some(raw) = str_of(payload.get("turn_id")) else {
                    continue;
                };
                let turn_id = scoped_id(&lineage_root_id, root_thread_id, &raw);
                current_turn = Some(turn_id.clone());
                let started = numeric_epoch_ms(
                    payload.get("started_at").unwrap_or(&Value::Null),
                    timestamp_ms,
                );
                let key = (thread_id.clone(), turn_id.clone());
                match turns.get_mut(&key) {
                    None => {
                        turns.insert(
                            key,
                            TurnBuilder {
                                turn_id,
                                thread_id: thread_id.clone(),
                                started_at_ms: started,
                                ended_at_ms: None,
                                status: "running".to_string(),
                                first_token_ms: None,
                                error: None,
                                last_agent_message: None,
                            },
                        );
                    }
                    Some(existing) => {
                        existing.started_at_ms = existing.started_at_ms.min(started);
                    }
                }
                continue;
            }
            if top_type == Some("event_msg")
                && matches!(payload_type, Some("task_complete") | Some("turn_aborted"))
            {
                let Some(raw) = str_of(payload.get("turn_id")) else {
                    continue;
                };
                let turn_id = scoped_id(&lineage_root_id, root_thread_id, &raw);
                let started = numeric_epoch_ms(
                    payload.get("started_at").unwrap_or(&Value::Null),
                    timestamp_ms,
                );
                let key = (thread_id.clone(), turn_id.clone());
                let builder = turns.entry(key).or_insert_with(|| TurnBuilder {
                    turn_id: turn_id.clone(),
                    thread_id: thread_id.clone(),
                    started_at_ms: started,
                    ended_at_ms: None,
                    status: "running".to_string(),
                    first_token_ms: None,
                    error: None,
                    last_agent_message: None,
                });
                builder.ended_at_ms = Some(numeric_epoch_ms(
                    payload.get("completed_at").unwrap_or(&Value::Null),
                    timestamp_ms,
                ));
                builder.status = if payload_type == Some("task_complete") {
                    "completed".to_string()
                } else {
                    "aborted".to_string()
                };
                builder.first_token_ms = int_of(payload.get("time_to_first_token_ms"));
                builder.error = textual(if truthy(payload.get("error")) {
                    payload.get("error")
                } else {
                    payload.get("reason")
                });
                builder.last_agent_message = str_of(payload.get("last_agent_message"));
                if current_turn.as_deref() == Some(turn_id.as_str()) {
                    current_turn = None;
                }
                continue;
            }
            if top_type == Some("event_msg") && payload_type == Some("user_message") {
                let Some(text) = sval(payload.get("message")) else {
                    continue;
                };
                let item_id = claim_prompt_id(&mut pairs, timestamp_ms, &text);
                let raw_event_id = item_id
                    .clone()
                    .unwrap_or_else(|| fallback_id(&thread_id, record, "user_prompt"));
                let event_id = scoped_id(&lineage_root_id, root_thread_id, &raw_event_id);
                events.insert(
                    (thread_id.clone(), event_id.clone()),
                    Event {
                        event_id,
                        thread_id: thread_id.clone(),
                        turn_id: current_turn.clone(),
                        timestamp_ms,
                        kind: "user_prompt".to_string(),
                        role: Some("user".to_string()),
                        phase: None,
                        text: Some(text),
                        content_availability: "plaintext".to_string(),
                        encrypted_content: None,
                        author: Some("user".to_string()),
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: Some("codex".to_string()),
                        author_kind: Some("owner_human".to_string()),
                        source_native_id: item_id,
                        classification_version: Some(CLASSIFICATION_VERSION.to_string()),
                    },
                );
                continue;
            }
            if top_type == Some("event_msg") && payload_type == Some("sub_agent_activity") {
                let raw_event_id = str_of(payload.get("event_id"))
                    .unwrap_or_else(|| fallback_id(&thread_id, record, "subagent_activity"));
                let event_id = scoped_id(&lineage_root_id, root_thread_id, &raw_event_id);
                let kind = sval(payload.get("kind")).unwrap_or_else(|| "interaction".to_string());
                let occurred_ms = int_of(payload.get("occurred_at_ms"))
                    .filter(|value| *value != 0)
                    .unwrap_or(timestamp_ms);
                events.insert(
                    (thread_id.clone(), event_id.clone()),
                    Event {
                        event_id,
                        thread_id: thread_id.clone(),
                        turn_id: current_turn.clone(),
                        timestamp_ms: occurred_ms,
                        kind: format!("subagent_{kind}"),
                        role: None,
                        phase: None,
                        text: str_of(payload.get("agent_path")),
                        content_availability: "none".to_string(),
                        encrypted_content: None,
                        author: None,
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: None,
                        author_kind: None,
                        source_native_id: None,
                        classification_version: None,
                    },
                );
                activities.push(Activity {
                    owner_thread_id: thread_id.clone(),
                    timestamp_ms: occurred_ms,
                    payload: payload.clone(),
                    source_line: record.line,
                    lineage_root_id: lineage_root_id.clone(),
                });
                continue;
            }
            if top_type == Some("event_msg") && payload_type == Some("context_compacted") {
                let event_id = scoped_id(
                    &lineage_root_id,
                    root_thread_id,
                    &fallback_id(&thread_id, record, "context_compacted"),
                );
                events.insert(
                    (thread_id.clone(), event_id.clone()),
                    Event {
                        event_id,
                        thread_id: thread_id.clone(),
                        turn_id: current_turn.clone(),
                        timestamp_ms,
                        kind: "context_compacted".to_string(),
                        role: None,
                        phase: None,
                        text: None,
                        content_availability: "none".to_string(),
                        encrypted_content: None,
                        author: None,
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: None,
                        author_kind: None,
                        source_native_id: None,
                        classification_version: None,
                    },
                );
                continue;
            }
            if top_type == Some("event_msg") && payload_type == Some("thread_goal_updated") {
                let goal = map_of(payload.get("goal"));
                let status = str_of(goal.get("status"));
                let objective = str_of(goal.get("objective"));
                let text = {
                    let parts: Vec<String> = [status, objective].into_iter().flatten().collect();
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join(" | "))
                    }
                };
                let event_id = scoped_id(
                    &lineage_root_id,
                    root_thread_id,
                    &fallback_id(&thread_id, record, "goal_updated"),
                );
                events.insert(
                    (thread_id.clone(), event_id.clone()),
                    Event {
                        event_id,
                        thread_id: thread_id.clone(),
                        turn_id: current_turn.clone(),
                        timestamp_ms,
                        kind: "goal_updated".to_string(),
                        role: None,
                        phase: None,
                        content_availability: if text.is_some() {
                            "plaintext".to_string()
                        } else {
                            "none".to_string()
                        },
                        text,
                        encrypted_content: None,
                        author: None,
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: None,
                        author_kind: None,
                        source_native_id: None,
                        classification_version: None,
                    },
                );
                continue;
            }
            if top_type != Some("response_item") {
                continue;
            }
            if payload_type == Some("message")
                && payload.get("role").and_then(Value::as_str) == Some("assistant")
            {
                let (text, encrypted, availability) = content_parts(&payload);
                let raw_item_id = sval(payload.get("id"))
                    .unwrap_or_else(|| fallback_id(&thread_id, record, "assistant_message"));
                let item_id = scoped_id(&lineage_root_id, root_thread_id, &raw_item_id);
                events.insert(
                    (thread_id.clone(), item_id.clone()),
                    Event {
                        event_id: item_id,
                        thread_id: thread_id.clone(),
                        turn_id: turn_id_from_payload(
                            &payload,
                            current_turn.clone(),
                            &lineage_root_id,
                            root_thread_id,
                        ),
                        timestamp_ms,
                        kind: "assistant_message".to_string(),
                        role: Some("assistant".to_string()),
                        phase: str_of(payload.get("phase")),
                        text,
                        content_availability: availability.to_string(),
                        encrypted_content: encrypted,
                        author: None,
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: None,
                        author_kind: None,
                        source_native_id: None,
                        classification_version: None,
                    },
                );
                continue;
            }
            if payload_type == Some("agent_message") {
                let (text, encrypted, availability) = content_parts(&payload);
                let raw_item_id = sval(payload.get("id"))
                    .unwrap_or_else(|| fallback_id(&thread_id, record, "inter_agent_message"));
                let item_id = scoped_id(&lineage_root_id, root_thread_id, &raw_item_id);
                events.insert(
                    (thread_id.clone(), item_id.clone()),
                    Event {
                        event_id: item_id,
                        thread_id: thread_id.clone(),
                        turn_id: turn_id_from_payload(
                            &payload,
                            current_turn.clone(),
                            &lineage_root_id,
                            root_thread_id,
                        ),
                        timestamp_ms,
                        kind: "inter_agent_message".to_string(),
                        role: None,
                        phase: None,
                        text,
                        content_availability: availability.to_string(),
                        encrypted_content: encrypted,
                        author: str_of(payload.get("author")),
                        recipient: str_of(payload.get("recipient")),
                        source_line: record.line,
                        ingress_kind: None,
                        author_kind: None,
                        source_native_id: None,
                        classification_version: None,
                    },
                );
                continue;
            }
            if matches!(
                payload_type,
                Some("custom_tool_call") | Some("function_call")
            ) {
                let (Some(raw_call_id), Some(name)) =
                    (str_of(payload.get("call_id")), str_of(payload.get("name")))
                else {
                    continue;
                };
                let call_id = scoped_id(&lineage_root_id, root_thread_id, &raw_call_id);
                let input_text = str_of(payload.get(if payload_type == Some("custom_tool_call") {
                    "input"
                } else {
                    "arguments"
                }));
                if let Some(existing) = tools.get(&call_id) {
                    if existing.lineage_root_id != lineage_root_id {
                        return Err(format!(
                            "tool call {} collides across Codex lineages",
                            py_repr_str(&raw_call_id)
                        ));
                    }
                }
                let nested = nested_tools(&name, input_text.as_deref());
                tools.insert(
                    call_id.clone(),
                    ToolBuilder {
                        call_id,
                        item_id: str_of(payload.get("id"))
                            .map(|raw| scoped_id(&lineage_root_id, root_thread_id, &raw)),
                        thread_id: thread_id.clone(),
                        turn_id: turn_id_from_payload(
                            &payload,
                            current_turn.clone(),
                            &lineage_root_id,
                            root_thread_id,
                        ),
                        name,
                        namespace: sval(payload.get("namespace")),
                        started_at_ms: timestamp_ms,
                        ended_at_ms: None,
                        status: sval(payload.get("status"))
                            .unwrap_or_else(|| "running".to_string()),
                        input_text,
                        output_text: None,
                        nested,
                        source_line: record.line,
                        lineage_root_id: lineage_root_id.clone(),
                    },
                );
                continue;
            }
            if matches!(
                payload_type,
                Some("custom_tool_call_output") | Some("function_call_output")
            ) {
                let Some(raw_call_id) = str_of(payload.get("call_id")) else {
                    continue;
                };
                let call_id = scoped_id(&lineage_root_id, root_thread_id, &raw_call_id);
                if let Some(builder) = tools.get_mut(&call_id) {
                    builder.ended_at_ms = Some(timestamp_ms);
                    builder.status = "completed".to_string();
                    builder.output_text = textual(payload.get("output"));
                }
                continue;
            }
        }
    }
    let mut turn_values: Vec<Turn> = turns
        .into_values()
        .map(|item| Turn {
            turn_id: item.turn_id,
            thread_id: item.thread_id,
            started_at_ms: item.started_at_ms,
            ended_at_ms: item.ended_at_ms,
            status: item.status,
            first_token_ms: item.first_token_ms,
            error: item.error,
            last_agent_message: item.last_agent_message,
        })
        .collect();
    turn_values.sort_by(|a, b| {
        (a.started_at_ms, a.thread_id.as_str(), a.turn_id.as_str()).cmp(&(
            b.started_at_ms,
            b.thread_id.as_str(),
            b.turn_id.as_str(),
        ))
    });
    let mut tool_values: Vec<ToolCall> = tools
        .values()
        .map(|item| ToolCall {
            call_id: item.call_id.clone(),
            item_id: item.item_id.clone(),
            thread_id: item.thread_id.clone(),
            turn_id: item.turn_id.clone(),
            name: item.name.clone(),
            namespace: item.namespace.clone(),
            started_at_ms: item.started_at_ms,
            ended_at_ms: item.ended_at_ms,
            status: item.status.clone(),
            input_text: item.input_text.clone(),
            output_text: item.output_text.clone(),
            nested_tools: item.nested.clone(),
            source_line: item.source_line,
            input_payload: None,
            output_payload: None,
        })
        .collect();
    tool_values.sort_by(|a, b| {
        (a.started_at_ms, a.thread_id.as_str(), a.call_id.as_str()).cmp(&(
            b.started_at_ms,
            b.thread_id.as_str(),
            b.call_id.as_str(),
        ))
    });
    // Edges from continuation links and sub-agent activity.
    let mut path_to_thread_by_lineage: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut known_threads_by_lineage: BTreeMap<String, std::collections::BTreeSet<String>> =
        BTreeMap::new();
    for seed in seeds.values() {
        let path_map = path_to_thread_by_lineage
            .entry(seed.lineage_root_id.clone())
            .or_default();
        if let Some(previous) = path_map.get(&seed.raw_agent_path) {
            if *previous != seed.thread_id {
                return Err(format!(
                    "duplicate agent path {} in Codex lineage {}",
                    py_repr_str(&seed.raw_agent_path),
                    py_repr_str(&seed.lineage_root_id)
                ));
            }
        }
        path_map.insert(seed.raw_agent_path.clone(), seed.thread_id.clone());
        known_threads_by_lineage
            .entry(seed.lineage_root_id.clone())
            .or_default()
            .insert(seed.thread_id.clone());
    }
    let mut edges_by_id: BTreeMap<String, Edge> = BTreeMap::new();
    for link in continuation_links {
        let edge_id = format!("codex-continuation-{}", link.thread_id);
        edges_by_id.insert(
            edge_id.clone(),
            Edge {
                edge_id: edge_id.clone(),
                call_id: edge_id,
                from_thread_id: link.predecessor_thread_id.clone(),
                to_thread_id: link.thread_id.clone(),
                kind: "continuation".to_string(),
                timestamp_ms: link.predecessor_at_ms,
                message_text: Some(format!(
                    "Explicit Codex session continuation. Predecessor source {} line {} \
                     was recorded at {} ms UTC; successor source {} started at {} ms UTC \
                     ({} ms later).",
                    link.predecessor_source_path,
                    link.predecessor_source_line,
                    link.predecessor_at_ms,
                    link.source_path,
                    link.started_at_ms,
                    link.gap_ms
                )),
                content_availability: "plaintext".to_string(),
                encrypted_content: None,
                source_line: link.predecessor_source_line,
            },
        );
    }
    let mut interrupted_at: BTreeMap<String, i64> = BTreeMap::new();
    let mut sorted_activities: Vec<&Activity> = activities.iter().collect();
    sorted_activities.sort_by(|a, b| {
        (a.timestamp_ms, a.owner_thread_id.as_str(), a.source_line).cmp(&(
            b.timestamp_ms,
            b.owner_thread_id.as_str(),
            b.source_line,
        ))
    });
    for activity in sorted_activities {
        let raw_call_id = str_of(activity.payload.get("event_id"));
        let activity_kind = str_of(activity.payload.get("kind"));
        let (Some(raw_call_id), Some(activity_kind)) = (raw_call_id, activity_kind) else {
            continue;
        };
        let call_id = scoped_id(&activity.lineage_root_id, root_thread_id, &raw_call_id);
        let known = known_threads_by_lineage
            .get(&activity.lineage_root_id)
            .cloned()
            .unwrap_or_default();
        let mut agent_thread_id = str_of(activity.payload.get("agent_thread_id"));
        if agent_thread_id
            .as_ref()
            .is_some_and(|id| !known.contains(id))
        {
            agent_thread_id = None;
        }
        let Some(tool) = tools.get(&call_id).cloned() else {
            if activity_kind == "interrupted" {
                if let Some(agent) = &agent_thread_id {
                    interrupted_at.insert(agent.clone(), activity.timestamp_ms);
                }
            }
            continue;
        };
        let arguments = call_arguments(&tool);
        let target_name = str_of(arguments.get("target"));
        let mut target_thread = resolve_target(
            target_name.as_deref(),
            path_to_thread_by_lineage
                .get(&activity.lineage_root_id)
                .cloned()
                .unwrap_or_default(),
            &known,
        );
        if activity_kind == "started" {
            target_thread = agent_thread_id.clone();
        }
        if target_thread.is_none() && agent_thread_id.as_deref() != Some(tool.thread_id.as_str()) {
            target_thread = agent_thread_id.clone();
        }
        if target_thread.is_none() && activity.owner_thread_id != tool.thread_id {
            target_thread = Some(activity.owner_thread_id.clone());
        }
        let Some(target_thread) = target_thread else {
            continue;
        };
        if target_thread == tool.thread_id {
            continue;
        }
        let edge_kind = match tool.name.as_str() {
            "spawn_agent" => "spawn".to_string(),
            "send_message" => "message".to_string(),
            "followup_task" => "followup".to_string(),
            "interrupt_agent" => "interrupt".to_string(),
            _ => activity_kind.clone(),
        };
        let (message, encrypted, availability) = message_fields(&arguments);
        if let Some(existing) = edges_by_id.get(&call_id) {
            if existing.kind == "continuation" {
                return Err(format!(
                    "collaboration call {} collides with a continuation edge",
                    py_repr_str(&raw_call_id)
                ));
            }
        }
        edges_by_id.insert(
            call_id.clone(),
            Edge {
                edge_id: call_id,
                call_id: {
                    // edge id and call id are the scoped call id
                    scoped_id(&activity.lineage_root_id, root_thread_id, &raw_call_id)
                },
                from_thread_id: tool.thread_id.clone(),
                to_thread_id: target_thread.clone(),
                kind: edge_kind.clone(),
                timestamp_ms: activity.timestamp_ms,
                message_text: message,
                content_availability: availability.to_string(),
                encrypted_content: encrypted,
                source_line: activity.source_line,
            },
        );
        if edge_kind == "interrupt" {
            interrupted_at.insert(target_thread, activity.timestamp_ms);
        }
    }
    let mut turns_by_thread: BTreeMap<String, Vec<Turn>> = BTreeMap::new();
    for turn in &turn_values {
        turns_by_thread
            .entry(turn.thread_id.clone())
            .or_default()
            .push(turn.clone());
    }
    let mut agents: Vec<Agent> = Vec::new();
    for seed in seeds.values() {
        let mut own_turns: Vec<Turn> = turns_by_thread
            .get(&seed.thread_id)
            .cloned()
            .unwrap_or_default();
        own_turns.sort_by_key(|turn| turn.started_at_ms);
        let interrupted = interrupted_at.get(&seed.thread_id).copied();
        let mut transitions: Vec<(i64, i64, String, Option<i64>)> =
            vec![(seed.started_at_ms, -1, "created".to_string(), None)];
        for (turn_index, turn) in own_turns.iter().enumerate() {
            let start_order = turn_index as i64 * 2;
            transitions.push((turn.started_at_ms, start_order, "running".to_string(), None));
            if let Some(ended) = turn.ended_at_ms {
                transitions.push((ended, start_order + 1, turn.status.clone(), Some(ended)));
            }
        }
        if let Some(interrupted) = interrupted {
            transitions.push((
                interrupted,
                own_turns.len() as i64 * 2 + 1,
                "interrupted".to_string(),
                Some(interrupted),
            ));
        }
        let (_, _, status, ended_at_ms) = transitions
            .into_iter()
            .max_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
            .unwrap();
        agents.push(Agent {
            thread_id: seed.thread_id.clone(),
            parent_thread_id: seed.parent_thread_id.clone(),
            agent_path: seed.agent_path.clone(),
            nickname: seed.nickname.clone(),
            role: seed.role.clone(),
            depth: seed.depth,
            started_at_ms: seed.started_at_ms,
            ended_at_ms,
            status,
            source_path: seed.source_path.clone(),
        });
    }
    agents.sort_by(|a, b| {
        (a.started_at_ms, a.depth, a.agent_path.as_str()).cmp(&(
            b.started_at_ms,
            b.depth,
            b.agent_path.as_str(),
        ))
    });
    let mut event_values: Vec<Event> = events.into_values().collect();
    event_values.sort_by(|a, b| {
        (
            a.timestamp_ms,
            a.thread_id.as_str(),
            a.source_line,
            a.event_id.as_str(),
        )
            .cmp(&(
                b.timestamp_ms,
                b.thread_id.as_str(),
                b.source_line,
                b.event_id.as_str(),
            ))
    });
    let mut edge_values: Vec<Edge> = edges_by_id.into_values().collect();
    edge_values.sort_by(|a, b| {
        (a.timestamp_ms, a.kind.as_str(), a.edge_id.as_str()).cmp(&(
            b.timestamp_ms,
            b.kind.as_str(),
            b.edge_id.as_str(),
        ))
    });
    let mut sources: Vec<SourceSnapshot> = rollouts
        .iter()
        .map(|rollout| rollout.snapshot.clone())
        .collect();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(TeamData {
        team_slug: team_slug.to_string(),
        provider: "codex".to_string(),
        root_thread_id: root_thread_id.to_string(),
        display_timezone: display_timezone.to_string(),
        sources,
        agents,
        turns: turn_values,
        events: event_values,
        tool_calls: tool_values,
        edges: edge_values,
        task_notes: Vec::new(),
        window_start_ms: None,
        window_end_ms: None,
    })
}

fn call_arguments(tool: &ToolBuilder) -> Map<String, Value> {
    let Some(text) = &tool.input_text else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(text) {
        Ok(value) => map_of(Some(&value)),
        Err(_) => Map::new(),
    }
}

fn message_fields(
    arguments: &Map<String, Value>,
) -> (Option<String>, Option<String>, &'static str) {
    let Some(message) = str_of(arguments.get("message")) else {
        return (None, None, "none");
    };
    if message.starts_with("gAAAA") {
        (None, Some(message), "encrypted")
    } else {
        (Some(message), None, "plaintext")
    }
}

fn resolve_target(
    target: Option<&str>,
    path_to_thread: BTreeMap<String, String>,
    known_threads: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let target = target?;
    if known_threads.contains(target) {
        return Some(target.to_string());
    }
    if let Some(thread) = path_to_thread.get(target) {
        return Some(thread.clone());
    }
    let mut candidates = vec![target.to_string()];
    if !target.starts_with('/') {
        candidates.push(format!("/root/{target}"));
    }
    for candidate in &candidates {
        if let Some(thread) = path_to_thread.get(candidate) {
            return Some(thread.clone());
        }
    }
    let suffix = format!("/{}", target.trim_matches('/'));
    let matches: Vec<&String> = path_to_thread
        .iter()
        .filter(|(path, _)| path.ends_with(&suffix))
        .map(|(_, thread)| thread)
        .collect();
    if matches.len() == 1 {
        Some(matches[0].clone())
    } else {
        None
    }
}

/// Validate one Codex source-copy record from a manifest
/// (the lenient single-message reader of `codex.py`).
pub fn codex_source_from_json(raw: &Value, where_: &str) -> Result<SourceCopy, String> {
    let item = as_object(raw, where_)?;
    let source_path = str_of(item.get("source_path"));
    let original_path = str_of(item.get("original_path"));
    let snapshot_path = str_of(item.get("snapshot_path"));
    let thread_id = str_of(item.get("thread_id"));
    let copied_bytes = int_of(item.get("copied_bytes"));
    let line_count = int_of(item.get("line_count"));
    let sha256 = str_of(item.get("sha256"));
    let updated_at = str_of(item.get("updated_at"));
    let raw_retired = item.get("upstream_retired_at");
    let upstream_retired_at = str_of(raw_retired);
    if raw_retired.is_some_and(|value| !value.is_null()) && upstream_retired_at.is_none() {
        return Err(format!("{where_}: malformed upstream_retired_at"));
    }
    let digest_ok = sha256.as_ref().is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    let (Some(source_path), Some(original_path), Some(snapshot_path), Some(thread_id)) =
        (source_path, original_path, snapshot_path, thread_id)
    else {
        return Err(format!("{where_}: malformed source-copy record"));
    };
    let (Some(copied_bytes), Some(line_count), Some(updated_at)) =
        (copied_bytes, line_count, updated_at)
    else {
        return Err(format!("{where_}: malformed source-copy record"));
    };
    if copied_bytes < 0 || line_count < 0 || !digest_ok {
        return Err(format!("{where_}: malformed source-copy record"));
    }
    safe_snapshot_relative(&source_path)?;
    if snapshot_path != source_path {
        return Err(format!(
            "{where_}: snapshot_path must match the safe relative source_path"
        ));
    }
    Ok(SourceCopy {
        source_path,
        original_path,
        snapshot_path,
        thread_id,
        copied_bytes,
        line_count,
        sha256: sha256.unwrap(),
        updated_at,
        upstream_retired_at,
    })
}
