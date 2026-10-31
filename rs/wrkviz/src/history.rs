//! Claude Code prompt-history importer:
//! `py/wrkviz/claude_history.py` at parity. Line decoding
//! lives in `agent_harness::history` (the one transcript
//! library); selection and team assembly live here.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::json::{as_array, as_object, as_string, py_repr_str, read_json};
use crate::team_data::{Agent, Event, SourceSnapshot, TeamData, Turn};
use crate::transcript_io::write_text_if_changed;

/// The provider name for history teams.
pub const PROVIDER: &str = "claude-history";
/// The ingress kind stamped on history events.
pub const INGRESS_KIND: &str = "claude_history";
const CLASSIFICATION_VERSION: &str = "authorship-v1";
/// Session id for prompts whose line carries none.
pub const UNATTRIBUTED_SESSION: &str = "unattributed";
const PLACEHOLDER_TURN_MS: i64 = 1_000;

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

fn safe_relative(value: &str) -> Result<(), String> {
    // PurePosixPath collapses empty and `.` segments; only an
    // absolute path, a `..` segment, or no segments refuse.
    let parts: Vec<&str> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if value.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(format!("unsafe Muse source path {}", py_repr_str(value)));
    }
    Ok(())
}

/// One archive-local copy of a Claude transcript, sidecar,
/// or history file (mirrors `ClaudeSourceCopy`).
#[derive(Clone, Debug)]
pub struct SourceCopy {
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

impl SourceCopy {
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

    /// Validate and decode one source-copy record.
    pub fn from_json(raw: &Value, where_: &str) -> Result<SourceCopy, String> {
        let item = as_object(raw, where_)?;
        let text = |key: &str| -> Result<String, String> {
            match item.get(key) {
                Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
                _ => Err(format!("{where_}.{key}: expected a non-empty string")),
            }
        };
        let count = |key: &str| -> Result<i64, String> {
            match item.get(key) {
                Some(value) => crate::json::as_int(value, &format!("{where_}.{key}")),
                None => Err(format!("{where_}.{key}: expected an integer")),
            }
        };
        // Field reads happen in the Python order, before any
        // validation, so a missing field is reported first.
        let source_path = text("source_path")?;
        let original_path = text("original_path")?;
        let snapshot_path = text("snapshot_path")?;
        let thread_id = text("thread_id")?;
        let copied_bytes = count("copied_bytes")?;
        let line_count = count("line_count")?;
        let sha256 = text("sha256")?;
        let updated_at = text("updated_at")?;
        let upstream_retired_at = match item.get("upstream_retired_at") {
            None | Some(Value::Null) => None,
            Some(_) => Some(text("upstream_retired_at")?),
        };
        safe_relative(&source_path)?;
        if snapshot_path != source_path {
            return Err(format!("{where_}: snapshot_path must equal source_path"));
        }
        if copied_bytes < 0 || line_count < 0 {
            return Err(format!("{where_}: source counts must be non-negative"));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("{where_}: invalid source digest"));
        }
        if !safe_id(&thread_id) {
            return Err(format!(
                "{where_}: unsafe thread id {}",
                py_repr_str(&thread_id)
            ));
        }
        Ok(SourceCopy {
            source_path,
            original_path,
            snapshot_path,
            thread_id,
            copied_bytes,
            line_count,
            sha256,
            updated_at,
            upstream_retired_at,
        })
    }
}

/// Result of copying a complete history file into the store.
pub struct HistorySnapshotResult {
    /// The source copy.
    pub source: SourceCopy,
    /// Files changed.
    pub files_changed: i64,
}

/// What one history ingest kept and left out.
#[derive(Clone, Debug, Default)]
pub struct HistorySelection {
    /// Matched prompts.
    pub matched_prompts: i64,
    /// Covered prompts left out.
    pub covered_prompts: i64,
    /// Covered sessions left out.
    pub covered_sessions: i64,
    /// Unmatched prompts.
    pub unmatched_prompts: i64,
}

struct HistoryPrompt {
    line: i64,
    timestamp_ms: i64,
    session_id: String,
    project: String,
    text: String,
}

fn complete_jsonl(data: &[u8]) -> &[u8] {
    match data.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => &data[..=index],
        None => &[],
    }
}

