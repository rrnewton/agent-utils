//! Import a Claude Code coordinator lineage:
//! `py/wrkviz/claude.py` at parity. Record-level format
//! facts come from `agent_harness::claude_records`;
//! lineage, snapshotting, and team assembly live here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_harness::claude_records::{
    parse_session_line, session_lines, timestamp_ms, SessionError, TimestampError,
};
use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::compressed::{
    iter_logical_glob_agents, read_source_bytes, resolve_source, source_exists,
};
use crate::history::SourceCopy;
use crate::json::{dump_compact, py_repr_str};
use crate::team_data::{Agent, Edge, Event, SourceSnapshot, TeamData, ToolCall, Turn};
use crate::transcript_io::write_text_if_changed;

const CLASSIFICATION_VERSION: &str = "authorship-v1";
const SCHEDULED_INPUT_PREFIXES: [&str; 3] = [
    "MISSION RE-READ (",
    "Stay in the triage/rebase/fix/land loop",
    "DISK STOPGAP (",
];

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

fn validate_thread_id(thread_id: &str, where_: &str) -> Result<(), String> {
    if safe_id(thread_id) {
        Ok(())
    } else {
        Err(format!(
            "{where_}: unsafe thread id {}",
            py_repr_str(thread_id)
        ))
    }
}

/// The filtered segments of a safe relative source path.
fn safe_relative_parts(value: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    if value.starts_with('/') || parts.is_empty() || parts.iter().any(|part| part == "..") {
        return Err(format!("unsafe Claude source path {}", py_repr_str(value)));
    }
    Ok(parts)
}

fn safe_relative(value: &str) -> Result<(), String> {
    safe_relative_parts(value).map(|_| ())
}

/// CPython `str.strip()`: Unicode whitespace plus
/// \\x1c-\\x1f, which Rust's `trim` keeps.
fn py_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}'))
}

fn opt_str(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn required_string(value: Option<&Value>, where_: &str) -> Result<String, String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(format!("{where_}: expected a non-empty string")),
    }
}

fn optional_mapping(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn textual(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(other) => Some(dump_compact(other)),
    }
}

fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::Null => "None".to_string(),
        Value::Number(number) => crate::json::py_number_str(number),
        other => dump_compact(other),
    }
}

/// CPython `==` over JSON values: numbers compare across
/// int/float, and `True`/`False` equal 1/0.
fn py_num_eq(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    // CPython compares int/float exactly, at full precision.
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return x == y;
    }
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => {
            if a.is_f64() && b.is_f64() {
                return x == y;
            }
            // One side is an integer type: the float must be
            // integral and exactly that integer.
            let (float, int_as_f64) = if a.is_f64() { (x, y) } else { (y, x) };
            float.fract() == 0.0 && float == int_as_f64 && {
                // Guard the 2^53 rounding cliff: the integer
                // must survive the round trip.
                if let Some(int) = if a.is_f64() { b.as_i64() } else { a.as_i64() } {
                    float as i64 == int
                } else if let Some(int) = if a.is_f64() { b.as_u64() } else { a.as_u64() } {
                    float >= 0.0 && float as u64 == int
                } else {
                    false
                }
            }
        }
        _ => false,
    }
}

fn py_deep_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => py_num_eq(a, b),
        (Value::Bool(a), Value::Number(b)) => Some(f64::from(*a)) == b.as_f64(),
        (Value::Number(a), Value::Bool(b)) => a.as_f64() == Some(f64::from(*b)),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| py_deep_eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| py_deep_eq(x, y)))
        }
        _ => left == right,
    }
}

fn complete_jsonl(data: &[u8]) -> &[u8] {
    agent_harness::rollout::complete_prefix(data)
}

fn count_splitlines(data: &[u8]) -> i64 {
    // CPython `bytes.splitlines` line count.
    let mut count = 0i64;
    let mut index = 0;
    let mut start = 0;
    while index < data.len() {
        let byte = data[index];
        if matches!(
            byte,
            b'\n' | b'\r' | 0x0b | 0x0c | 0x1c | 0x1d | 0x1e | 0x85
        ) {
            count += 1;
            if byte == b'\r' && data.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            index += 1;
            start = index;
        } else {
            index += 1;
        }
    }
    if start < data.len() {
        count += 1;
    }
    count
}

#[derive(Clone)]
struct CRecord {
    source_path: String,
    line: i64,
    raw: Vec<u8>,
    value: Value,
    thread_id: String,
    timestamp_ms: Option<i64>,
}

#[derive(Clone)]
struct Meta {
    agent_id: String,
    tool_use_id: Option<String>,
    parent_agent_id: Option<String>,
    description: Option<String>,
    agent_type: Option<String>,
    #[allow(dead_code)]
    spawn_depth: Option<i64>,
}

/// (ended ms, status, output text) of one tool call.
type ToolResult = (i64, String, Option<String>);

struct ToolBuilder {
    call_id: String,
    item_id: Option<String>,
    thread_id: String,
    started_at_ms: i64,
    name: String,
    input_text: Option<String>,
    source_line: i64,
    input_object: Map<String, Value>,
}

fn parse_timestamp_field(value: Option<&Value>, where_: &str) -> Result<Option<i64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = required_string(Some(value), where_)?;
    match timestamp_ms(&Value::String(text.clone())) {
        Ok(result) => Ok(result),
        Err(TimestampError::Invalid(_)) => Err(format!(
            "{where_}: invalid ISO timestamp {}",
            py_repr_str(&text)
        )),
        Err(TimestampError::NoTimezone) => Err(format!("{where_}: timestamp has no timezone")),
    }
}

fn agent_id_from_name(name: &str) -> Option<String> {
    let inner = name.strip_prefix("agent-")?.strip_suffix(".jsonl")?;
    if !inner.is_empty()
        && inner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        Some(inner.to_string())
    } else {
        None
    }
}

fn read_records(
    path: &Path,
    source_path: &str,
    root_thread_id: &str,
    is_root: bool,
) -> Result<(Vec<CRecord>, SourceSnapshot, String), String> {
    let data = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    let complete = complete_jsonl(&data);
    if complete.is_empty() {
        return Err(format!(
            "{}: no newline-complete JSON records",
            path.display()
        ));
    }
    let mut records: Vec<CRecord> = Vec::new();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut thread_id: Option<String> = if is_root {
        Some(root_thread_id.to_string())
    } else {
        agent_id_from_name(&file_name)
    };
    for (index, raw) in session_lines(&data).into_iter().enumerate() {
        let line = index as i64 + 1;
        let where_ = format!("{}:{line}", path.display());
        let value = parse_session_line(raw, line).map_err(|error| match error {
            SessionError::InvalidJson(_) => {
                let detail = serde_json::from_slice::<Value>(raw)
                    .err()
                    .map(|err| err.to_string())
                    .unwrap_or_default();
                format!("{where_}: invalid JSON ({detail})")
            }
            SessionError::NotAnObject(_) => format!("{where_}: expected an object"),
        })?;
        if let Some(record_session) = opt_str(value.get("sessionId")) {
            if record_session != root_thread_id {
                return Err(format!(
                    "{where_}: session id {} does not match root",
                    py_repr_str(&record_session)
                ));
            }
        }
        if !is_root {
            if let Some(record_agent) = opt_str(value.get("agentId")) {
                if let Some(known) = &thread_id {
                    if *known != record_agent {
                        return Err(format!(
                            "{where_}: inconsistent subagent id {}",
                            py_repr_str(&record_agent)
                        ));
                    }
                }
                thread_id = Some(record_agent);
            }
        }
        let timestamp = parse_timestamp_field(
            value.get("timestamp"),
            &format!("{}:{line}.timestamp", path.display()),
        )?;
        records.push(CRecord {
            source_path: source_path.to_string(),
            line,
            raw: raw.to_vec(),
            value,
            thread_id: String::new(),
            timestamp_ms: timestamp,
        });
    }
    let Some(thread_id) = thread_id else {
        return Err(format!(
            "{}: cannot determine a subagent id",
            path.display()
        ));
    };
    validate_thread_id(&thread_id, &path.display().to_string())?;
    for record in &mut records {
        record.thread_id = thread_id.clone();
    }
    let metadata = std::fs::metadata(path).map_err(|error| crate::json::os_error(&error, path))?;
    use std::os::unix::fs::MetadataExt as _;
    let snapshot = SourceSnapshot {
        path: source_path.to_string(),
        thread_id: thread_id.clone(),
        size_bytes: metadata.size() as i64,
        mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
        sha256: sha256_hex(complete),
        complete_bytes: complete.len() as i64,
        line_count: records.len() as i64,
        ..SourceSnapshot::default()
    };
    Ok((records, snapshot, thread_id))
}

