//! The normalized team snapshot (`raw/team.json`): the Rust
//! reader for the format `py/wrkviz/model.py` defines and
//! `py/wrkviz/model_io.py` strictly decodes. Field-by-field
//! parity with that decoder, including its error spellings,
//! because a load failure here becomes a skipped team's
//! reported cause.

use serde_json::Value;

use crate::json::{as_array, as_object};

fn int_of(value: Option<&Value>, where_: &str) -> Result<i64, String> {
    match value {
        Some(Value::Number(number)) if number.is_i64() || number.is_u64() => number
            .as_i64()
            .ok_or_else(|| format!("{where_}: expected an integer")),
        _ => Err(format!("{where_}: expected an integer")),
    }
}

fn opt_int(value: Option<&Value>, where_: &str) -> Result<Option<i64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        other => int_of(other, where_).map(Some),
    }
}

fn string_of(value: Option<&Value>, where_: &str) -> Result<String, String> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(format!("{where_}: expected a string")),
    }
}

fn opt_string(value: Option<&Value>, where_: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        _ => Err(format!("{where_}: expected a string")),
    }
}

fn items<'a>(root: &'a Value, key: &str) -> Result<Vec<&'a Value>, String> {
    let root = as_object(root, "team")?;
    match root.get(key) {
        Some(value) => Ok(as_array(value, &format!("team.{key}"))?.iter().collect()),
        None => Err(format!("team.{key}: expected an array")),
    }
}

fn item_object<'a>(
    value: &'a Value,
    key: &str,
    index: usize,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    as_object(value, &format!("team.{key}[{index}]"))
}

/// One source snapshot record.
#[derive(Clone, Debug, Default)]
pub struct SourceSnapshot {
    /// Source path.
    pub path: String,
    /// Thread id.
    pub thread_id: String,
    /// File size.
    pub size_bytes: i64,
    /// Mtime in ns.
    pub mtime_ns: i64,
    /// Content digest.
    pub sha256: String,
    /// Complete bytes.
    pub complete_bytes: i64,
    /// Line count.
    pub line_count: i64,
    /// Working directory.
    pub working_directory: Option<String>,
    /// Repository URL.
    pub repository_url: Option<String>,
    /// Semantic digest.
    pub semantic_sha256: Option<String>,
    /// Semantic complete bytes.
    pub semantic_complete_bytes: Option<i64>,
}

/// One agent record.
#[derive(Clone, Debug, Default)]
pub struct Agent {
    /// Thread id.
    pub thread_id: String,
    /// Parent thread id.
    pub parent_thread_id: Option<String>,
    /// Agent path.
    pub agent_path: String,
    /// Nickname.
    pub nickname: Option<String>,
    /// Role.
    pub role: Option<String>,
    /// Depth.
    pub depth: i64,
    /// Started at ms.
    pub started_at_ms: i64,
    /// Ended at ms.
    pub ended_at_ms: Option<i64>,
    /// Status.
    pub status: String,
    /// Source path.
    pub source_path: String,
}

/// One event record.
#[derive(Clone, Debug, Default)]
pub struct Event {
    /// Event id.
    pub event_id: String,
    /// Thread id.
    pub thread_id: String,
    /// Turn id.
    pub turn_id: Option<String>,
    /// Timestamp ms.
    pub timestamp_ms: i64,
    /// Kind.
    pub kind: String,
    /// Role.
    pub role: Option<String>,
    /// Phase.
    pub phase: Option<String>,
    /// Text.
    pub text: Option<String>,
    /// Content availability.
    pub content_availability: String,
    /// Encrypted content.
    pub encrypted_content: Option<String>,
    /// Author.
    pub author: Option<String>,
    /// Recipient.
    pub recipient: Option<String>,
    /// Source line.
    pub source_line: i64,
    /// Ingress kind.
    pub ingress_kind: Option<String>,
    /// Author kind.
    pub author_kind: Option<String>,
    /// Source native id.
    pub source_native_id: Option<String>,
    /// Classification version.
    pub classification_version: Option<String>,
}

/// One turn record.
#[derive(Clone, Debug, Default)]
pub struct Turn {
    /// Turn id.
    pub turn_id: String,
    /// Thread id.
    pub thread_id: String,
    /// Started at ms.
    pub started_at_ms: i64,
    /// Ended at ms.
    pub ended_at_ms: Option<i64>,
    /// Status.
    pub status: String,
    /// First token ms.
    pub first_token_ms: Option<i64>,
    /// Error.
    pub error: Option<String>,
    /// Last agent message.
    pub last_agent_message: Option<String>,
}