/// Compile the operator's working-directory selector.
pub fn compile_project_pattern(pattern: &str) -> Result<regex::Regex, String> {
    if pattern.is_empty() {
        return Err("project pattern must not be empty".to_string());
    }
    regex::Regex::new(pattern)
        .map_err(|error| format!("invalid project pattern {}: {error}", py_repr_str(pattern)))
}

fn parse_prompt(raw: &[u8], line: i64, where_: &str) -> Result<HistoryPrompt, String> {
    // Classify invalid JSON vs non-object the way CPython's
    // two-step (decode, then type check) reports it. The
    // wording of the JSON complaint itself is the named
    // CPython-vs-serde gap the differentials normalize.
    if let Err(error) = serde_json::from_slice::<Value>(raw) {
        return Err(format!("{where_}: invalid JSON: {error}"));
    }
    let prompt = agent_harness::parse_history_line(raw).map_err(|error| {
        use agent_harness::HistoryError;
        match error {
            HistoryError::NotAnObject => format!("{where_}: expected a JSON object"),
            HistoryError::Field("display") => {
                format!("{where_}.display: expected a non-empty string")
            }
            HistoryError::Field("timestamp") => {
                format!("{where_}.timestamp: expected a millisecond integer")
            }
            HistoryError::Field("project") => {
                format!("{where_}.project: expected a non-empty string")
            }
            HistoryError::Field("sessionId") => {
                format!("{where_}.sessionId: expected a non-empty string")
            }
            HistoryError::Field(_) => format!("{where_}: expected a JSON object"),
            HistoryError::PastedNotObject(key) => format!(
                "{where_}.pastedContents[{}]: expected an object",
                py_repr_str(&key)
            ),
        }
    })?;
    let session_id = prompt
        .session_id
        .clone()
        .unwrap_or_else(|| UNATTRIBUTED_SESSION.to_string());
    if prompt.session_id.is_some() && !safe_id(&session_id) {
        return Err(format!(
            "{where_}.sessionId: unsafe session id {}",
            py_repr_str(&session_id)
        ));
    }
    Ok(HistoryPrompt {
        line,
        timestamp_ms: prompt.timestamp_ms,
        session_id,
        project: prompt.project.clone(),
        text: prompt.text(),
    })
}

fn read_history_prompts(path: &Path) -> Result<(Vec<HistoryPrompt>, Vec<u8>), String> {
    let data = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    let complete = complete_jsonl(&data);
    if complete.is_empty() {
        return Err(format!(
            "{}: no newline-complete JSON records",
            path.display()
        ));
    }
    let mut prompts = Vec::new();
    let lines: Vec<&[u8]> = complete[..complete.len() - 1]
        .split(|byte| *byte == b'\n')
        .collect();
    for (index, raw) in lines.iter().enumerate() {
        if raw
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
        {
            continue;
        }
        prompts.push(parse_prompt(
            raw,
            index as i64 + 1,
            &format!("{}:{}", path.display(), index + 1),
        )?);
    }
    Ok((prompts, complete.to_vec()))
}