fn read_meta(
    path: &Path,
    source_path: &str,
    agent_id: &str,
) -> Result<(Meta, SourceSnapshot), String> {
    let data = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    let value: Value = serde_json::from_slice(&data)
        .map_err(|error| format!("{}: invalid JSON ({error})", path.display()))?;
    let item = value
        .as_object()
        .ok_or_else(|| format!("{}: expected an object", path.display()))?;
    let spawn_depth = match item.get("spawnDepth") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) if number.is_i64() || number.is_u64() => {
            Some(number.as_i64().unwrap_or(i64::MAX))
        }
        Some(_) => {
            return Err(format!(
                "{}.spawnDepth: expected an integer",
                path.display()
            ))
        }
    };
    let parent_agent_id = opt_str(item.get("parentAgentId"));
    if let Some(parent) = &parent_agent_id {
        validate_thread_id(parent, &path.display().to_string())?;
    }
    let metadata = std::fs::metadata(path).map_err(|error| crate::json::os_error(&error, path))?;
    use std::os::unix::fs::MetadataExt as _;
    let snapshot = SourceSnapshot {
        path: source_path.to_string(),
        thread_id: agent_id.to_string(),
        size_bytes: metadata.size() as i64,
        mtime_ns: metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
        sha256: sha256_hex(&data),
        complete_bytes: data.len() as i64,
        line_count: std::cmp::max(1, count_splitlines(&data)),
        ..SourceSnapshot::default()
    };
    Ok((
        Meta {
            agent_id: agent_id.to_string(),
            tool_use_id: opt_str(item.get("toolUseId")),
            parent_agent_id,
            description: opt_str(item.get("description")),
            agent_type: opt_str(item.get("agentType")),
            spawn_depth,
        },
        snapshot,
    ))
}

// ---------------------------------------------------------------- discovery

/// Discover one root JSONL and its Claude-managed subagent
/// files.
pub fn discover_claude_sources(session_file: &Path) -> Result<Vec<PathBuf>, String> {
    if !source_exists(session_file) {
        return Err(format!(
            "Claude session is not a regular file: {}",
            session_file.display()
        ));
    }
    let name = session_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let has_jsonl_suffix = name
        .rfind('.')
        .is_some_and(|index| index > 0 && &name[index..] == ".jsonl");
    if !has_jsonl_suffix {
        return Err("Claude session filename must end in .jsonl".to_string());
    }
    let mut sources = vec![session_file.to_path_buf()];
    sources.extend(discover_subagent_sources(session_file)?);
    Ok(sources)
}

fn discover_subagent_sources(session_file: &Path) -> Result<Vec<PathBuf>, String> {
    let mut sources: Vec<PathBuf> = Vec::new();
    let stem = session_file
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let subagents = session_file
        .parent()
        .map(|parent| parent.join(&stem).join("subagents"))
        .unwrap_or_default();
    if subagents.is_dir() {
        for path in iter_logical_glob_agents(&subagents) {
            if !source_exists(&path) {
                return Err(format!(
                    "Claude subagent source is not regular: {}",
                    path.display()
                ));
            }
            sources.push(path.clone());
            let meta = meta_path_for(&path);
            if let Some(physical) = resolve_source(&meta) {
                let is_symlink = std::fs::symlink_metadata(&physical)
                    .map(|held| held.file_type().is_symlink())
                    .unwrap_or(false);
                if is_symlink {
                    return Err(format!(
                        "Claude agent metadata is a symlink: {}",
                        meta.display()
                    ));
                }
                if physical.is_file() {
                    sources.push(meta);
                }
            }
        }
    }
    Ok(sources)
}

fn meta_path_for(agent_path: &Path) -> PathBuf {
    let name = agent_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".jsonl").unwrap_or(&name);
    agent_path.with_file_name(format!("{stem}.meta.json"))
}

fn relative_sources(
    session_file: &Path,
    source_paths: Option<&[String]>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let root = session_file.parent().unwrap_or(Path::new("")).to_path_buf();
    let root_name = session_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut result: Vec<(String, PathBuf)> = Vec::new();
    match source_paths {
        None => {
            for path in discover_claude_sources(session_file)? {
                let relative = path
                    .strip_prefix(&root)
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                    .map_err(|_| {
                        format!(
                            "Claude source is outside the session root: {}",
                            path.display()
                        )
                    })?;
                result.push((relative, path));
            }
        }
        Some(paths) => {
            for raw in paths {
                let parts = safe_relative_parts(raw)?;
                let mut path = root.clone();
                for part in &parts {
                    path.push(part);
                }
                let is_symlink = std::fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
                if is_symlink || !path.is_file() {
                    return Err(format!("missing Claude source {}", path.display()));
                }
                result.push((parts.join("/"), path));
            }
        }
    }
    if !result.iter().any(|(relative, _)| *relative == root_name) {
        return Err("Claude source set omits the root transcript".to_string());
    }
    result.sort_by(|a, b| (a.0 != root_name, &a.0).cmp(&(b.0 != root_name, &b.0)));
    Ok(result)
}

// ---------------------------------------------------------------- dedup / forks

const REWRITE_DECORATIONS: [&str; 5] = ["slug", "parentUuid", "agentId", "promptId", "isSidechain"];
const STREAMING_FIELDS: [&str; 3] = ["stop_reason", "stop_sequence", "usage"];

fn record_identity(value: &Value) -> Value {
    let Some(item) = value.as_object() else {
        return value.clone();
    };
    let mut identity = Map::new();
    for (key, held) in item {
        if REWRITE_DECORATIONS.contains(&key.as_str()) {
            continue;
        }
        if key == "message" {
            if let Some(message) = held.as_object() {
                let mut cleaned = Map::new();
                for (mkey, mheld) in message {
                    if !STREAMING_FIELDS.contains(&mkey.as_str()) {
                        cleaned.insert(mkey.clone(), mheld.clone());
                    }
                }
                identity.insert(key.clone(), Value::Object(cleaned));
                continue;
            }
        }
        identity.insert(key.clone(), held.clone());
    }
    Value::Object(identity)
}

fn same_record(left: &CRecord, right: &CRecord) -> bool {
    left.raw == right.raw
        || py_deep_eq(
            &record_identity(&left.value),
            &record_identity(&right.value),
        )
}

fn deduplicate(records: &[CRecord]) -> Result<Vec<CRecord>, String> {
    let mut seen: BTreeMap<(String, String), CRecord> = BTreeMap::new();
    let mut result: Vec<CRecord> = Vec::new();
    for record in records {
        let Some(uuid) = opt_str(record.value.get("uuid")) else {
            result.push(record.clone());
            continue;
        };
        let key = (record.thread_id.clone(), uuid.clone());
        match seen.get(&key) {
            None => {
                seen.insert(key, record.clone());
                result.push(record.clone());
            }
            Some(previous) => {
                if !same_record(previous, record) {
                    return Err(format!(
                        "{}:{}: conflicting duplicate UUID {uuid}",
                        record.source_path, record.line
                    ));
                }
            }
        }
    }
    Ok(result)
}

