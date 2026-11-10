//! MCP stdio server and WebSocket event broadcaster for foreign workers.
//!
//! Slice B3-8: newline-delimited JSON-RPC on stdio (`initialize`,
//! `ping`, `tools/list`, `tools/call`) dispatching the `subagent_*`
//! tools onto the lifecycle, plus the local WebSocket stream that
//! republishes lifecycle events and transcript `TURN-DONE` markers.
//! Tool failures surface as `isError` results carrying an
//! `ok: false` payload with a structured error code and message.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha1::{Digest as _, Sha1};

use super::lifecycle::{
    bring_down_agent_at, bring_up_agent_at, event_log_path, last_message_preview_at,
    migrate_agent_at, read_agent_output_at, recreate_window_at, reset_agent_context_at,
    send_message_to_agent_at, status_snapshot_at, transcript_path_at, AgentStatus, DownResult,
    MigrationResult, ReadResult, RecreateWindowResult, ResetResult, SendResult, StatusResult,
    UpResult,
};
use super::ownership::ForeignOwnershipError;
use super::registry::{foreign_base, read_registry_at};

/// Environment variable selecting the WebSocket event-stream port.
pub const WS_PORT_ENV: &str = "SUBAGENTS_MCP_WS_PORT";
/// Default event-stream port (kept away from browser test servers).
pub const DEFAULT_WS_PORT: i64 = 18765;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// A structured tool failure reported as an `isError` tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailure {
    /// Machine-stable failure code.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

impl ToolFailure {
    fn new(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
        }
    }

    fn bad_argument(key: &str, kind: &str) -> Self {
        Self::new("bad_argument", format!("'{key}' must be {kind}"))
    }
}

impl From<ForeignOwnershipError> for ToolFailure {
    fn from(error: ForeignOwnershipError) -> Self {
        Self::new(error.code, error.message)
    }
}

type ToolResult<T> = Result<T, ToolFailure>;

fn req_string(args: &Map<String, Value>, key: &str) -> ToolResult<String> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(ToolFailure::bad_argument(key, "a string")),
    }
}

fn opt_string(args: &Map<String, Value>, key: &str) -> ToolResult<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(ToolFailure::bad_argument(key, "a string")),
    }
}

fn opt_nonempty_string(args: &Map<String, Value>, key: &str) -> ToolResult<Option<String>> {
    Ok(opt_string(args, key)?.filter(|value| !value.is_empty()))
}

fn opt_int(args: &Map<String, Value>, key: &str) -> ToolResult<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| ToolFailure::bad_argument(key, "an integer")),
    }
}

fn opt_bool(args: &Map<String, Value>, key: &str, default: bool) -> ToolResult<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(ToolFailure::bad_argument(key, "a boolean")),
    }
}

fn agent_status_json(agent: &AgentStatus) -> Value {
    json!({
        "name": agent.name,
        "harness": agent.harness,
        "backend": agent.backend,
        "tmux_target": agent.tmux_target,
        "cwd": agent.cwd,
        "model": agent.model,
        "session_id": agent.session_id,
        "status": agent.status,
        "runner_pid": agent.runner_pid,
        "runner_started_at": agent.runner_started_at,
        "next_seq": agent.next_seq,
        "created_at": agent.created_at,
        "last_turn_at": agent.last_turn_at,
        "window_alive": agent.window_alive,
        "runner_alive": agent.runner_alive,
        "presentation_degraded": agent.presentation_degraded,
        "pending": agent.pending,
        "transcript": agent.transcript,
        "last_message_preview": agent.last_message_preview,
        "mode": agent.mode,
        "presentation_pane": agent.presentation_pane,
        "automation_paused": agent.automation_paused,
    })
}

fn status_json(result: &StatusResult) -> Value {
    json!({
        "agents": result.agents.iter().map(agent_status_json).collect::<Vec<_>>(),
        "gc_notes": result.gc_notes,
        "ok": true,
    })
}