/// Copy the newline-complete prefix of the history file,
/// refusing a rewrite of what was copied.
pub fn snapshot_claude_history(
    history_file: &Path,
    snapshot_root: &Path,
    previous: Option<&SourceCopy>,
    updated_at: &str,
) -> Result<HistorySnapshotResult, String> {
    let relative = history_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if relative.contains('/') || matches!(relative.as_str(), "" | "." | "..") {
        return Err(format!(
            "unsafe history file name {}",
            py_repr_str(&relative)
        ));
    }
    let data =
        std::fs::read(history_file).map_err(|error| crate::json::os_error(&error, history_file))?;
    let complete = complete_jsonl(&data);
    if complete.is_empty() {
        return Err(format!(
            "{}: no newline-complete JSON records",
            history_file.display()
        ));
    }
    let target = snapshot_root.join(&relative);
    if target.is_symlink() {
        return Err(format!(
            "history snapshot target is a symlink: {}",
            target.display()
        ));
    }
    let existing: Vec<u8> = if target.is_file() {
        std::fs::read(&target).map_err(|error| crate::json::os_error(&error, &target))?
    } else {
        Vec::new()
    };
    if let Some(previous) = previous {
        if previous.source_path != relative {
            return Err(format!(
                "append-only source violation: previously observed history file {} is not {}",
                py_repr_str(&previous.source_path),
                py_repr_str(&relative)
            ));
        }
        if (existing.len() as i64) < previous.copied_bytes {
            return Err(format!(
                "history snapshot is shorter than its manifest: {}",
                target.display()
            ));
        }
        if sha256_hex(&existing[..previous.copied_bytes as usize]) != previous.sha256 {
            return Err(format!(
                "history snapshot differs from its manifest: {}",
                target.display()
            ));
        }
    }
    if complete.len() < existing.len() || &complete[..existing.len()] != existing.as_slice() {
        return Err(format!(
            "append-only source violation: history file was truncated or rewritten: {relative}"
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
    let changed = write_text_if_changed(&target, text)?;
    let digest = sha256_hex(complete);
    let copy_updated = match previous {
        Some(previous)
            if previous.copied_bytes == complete.len() as i64 && previous.sha256 == digest =>
        {
            previous.updated_at.clone()
        }
        _ => updated_at.to_string(),
    };
    Ok(HistorySnapshotResult {
        source: SourceCopy {
            source_path: relative.clone(),
            original_path: crate::json::resolve_path(history_file)
                .display()
                .to_string(),
            snapshot_path: relative.clone(),
            thread_id: root_thread_id(&relative),
            copied_bytes: complete.len() as i64,
            line_count: complete.iter().filter(|byte| **byte == b'\n').count() as i64,
            sha256: digest,
            updated_at: copy_updated,
            upstream_retired_at: None,
        },
        files_changed: i64::from(changed),
    })
}

fn root_thread_id(relative: &str) -> String {
    let stem = Path::new(relative)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history".to_string());
    let candidate: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let candidate = if candidate.is_empty() {
        "history".to_string()
    } else {
        candidate
    };
    if safe_id(&candidate) {
        candidate
    } else {
        "history".to_string()
    }
}

/// Normalize the prompts whose working directory matches,
/// one coordinator agent per session.
pub fn load_claude_history_team(
    snapshot_file: &Path,
    team_slug: &str,
    display_timezone: &str,
    project_pattern: &str,
    covered_session_ids: &[String],
) -> Result<(TeamData, HistorySelection), String> {
    let pattern = compile_project_pattern(project_pattern)?;
    let covered: std::collections::BTreeSet<&str> =
        covered_session_ids.iter().map(String::as_str).collect();
    let (prompts, complete) = read_history_prompts(snapshot_file)?;
    let relative = snapshot_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut matched: Vec<HistoryPrompt> = Vec::new();
    let mut selection = HistorySelection::default();
    let mut covered_sessions: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for prompt in prompts {
        if !pattern.is_match(&prompt.project) {
            selection.unmatched_prompts += 1;
            continue;
        }
        if covered.contains(prompt.session_id.as_str()) {
            selection.covered_prompts += 1;
            covered_sessions.insert(prompt.session_id.clone());
            continue;
        }
        matched.push(prompt);
    }
    selection.covered_sessions = covered_sessions.len() as i64;
    selection.matched_prompts = matched.len() as i64;
    if matched.is_empty() {
        let mut message = format!(
            "no history prompt matched project pattern {}",
            py_repr_str(project_pattern)
        );
        if !covered_sessions.is_empty() {
            message.push_str(&format!(
                " outside the {} covered session(s)",
                covered_sessions.len()
            ));
        }
        return Err(message);
    }
    let mut events: Vec<Event> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut by_session: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut session_order: Vec<String> = Vec::new();
    for prompt in &matched {
        if !by_session.contains_key(&prompt.session_id) {
            session_order.push(prompt.session_id.clone());
        }
        by_session
            .entry(prompt.session_id.clone())
            .or_default()
            .push(prompt.timestamp_ms);
        let event_id = format!("{}:history:{}", prompt.session_id, prompt.line);
        events.push(Event {
            event_id: event_id.clone(),
            thread_id: prompt.session_id.clone(),
            turn_id: Some(event_id.clone()),
            timestamp_ms: prompt.timestamp_ms,
            kind: "user_prompt".to_string(),
            role: Some("user".to_string()),
            phase: None,
            text: Some(prompt.text.clone()),
            content_availability: "plain".to_string(),
            encrypted_content: None,
            author: Some("user".to_string()),
            recipient: Some(prompt.session_id.clone()),
            source_line: prompt.line,
            ingress_kind: Some(INGRESS_KIND.to_string()),
            author_kind: Some("owner_human".to_string()),
            source_native_id: Some(format!("{relative}:{}", prompt.line)),
            classification_version: Some(CLASSIFICATION_VERSION.to_string()),
        });
        turns.push(Turn {
            turn_id: event_id,
            thread_id: prompt.session_id.clone(),
            started_at_ms: prompt.timestamp_ms,
            ended_at_ms: Some(prompt.timestamp_ms + PLACEHOLDER_TURN_MS),
            status: "unknown".to_string(),
            first_token_ms: None,
            error: None,
            last_agent_message: None,
        });
    }
    let mut agents: Vec<Agent> = Vec::new();
    let mut ordered_sessions: Vec<(i64, String)> = session_order
        .iter()
        .map(|session| (by_session[session][0], session.clone()))
        .collect();
    ordered_sessions.sort();
    for (_, session_id) in &ordered_sessions {
        let times = &by_session[session_id];
        agents.push(Agent {
            thread_id: session_id.clone(),
            parent_thread_id: None,
            agent_path: format!("/{session_id}"),
            nickname: None,
            role: Some("coordinator".to_string()),
            depth: 0,
            started_at_ms: *times.iter().min().unwrap_or(&0),
            ended_at_ms: times.iter().max().map(|max| max + PLACEHOLDER_TURN_MS),
            status: "unknown".to_string(),
            source_path: relative.clone(),
        });
    }
    turns.sort_by(|a, b| {
        (a.started_at_ms, a.turn_id.as_str()).cmp(&(b.started_at_ms, b.turn_id.as_str()))
    });
    events.sort_by(|a, b| {
        (a.timestamp_ms, a.event_id.as_str()).cmp(&(b.timestamp_ms, b.event_id.as_str()))
    });
    let metadata = std::fs::metadata(snapshot_file)
        .map_err(|error| crate::json::os_error(&error, snapshot_file))?;
    let mtime_ns = {
        use std::os::unix::fs::MetadataExt as _;
        metadata.mtime_nsec() + metadata.mtime() * 1_000_000_000
    };
    let source = SourceSnapshot {
        path: relative.clone(),
        thread_id: agents[0].thread_id.clone(),
        size_bytes: metadata.len() as i64,
        mtime_ns,
        sha256: sha256_hex(&complete),
        complete_bytes: complete.len() as i64,
        line_count: complete.iter().filter(|byte| **byte == b'\n').count() as i64,
        working_directory: None,
        repository_url: None,
        semantic_sha256: None,
        semantic_complete_bytes: None,
    };
    let team = TeamData {
        team_slug: team_slug.to_string(),
        provider: PROVIDER.to_string(),
        root_thread_id: agents[0].thread_id.clone(),
        display_timezone: display_timezone.to_string(),
        sources: vec![source],
        agents,
        turns,
        events,
        tool_calls: Vec::new(),
        edges: Vec::new(),
        task_notes: Vec::new(),
        window_start_ms: None,
        window_end_ms: None,
    };
    Ok((team, selection))
}

/// The previous history source manifest's one source, if any.
pub fn load_history_source_manifest(
    path: &Path,
    team_slug: &str,
    project_pattern: &str,
) -> Result<Option<SourceCopy>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let document = read_json(path)?;
    let object = as_object(&document, &path.display().to_string())?;
    if !crate::json::json_eq_one(object.get("schema_version"))
        || object.get("provider").and_then(Value::as_str) != Some(PROVIDER)
    {
        return Err(format!(
            "invalid Claude history source manifest at {}",
            path.display()
        ));
    }
    let recorded_pattern = as_string(
        object.get("project_pattern").unwrap_or(&Value::Null),
        &format!("{}: project_pattern", path.display()),
    )?;
    if recorded_pattern != project_pattern {
        return Err(format!(
            "team {} was ingested with project pattern {}, not {}; register a different team slug for a different selection",
            py_repr_str(team_slug),
            py_repr_str(recorded_pattern),
            py_repr_str(project_pattern)
        ));
    }
    let raw_sources = as_array(
        object.get("sources").unwrap_or(&Value::Null),
        &format!("{}: sources", path.display()),
    )?;
    if raw_sources.len() != 1 {
        return Err(format!(
            "{}: expected exactly one history source",
            path.display()
        ));
    }
    Ok(Some(SourceCopy::from_json(
        &raw_sources[0],
        &format!("{}: sources[0]", path.display()),
    )?))
}