fn tool_use_blocks(record: &CRecord) -> Vec<(String, Value, Value)> {
    let mut out = Vec::new();
    let Some(content) = record
        .value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return out;
    };
    for block in content {
        let Some(block) = block.as_object() else {
            continue;
        };
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let Some(id) = block.get("id") else {
            continue;
        };
        let truthy = match id {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            Value::String(text) => !text.is_empty(),
            Value::Number(number) => number.as_f64().is_some_and(|held| held != 0.0),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
        };
        if !truthy {
            continue;
        }
        out.push((
            py_str(id),
            block.get("name").cloned().unwrap_or(Value::Null),
            block.get("input").cloned().unwrap_or(Value::Null),
        ));
    }
    out
}

fn drop_fork_prefixes(
    records_by_thread: &[(String, Vec<CRecord>)],
) -> Result<Vec<(String, Vec<CRecord>)>, String> {
    let mut ordered: Vec<&(String, Vec<CRecord>)> = records_by_thread.iter().collect();
    ordered.sort_by_key(|(_, records)| {
        records.iter().take(3).any(|record| {
            record.value.get("type").and_then(Value::as_str) == Some("fork-context-ref")
        })
    });
    let mut first: BTreeMap<String, CRecord> = BTreeMap::new();
    let mut calls_by_thread: BTreeMap<String, (String, Value, Value)> = BTreeMap::new();
    let mut kept_by_thread: BTreeMap<String, Vec<CRecord>> = BTreeMap::new();
    for (thread_id, records) in &ordered {
        let mut kept: Vec<CRecord> = Vec::new();
        for record in records.iter() {
            let calls = tool_use_blocks(record);
            if !calls.is_empty()
                && calls.iter().all(|(call_id, name, input)| {
                    calls_by_thread
                        .get(call_id)
                        .is_some_and(|(owner, oname, oinput)| {
                            owner.as_str() != thread_id.as_str()
                                && py_deep_eq(oname, name)
                                && py_deep_eq(oinput, input)
                        })
                })
            {
                continue;
            }
            for (call_id, name, input) in &calls {
                calls_by_thread
                    .entry(call_id.clone())
                    .or_insert_with(|| ((*thread_id).clone(), name.clone(), input.clone()));
            }
            let uuid = opt_str(record.value.get("uuid"));
            let previous = uuid.as_ref().and_then(|held| first.get(held));
            match previous {
                None => {
                    if let Some(uuid) = &uuid {
                        first.insert(uuid.clone(), record.clone());
                    }
                    kept.push(record.clone());
                }
                Some(previous) if previous.thread_id == *thread_id => {
                    kept.push(record.clone());
                }
                Some(previous) => {
                    if !same_record(previous, record) {
                        return Err(format!(
                            "{}:{}: conflicting duplicate UUID {} (first seen in {}:{})",
                            record.source_path,
                            record.line,
                            uuid.unwrap_or_default(),
                            previous.source_path,
                            previous.line
                        ));
                    }
                }
            }
        }
        kept_by_thread.insert((*thread_id).clone(), kept);
    }
    Ok(records_by_thread
        .iter()
        .map(|(thread_id, _)| {
            (
                thread_id.clone(),
                kept_by_thread.get(thread_id).cloned().unwrap_or_default(),
            )
        })
        .collect())
}

// ---------------------------------------------------------------- provenance

fn origin_kind(value: Option<&Value>) -> Option<String> {
    opt_str(optional_mapping(value).get("kind"))
}

fn is_slash_command(text: &str) -> bool {
    text.contains("<command-name>") || text.contains("<command-message>")
}

fn is_synthetic_user_text(record: &CRecord, text: &str) -> bool {
    if record.value.get("isCompactSummary") == Some(&Value::Bool(true))
        || record.value.get("isMeta") == Some(&Value::Bool(true))
    {
        return true;
    }
    text.starts_with("<local-command-caveat>")
        || text.starts_with("<local-command-stdout>")
        || text.starts_with("A session-scoped Stop hook is now active")
        || text.starts_with("Stop hook feedback:")
        || text.starts_with("[Request interrupted by user")
        || SCHEDULED_INPUT_PREFIXES
            .iter()
            .any(|prefix| text.starts_with(prefix))
}

fn input_provenance(
    record: &CRecord,
    text: &str,
    origin_kind: Option<&str>,
    queued: bool,
) -> (&'static str, &'static str, &'static str, &'static str) {
    if origin_kind == Some("human") {
        return (
            "user_prompt",
            "user",
            "owner_human",
            if queued {
                "claude_queued"
            } else {
                "claude_typed"
            },
        );
    }
    if is_slash_command(text) {
        return ("user_prompt", "user", "owner_human", "claude_slash");
    }
    if origin_kind.is_some() || is_synthetic_user_text(record, text) {
        return ("system_input", "system", "system", "claude_system");
    }
    ("user_prompt", "user", "unknown", "claude_legacy")
}

fn record_time(record: &CRecord) -> Result<i64, String> {
    record.timestamp_ms.ok_or_else(|| {
        format!(
            "{}:{}: message record lacks a timestamp",
            record.source_path, record.line
        )
    })
}

fn event_id(record: &CRecord, suffix: &str) -> String {
    if let Some(uuid) = opt_str(record.value.get("uuid")) {
        return format!("{uuid}:{suffix}");
    }
    let digest = sha256_hex(&record.raw);
    format!(
        "claude-{}-{}-{}:{suffix}",
        record.thread_id,
        record.line,
        &digest[..20]
    )
}

fn message_of(record: &CRecord) -> Result<&Map<String, Value>, String> {
    record
        .value
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!(
                "{}:{}.message: expected an object",
                record.source_path, record.line
            )
        })
}

/// What one thread's records extract to.
type Extracted = (
    Vec<Event>,
    Vec<ToolBuilder>,
    BTreeMap<String, ToolResult>,
    Vec<i64>,
);