/// A tool payload reference.
#[derive(Clone, Debug, Default)]
pub struct PayloadRef {
    /// Digest.
    pub sha256: String,
    /// Byte length.
    pub byte_length: i64,
}

fn payload_ref(value: Option<&Value>, where_: &str) -> Result<Option<PayloadRef>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let item = as_object(value, where_)?;
    if item.len() != 2 || !item.contains_key("sha256") || !item.contains_key("byte_length") {
        let mut keys: Vec<&str> = item.keys().map(String::as_str).collect();
        keys.sort();
        let keys = Value::Array(
            keys.iter()
                .map(|key| Value::String(key.to_string()))
                .collect(),
        );
        return Err(format!(
            "{where_}: invalid payload reference fields: {}",
            crate::json::py_repr(&keys)
        ));
    }
    let byte_length = int_of(item.get("byte_length"), &format!("{where_}.byte_length"))?;
    if byte_length < 0 {
        return Err(format!(
            "{where_}.byte_length: expected a non-negative integer"
        ));
    }
    Ok(Some(PayloadRef {
        sha256: string_of(item.get("sha256"), &format!("{where_}.sha256"))?,
        byte_length,
    }))
}

/// One tool call record.
#[derive(Clone, Debug, Default)]
pub struct ToolCall {
    /// Call id.
    pub call_id: String,
    /// Item id.
    pub item_id: Option<String>,
    /// Thread id.
    pub thread_id: String,
    /// Turn id.
    pub turn_id: Option<String>,
    /// Name.
    pub name: String,
    /// Namespace.
    pub namespace: Option<String>,
    /// Started at ms.
    pub started_at_ms: i64,
    /// Ended at ms.
    pub ended_at_ms: Option<i64>,
    /// Status.
    pub status: String,
    /// Input text.
    pub input_text: Option<String>,
    /// Output text.
    pub output_text: Option<String>,
    /// Nested tools.
    pub nested_tools: Vec<(String, i64)>,
    /// Source line.
    pub source_line: i64,
    /// Input payload.
    pub input_payload: Option<PayloadRef>,
    /// Output payload.
    pub output_payload: Option<PayloadRef>,
}

/// One edge record.
#[derive(Clone, Debug, Default)]
pub struct Edge {
    /// Edge id.
    pub edge_id: String,
    /// Call id.
    pub call_id: String,
    /// From thread id.
    pub from_thread_id: String,
    /// To thread id.
    pub to_thread_id: String,
    /// Kind.
    pub kind: String,
    /// Timestamp ms.
    pub timestamp_ms: i64,
    /// Message text.
    pub message_text: Option<String>,
    /// Content availability.
    pub content_availability: String,
    /// Encrypted content.
    pub encrypted_content: Option<String>,
    /// Source line.
    pub source_line: i64,
}

/// One promoted task note.
#[derive(Clone, Debug, Default)]
pub struct TaskNote {
    /// Note id.
    pub note_id: i64,
    /// Source path.
    pub source_path: String,
    /// Task source ordinal.
    pub task_source_ordinal: i64,
    /// Task id.
    pub task_id: String,
    /// Title.
    pub title: String,
    /// Content.
    pub content: String,
    /// Created at.
    pub created_at: String,
    /// Server author.
    pub server_author: Option<String>,
    /// Task owner.
    pub task_owner: Option<String>,
    /// Upstream present.
    pub upstream_present: bool,
    /// Projection policy.
    pub projection_policy: Option<String>,
    /// Projection digest.
    pub projection_sha256: Option<String>,
}

/// The identity a promoted note is merged and ordered on.
pub fn task_note_key(note: &TaskNote) -> (String, i64) {
    (note.source_path.clone(), note.note_id)
}

