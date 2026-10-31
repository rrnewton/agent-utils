//! Import a Muse Code session and the subagents it
//! spawned: `py/wrkviz/muse.py` at parity. Journal record
//! facts live in `agent_harness::muse_records`; lineage,
//! snapshotting, and team assembly live here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_harness::muse_records::{
    complete_journal, parse_journal_records, recorded_at_ms, JournalError,
};
use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::compressed::{read_source_bytes, resolve_source, source_exists};
use crate::json::{dump_compact, py_repr_str};
use crate::team_data::{Agent, Edge, Event, SourceSnapshot, TeamData, ToolCall, Turn};
use crate::transcript_io::write_text_if_changed;

const SESSION_LOG: &str = "session.jsonl";
const CLASSIFICATION_VERSION: &str = "authorship-v1";
const SPAWN_TOOL: &str = "subagent_spawn";

/// `str(Path)` in the oracle drops trailing slashes.
fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn safe_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn validate_id(value: &str, where_: &str) -> Result<(), String> {
    if safe_id(value) {
        Ok(())
    } else {
        Err(format!(
            "{where_}: unsafe Muse session id {}",
            py_repr_str(value)
        ))
    }
}

fn safe_relative_parts(value: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    if value.starts_with('/') || parts.is_empty() || parts.iter().any(|part| part == "..") {
        return Err(format!("unsafe Muse source path {}", py_repr_str(value)));
    }
    Ok(parts)
}

fn opt_str(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn optional_mapping(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// One archive-local copy of a Muse session log.
#[derive(Clone, Debug)]
pub struct MuseSourceCopy {
    /// Archive-relative source path.
    pub source_path: String,
    /// Original upstream path.
    pub original_path: String,
    /// Snapshot path (equals source_path).
    pub snapshot_path: String,
    /// Thread id.
    pub thread_id: String,
    /// Copied bytes.
    pub copied_bytes: i64,
    /// Line count.
    pub line_count: i64,
    /// Digest.
    pub sha256: String,
    /// Updated-at stamp.
    pub updated_at: String,
    /// When upstream retired the file.
    pub upstream_retired_at: Option<String>,
}

impl MuseSourceCopy {
    /// The JSON record.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert(
            "original_path".into(),
            Value::String(self.original_path.clone()),
        );
        out.insert(
            "snapshot_path".into(),
            Value::String(self.snapshot_path.clone()),
        );
        out.insert("thread_id".into(), Value::String(self.thread_id.clone()));
        out.insert("copied_bytes".into(), self.copied_bytes.into());
        out.insert("line_count".into(), self.line_count.into());
        out.insert("sha256".into(), Value::String(self.sha256.clone()));
        out.insert("updated_at".into(), Value::String(self.updated_at.clone()));
        if let Some(retired) = &self.upstream_retired_at {
            out.insert("upstream_retired_at".into(), Value::String(retired.clone()));
        }
        Value::Object(out)
    }

    /// Validate and decode one source-copy record (the
    /// Muse manifest's own wording).
    pub fn from_json(raw: &Value, where_: &str) -> Result<MuseSourceCopy, String> {
        let item = raw.as_object().ok_or_else(|| {
            // as_object in the pipeline reports first; this is
            // only reached for non-objects from that caller.
            format!("{where_}: expected an object")
        })?;
        let text = |key: &str| -> Result<String, String> {
            match item.get(key) {
                Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
                _ => Err(format!("{where_}.{key} must be a non-empty string")),
            }
        };
        let optional_text = |key: &str| -> Result<Option<String>, String> {
            match item.get(key) {
                None | Some(Value::Null) => Ok(None),
                _ => text(key).map(Some),
            }
        };
        let count = |key: &str| -> Result<i64, String> {
            match item.get(key) {
                Some(value) => {
                    let held = crate::json::as_int(value, &format!("{where_}.{key}"))
                        .map_err(|_| format!("{where_}.{key} must be a non-negative integer"))?;
                    if held < 0 {
                        return Err(format!("{where_}.{key} must be a non-negative integer"));
                    }
                    Ok(held)
                }
                None => Err(format!("{where_}.{key} must be a non-negative integer")),
            }
        };
        let source_path = text("source_path")?;
        let snapshot_path = text("snapshot_path")?;
        let sha256 = text("sha256")?;
        let thread_id = text("thread_id")?;
        safe_relative_parts(&source_path)
            .map_err(|_| format!("unsafe Muse source path {}", py_repr_str(&source_path)))?;
        if snapshot_path != source_path {
            return Err(format!("{where_}: snapshot_path must equal source_path"));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("{where_}: invalid source digest"));
        }
        validate_id(&thread_id, where_)?;
        Ok(MuseSourceCopy {
            source_path,
            original_path: text("original_path")?,
            snapshot_path,
            thread_id,
            copied_bytes: count("copied_bytes")?,
            line_count: count("line_count")?,
            sha256,
            updated_at: text("updated_at")?,
            upstream_retired_at: optional_text("upstream_retired_at")?,
        })
    }
}