fn extract(records: &[CRecord]) -> Result<Extracted, String> {
    let mut events: Vec<Event> = Vec::new();
    let mut tools: Vec<ToolBuilder> = Vec::new();
    let mut results: BTreeMap<String, ToolResult> = BTreeMap::new();
    let mut assistant_times: Vec<i64> = Vec::new();
    for record in deduplicate(records)? {
        let record_type = opt_str(record.value.get("type"));
        if record_type.as_deref() == Some("attachment") {
            let attachment = optional_mapping(record.value.get("attachment"));
            if opt_str(attachment.get("type")).as_deref() != Some("queued_command") {
                continue;
            }
            let prompt = opt_str(attachment.get("prompt")).unwrap_or_default();
            let prompt = py_trim(&prompt).to_string();
            let attachment_origin = origin_kind(attachment.get("origin"));
            if prompt.is_empty() || attachment_origin.as_deref() != Some("human") {
                continue;
            }
            let (kind, author, author_kind, ingress_kind) =
                input_provenance(&record, &prompt, attachment_origin.as_deref(), true);
            events.push(Event {
                event_id: event_id(&record, "queued-user"),
                thread_id: record.thread_id.clone(),
                turn_id: None,
                timestamp_ms: record_time(&record)?,
                kind: kind.to_string(),
                role: Some("user".to_string()),
                phase: None,
                text: Some(prompt),
                content_availability: "plain".to_string(),
                encrypted_content: None,
                author: Some(author.to_string()),
                recipient: Some(record.thread_id.clone()),
                source_line: record.line,
                ingress_kind: Some(ingress_kind.to_string()),
                author_kind: Some(author_kind.to_string()),
                source_native_id: opt_str(record.value.get("uuid")),
                classification_version: Some(CLASSIFICATION_VERSION.to_string()),
            });
            continue;
        }
        if !matches!(record_type.as_deref(), Some("user" | "assistant")) {
            continue;
        }
        let at_ms = record_time(&record)?;
        let message = message_of(&record)?.clone();
        let role = opt_str(message.get("role"));
        let where_ = format!("{}:{}.message", record.source_path, record.line);
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if record_type.as_deref() == Some("user") {
            let raw_origin = origin_kind(record.value.get("origin"));
            if let Value::String(text) = &content {
                let text = py_trim(text).to_string();
                if !text.is_empty() {
                    let (kind, author, author_kind, ingress_kind) =
                        input_provenance(&record, &text, raw_origin.as_deref(), false);
                    events.push(Event {
                        event_id: event_id(&record, "user"),
                        thread_id: record.thread_id.clone(),
                        turn_id: None,
                        timestamp_ms: at_ms,
                        kind: kind.to_string(),
                        role: Some(
                            role.clone()
                                .filter(|held| !held.is_empty())
                                .unwrap_or_else(|| "user".to_string()),
                        ),
                        phase: None,
                        text: Some(text),
                        content_availability: "plain".to_string(),
                        encrypted_content: None,
                        author: Some(author.to_string()),
                        recipient: Some(record.thread_id.clone()),
                        source_line: record.line,
                        ingress_kind: Some(ingress_kind.to_string()),
                        author_kind: Some(author_kind.to_string()),
                        source_native_id: opt_str(record.value.get("uuid")),
                        classification_version: Some(CLASSIFICATION_VERSION.to_string()),
                    });
                }
                continue;
            }
            for (index, block) in blocks_of(&message, &where_)?.into_iter().enumerate() {
                let block_type = opt_str(block.get("type"));
                if block_type.as_deref() == Some("tool_result") {
                    let call_id = required_string(
                        block.get("tool_use_id"),
                        &format!("{where_}.tool_result.tool_use_id"),
                    )?;
                    if results.contains_key(&call_id) {
                        return Err(format!("{where_}: duplicate result for {call_id}"));
                    }
                    let failed = block.get("is_error") == Some(&Value::Bool(true));
                    results.insert(
                        call_id,
                        (
                            at_ms,
                            if failed { "failed" } else { "completed" }.to_string(),
                            textual(block.get("content")),
                        ),
                    );
                } else if block_type.as_deref() == Some("text") {
                    let text = py_trim(&opt_str(block.get("text")).unwrap_or_default()).to_string();
                    if !text.is_empty() {
                        let (kind, author, author_kind, ingress_kind) =
                            input_provenance(&record, &text, raw_origin.as_deref(), false);
                        events.push(Event {
                            event_id: event_id(&record, &format!("user-{index}")),
                            thread_id: record.thread_id.clone(),
                            turn_id: None,
                            timestamp_ms: at_ms,
                            kind: kind.to_string(),
                            role: Some(
                                role.clone()
                                    .filter(|held| !held.is_empty())
                                    .unwrap_or_else(|| "user".to_string()),
                            ),
                            phase: None,
                            text: Some(text),
                            content_availability: "plain".to_string(),
                            encrypted_content: None,
                            author: Some(author.to_string()),
                            recipient: Some(record.thread_id.clone()),
                            source_line: record.line,
                            ingress_kind: Some(ingress_kind.to_string()),
                            author_kind: Some(author_kind.to_string()),
                            source_native_id: opt_str(record.value.get("uuid")),
                            classification_version: Some(CLASSIFICATION_VERSION.to_string()),
                        });
                    }
                }
            }
            continue;
        }
        assistant_times.push(at_ms);
        let stop_reason = opt_str(message.get("stop_reason"));
        for (index, block) in blocks_of(&message, &where_)?.into_iter().enumerate() {
            let block_type = opt_str(block.get("type"));
            if block_type.as_deref() == Some("text") {
                let text = py_trim(&opt_str(block.get("text")).unwrap_or_default()).to_string();
                if !text.is_empty() {
                    events.push(Event {
                        event_id: event_id(&record, &format!("assistant-{index}")),
                        thread_id: record.thread_id.clone(),
                        turn_id: None,
                        timestamp_ms: at_ms,
                        kind: "assistant_message".to_string(),
                        role: Some(
                            role.clone()
                                .filter(|held| !held.is_empty())
                                .unwrap_or_else(|| "assistant".to_string()),
                        ),
                        phase: Some(
                            if matches!(stop_reason.as_deref(), Some("end_turn" | "stop_sequence"))
                            {
                                "final_answer"
                            } else {
                                "commentary"
                            }
                            .to_string(),
                        ),
                        text: Some(text),
                        content_availability: "plain".to_string(),
                        encrypted_content: None,
                        author: Some(record.thread_id.clone()),
                        recipient: None,
                        source_line: record.line,
                        ingress_kind: Some("claude".to_string()),
                        author_kind: Some("agent".to_string()),
                        source_native_id: opt_str(record.value.get("uuid")),
                        classification_version: Some(CLASSIFICATION_VERSION.to_string()),
                    });
                }
            } else if block_type.as_deref() == Some("tool_use") {
                let call_id = required_string(block.get("id"), &format!("{where_}.tool_use.id"))?;
                let name = required_string(block.get("name"), &format!("{where_}.tool_use.name"))?;
                tools.push(ToolBuilder {
                    call_id,
                    item_id: opt_str(record.value.get("uuid")),
                    thread_id: record.thread_id.clone(),
                    started_at_ms: at_ms,
                    name,
                    input_text: textual(block.get("input")),
                    source_line: record.line,
                    input_object: optional_mapping(block.get("input")),
                });
            }
        }
    }
    Ok((events, tools, results, assistant_times))
}

fn turns_for_thread(
    thread_id: &str,
    events: &[Event],
    tools: &[ToolBuilder],
    assistant_times: &[i64],
) -> Vec<Turn> {
    let mut prompts: Vec<&Event> = events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "user_prompt" | "system_input"))
        .collect();
    prompts.sort_by(|a, b| (a.timestamp_ms, &a.event_id).cmp(&(b.timestamp_ms, &b.event_id)));
    if prompts.is_empty() {
        return Vec::new();
    }
    let mut activity: Vec<i64> = events.iter().map(|event| event.timestamp_ms).collect();
    activity.extend(tools.iter().map(|tool| tool.started_at_ms));
    activity.extend(assistant_times.iter().copied());
    let last_activity = activity
        .iter()
        .max()
        .copied()
        .unwrap_or_else(|| prompts[prompts.len() - 1].timestamp_ms);
    let mut turns = Vec::new();
    for (index, prompt) in prompts.iter().enumerate() {
        let next_start = prompts.get(index + 1).map(|next| next.timestamp_ms);
        let upper = next_start.map_or(last_activity, |start| start - 1);
        let mut responses: Vec<&Event> = events
            .iter()
            .filter(|event| {
                event.kind == "assistant_message"
                    && prompt.timestamp_ms <= event.timestamp_ms
                    && event.timestamp_ms <= upper
            })
            .collect();
        responses.sort_by(|a, b| (a.timestamp_ms, &a.event_id).cmp(&(b.timestamp_ms, &b.event_id)));
        let first_token = assistant_times
            .iter()
            .copied()
            .filter(|stamp| prompt.timestamp_ms <= *stamp && *stamp <= upper)
            .min();
        let mut ended = prompt.timestamp_ms;
        for event in &responses {
            ended = ended.max(event.timestamp_ms);
        }
        for tool in tools {
            if prompt.timestamp_ms <= tool.started_at_ms && tool.started_at_ms <= upper {
                ended = ended.max(tool.started_at_ms);
            }
        }
        let final_event = responses
            .iter()
            .rev()
            .find(|event| event.phase.as_deref() == Some("final_answer"));
        turns.push(Turn {
            turn_id: prompt.event_id.clone(),
            thread_id: thread_id.to_string(),
            started_at_ms: prompt.timestamp_ms,
            ended_at_ms: Some(std::cmp::max(prompt.timestamp_ms + 1, ended)),
            status: if final_event.is_some() {
                "completed"
            } else {
                "unknown"
            }
            .to_string(),
            first_token_ms: first_token,
            error: None,
            last_agent_message: responses.last().and_then(|event| event.text.clone()),
        });
    }
    turns
}