/// Dispatch one validated MCP tool call onto the lifecycle at `base`.
pub fn call_tool_at(
    base: &Path,
    proc_root: &Path,
    name: &str,
    args: &Map<String, Value>,
) -> ToolResult<Value> {
    match name {
        "subagent_up" => {
            let result: UpResult = bring_up_agent_at(
                base,
                proc_root,
                &req_string(args, "name")?,
                &req_string(args, "cwd")?,
                Some(&req_string(args, "brief")?),
                opt_string(args, "model")?.as_deref(),
                opt_nonempty_string(args, "harness")?
                    .as_deref()
                    .unwrap_or("codex"),
                opt_string(args, "backend")?.as_deref(),
                opt_string(args, "mode")?.as_deref(),
                &[],
            )?;
            Ok(json!({
                "agent": agent_status_json(&result.agent),
                "queued_turn": result.queued_turn,
                "gc_notes": result.gc_notes,
                "ok": true,
            }))
        }
        "subagent_send" => {
            let result: SendResult = send_message_to_agent_at(
                base,
                proc_root,
                &req_string(args, "name")?,
                &req_string(args, "message")?,
                opt_string(args, "model")?.as_deref(),
            )?;
            Ok(json!({
                "name": result.name,
                "seq": result.seq,
                "model": result.model,
                "transcript": result.transcript,
                "mode": result.mode,
                "presentation_pane": result.presentation_pane,
                "quarantined": result.quarantined,
                "ok": true,
            }))
        }
        "subagent_read" => {
            // Default mode is "all", matching the library and CLI (a
            // "last" default here silently diverged from both).
            let mode = opt_nonempty_string(args, "mode")?.unwrap_or_else(|| "all".to_string());
            let result: ReadResult = read_agent_output_at(
                base,
                &req_string(args, "name")?,
                &mode,
                opt_int(args, "since_turn")?,
                opt_int(args, "tail")?,
            )?;
            Ok(json!({
                "name": result.name,
                "mode": result.mode,
                "text": result.text,
                "ok": true,
            }))
        }
        "subagent_status" => {
            let name = opt_string(args, "name")?;
            Ok(status_json(&status_snapshot_at(
                base,
                proc_root,
                name.as_deref(),
                true,
            )?))
        }
        "subagent_list" => Ok(status_json(&status_snapshot_at(
            base, proc_root, None, true,
        )?)),
        "subagent_down" => {
            let name = req_string(args, "name")?;
            let result: DownResult = bring_down_agent_at(
                base,
                proc_root,
                &name,
                Duration::from_secs(10),
                opt_bool(args, "archive", true)?,
                false,
            )?;
            if !result.was_registered
                && !result.killed_window
                && result.archived_to.is_none()
                && result.state_path.is_none()
            {
                return Err(ToolFailure::new(
                    "unknown_agent",
                    format!("unknown agent '{name}'"),
                ));
            }
            Ok(json!({
                "name": result.name,
                "killed_window": result.killed_window,
                "archived_to": result.archived_to,
                "state_path": result.state_path,
                "was_registered": result.was_registered,
                "forced": result.forced,
                "unverified_presentation": result.unverified_presentation,
                "ok": true,
            }))
        }
        "subagent_reset" => {
            let result: ResetResult = reset_agent_context_at(base, &req_string(args, "name")?)?;
            Ok(json!({
                "name": result.name,
                "previous_session_id": result.previous_session_id,
                "transcript": result.transcript,
                "ok": true,
            }))
        }
        "subagent_recreate_window" => {
            let result: RecreateWindowResult =
                recreate_window_at(base, proc_root, &req_string(args, "name")?)?;
            Ok(json!({
                "name": result.name,
                "mode": result.mode,
                "backend": result.backend,
                "tmux_target": result.tmux_target,
                "pane_id": result.pane_id,
                "window_alive": result.window_alive,
                "ok": true,
            }))
        }
        "subagent_migrate" => {
            let result: MigrationResult = migrate_agent_at(
                base,
                proc_root,
                &req_string(args, "name")?,
                opt_nonempty_string(args, "to_backend")?
                    .as_deref()
                    .unwrap_or("herdr"),
                opt_string(args, "to_mode")?.as_deref(),
            )?;
            Ok(json!({
                "name": result.name,
                "from_backend": result.from_backend,
                "to_backend": result.to_backend,
                "from_mode": result.from_mode,
                "to_mode": result.to_mode,
                "tmux_target": result.tmux_target,
                "session_id": result.session_id,
                "transcript": result.transcript,
                "ok": true,
            }))
        }
        _ => Err(ToolFailure::new(
            "unknown_tool",
            format!("unknown MCP tool '{name}'"),
        )),
    }
}