/// The complete normalized team snapshot.
#[derive(Clone, Debug, Default)]
pub struct TeamData {
    /// Team slug.
    pub team_slug: String,
    /// Provider.
    pub provider: String,
    /// Root thread id.
    pub root_thread_id: String,
    /// Display timezone.
    pub display_timezone: String,
    /// Sources.
    pub sources: Vec<SourceSnapshot>,
    /// Agents.
    pub agents: Vec<Agent>,
    /// Turns.
    pub turns: Vec<Turn>,
    /// Events.
    pub events: Vec<Event>,
    /// Tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// Edges.
    pub edges: Vec<Edge>,
    /// Task notes.
    pub task_notes: Vec<TaskNote>,
    /// Window start ms.
    pub window_start_ms: Option<i64>,
    /// Window end ms.
    pub window_end_ms: Option<i64>,
}

/// Reconstruct the team written by `TeamData.to_json_obj`.
pub fn team_from_json(value: &Value) -> Result<TeamData, String> {
    let root = as_object(value, "team")?;
    let mut sources = Vec::new();
    for (index, item) in items(value, "sources")?.into_iter().enumerate() {
        let item = item_object(item, "sources", index)?;
        sources.push(SourceSnapshot {
            path: string_of(item.get("path"), "source.path")?,
            thread_id: string_of(item.get("thread_id"), "source.thread_id")?,
            size_bytes: int_of(item.get("size_bytes"), "source.size_bytes")?,
            mtime_ns: int_of(item.get("mtime_ns"), "source.mtime_ns")?,
            sha256: string_of(item.get("sha256"), "source.sha256")?,
            complete_bytes: int_of(item.get("complete_bytes"), "source.complete_bytes")?,
            line_count: int_of(item.get("line_count"), "source.line_count")?,
            working_directory: opt_string(
                item.get("working_directory"),
                "source.working_directory",
            )?,
            repository_url: opt_string(item.get("repository_url"), "source.repository_url")?,
            semantic_sha256: opt_string(item.get("semantic_sha256"), "source.semantic_sha256")?,
            semantic_complete_bytes: opt_int(
                item.get("semantic_complete_bytes"),
                "source.semantic_complete_bytes",
            )?,
        });
    }
    let mut agents = Vec::new();
    for (index, item) in items(value, "agents")?.into_iter().enumerate() {
        let item = item_object(item, "agents", index)?;
        agents.push(Agent {
            thread_id: string_of(item.get("thread_id"), "agent.thread_id")?,
            parent_thread_id: opt_string(item.get("parent_thread_id"), "agent.parent_thread_id")?,
            agent_path: string_of(item.get("agent_path"), "agent.agent_path")?,
            nickname: opt_string(item.get("nickname"), "agent.nickname")?,
            role: opt_string(item.get("role"), "agent.role")?,
            depth: int_of(item.get("depth"), "agent.depth")?,
            started_at_ms: int_of(item.get("started_at_ms"), "agent.started_at_ms")?,
            ended_at_ms: opt_int(item.get("ended_at_ms"), "agent.ended_at_ms")?,
            status: string_of(item.get("status"), "agent.status")?,
            source_path: string_of(item.get("source_path"), "agent.source_path")?,
        });
    }
    let mut turns = Vec::new();
    for (index, item) in items(value, "turns")?.into_iter().enumerate() {
        let item = item_object(item, "turns", index)?;
        turns.push(Turn {
            turn_id: string_of(item.get("turn_id"), "turn.turn_id")?,
            thread_id: string_of(item.get("thread_id"), "turn.thread_id")?,
            started_at_ms: int_of(item.get("started_at_ms"), "turn.started_at_ms")?,
            ended_at_ms: opt_int(item.get("ended_at_ms"), "turn.ended_at_ms")?,
            status: string_of(item.get("status"), "turn.status")?,
            first_token_ms: opt_int(item.get("first_token_ms"), "turn.first_token_ms")?,
            error: opt_string(item.get("error"), "turn.error")?,
            last_agent_message: opt_string(
                item.get("last_agent_message"),
                "turn.last_agent_message",
            )?,
        });
    }
    let mut tool_calls = Vec::new();
    for (index, item) in items(value, "tool_calls")?.into_iter().enumerate() {
        let item = item_object(item, "tool_calls", index)?;
        let mut nested = Vec::new();
        let raw_nested = match item.get("nested_tools") {
            Some(value) => as_array(value, "tool.nested_tools")?,
            None => return Err("tool.nested_tools: expected an array".to_string()),
        };
        for entry in raw_nested {
            let entry = as_object(entry, "tool.nested_tools[]")?;
            nested.push((
                string_of(entry.get("name"), "tool.nested_tools.name")?,
                int_of(entry.get("count"), "tool.nested_tools.count")?,
            ));
        }
        tool_calls.push(ToolCall {
            call_id: string_of(item.get("call_id"), "tool.call_id")?,
            item_id: opt_string(item.get("item_id"), "tool.item_id")?,
            thread_id: string_of(item.get("thread_id"), "tool.thread_id")?,
            turn_id: opt_string(item.get("turn_id"), "tool.turn_id")?,
            name: string_of(item.get("name"), "tool.name")?,
            namespace: opt_string(item.get("namespace"), "tool.namespace")?,
            started_at_ms: int_of(item.get("started_at_ms"), "tool.started_at_ms")?,
            ended_at_ms: opt_int(item.get("ended_at_ms"), "tool.ended_at_ms")?,
            status: string_of(item.get("status"), "tool.status")?,
            input_text: opt_string(item.get("input_text"), "tool.input_text")?,
            output_text: opt_string(item.get("output_text"), "tool.output_text")?,
            nested_tools: nested,
            source_line: int_of(item.get("source_line"), "tool.source_line")?,
            input_payload: payload_ref(item.get("input_payload"), "tool.input_payload")?,
            output_payload: payload_ref(item.get("output_payload"), "tool.output_payload")?,
        });
    }
    let mut edges = Vec::new();
    for (index, item) in items(value, "edges")?.into_iter().enumerate() {
        let item = item_object(item, "edges", index)?;
        edges.push(Edge {
            edge_id: string_of(item.get("edge_id"), "edge.edge_id")?,
            call_id: string_of(item.get("call_id"), "edge.call_id")?,
            from_thread_id: string_of(item.get("from_thread_id"), "edge.from_thread_id")?,
            to_thread_id: string_of(item.get("to_thread_id"), "edge.to_thread_id")?,
            kind: string_of(item.get("kind"), "edge.kind")?,
            timestamp_ms: int_of(item.get("timestamp_ms"), "edge.timestamp_ms")?,
            message_text: opt_string(item.get("message_text"), "edge.message_text")?,
            content_availability: string_of(
                item.get("content_availability"),
                "edge.content_availability",
            )?,
            encrypted_content: opt_string(item.get("encrypted_content"), "edge.encrypted_content")?,
            source_line: int_of(item.get("source_line"), "edge.source_line")?,
        });
    }
    let mut events = Vec::new();
    for (index, item) in items(value, "events")?.into_iter().enumerate() {
        let item = item_object(item, "events", index)?;
        events.push(Event {
            event_id: string_of(item.get("event_id"), "event.event_id")?,
            thread_id: string_of(item.get("thread_id"), "event.thread_id")?,
            turn_id: opt_string(item.get("turn_id"), "event.turn_id")?,
            timestamp_ms: int_of(item.get("timestamp_ms"), "event.timestamp_ms")?,
            kind: string_of(item.get("kind"), "event.kind")?,
            role: opt_string(item.get("role"), "event.role")?,
            phase: opt_string(item.get("phase"), "event.phase")?,
            text: opt_string(item.get("text"), "event.text")?,
            content_availability: string_of(
                item.get("content_availability"),
                "event.content_availability",
            )?,
            encrypted_content: opt_string(
                item.get("encrypted_content"),
                "event.encrypted_content",
            )?,
            author: opt_string(item.get("author"), "event.author")?,
            recipient: opt_string(item.get("recipient"), "event.recipient")?,
            source_line: int_of(item.get("source_line"), "event.source_line")?,
            ingress_kind: opt_string(item.get("ingress_kind"), "event.ingress_kind")?,
            author_kind: opt_string(item.get("author_kind"), "event.author_kind")?,
            source_native_id: opt_string(item.get("source_native_id"), "event.source_native_id")?,
            classification_version: opt_string(
                item.get("classification_version"),
                "event.classification_version",
            )?,
        });
    }
    let mut task_notes = Vec::new();
    if let Some(raw) = root.get("task_notes") {
        for (index, item) in as_array(raw, "team.task_notes")?.iter().enumerate() {
            task_notes.push(task_note_from_json(
                item,
                &format!("team.task_notes[{index}]"),
            )?);
        }
    }
    Ok(TeamData {
        team_slug: string_of(root.get("team_slug"), "team.team_slug")?,
        provider: string_of(root.get("provider"), "team.provider")?,
        root_thread_id: string_of(root.get("root_thread_id"), "team.root_thread_id")?,
        display_timezone: string_of(root.get("display_timezone"), "team.display_timezone")?,
        sources,
        agents,
        turns,
        events,
        tool_calls,
        edges,
        task_notes,
        window_start_ms: opt_int(root.get("window_start_ms"), "team.window_start_ms")?,
        window_end_ms: opt_int(root.get("window_end_ms"), "team.window_end_ms")?,
    })
}