fn turn_id(turns: &[Turn], at_ms: i64) -> Option<String> {
    // CPython `max` keeps the first maximal item.
    let mut best: Option<&Turn> = None;
    for turn in turns.iter().filter(|turn| turn.started_at_ms <= at_ms) {
        if best.is_none_or(|held| turn.started_at_ms > held.started_at_ms) {
            best = Some(turn);
        }
    }
    best.map(|turn| turn.turn_id.clone())
}

fn slug(value: &str, fallback: &str) -> String {
    let lowered = value.to_lowercase();
    let mut out = String::new();
    let mut last_was_underscore = true; // also strips leading runs
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_was_underscore = false;
        } else if !last_was_underscore {
            out.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = out.trim_end_matches('_').to_string();
    let candidate: String = if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    };
    candidate.chars().take(80).collect()
}

fn blocks_of(
    message: &Map<String, Value>,
    where_: &str,
) -> Result<Vec<Map<String, Value>>, String> {
    match message.get("content") {
        Some(Value::String(_)) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let block = item
                    .as_object()
                    .ok_or_else(|| format!("{where_}.content[{index}]: expected an object"))?;
                out.push(block.clone());
            }
            Ok(out)
        }
        _ => Err(format!("{where_}.content: expected an array")),
    }
}

// ---------------------------------------------------------------- team assembly

fn or_str(first: Option<String>, second: Option<String>) -> Option<String> {
    match first {
        Some(text) if !text.is_empty() => Some(text),
        _ => second,
    }
}