/// Result of copying a Muse lineage into the store.
pub struct MuseSnapshotResult {
    /// Source copies, in path order.
    pub sources: Vec<MuseSourceCopy>,
    /// Files changed.
    pub files_changed: i64,
    /// Sources kept from the archive.
    pub upstream_retired: Vec<String>,
}

type JournalRecords = Vec<(i64, Map<String, Value>)>;

fn journal_records(data: &[u8], where_: &str) -> Result<JournalRecords, String> {
    parse_journal_records(data)
        .map(|records| {
            records
                .into_iter()
                .map(|record| {
                    (
                        record.line,
                        record.value.as_object().cloned().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .map_err(|error| match error {
            JournalError::InvalidJson(line) => {
                let lines = agent_harness::muse_records::journal_lines(data);
                let raw = lines.get((line - 1) as usize).copied().unwrap_or(b"");
                let detail = serde_json::from_slice::<Value>(raw)
                    .err()
                    .map(|err| err.to_string())
                    .unwrap_or_default();
                format!("{where_}:{line}: invalid JSON: {detail}")
            }
        })
}

fn bound_children(data: &[u8], where_: &str) -> Result<Vec<(String, i64)>, String> {
    let mut children = Vec::new();
    for (line, record) in journal_records(data, where_)? {
        if opt_str(record.get("payload_type")).as_deref()
            != Some("subagent.control.child_session_bound")
        {
            continue;
        }
        let payload = optional_mapping(record.get("payload"));
        let inner = optional_mapping(payload.get("record"));
        if let Some(child) = opt_str(inner.get("child_session_id")) {
            validate_id(&child, &format!("{where_}:{line}"))?;
            children.push((child, line));
        }
    }
    Ok(children)
}

/// The session's own log and the logs of every subagent it
/// (transitively) spawned.
pub fn discover_muse_sources(session_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let root_log = session_dir.join(SESSION_LOG);
    if !source_exists(&root_log) {
        return Err(format!(
            "Muse session log is not a regular file: {}",
            root_log.display()
        ));
    }
    let root_name = session_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    validate_id(&root_name, &display_path(session_dir))?;
    let mut found = vec![root_log.clone()];
    let mut pending: std::collections::VecDeque<PathBuf> = [root_log].into();
    let mut seen: Vec<String> = vec![root_name];
    while let Some(log) = pending.pop_front() {
        let data = read_source_bytes(&log)?;
        for (child, _line) in bound_children(&data, &log.display().to_string())? {
            if seen.contains(&child) {
                continue;
            }
            seen.push(child.clone());
            let candidates = [
                session_dir.join("subagent").join(&child).join(SESSION_LOG),
                log.parent()
                    .map(|parent| parent.join("subagent").join(&child).join(SESSION_LOG))
                    .unwrap_or_default(),
            ];
            for candidate in candidates {
                if source_exists(&candidate) {
                    found.push(candidate.clone());
                    pending.push_back(candidate);
                    break;
                }
            }
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------- snapshot

/// Copy the latest append-only Muse logs before parsing.
pub fn snapshot_muse_lineage(
    session_dir: &Path,
    snapshot_root: &Path,
    previous_sources: &[MuseSourceCopy],
    updated_at: &str,
) -> Result<MuseSnapshotResult, String> {
    let mut previous_by_path: BTreeMap<String, MuseSourceCopy> = BTreeMap::new();
    for source in previous_sources {
        previous_by_path.insert(source.source_path.clone(), source.clone());
    }
    let root_log = session_dir.join(SESSION_LOG);
    let paths: Vec<PathBuf> = if resolve_source(&root_log).is_none() && !previous_by_path.is_empty()
    {
        Vec::new()
    } else {
        discover_muse_sources(session_dir)?
    };
    let mut relative_to_path: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in paths {
        let relative = path
            .strip_prefix(session_dir)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .map_err(|_| format!("Muse source is outside the session dir: {}", path.display()))?;
        relative_to_path.insert(relative, path);
    }
    let mut copies: Vec<MuseSourceCopy> = Vec::new();
    let mut retired: Vec<String> = Vec::new();
    let mut changed = 0i64;
    let previous_paths: Vec<String> = previous_by_path.keys().cloned().collect();
    for relative in previous_paths {
        if relative_to_path.contains_key(&relative) {
            continue;
        }
        let recorded = previous_by_path[&relative].clone();
        let parts = safe_relative_parts(&relative)?;
        let mut upstream = session_dir.to_path_buf();
        let mut target = snapshot_root.to_path_buf();
        for part in &parts {
            upstream.push(part);
            target.push(part);
        }
        if resolve_source(&upstream).is_some() {
            return Err(format!(
                "append-only source violation: previously observed Muse log is still present but no longer reachable from the session: {relative}"
            ));
        }
        let is_symlink = std::fs::symlink_metadata(&target)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !target.is_file() {
            return Err(format!(
                "previously observed Muse log is gone upstream and the archive's own snapshot is missing too: {relative} (expected {})",
                target.display()
            ));
        }
        let held =
            std::fs::read(&target).map_err(|error| crate::json::os_error(&error, &target))?;
        let copied = recorded.copied_bytes as usize;
        if held.len() < copied || sha256_hex(&held[..copied]) != recorded.sha256 {
            return Err(format!(
                "previously observed Muse log is gone upstream and the archive's own snapshot does not match its manifest: {}",
                target.display()
            ));
        }
        retired.push(relative.clone());
        copies.push(MuseSourceCopy {
            upstream_retired_at: Some(
                recorded
                    .upstream_retired_at
                    .clone()
                    .unwrap_or_else(|| updated_at.to_string()),
            ),
            ..recorded
        });
    }
    let root_name = session_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ordered: Vec<(String, PathBuf)> = relative_to_path.into_iter().collect();
    for (relative, source) in &ordered {
        safe_relative_parts(relative)?;
        let complete = complete_journal(&read_source_bytes(source)?).to_vec();
        if complete.is_empty() {
            return Err(format!(
                "Muse log has no newline-complete record: {}",
                source.display()
            ));
        }
        let parts = safe_relative_parts(relative)?;
        let mut target = snapshot_root.to_path_buf();
        for part in &parts {
            target.push(part);
        }
        let is_symlink = std::fs::symlink_metadata(&target)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            return Err(format!(
                "Muse snapshot target is a symlink: {}",
                target.display()
            ));
        }
        let existing: Vec<u8> = if target.is_file() {
            std::fs::read(&target).map_err(|error| crate::json::os_error(&error, &target))?
        } else {
            Vec::new()
        };
        let previous = previous_by_path.get(relative);
        if let Some(previous) = previous {
            let copied = previous.copied_bytes as usize;
            if existing.len() < copied || sha256_hex(&existing[..copied]) != previous.sha256 {
                return Err(format!(
                    "Muse snapshot differs from its manifest: {}",
                    target.display()
                ));
            }
        }
        if complete.len() < existing.len() || &complete[..existing.len()] != existing.as_slice() {
            return Err(format!(
                "append-only source violation: Muse log was truncated or rewritten: {relative}"
            ));
        }
        let text = std::str::from_utf8(&complete).map_err(|error| {
            let position = error.valid_up_to();
            let byte = complete.get(position).copied().unwrap_or(0);
            let reason = match error.error_len() {
                None => "unexpected end of data",
                Some(_) if matches!(byte, 0x80..=0xbf) || byte > 0xf4 => "invalid start byte",
                Some(_) => "invalid continuation byte",
            };
            format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}")
        })?;
        if write_text_if_changed(&target, text)? {
            changed += 1;
        }
        let digest = sha256_hex(&complete);
        let copy_updated = match previous {
            Some(previous)
                if previous.copied_bytes == complete.len() as i64 && previous.sha256 == digest =>
            {
                previous.updated_at.clone()
            }
            _ => updated_at.to_string(),
        };
        copies.push(MuseSourceCopy {
            source_path: relative.clone(),
            original_path: crate::json::resolve_path(source).display().to_string(),
            snapshot_path: relative.clone(),
            thread_id: if relative == SESSION_LOG {
                root_name.clone()
            } else {
                source
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            copied_bytes: complete.len() as i64,
            line_count: complete.iter().filter(|byte| **byte == b'\n').count() as i64,
            sha256: digest,
            updated_at: copy_updated,
            upstream_retired_at: None,
        });
    }
    copies.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    Ok(MuseSnapshotResult {
        sources: copies,
        files_changed: changed,
        upstream_retired: retired,
    })
}

// ---------------------------------------------------------------- team assembly

#[derive(Default)]
struct ThreadState {
    thread_id: String,
    source_path: String,
    events: Vec<Event>,
    turns: Vec<(String, Vec<i64>)>,
    turn_status: BTreeMap<String, String>,
    first_token: BTreeMap<String, i64>,
    last_message: BTreeMap<String, String>,
    activity: Vec<i64>,
}

#[derive(Default)]
struct ToolEntry {
    thread: String,
    line: i64,
    name: Option<String>,
    name_seen: bool,
    started: Option<i64>,
    ended: Option<i64>,
    status: Option<String>,
    item_id: Option<String>,
    input: Option<String>,
    committed: Option<i64>,
    turn: Option<String>,
    spawn_label: Option<String>,
    spawn_text: Option<String>,
    output: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn make_event(
    thread_id: &str,
    turn_id: Option<String>,
    at_ms: i64,
    kind: &str,
    role: &str,
    text: String,
    line: i64,
    native_id: &str,
    author_kind: &str,
    author: Option<String>,
    ingress: &str,
) -> Event {
    Event {
        event_id: format!("muse-{thread_id}-{line}"),
        thread_id: thread_id.to_string(),
        turn_id,
        timestamp_ms: at_ms,
        kind: kind.to_string(),
        role: Some(role.to_string()),
        phase: None,
        text: Some(text),
        content_availability: "plain".to_string(),
        encrypted_content: None,
        author,
        recipient: if role == "user" {
            Some(thread_id.to_string())
        } else {
            None
        },
        source_line: line,
        ingress_kind: Some(ingress.to_string()),
        author_kind: Some(author_kind.to_string()),
        source_native_id: Some(native_id.to_string()),
        classification_version: Some(CLASSIFICATION_VERSION.to_string()),
    }
}

fn user_text(payload: &Map<String, Value>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let messages: &[Value] = match payload.get("model_messages") {
        Some(Value::Array(items)) => items,
        _ => &[],
    };
    for message in messages {
        let content = optional_mapping(Some(message)).get("content").cloned();
        let blocks: Vec<Value> = match &content {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        for block in &blocks {
            if let Some(text) = opt_str(optional_mapping(Some(block)).get("text")) {
                parts.push(text);
            }
        }
    }
    parts.join("\n\n")
}

/// Normalize the archive-local copy of one Muse session and
/// its spawned subagents.
pub fn load_muse_team(
    snapshot_dir: &Path,
    root_session_id: &str,
    team_slug: &str,
    display_timezone: &str,
    source_paths: &[String],
) -> Result<TeamData, String> {
    let root_id = root_session_id;
    validate_id(root_id, "root session id")?;
    if !source_paths.iter().any(|path| path == SESSION_LOG) {
        return Err("Muse source set omits the root session log".to_string());
    }
    let mut ordered_paths: Vec<&String> = source_paths.iter().collect();
    ordered_paths.sort_by(|a, b| {
        (a.as_str() != SESSION_LOG, a.as_str()).cmp(&(b.as_str() != SESSION_LOG, b.as_str()))
    });
    let mut threads: Vec<ThreadState> = Vec::new();
    let mut snapshots: Vec<SourceSnapshot> = Vec::new();
    let mut tool_calls: BTreeMap<String, ToolEntry> = BTreeMap::new();
    let mut calls_by_batch: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
    let mut spawn_call_of: BTreeMap<String, String> = BTreeMap::new();
    let mut description_of: BTreeMap<String, String> = BTreeMap::new();
    let mut pending_bound: Vec<(String, (String, i64, i64))> = Vec::new();
    for relative in &ordered_paths {
        let relative: &str = relative.as_str();
        let parts = safe_relative_parts(relative)?;
        let mut path = snapshot_dir.to_path_buf();
        for part in &parts {
            path.push(part);
        }
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !path.is_file() {
            return Err(format!("missing Muse source {}", path.display()));
        }
        let data = std::fs::read(&path).map_err(|error| crate::json::os_error(&error, &path))?;
        let complete = complete_journal(&data).to_vec();
        let thread_id = if relative == SESSION_LOG {
            root_id.to_string()
        } else {
            path.parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        validate_id(&thread_id, relative)?;
        let thread_index = match threads.iter().position(|held| held.thread_id == thread_id) {
            Some(index) => {
                threads[index] = ThreadState {
                    thread_id: thread_id.clone(),
                    source_path: relative.to_string(),
                    ..ThreadState::default()
                };
                index
            }
            None => {
                threads.push(ThreadState {
                    thread_id: thread_id.clone(),
                    source_path: relative.to_string(),
                    ..ThreadState::default()
                });
                threads.len() - 1
            }
        };
        let metadata =
            std::fs::metadata(&path).map_err(|error| crate::json::os_error(&error, &path))?;
        use std::os::unix::fs::MetadataExt as _;
        snapshots.push(SourceSnapshot {
            path: relative.to_string(),
            thread_id: thread_id.clone(),
            size_bytes: metadata.size() as i64,
            mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
            sha256: sha256_hex(&complete),
            complete_bytes: complete.len() as i64,
            line_count: complete.iter().filter(|byte| **byte == b'\n').count() as i64,
            ..SourceSnapshot::default()
        });
        let mut prompted_runs: Vec<String> = Vec::new();
        for (line_number, record) in journal_records(&complete, relative)? {
            let Some(at_ms) = recorded_at_ms(&Value::Object(record.clone())) else {
                continue;
            };
            let payload_type = opt_str(record.get("payload_type")).unwrap_or_default();
            let payload = optional_mapping(record.get("payload"));
            let record_id =
                opt_str(record.get("id")).unwrap_or_else(|| format!("{relative}:{line_number}"));
            if payload_type == "runtime.user_intent.accepted" {
                let text = user_text(&payload);
                if !text.is_empty() {
                    let run_id = opt_str(payload.get("intent_id"));
                    if let Some(run_id) = &run_id {
                        if !prompted_runs.contains(run_id) {
                            prompted_runs.push(run_id.clone());
                        }
                    }
                    let _ = &run_id;
                    threads[thread_index].events.push(make_event(
                        &thread_id,
                        None,
                        at_ms,
                        "user_prompt",
                        "user",
                        text,
                        line_number,
                        &record_id,
                        "unknown",
                        None,
                        "muse",
                    ));
                    threads[thread_index].activity.push(at_ms);
                }
                continue;
            }
            if payload_type == "tool_batch.effect.started"
                || payload_type == "tool_batch.effect.terminal"
            {
                let effect = optional_mapping(payload.get("record"));
                let Some(call_id) = opt_str(effect.get("call_id")) else {
                    continue;
                };
                let entry = tool_calls.entry(call_id).or_insert_with(|| ToolEntry {
                    thread: thread_id.clone(),
                    line: line_number,
                    ..ToolEntry::default()
                });
                if !entry.name_seen {
                    entry.name = opt_str(effect.get("tool_name"));
                    entry.name_seen = true;
                }
                if payload_type.ends_with("started") {
                    if entry.started.is_none() {
                        entry.started = Some(at_ms);
                    }
                } else {
                    entry.ended = Some(at_ms);
                    let outcome = opt_str(optional_mapping(effect.get("outcome")).get("kind"));
                    entry.status = Some(outcome.unwrap_or_else(|| "completed".to_string()));
                }
                threads[thread_index].activity.push(at_ms);
                continue;
            }
            if payload_type == "subagent.control.child_session_bound" {
                let bound = optional_mapping(payload.get("record"));
                if let Some(child) = opt_str(bound.get("child_session_id")) {
                    match pending_bound.iter_mut().find(|(held, _)| *held == child) {
                        Some((_, held)) => *held = (thread_id.clone(), at_ms, line_number),
                        None => {
                            pending_bound.push((child, (thread_id.clone(), at_ms, line_number)))
                        }
                    }
                }
                continue;
            }
            if payload_type == "subagent.control.start_attested" {
                let attested = optional_mapping(payload.get("record"));
                let child = opt_str(attested.get("subagent_session_id"));
                let call = opt_str(attested.get("dispatched_via_tool_call_id"));
                if let (Some(child), Some(call)) = (child, call) {
                    spawn_call_of.insert(child, call);
                }
                continue;
            }
            if payload.get("kind").and_then(Value::as_str) != Some("run") {
                continue;
            }
            let run_id = opt_str(payload.get("run_id"));
            let event = optional_mapping(payload.get("event"));
            let kind = opt_str(event.get("kind"));
            if let Some(run_id) = &run_id {
                match threads[thread_index]
                    .turns
                    .iter_mut()
                    .find(|(held, _)| held == run_id)
                {
                    Some((_, times)) => times.push(at_ms),
                    None => threads[thread_index]
                        .turns
                        .push((run_id.clone(), vec![at_ms])),
                }
            }
            match kind.as_deref() {
                Some("started") => {
                    let prompt = opt_str(event.get("prompt"));
                    if let Some(prompt) = prompt {
                        let already = run_id
                            .as_ref()
                            .is_some_and(|held| prompted_runs.contains(held));
                        if !already {
                            if let Some(run_id) = &run_id {
                                if !prompted_runs.contains(run_id) {
                                    prompted_runs.push(run_id.clone());
                                }
                            }
                            threads[thread_index].events.push(make_event(
                                &thread_id,
                                run_id.clone(),
                                at_ms,
                                "user_prompt",
                                "user",
                                prompt,
                                line_number,
                                &record_id,
                                "unknown",
                                None,
                                "muse",
                            ));
                            threads[thread_index].activity.push(at_ms);
                        }
                    }
                }
                Some("inbox_item_drained") => {
                    let body =
                        opt_str(optional_mapping(event.get("delivery_snapshot")).get("body"));
                    if let Some(body) = body {
                        threads[thread_index].events.push(make_event(
                            &thread_id,
                            run_id.clone(),
                            at_ms,
                            "system_input",
                            "system",
                            body,
                            line_number,
                            &record_id,
                            "system",
                            None,
                            "muse_inbox",
                        ));
                        threads[thread_index].activity.push(at_ms);
                    }
                }
                Some("assistant_message_committed") => {
                    if let Some(reply) = opt_str(event.get("text")) {
                        threads[thread_index].events.push(make_event(
                            &thread_id,
                            run_id.clone(),
                            at_ms,
                            "assistant_message",
                            "assistant",
                            reply.clone(),
                            line_number,
                            &record_id,
                            "agent",
                            Some(thread_id.clone()),
                            "muse",
                        ));
                        threads[thread_index].activity.push(at_ms);
                        if let Some(run_id) = &run_id {
                            threads[thread_index]
                                .last_message
                                .insert(run_id.clone(), reply);
                        }
                    }
                }
                Some("assistant_tool_calls_committed") => {
                    let batch =
                        opt_str(event.get("message_id")).unwrap_or_else(|| record_id.clone());
                    let committed_calls: &[Value] = match event.get("tool_calls") {
                        Some(Value::Array(items)) => items.as_slice(),
                        _ => &[],
                    };
                    // Clone for the borrow of tool_calls below.
                    let committed_calls: Vec<Value> = committed_calls.to_vec();
                    for call in &committed_calls {
                        let call_map = optional_mapping(Some(call));
                        let Some(call_id) = opt_str(call_map.get("call_id")) else {
                            continue;
                        };
                        let entry =
                            tool_calls
                                .entry(call_id.clone())
                                .or_insert_with(|| ToolEntry {
                                    thread: thread_id.clone(),
                                    line: line_number,
                                    ..ToolEntry::default()
                                });
                        entry.name = opt_str(call_map.get("name")).or_else(|| entry.name.clone());
                        entry.name_seen = true;
                        entry.item_id = opt_str(call_map.get("id"));
                        entry.input = opt_str(call_map.get("args"));
                        entry.committed = Some(at_ms);
                        entry.turn = run_id.clone();
                        calls_by_batch
                            .entry((thread_id.clone(), batch.clone()))
                            .or_default()
                            .push(call_id.clone());
                        if entry.name.as_deref() == Some(SPAWN_TOOL) {
                            let arguments: Map<String, Value> = entry
                                .input
                                .as_deref()
                                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                                .and_then(|value| value.as_object().cloned())
                                .unwrap_or_default();
                            entry.spawn_label = opt_str(arguments.get("role"))
                                .or_else(|| opt_str(arguments.get("command_id")));
                            entry.spawn_text = opt_str(arguments.get("objective"));
                        }
                    }
                    threads[thread_index].activity.push(at_ms);
                }
                Some("tool_result_batch_committed") => {
                    let batch = opt_str(event.get("batch_id")).unwrap_or_default();
                    let results: &[Value] = match event.get("results") {
                        Some(Value::Array(items)) => items.as_slice(),
                        _ => &[],
                    };
                    let ordered = calls_by_batch
                        .get(&(thread_id.clone(), batch))
                        .cloned()
                        .unwrap_or_default();
                    for (call_id, result) in ordered.iter().zip(results.iter()) {
                        let output = opt_str(optional_mapping(Some(result)).get("text"));
                        if let Some(output) = output {
                            if let Some(entry) = tool_calls.get_mut(call_id) {
                                entry.output = Some(output);
                            }
                        }
                    }
                }
                Some("terminal") => {
                    if let Some(run_id) = &run_id {
                        let status = opt_str(event.get("terminal"))
                            .unwrap_or_else(|| "completed".to_string());
                        threads[thread_index]
                            .turn_status
                            .insert(run_id.clone(), status);
                        if let Some(first) = event.get("time_to_first_token_ms") {
                            let first = match first {
                                Value::Number(number) if !number.is_f64() => {
                                    number.as_i64().or_else(|| {
                                        number.as_u64().and_then(|v| i64::try_from(v).ok())
                                    })
                                }
                                _ => None,
                            };
                            if let Some(first) = first {
                                let start = threads[thread_index]
                                    .turns
                                    .iter()
                                    .find(|(held, _)| held == run_id)
                                    .and_then(|(_, times)| times.iter().min().copied())
                                    .unwrap_or(at_ms);
                                threads[thread_index]
                                    .first_token
                                    .insert(run_id.clone(), start.saturating_add(first));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for (child, (parent, at_ms, line_number)) in &pending_bound {
        if !threads.iter().any(|held| held.thread_id == *child) {
            continue;
        }
        parent_of.insert(child.clone(), parent.clone());
        let call = spawn_call_of
            .get(child)
            .cloned()
            .unwrap_or_else(|| format!("muse-spawn-{child}"));
        let spawn = tool_calls.get(&call);
        description_of.insert(
            child.clone(),
            spawn
                .and_then(|entry| entry.spawn_label.clone())
                .unwrap_or_else(|| child.clone()),
        );
        edges.push(Edge {
            edge_id: format!("muse-spawn-{child}"),
            call_id: call,
            from_thread_id: parent.clone(),
            to_thread_id: child.clone(),
            kind: "spawn".to_string(),
            timestamp_ms: *at_ms,
            message_text: spawn.and_then(|entry| entry.spawn_text.clone()),
            content_availability: "plain".to_string(),
            encrypted_content: None,
            source_line: *line_number,
        });
    }
    let mut turns: Vec<Turn> = Vec::new();
    for thread in &threads {
        let mut thread_turns: Vec<&(String, Vec<i64>)> = thread.turns.iter().collect();
        thread_turns.sort_by(|a, b| {
            (a.1.iter().min().copied().unwrap_or(0), &a.0)
                .cmp(&(b.1.iter().min().copied().unwrap_or(0), &b.0))
        });
        for (run_id, times) in thread_turns {
            let start = times.iter().min().copied().unwrap_or(0);
            let end = times.iter().max().copied().unwrap_or(start);
            turns.push(Turn {
                turn_id: run_id.clone(),
                thread_id: thread.thread_id.clone(),
                started_at_ms: start,
                ended_at_ms: Some(std::cmp::max(start + 1, end)),
                status: thread
                    .turn_status
                    .get(run_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string()),
                first_token_ms: thread.first_token.get(run_id).copied(),
                error: None,
                last_agent_message: thread.last_message.get(run_id).cloned(),
            });
        }
    }
    let mut calls: Vec<ToolCall> = Vec::new();
    for (call_id, entry) in &tool_calls {
        let started = entry.started.or(entry.committed);
        let Some(started) = started else {
            continue;
        };
        let name = entry.name.clone().unwrap_or_else(|| "unknown".to_string());
        let namespace = if name.starts_with("mcp__") && name.matches("__").count() >= 2 {
            name.split("__").nth(1).map(str::to_owned)
        } else {
            None
        };
        let raw_ended = entry.ended;
        calls.push(ToolCall {
            call_id: call_id.clone(),
            item_id: entry.item_id.clone(),
            thread_id: entry.thread.clone(),
            turn_id: entry.turn.clone(),
            name,
            namespace,
            started_at_ms: started,
            ended_at_ms: raw_ended.filter(|ended| *ended >= started),
            status: entry.status.clone().unwrap_or_else(|| {
                if raw_ended.is_none() {
                    "running".to_string()
                } else {
                    "completed".to_string()
                }
            }),
            input_text: entry.input.clone(),
            output_text: entry.output.clone(),
            nested_tools: Vec::new(),
            source_line: entry.line,
            input_payload: None,
            output_payload: None,
        });
    }
    // Agent paths.
    let mut paths: Vec<(String, String)> = Vec::new();
    fn agent_path(
        thread_id: &str,
        root_id: &str,
        parent_of: &BTreeMap<String, String>,
        description_of: &BTreeMap<String, String>,
        paths: &mut Vec<(String, String)>,
        chain: &mut Vec<String>,
    ) -> Result<String, String> {
        if let Some((_, held)) = paths.iter().find(|(held, _)| held == thread_id) {
            return Ok(held.clone());
        }
        // The oracle recurses without a guard and dies with
        // a RecursionError; a parent cycle refuses cleanly
        // here instead of overflowing the stack. A chain
        // cannot revisit a thread already on it.
        if chain.contains(&thread_id.to_string()) {
            return Err("cycle in Muse subagent parent metadata".to_string());
        }
        chain.push(thread_id.to_string());
        let Some(parent) = parent_of.get(thread_id) else {
            paths.push((thread_id.to_string(), "/root".to_string()));
            return Ok("/root".to_string());
        };
        let _ = root_id;
        let base = agent_path(parent, root_id, parent_of, description_of, paths, chain)?;
        let description = description_of
            .get(thread_id)
            .map(String::as_str)
            .unwrap_or(thread_id);
        let leaf = slug_dash(description);
        let tail8: String = thread_id
            .chars()
            .rev()
            .take(8)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let leaf = if leaf.is_empty() { tail8 } else { leaf };
        let mut candidate = format!("{base}/{leaf}");
        if paths.iter().any(|(_, held)| *held == candidate) {
            let tail6: String = thread_id
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect();
            candidate = format!("{candidate}_{tail6}");
        }
        paths.push((thread_id.to_string(), candidate.clone()));
        Ok(candidate)
    }
    let mut agents: Vec<Agent> = Vec::new();
    for thread in &threads {
        if thread.thread_id != root_id && !parent_of.contains_key(&thread.thread_id) {
            continue;
        }
        let activity: Vec<i64> = if thread.activity.is_empty() {
            let earliest = thread
                .turns
                .iter()
                .flat_map(|(_, times)| times.iter().copied())
                .min()
                .unwrap_or(0);
            vec![earliest]
        } else {
            thread.activity.clone()
        };
        let path_text = agent_path(
            &thread.thread_id,
            root_id,
            &parent_of,
            &description_of,
            &mut paths,
            &mut Vec::new(),
        )?;
        agents.push(Agent {
            thread_id: thread.thread_id.clone(),
            parent_thread_id: parent_of.get(&thread.thread_id).cloned(),
            agent_path: path_text.clone(),
            nickname: None,
            role: description_of.get(&thread.thread_id).cloned(),
            depth: path_text.matches('/').count() as i64 - 1,
            started_at_ms: activity.iter().min().copied().unwrap_or(0),
            ended_at_ms: Some(activity.iter().max().copied().unwrap_or(0) + 1),
            status: if thread
                .turn_status
                .values()
                .any(|status| status == "completed")
            {
                "completed"
            } else {
                "unknown"
            }
            .to_string(),
            source_path: thread.source_path.clone(),
        });
    }
    let included: Vec<String> = agents.iter().map(|agent| agent.thread_id.clone()).collect();
    let mut events: Vec<Event> = Vec::new();
    for thread in &threads {
        if !included.contains(&thread.thread_id) {
            continue;
        }
        events.extend(thread.events.iter().cloned());
    }
    events.sort_by(|a, b| {
        (a.timestamp_ms, &a.thread_id, a.source_line).cmp(&(
            b.timestamp_ms,
            &b.thread_id,
            b.source_line,
        ))
    });
    if events.is_empty() && calls.is_empty() {
        return Err("Muse session contains no renderable activity".to_string());
    }
    let mut sources_filtered: Vec<SourceSnapshot> = snapshots
        .into_iter()
        .filter(|snapshot| included.contains(&snapshot.thread_id))
        .collect();
    sources_filtered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut agents_sorted = agents.clone();
    agents_sorted.sort_by(|a, b| {
        (a.depth, a.started_at_ms, &a.thread_id).cmp(&(b.depth, b.started_at_ms, &b.thread_id))
    });
    let mut turns_filtered: Vec<Turn> = turns
        .into_iter()
        .filter(|turn| included.contains(&turn.thread_id))
        .collect();
    turns_filtered
        .sort_by(|a, b| (a.started_at_ms, &a.turn_id).cmp(&(b.started_at_ms, &b.turn_id)));
    let mut calls_filtered: Vec<ToolCall> = calls
        .into_iter()
        .filter(|call| included.contains(&call.thread_id))
        .collect();
    calls_filtered
        .sort_by(|a, b| (a.started_at_ms, &a.call_id).cmp(&(b.started_at_ms, &b.call_id)));
    let mut edges_sorted = edges.clone();
    edges_sorted.sort_by(|a, b| (a.timestamp_ms, &a.edge_id).cmp(&(b.timestamp_ms, &b.edge_id)));
    let _ = dump_compact(&Value::Null);
    Ok(TeamData {
        team_slug: team_slug.to_string(),
        provider: "muse".to_string(),
        root_thread_id: root_id.to_string(),
        display_timezone: display_timezone.to_string(),
        sources: sources_filtered,
        agents: agents_sorted,
        turns: turns_filtered,
        events,
        tool_calls: calls_filtered,
        edges: edges_sorted,
        task_notes: Vec::new(),
        window_start_ms: None,
        window_end_ms: None,
    })
}

fn slug_dash(value: &str) -> String {
    let lowered = value.to_lowercase();
    let mut out = String::new();
    let mut last_dash = true;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    trimmed.chars().take(48).collect()
}