/// The compatibility-shaped cache digest for normalized source prefixes.
pub fn source_digest(team: &TeamData) -> String {
    use sha2::Digest as _;
    let snapshots: Vec<Value> = team
        .sources
        .iter()
        .map(|source| {
            let mut entry = serde_json::Map::new();
            entry.insert(
                "thread_id".to_string(),
                Value::String(source.thread_id.clone()),
            );
            entry.insert(
                "complete_bytes".to_string(),
                Value::Number(
                    source
                        .semantic_complete_bytes
                        .unwrap_or(source.complete_bytes)
                        .into(),
                ),
            );
            entry.insert(
                "sha256".to_string(),
                Value::String(
                    source
                        .semantic_sha256
                        .clone()
                        .unwrap_or_else(|| source.sha256.clone()),
                ),
            );
            Value::Object(entry)
        })
        .collect();
    let material = crate::json::dump_canonical(&Value::Array(snapshots));
    let digest = sha2::Sha256::digest(material.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Strictly decode one promoted task note.
pub fn task_note_from_json(value: &Value, where_: &str) -> Result<TaskNote, String> {
    let item = as_object(value, where_)?;
    const EXPECTED: [&str; 12] = [
        "note_id",
        "source_path",
        "task_source_ordinal",
        "task_id",
        "title",
        "content",
        "created_at",
        "server_author",
        "task_owner",
        "upstream_present",
        "projection_policy",
        "projection_sha256",
    ];
    let keys: Vec<&str> = item.keys().map(String::as_str).collect();
    if keys.len() != EXPECTED.len() || !EXPECTED.iter().all(|key| item.contains_key(*key)) {
        let mut missing: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|key| !item.contains_key(*key))
            .collect();
        let mut unknown: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|key| !EXPECTED.contains(key))
            .collect();
        missing.sort();
        unknown.sort();
        let missing = Value::Array(
            missing
                .iter()
                .map(|key| Value::String(key.to_string()))
                .collect(),
        );
        let unknown = Value::Array(
            unknown
                .iter()
                .map(|key| Value::String(key.to_string()))
                .collect(),
        );
        return Err(format!(
            "{where_}: invalid task note fields: missing={}, unknown={}",
            crate::json::py_repr(&missing),
            crate::json::py_repr(&unknown)
        ));
    }
    let upstream_present = match item.get("upstream_present") {
        Some(Value::Bool(value)) => *value,
        _ => return Err(format!("{where_}.upstream_present: expected a boolean")),
    };
    let note_id = int_of(item.get("note_id"), &format!("{where_}.note_id"))?;
    if note_id < 0 {
        return Err(format!("{where_}.note_id: expected a non-negative integer"));
    }
    let ordinal = int_of(
        item.get("task_source_ordinal"),
        &format!("{where_}.task_source_ordinal"),
    )?;
    if ordinal < 0 {
        return Err(format!(
            "{where_}.task_source_ordinal: expected a non-negative integer"
        ));
    }
    Ok(TaskNote {
        note_id,
        source_path: string_of(item.get("source_path"), &format!("{where_}.source_path"))?,
        task_source_ordinal: ordinal,
        task_id: string_of(item.get("task_id"), &format!("{where_}.task_id"))?,
        title: string_of(item.get("title"), &format!("{where_}.title"))?,
        content: string_of(item.get("content"), &format!("{where_}.content"))?,
        created_at: string_of(item.get("created_at"), &format!("{where_}.created_at"))?,
        server_author: opt_string(
            item.get("server_author"),
            &format!("{where_}.server_author"),
        )?,
        task_owner: opt_string(item.get("task_owner"), &format!("{where_}.task_owner"))?,
        upstream_present,
        projection_policy: opt_string(
            item.get("projection_policy"),
            &format!("{where_}.projection_policy"),
        )?,
        projection_sha256: opt_string(
            item.get("projection_sha256"),
            &format!("{where_}.projection_sha256"),
        )?,
    })
}