/// Normalize one Claude session and its subagents.
pub fn load_claude_team(
    session_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    source_paths: Option<&[String]>,
) -> Result<TeamData, String> {
    let root_thread_id = session_file
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    validate_thread_id(&root_thread_id, &session_file.display().to_string())?;
    let sources_list = relative_sources(session_file, source_paths)?;
    let mut records_by_thread: Vec<(String, Vec<CRecord>)> = Vec::new();
    let mut snapshots: Vec<SourceSnapshot> = Vec::new();
    let mut agent_by_log: BTreeMap<String, String> = BTreeMap::new();
    let mut meta_paths: Vec<(String, PathBuf)> = Vec::new();
    let root_name = session_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (relative, path) in &sources_list {
        if path
            .file_name()
            .map(|name| name.to_string_lossy().ends_with(".meta.json"))
            .unwrap_or(false)
        {
            meta_paths.push((relative.clone(), path.clone()));
            continue;
        }
        let is_root = *relative == root_name;
        let (records, snapshot, thread_id) =
            read_records(path, relative, &root_thread_id, is_root)?;
        match records_by_thread
            .iter_mut()
            .find(|(held, _)| *held == thread_id)
        {
            Some((_, held)) => held.extend(records),
            None => records_by_thread.push((thread_id.clone(), records)),
        }
        snapshots.push(snapshot);
        let log_key = path.with_extension("").to_string_lossy().replace('\\', "/");
        agent_by_log.insert(log_key, thread_id);
    }
    if !records_by_thread
        .iter()
        .any(|(thread_id, _)| *thread_id == root_thread_id)
    {
        return Err("Claude root transcript did not identify the root session".to_string());
    }
    let records_by_thread = drop_fork_prefixes(&records_by_thread)?;
    let mut metas: Vec<Meta> = Vec::new();
    let mut meta_by_agent: BTreeMap<String, Meta> = BTreeMap::new();
    for (relative, path) in &meta_paths {
        let log_key = path
            .with_extension("")
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        let agent_id = agent_by_log.get(&log_key).cloned().unwrap_or_else(|| {
            let name = path
                .file_name()
                .map(|held| held.to_string_lossy().into_owned())
                .unwrap_or_default();
            let without_prefix = name.strip_prefix("agent-").unwrap_or(&name);
            without_prefix
                .strip_suffix(".meta.json")
                .unwrap_or(without_prefix)
                .to_string()
        });
        let (meta, snapshot) = read_meta(path, relative, &agent_id)?;
        meta_by_agent.insert(agent_id.clone(), meta.clone());
        metas.push(meta);
        snapshots.push(snapshot);
    }
    let mut all_events: Vec<Event> = Vec::new();
    let mut tool_builders: Vec<ToolBuilder> = Vec::new();
    let mut results_by_call: Vec<(String, Vec<(String, ToolResult)>)> = Vec::new();
    let mut assistant_by_thread: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut last_extract_empty = (true, true);
    for (thread_id, thread_records) in &records_by_thread {
        let (events, tools, results, assistant_times) = extract(thread_records)?;
        last_extract_empty = (events.is_empty(), tools.is_empty());
        all_events.extend(events);
        tool_builders.extend(tools);
        assistant_by_thread.insert(thread_id.clone(), assistant_times);
        for (call_id, extracted) in results {
            match results_by_call
                .iter_mut()
                .find(|(held, _)| *held == call_id)
            {
                Some((_, held)) => held.push((thread_id.clone(), extracted)),
                None => results_by_call.push((call_id, vec![(thread_id.clone(), extracted)])),
            }
        }
    }
    {
        let mut seen: Vec<&str> = Vec::new();
        for tool in &tool_builders {
            if seen.contains(&tool.call_id.as_str()) {
                return Err("duplicate Claude tool-use id".to_string());
            }
            seen.push(tool.call_id.as_str());
        }
    }
    let caller_by_call: BTreeMap<String, String> = tool_builders
        .iter()
        .map(|tool| (tool.call_id.clone(), tool.thread_id.clone()))
        .collect();
    let mut result_by_call: BTreeMap<String, ToolResult> = BTreeMap::new();
    for (call_id, candidates) in &results_by_call {
        let caller = caller_by_call.get(call_id);
        let own: Vec<&ToolResult> = candidates
            .iter()
            .filter(|(thread_id, _)| Some(thread_id) == caller)
            .map(|(_, result)| result)
            .collect();
        if own.len() > 1 || (caller.is_none() && candidates.len() > 1) {
            return Err(format!("duplicate Claude tool result {call_id}"));
        }
        if let Some(first) = own.first() {
            result_by_call.insert(call_id.clone(), (*first).clone());
        } else if caller.is_none() {
            result_by_call.insert(call_id.clone(), candidates[0].1.clone());
        }
    }
    let mut turns_by_thread: Vec<(String, Vec<Turn>)> = Vec::new();
    for (thread_id, _) in &records_by_thread {
        let thread_events: Vec<Event> = all_events
            .iter()
            .filter(|event| event.thread_id == *thread_id)
            .cloned()
            .collect();
        let owned_tools: Vec<ToolBuilder> = tool_builders
            .iter()
            .filter(|tool| tool.thread_id == *thread_id)
            .map(|tool| ToolBuilder {
                call_id: tool.call_id.clone(),
                item_id: tool.item_id.clone(),
                thread_id: tool.thread_id.clone(),
                started_at_ms: tool.started_at_ms,
                name: tool.name.clone(),
                input_text: tool.input_text.clone(),
                source_line: tool.source_line,
                input_object: tool.input_object.clone(),
            })
            .collect();
        let turns = turns_for_thread(
            thread_id,
            &thread_events,
            &owned_tools,
            assistant_by_thread
                .get(thread_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        turns_by_thread.push((thread_id.clone(), turns));
    }
    let turns_of = |thread_id: &str| -> &[Turn] {
        turns_by_thread
            .iter()
            .find(|(held, _)| held == thread_id)
            .map(|(_, turns)| turns.as_slice())
            .unwrap_or(&[])
    };
    for event in &mut all_events {
        event.turn_id = turn_id(turns_of(&event.thread_id.clone()), event.timestamp_ms);
    }
    let mut all_tools: Vec<ToolCall> = Vec::new();
    for builder in &tool_builders {
        let tool_result = result_by_call.get(&builder.call_id);
        let namespace = builder
            .name
            .split_once("__")
            .map(|(head, _)| head.to_string());
        all_tools.push(ToolCall {
            call_id: builder.call_id.clone(),
            item_id: builder.item_id.clone(),
            thread_id: builder.thread_id.clone(),
            turn_id: turn_id(turns_of(&builder.thread_id.clone()), builder.started_at_ms),
            name: builder.name.clone(),
            namespace,
            started_at_ms: builder.started_at_ms,
            ended_at_ms: tool_result.map(|result| result.0),
            status: tool_result
                .map(|result| result.1.clone())
                .unwrap_or_else(|| "running".to_string()),
            input_text: builder.input_text.clone(),
            output_text: tool_result.and_then(|result| result.2.clone()),
            nested_tools: Vec::new(),
            source_line: builder.source_line,
            input_payload: None,
            output_payload: None,
        });
    }
    // Spawn and parent resolution.
    let mut meta_by_tool: BTreeMap<String, Meta> = BTreeMap::new();
    for meta in &metas {
        if let Some(tool_use_id) = &meta.tool_use_id {
            meta_by_tool.insert(tool_use_id.clone(), meta.clone());
        }
    }
    let mut spawn_by_agent: BTreeMap<String, &ToolBuilder> = BTreeMap::new();
    for tool in &tool_builders {
        if tool.name == "Agent" {
            if let Some(meta) = meta_by_tool.get(&tool.call_id) {
                spawn_by_agent.insert(meta.agent_id.clone(), tool);
            }
        }
    }
    let thread_ids: Vec<String> = records_by_thread
        .iter()
        .map(|(thread_id, _)| thread_id.clone())
        .collect();
    let mut parent_by_agent: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut description_by_agent: BTreeMap<String, String> = BTreeMap::new();
    let mut role_by_agent: BTreeMap<String, Option<String>> = BTreeMap::new();
    parent_by_agent.insert(root_thread_id.clone(), None);
    description_by_agent.insert(root_thread_id.clone(), "root".to_string());
    role_by_agent.insert(root_thread_id.clone(), Some("coordinator".to_string()));
    for agent_id in &thread_ids {
        if *agent_id == root_thread_id {
            continue;
        }
        let agent_meta = meta_by_agent.get(agent_id);
        let spawn = spawn_by_agent.get(agent_id).copied();
        let mut parent: Option<String> = agent_meta.and_then(|meta| meta.parent_agent_id.clone());
        if parent.is_none() {
            if let Some(spawn) = spawn {
                parent = Some(spawn.thread_id.clone());
            }
        }
        if parent
            .as_ref()
            .is_none_or(|held| !thread_ids.contains(held))
        {
            parent = Some(root_thread_id.clone());
        }
        parent_by_agent.insert(agent_id.clone(), parent);
        let mut description: Option<String> = agent_meta.and_then(|meta| meta.description.clone());
        if description.is_none() {
            if let Some(spawn) = spawn {
                description = opt_str(spawn.input_object.get("description"));
            }
        }
        description_by_agent.insert(
            agent_id.clone(),
            description
                .filter(|held| !held.is_empty())
                .unwrap_or_else(|| agent_id.clone()),
        );
        role_by_agent.insert(
            agent_id.clone(),
            agent_meta.and_then(|meta| meta.agent_type.clone()),
        );
    }
    // Route subagent prompts and final answers.
    let mut routed_events: Vec<Event> = Vec::new();
    for event in &all_events {
        let parent = parent_by_agent
            .get(&event.thread_id)
            .cloned()
            .unwrap_or(None);
        match parent {
            None => routed_events.push(event.clone()),
            Some(parent) => {
                if event.kind == "user_prompt" {
                    let mut routed = event.clone();
                    routed.kind = "inter_agent_message".to_string();
                    routed.role = None;
                    routed.phase = Some("instruction".to_string());
                    routed.author = Some(parent);
                    routed.recipient = Some(event.thread_id.clone());
                    routed.ingress_kind = Some("claude_subagent".to_string());
                    routed.author_kind = Some("agent".to_string());
                    routed_events.push(routed);
                } else if event.kind == "assistant_message"
                    && event.phase.as_deref() == Some("final_answer")
                {
                    let mut routed = event.clone();
                    routed.kind = "inter_agent_message".to_string();
                    routed.role = None;
                    routed.author = Some(event.thread_id.clone());
                    routed.recipient = Some(parent);
                    routed.ingress_kind = Some("claude_subagent".to_string());
                    routed.author_kind = Some("agent".to_string());
                    routed_events.push(routed);
                } else {
                    routed_events.push(event.clone());
                }
            }
        }
    }
    for tool in &tool_builders {
        if tool.name != "SendMessage" {
            continue;
        }
        let recipient = or_str(
            opt_str(tool.input_object.get("recipient")),
            opt_str(tool.input_object.get("to")),
        );
        let message = or_str(
            opt_str(tool.input_object.get("message")),
            opt_str(tool.input_object.get("prompt")),
        );
        let (Some(recipient), Some(message)) = (recipient, message) else {
            continue;
        };
        if !thread_ids.contains(&recipient) {
            continue;
        }
        let already_recorded = routed_events.iter().any(|event| {
            event.kind == "inter_agent_message"
                && event.author.as_deref() == Some(tool.thread_id.as_str())
                && event.recipient.as_deref() == Some(recipient.as_str())
                && py_trim(event.text.as_deref().unwrap_or("")) == py_trim(&message)
                && (event.timestamp_ms - tool.started_at_ms).abs() <= 5_000
        });
        if already_recorded {
            continue;
        }
        routed_events.push(Event {
            event_id: format!("claude-message-{}", tool.call_id),
            thread_id: tool.thread_id.clone(),
            turn_id: turn_id(turns_of(&tool.thread_id.clone()), tool.started_at_ms),
            timestamp_ms: tool.started_at_ms,
            kind: "inter_agent_message".to_string(),
            role: None,
            phase: Some("instruction".to_string()),
            text: Some(message),
            content_availability: "plain".to_string(),
            encrypted_content: None,
            author: Some(tool.thread_id.clone()),
            recipient: Some(recipient),
            source_line: tool.source_line,
            ingress_kind: Some("claude_send_message".to_string()),
            author_kind: Some("agent".to_string()),
            source_native_id: Some(tool.call_id.clone()),
            classification_version: Some(CLASSIFICATION_VERSION.to_string()),
        });
    }
    let all_events = routed_events;
    // Activity and inclusion (no ingest window: everything).
    let mut raw_activity: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for thread_id in &thread_ids {
        raw_activity.insert(thread_id.clone(), Vec::new());
    }
    for event in &all_events {
        raw_activity
            .entry(event.thread_id.clone())
            .or_default()
            .push(event.timestamp_ms);
    }
    for tool in &all_tools {
        raw_activity
            .entry(tool.thread_id.clone())
            .or_default()
            .push(tool.started_at_ms);
        if let Some(ended) = tool.ended_at_ms {
            raw_activity
                .entry(tool.thread_id.clone())
                .or_default()
                .push(ended);
        }
    }
    for (agent_id, spawn) in &spawn_by_agent {
        raw_activity
            .entry(agent_id.clone())
            .or_default()
            .push(spawn.started_at_ms);
    }
    let mut included: Vec<String> = thread_ids
        .iter()
        .filter(|thread_id| {
            raw_activity
                .get(*thread_id)
                .is_some_and(|times| !times.is_empty())
        })
        .cloned()
        .collect();
    if !included.contains(&root_thread_id) && !included.is_empty() {
        included.push(root_thread_id.clone());
    }
    for agent_id in included.clone() {
        let mut parent = parent_by_agent.get(&agent_id).cloned().unwrap_or(None);
        let mut seen: Vec<String> = Vec::new();
        while let Some(held) = parent {
            if seen.contains(&held) {
                return Err("cycle in Claude subagent parent metadata".to_string());
            }
            seen.push(held.clone());
            if !included.contains(&held) {
                included.push(held.clone());
            }
            parent = parent_by_agent.get(&held).cloned().unwrap_or(None);
        }
    }
    if included.is_empty() {
        return Err("requested Claude window contains no transcript activity".to_string());
    }
    // Agent paths.
    let mut segment_by_agent: BTreeMap<String, String> = BTreeMap::new();
    let mut used_paths: Vec<String> = Vec::new();
    fn agent_path(
        agent_id: &str,
        root_thread_id: &str,
        parent_by_agent: &BTreeMap<String, Option<String>>,
        description_by_agent: &BTreeMap<String, String>,
        segment_by_agent: &mut BTreeMap<String, String>,
        used_paths: &mut Vec<String>,
    ) -> String {
        if let Some(existing) = segment_by_agent.get(agent_id) {
            return existing.clone();
        }
        let parent = parent_by_agent.get(agent_id).cloned().unwrap_or(None);
        let Some(parent) = parent else {
            segment_by_agent.insert(agent_id.to_string(), "/root".to_string());
            return "/root".to_string();
        };
        let _ = root_thread_id;
        let base = agent_path(
            &parent,
            root_thread_id,
            parent_by_agent,
            description_by_agent,
            segment_by_agent,
            used_paths,
        );
        let leaf = slug(
            description_by_agent
                .get(agent_id)
                .map(String::as_str)
                .unwrap_or(agent_id),
            agent_id,
        );
        let mut candidate = format!("{base}/{leaf}");
        if used_paths.contains(&candidate) {
            let suffix: String = agent_id
                .chars()
                .rev()
                .take(6)
                .collect::<Vec<char>>()
                .into_iter()
                .rev()
                .collect();
            candidate = format!("{candidate}_{suffix}");
        }
        used_paths.push(candidate.clone());
        segment_by_agent.insert(agent_id.to_string(), candidate.clone());
        candidate
    }
    let mut agent_order: Vec<String> = included.clone();
    agent_order.sort_by(|a, b| (a != &root_thread_id, a).cmp(&(b != &root_thread_id, b)));
    let mut agents: Vec<Agent> = Vec::new();
    for thread_id in &agent_order {
        let times = raw_activity.get(thread_id).cloned().unwrap_or_default();
        let times = if times.is_empty() { vec![0] } else { times };
        let original_start = *times.iter().min().unwrap_or(&0);
        let original_end = *times.iter().max().unwrap_or(&0) + 1;
        let (clipped_start, mut clipped_end) = (original_start, original_end);
        if clipped_end <= clipped_start {
            clipped_end = clipped_start + 1;
        }
        let path = agent_path(
            thread_id,
            &root_thread_id,
            &parent_by_agent,
            &description_by_agent,
            &mut segment_by_agent,
            &mut used_paths,
        );
        let depth = path.matches('/').count() as i64 - 1;
        let source_path = snapshots
            .iter()
            .find(|snapshot| snapshot.thread_id == *thread_id && snapshot.path.ends_with(".jsonl"))
            .map(|snapshot| snapshot.path.clone())
            .unwrap_or_default();
        agents.push(Agent {
            thread_id: thread_id.clone(),
            parent_thread_id: parent_by_agent.get(thread_id).cloned().unwrap_or(None),
            agent_path: path,
            nickname: None,
            role: role_by_agent.get(thread_id).cloned().unwrap_or(None),
            depth,
            started_at_ms: clipped_start,
            ended_at_ms: Some(clipped_end),
            status: if turns_of(thread_id)
                .iter()
                .any(|turn| turn.status == "completed")
            {
                "completed"
            } else {
                "unknown"
            }
            .to_string(),
            source_path,
        });
    }
    let mut filtered_events: Vec<Event> = all_events
        .iter()
        .filter(|event| included.contains(&event.thread_id))
        .cloned()
        .collect();
    filtered_events
        .sort_by(|a, b| (a.timestamp_ms, &a.event_id).cmp(&(b.timestamp_ms, &b.event_id)));
    let mut filtered_tools: Vec<ToolCall> = all_tools
        .iter()
        .filter(|tool| included.contains(&tool.thread_id))
        .cloned()
        .collect();
    filtered_tools
        .sort_by(|a, b| (a.started_at_ms, &a.call_id).cmp(&(b.started_at_ms, &b.call_id)));
    let mut turns: Vec<Turn> = Vec::new();
    for (thread_id, thread_turns) in &turns_by_thread {
        if !included.contains(thread_id) {
            continue;
        }
        turns.extend(thread_turns.iter().cloned());
    }
    turns.sort_by(|a, b| (a.started_at_ms, &a.turn_id).cmp(&(b.started_at_ms, &b.turn_id)));
    let mut edges: Vec<Edge> = Vec::new();
    let mut included_sorted = included.clone();
    included_sorted.sort();
    for agent_id in &included_sorted {
        if *agent_id == root_thread_id {
            continue;
        }
        let parent = parent_by_agent
            .get(agent_id)
            .cloned()
            .flatten()
            .unwrap_or_else(|| root_thread_id.clone());
        let spawn = spawn_by_agent.get(agent_id).copied();
        let raw_at = match spawn {
            Some(spawn) => spawn.started_at_ms,
            None => match raw_activity
                .get(agent_id)
                .and_then(|times| times.iter().min().copied())
            {
                Some(earliest) => earliest,
                None => return Err("min() arg is an empty sequence".to_string()),
            },
        };
        let call_id = spawn
            .map(|held| held.call_id.clone())
            .unwrap_or_else(|| format!("claude-spawn-{agent_id}"));
        let prompt = match spawn {
            Some(spawn) => opt_str(spawn.input_object.get("prompt")),
            None => description_by_agent.get(agent_id).cloned(),
        };
        edges.push(Edge {
            edge_id: format!("claude-spawn-{call_id}"),
            call_id,
            from_thread_id: parent,
            to_thread_id: agent_id.clone(),
            kind: "spawn".to_string(),
            timestamp_ms: raw_at,
            message_text: prompt,
            content_availability: "plain".to_string(),
            encrypted_content: None,
            source_line: spawn.map_or(1, |held| held.source_line),
        });
    }
    for tool in &tool_builders {
        if tool.name != "SendMessage" || !included.contains(&tool.thread_id) {
            continue;
        }
        let recipient = or_str(
            opt_str(tool.input_object.get("recipient")),
            opt_str(tool.input_object.get("to")),
        );
        let Some(recipient) = recipient else {
            continue;
        };
        if !included.contains(&recipient) {
            continue;
        }
        let message = or_str(
            opt_str(tool.input_object.get("message")),
            opt_str(tool.input_object.get("prompt")),
        );
        edges.push(Edge {
            edge_id: format!("claude-message-{}", tool.call_id),
            call_id: tool.call_id.clone(),
            from_thread_id: tool.thread_id.clone(),
            to_thread_id: recipient,
            kind: "followup".to_string(),
            timestamp_ms: tool.started_at_ms,
            message_text: message,
            content_availability: "plain".to_string(),
            encrypted_content: None,
            source_line: tool.source_line,
        });
    }
    if last_extract_empty.0 && last_extract_empty.1 {
        return Err("requested Claude window contains no renderable activity".to_string());
    }
    let mut snapshots_sorted = snapshots.clone();
    snapshots_sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut agents_sorted = agents.clone();
    agents_sorted.sort_by(|a, b| {
        (a.depth, a.started_at_ms, &a.thread_id).cmp(&(b.depth, b.started_at_ms, &b.thread_id))
    });
    let mut edges_sorted = edges.clone();
    edges_sorted.sort_by(|a, b| (a.timestamp_ms, &a.edge_id).cmp(&(b.timestamp_ms, &b.edge_id)));
    Ok(TeamData {
        team_slug: team_slug.to_string(),
        provider: "claude".to_string(),
        root_thread_id,
        display_timezone: display_timezone.to_string(),
        sources: snapshots_sorted,
        agents: agents_sorted,
        turns,
        events: filtered_events,
        tool_calls: filtered_tools,
        edges: edges_sorted,
        task_notes: Vec::new(),
        window_start_ms: None,
        window_end_ms: None,
    })
}

// ---------------------------------------------------------------- snapshot

/// Result of copying a complete Claude lineage.
pub struct ClaudeSnapshotResult {
    /// Source copies, in path order.
    pub sources: Vec<SourceCopy>,
    /// Files changed.
    pub files_changed: i64,
    /// Sources kept from the archive.
    pub upstream_retired: Vec<String>,
}

fn source_thread_id(path: &Path, root_id: &str) -> Result<String, String> {
    let name = path
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name == format!("{root_id}.jsonl") {
        return Ok(root_id.to_string());
    }
    let mut candidate = name.strip_prefix("agent-").unwrap_or(&name).to_string();
    if candidate.ends_with(".meta.json") {
        candidate = candidate
            .strip_suffix(".meta.json")
            .unwrap_or(&candidate)
            .to_string();
    } else if candidate.ends_with(".jsonl") {
        candidate = candidate
            .strip_suffix(".jsonl")
            .unwrap_or(&candidate)
            .to_string();
    }
    validate_thread_id(&candidate, &path.display().to_string())?;
    Ok(candidate)
}

/// Copy the latest append-only Claude lineage before
/// parsing it.
pub fn snapshot_claude_lineage(
    session_file: &Path,
    snapshot_root: &Path,
    previous_sources: &[SourceCopy],
    updated_at: &str,
) -> Result<ClaudeSnapshotResult, String> {
    let source_root = session_file.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut previous_by_path: BTreeMap<String, SourceCopy> = BTreeMap::new();
    for source in previous_sources {
        previous_by_path.insert(source.source_path.clone(), source.clone());
    }
    let root_missing = resolve_source(session_file).is_none();
    let paths: Vec<PathBuf> = if root_missing && !previous_by_path.is_empty() {
        discover_subagent_sources(session_file)?
    } else {
        discover_claude_sources(session_file)?
    };
    let mut relative_to_path: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in paths {
        let relative = path
            .strip_prefix(&source_root)
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .map_err(|_| {
                format!(
                    "Claude source is outside the session root: {}",
                    path.display()
                )
            })?;
        relative_to_path.insert(relative, path);
    }
    let mut changed = 0i64;
    let mut copies: Vec<SourceCopy> = Vec::new();
    let mut retired: Vec<String> = Vec::new();
    let previous_paths: Vec<String> = previous_by_path.keys().cloned().collect();
    for relative in previous_paths {
        if relative_to_path.contains_key(&relative) {
            continue;
        }
        let recorded = previous_by_path[&relative].clone();
        let parts = safe_relative_parts(&relative)?;
        let mut target = snapshot_root.to_path_buf();
        for part in &parts {
            target.push(part);
        }
        let is_symlink = std::fs::symlink_metadata(&target)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !target.is_file() {
            return Err(format!(
                "previously observed Claude source is gone upstream and the archive's own snapshot is missing too: {relative} (expected {})",
                target.display()
            ));
        }
        let held =
            std::fs::read(&target).map_err(|error| crate::json::os_error(&error, &target))?;
        let copied = recorded.copied_bytes as usize;
        if held.len() < copied || sha256_hex(&held[..copied]) != recorded.sha256 {
            return Err(format!(
                "previously observed Claude source is gone upstream and the archive's own snapshot does not match its manifest: {}",
                target.display()
            ));
        }
        retired.push(relative.clone());
        copies.push(SourceCopy {
            upstream_retired_at: Some(
                recorded
                    .upstream_retired_at
                    .clone()
                    .unwrap_or_else(|| updated_at.to_string()),
            ),
            ..recorded
        });
    }
    let root_id = session_file
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ordered: Vec<(String, PathBuf)> = relative_to_path.into_iter().collect();
    for (relative, source) in &ordered {
        safe_relative(relative)?;
        let data = read_source_bytes(source)?;
        let is_jsonl = source
            .file_name()
            .map(|name| name.to_string_lossy().ends_with(".jsonl"))
            .unwrap_or(false);
        let complete: &[u8] = if is_jsonl {
            complete_jsonl(&data)
        } else {
            &data
        };
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
                "Claude snapshot target is a symlink: {}",
                target.display()
            ));
        }
        let existing: Vec<u8> = if target.is_file() {
            std::fs::read(&target).map_err(|error| crate::json::os_error(&error, &target))?
        } else {
            Vec::new()
        };
        let previous = previous_by_path.get(relative);
        if complete.is_empty() {
            if previous.is_some() || !existing.is_empty() {
                return Err(format!(
                    "append-only source violation: Claude transcript was truncated or rewritten: {relative}"
                ));
            }
            return Err(format!(
                "Claude source has no complete JSON: {}",
                source.display()
            ));
        }
        if let Some(previous) = previous {
            if (existing.len() as i64) < previous.copied_bytes {
                return Err(format!(
                    "Claude snapshot is shorter than its manifest: {}",
                    target.display()
                ));
            }
            if sha256_hex(&existing[..previous.copied_bytes as usize]) != previous.sha256 {
                return Err(format!(
                    "Claude snapshot differs from its manifest: {}",
                    target.display()
                ));
            }
        }
        let is_meta = source
            .file_name()
            .map(|name| name.to_string_lossy().ends_with(".meta.json"))
            .unwrap_or(false);
        if is_meta {
            if !existing.is_empty() && existing != complete {
                return Err(format!(
                    "append-only source violation: Claude agent metadata changed: {relative}"
                ));
            }
        } else if complete.len() < existing.len()
            || &complete[..existing.len()] != existing.as_slice()
        {
            return Err(format!(
                "append-only source violation: Claude transcript was truncated or rewritten: {relative}"
            ));
        }
        let text = std::str::from_utf8(complete).map_err(|error| {
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
        let digest = sha256_hex(complete);
        let copy_updated = match previous {
            Some(previous)
                if previous.copied_bytes == complete.len() as i64 && previous.sha256 == digest =>
            {
                previous.updated_at.clone()
            }
            _ => updated_at.to_string(),
        };
        copies.push(SourceCopy {
            source_path: relative.clone(),
            original_path: crate::json::resolve_path(source).display().to_string(),
            snapshot_path: relative.clone(),
            thread_id: source_thread_id(source, &root_id)?,
            copied_bytes: complete.len() as i64,
            line_count: std::cmp::max(
                1,
                complete.iter().filter(|byte| **byte == b'\n').count() as i64,
            ),
            sha256: digest,
            updated_at: copy_updated,
            upstream_retired_at: None,
        });
    }
    copies.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    Ok(ClaudeSnapshotResult {
        sources: copies,
        files_changed: changed,
        upstream_retired: retired,
    })
}