/// The `tools/list` payload, verbatim from the established schema.
pub fn tool_schemas() -> Value {
    let string = json!({"type": "string"});
    let nullable_string = json!({"type": ["string", "null"]});
    json!([
        {
            "name": "subagent_up",
            "description": "Start a named subagent and queue its first brief. TUI mode is interactive Codex in Herdr.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "cwd", "brief"],
                "properties": {
                    "name": string,
                    "cwd": string,
                    "brief": string,
                    "model": nullable_string,
                    "harness": {"type": "string", "enum": ["codex", "agy"], "default": "codex"},
                    "backend": {"type": ["string", "null"], "enum": ["tmux", "herdr", null]},
                    "mode": {
                        "type": ["string", "null"],
                        "enum": ["headless", "tui", null],
                        "description": "Per-agent override; omitted uses SUBAGENTS_MODE then project_defaults.json."
                    }
                }
            }
        },
        {
            "name": "subagent_send",
            "description": "Queue a plain string message to a running subagent.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "message"],
                "properties": {"name": string, "message": string, "model": nullable_string}
            }
        },
        {
            "name": "subagent_read",
            "description": "Read a subagent transcript or last answer.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": string,
                    "mode": {"type": "string", "enum": ["last", "since_turn", "tail", "all"]},
                    "since_turn": {"type": "integer"},
                    "tail": {"type": "integer"}
                }
            }
        },
        {
            "name": "subagent_status",
            "description": "Show one subagent, or all live subagents when name is omitted.",
            "inputSchema": {"type": "object", "properties": {"name": nullable_string}}
        },
        {
            "name": "subagent_down",
            "description": "Stop a subagent, close its presentation, and optionally archive state.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {"name": string, "archive": {"type": "boolean", "default": true}}
            }
        },
        {
            "name": "subagent_reset",
            "description": "Clear an idle subagent's harness context while keeping its presentation and state.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {"name": string}
            }
        },
        {
            "name": "subagent_recreate_window",
            "description": "Repair presentation for a live runner whose window is missing.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {"name": string}
            }
        },
        {
            "name": "subagent_migrate",
            "description": "Move an idle supported non-Codex agent between presentation backends.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": string,
                    "to_backend": {"type": "string", "enum": ["tmux", "herdr"], "default": "herdr"},
                    "to_mode": {"type": ["string", "null"], "enum": ["headless", "tui", null]}
                }
            }
        },
        {
            "name": "subagent_list",
            "description": "List all live subagents.",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

/// Serialize in the established wire style: sorted keys, `, ` / `: `
/// separators, and ASCII-escaped strings, so a client that compares
/// the `content` text byte-for-byte sees the same payload.
fn py_json_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => {
            let escaped = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
            let mut out = String::with_capacity(escaped.len());
            for ch in escaped.chars() {
                if ch.is_ascii() {
                    out.push(ch);
                } else {
                    let mut buffer = [0_u16; 2];
                    for unit in ch.encode_utf16(&mut buffer) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
            out
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(py_json_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, item)| format!(
                    "{}: {}",
                    py_json_text(&Value::String(key.clone())),
                    py_json_text(item)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn result_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle_request(base: &Path, proc_root: &Path, request: &Value) -> Option<Value> {
    let empty = Map::new();
    let request = request.as_object().unwrap_or(&empty);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(error_response(
            id,
            -32600,
            "JSON-RPC request missing string method".to_string(),
        ));
    };
    if matches!(id, Value::Null) && method.starts_with("notifications/") {
        return None;
    }
    match method {
        "initialize" => Some(result_response(
            id,
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "herdr-foreign-subagents", "version": "0.1.0"},
            }),
        )),
        "ping" => Some(result_response(id, json!({}))),
        "tools/list" => Some(result_response(id, json!({"tools": tool_schemas()}))),
        "tools/call" => Some(tools_call(base, proc_root, id, request)),
        _ => Some(error_response(
            id,
            -32601,
            format!("method not found: {method}"),
        )),
    }
}

fn tools_call(base: &Path, proc_root: &Path, id: Value, request: &Map<String, Value>) -> Value {
    let empty = Map::new();
    let params = request
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let Some(tool_name) = params.get("name").and_then(Value::as_str) else {
        return error_response(
            id,
            -32602,
            "tools/call requires string params.name".to_string(),
        );
    };
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    match call_tool_at(base, proc_root, tool_name, arguments) {
        Ok(payload) => result_response(
            id,
            json!({
                "content": [{"type": "text", "text": py_json_text(&payload)}],
                "structuredContent": payload,
                "isError": false,
            }),
        ),
        Err(failure) => {
            let payload = json!({
                "ok": false,
                "error": {"code": failure.code, "message": failure.message},
            });
            result_response(
                id,
                json!({
                    "content": [{"type": "text", "text": py_json_text(&payload)}],
                    "isError": true,
                }),
            )
        }
    }
}

/// Read the configured WebSocket port, rejecting invalid integer values.
pub fn websocket_port_from_env() -> Result<i64, String> {
    let raw = std::env::var(WS_PORT_ENV).unwrap_or_else(|_| DEFAULT_WS_PORT.to_string());
    raw.parse::<i64>()
        .map_err(|_| format!("{WS_PORT_ENV} must be an integer, got '{raw}'"))
}

/// Serve MCP requests on stdio until end-of-input; also start the
/// WebSocket broadcaster for the configured port (0 disables it).
pub fn run() -> i32 {
    let port = match websocket_port_from_env() {
        Ok(port) => port,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    let base = foreign_base();
    WebSocketBroadcaster::start(base.clone(), port);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return 0,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&base, Path::new("/proc"), &request),
            Err(error) => Some(error_response(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            if writeln!(output, "{}", py_json_text(&response)).is_err() || output.flush().is_err() {
                return 0;
            }
        }
    }
}

fn websocket_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

fn text_frame(payload: &[u8]) -> Vec<u8> {
    let size = payload.len();
    let mut frame = Vec::with_capacity(size + 10);
    frame.push(0x81);
    if size < 126 {
        frame.push(u8::try_from(size).unwrap_or_default());
    } else if size <= 65535 {
        frame.push(126);
        frame.extend_from_slice(&u16::try_from(size).unwrap_or_default().to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(size as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn parse_turn_done(line: &str) -> Option<(i64, String)> {
    let rest = line.strip_prefix("===TURN-DONE")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let mut tokens = rest.split_whitespace();
    let seq_token = tokens.next()?;
    if seq_token.is_empty() || !seq_token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rc = tokens.next()?.strip_prefix("rc=")?;
    if rc.is_empty() {
        return None;
    }
    seq_token
        .parse::<i64>()
        .ok()
        .map(|seq| (seq, rc.to_string()))
}

/// Publishes lifecycle and turn-completion events to local WebSocket
/// clients, with the established deduplication keys.
struct WebSocketBroadcaster {
    clients: Arc<Mutex<Vec<(u64, TcpStream)>>>,
    seen: Arc<Mutex<HashSet<String>>>,
}

static CLIENT_IDS: AtomicU64 = AtomicU64::new(1);

impl WebSocketBroadcaster {
    fn start(base: PathBuf, port: i64) {
        if port <= 0 {
            return;
        }
        let broadcaster = Arc::new(Self {
            clients: Arc::new(Mutex::new(Vec::new())),
            seen: Arc::new(Mutex::new(HashSet::new())),
        });
        std::thread::spawn(move || {
            let listener =
                match TcpListener::bind(("127.0.0.1", u16::try_from(port).unwrap_or_default())) {
                    Ok(listener) => listener,
                    Err(error) => {
                        eprintln!("[subagents-mcp] websocket broadcaster failed: {error}");
                        return;
                    }
                };
            let watcher = broadcaster.clone();
            let watch_base = base.clone();
            std::thread::spawn(move || watcher.watch(watch_base));
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let client = broadcaster.clone();
                std::thread::spawn(move || client.handle_client(stream));
            }
        });
    }

    fn handle_client(&self, stream: TcpStream) {
        let Ok(reader_stream) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(reader_stream);
        let mut headers: HashMap<String, String> = HashMap::new();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((key, value)) = trimmed.split_once(':') {
                        headers.insert(key.to_lowercase(), value.trim().to_string());
                    }
                }
                Err(_) => return,
            }
        }
        let Some(key) = headers.get("sec-websocket-key") else {
            return;
        };
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            websocket_accept(key)
        );
        let mut writer = &stream;
        if writer.write_all(response.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
        let id = CLIENT_IDS.fetch_add(1, Ordering::Relaxed);
        if let Ok(cloned) = stream.try_clone() {
            if let Ok(mut clients) = self.clients.lock() {
                clients.push((id, cloned));
            }
        }
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        if let Ok(mut clients) = self.clients.lock() {
            clients.retain(|(client_id, _)| *client_id != id);
        }
    }

    fn broadcast(&self, event: &Value) {
        let key = dedupe_key(event);
        if let Ok(mut seen) = self.seen.lock() {
            if !seen.insert(key) {
                return;
            }
        } else {
            return;
        }
        let payload = py_json_text(event).into_bytes();
        let frame = text_frame(&payload);
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        let mut dead = Vec::new();
        for (id, stream) in clients.iter_mut() {
            if stream.write_all(&frame).is_err() || stream.flush().is_err() {
                dead.push(*id);
            }
        }
        clients.retain(|(id, _)| !dead.contains(id));
    }

    fn watch(&self, base: PathBuf) {
        let events_path = event_log_path(&base);
        let mut event_offset = std::fs::metadata(&events_path).map_or(0, |meta| meta.len());
        let mut transcript_offsets: HashMap<String, u64> = HashMap::new();
        let mut polls = 0_u64;
        loop {
            polls += 1;
            if let Some(chunk) = read_appended(&events_path, &mut event_offset) {
                for line in chunk.lines() {
                    match serde_json::from_str::<Value>(line) {
                        Ok(event) if event.is_object() => self.broadcast(&event),
                        Ok(_) => {}
                        Err(_) => {
                            eprintln!(
                                "[subagents-mcp] bad event JSON ignored: {}",
                                line.chars().take(160).collect::<String>()
                            );
                        }
                    }
                }
            }
            if polls % 2 == 0 {
                self.watch_transcripts(&base, &mut transcript_offsets);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn watch_transcripts(&self, base: &Path, offsets: &mut HashMap<String, u64>) {
        let Ok(registry) = read_registry_at(base) else {
            return;
        };
        for name in registry.keys() {
            let Ok(path) = transcript_path_at(base, name) else {
                continue;
            };
            if !path.exists() {
                continue;
            }
            let offset = offsets
                .entry(name.clone())
                .or_insert_with(|| std::fs::metadata(&path).map_or(0, |meta| meta.len()));
            let Some(chunk) = read_appended(&path, offset) else {
                continue;
            };
            for line in chunk.lines() {
                if let Some((seq, rc)) = parse_turn_done(line.trim()) {
                    self.broadcast(&json!({
                        "type": "TURN-DONE",
                        "name": name,
                        "seq": seq,
                        "rc": rc,
                        "ts": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z").to_string(),
                        "preview": last_message_preview_at(base, name, 240).unwrap_or_default(),
                    }));
                }
            }
        }
    }
}

fn dedupe_key(event: &Value) -> String {
    if event.get("type").and_then(Value::as_str) == Some("TURN-DONE") {
        let field = |key: &str| match event.get(key) {
            Some(Value::String(value)) => value.clone(),
            Some(other) => other.to_string(),
            None => "None".to_string(),
        };
        return format!(
            "TURN-DONE:{}:{}:{}",
            field("name"),
            field("seq"),
            field("rc")
        );
    }
    match event.get("id") {
        Some(Value::String(value)) => value.clone(),
        Some(other) => other.to_string(),
        None => py_json_text(event),
    }
}

/// Read bytes appended since `offset`, advancing it past the last
/// complete line. Returns the new text, if any.
fn read_appended(path: &Path, offset: &mut u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < *offset {
        *offset = len;
        return None;
    }
    if len == *offset {
        return None;
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let last_newline = bytes.iter().rposition(|byte| *byte == b'\n')?;
    let complete = &bytes[..=last_newline];
    *offset += complete.len() as u64;
    Some(String::from_utf8_lossy(complete).into_owned())
}
