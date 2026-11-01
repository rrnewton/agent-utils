//! `wrkviz discover-agents`: rebuild a project's agent tree.
//!
//! Ports `py/wrkviz/discover.py` at differential parity: native
//! links (Claude subagent files, Codex `thread_spawn`, Orc
//! `parent_id`), manager launches (`agentctl ... start NAME` in a
//! log, matched to the registry record and from there to the
//! child's own log), restart links from queued prompts, shell-exec
//! links, and the orphan sweep by working directory. Reads only.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};

use crate::compressed::{
    is_compressed, iter_logical_glob_agents, iter_logical_glob_rollouts, read_first_line,
    read_source_bytes, resolve_source,
};

/// A launch command and the registry record it created are
/// seconds apart (oracle `LAUNCH_WINDOW_S`).
const LAUNCH_WINDOW_S: f64 = 900.0;
/// A registry record and its harness's first log record are
/// normally one or two seconds apart (`START_WINDOW_S`).
const START_WINDOW_S: f64 = 300.0;
/// A directly-run harness log starts within this window of the
/// command (`EXEC_WINDOW_S`).
const EXEC_WINDOW_S: f64 = 120.0;
/// Records read from the head of a log (`HEAD_RECORDS`).
const HEAD_RECORDS: usize = 200;

const WS: &str = r"[\t\n\x0b\f\r ]";

fn launch_re() -> regex::bytes::Regex {
    regex::bytes::Regex::new(&format!(
        r#"agentctl(?:[^{WS}"\\]*)(?:{WS}+--?[A-Za-z][0-9A-Za-z_-]*(?:[ =][^{WS}"\\-][^{WS}"\\]*)?)*{WS}+start{WS}+([A-Za-z0-9][A-Za-z0-9._-]*)"#
    ))
    .expect("launch pattern")
}

fn variable_launch_re() -> regex::bytes::Regex {
    regex::bytes::Regex::new(&format!(
        r"\$\{{?[A-Za-z_][A-Za-z0-9_]*\}}?{WS}+start{WS}+([A-Za-z0-9][A-Za-z0-9._-]*)"
    ))
    .expect("variable launch pattern")
}

fn exec_re() -> regex::bytes::Regex {
    regex::bytes::Regex::new(&format!(
        r"\b(?:codex{WS}+exec|claude{WS}+(?:-p|--print)|muse{WS}+(?:exec|run|-p))\b"
    ))
    .expect("exec pattern")
}

fn addressed_re() -> regex::Regex {
    regex::Regex::new(
        r"(?i)^\s*(?:→\s*|[A-Za-z0-9][\w.-]*\s*(?:->|→)\s*|you are\s+(?:lane\s+)?)`?([A-Za-z0-9][\w.-]*[A-Za-z0-9])`?",
    )
    .expect("addressed pattern")
}

fn provider_by_harness(harness: &str) -> Option<&'static str> {
    match harness {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "muse" => Some("muse"),
        _ => None,
    }
}

/// One agent log: a coordinator, a lane, or a native subagent.
#[derive(Clone)]
pub struct Session {
    /// Harness name (`claude`, `codex`, `orc`, `muse`, `claude-subagent`).
    pub provider: String,
    /// The harness's own session identity.
    pub session_id: String,
    /// Host label the session was found under.
    pub host: String,
    /// The session's log path, when it has one.
    pub path: Option<PathBuf>,
    /// Working directory the session recorded.
    pub cwd: Option<String>,
    /// Start instant (epoch seconds), when recorded.
    pub started_at: Option<f64>,
    /// Registered agent name, when matched.
    pub name: Option<String>,
    /// The name as a raw JSON value, when a source recorded a
    /// non-string name (the Orc index); JSON rows emit this.
    pub name_raw: Option<Value>,
    /// Whether the recorded name is falsy as a Python value
    /// (`0`, `0.0`, `""`, `b""` in the Orc index), so a later
    /// `name or other` assignment in the oracle replaces it.
    pub name_falsy: bool,
    /// Parent session key, once linked.
    pub parent: Option<String>,
    /// How this session was linked to its parent.
    pub link: Option<String>,
    /// Parent session key recorded by the harness itself.
    pub native_parent: Option<String>,
    /// Child session keys, in link order.
    pub children: Vec<String>,
}

impl Session {
    fn new(
        provider: &str,
        session_id: String,
        host: &str,
        path: Option<PathBuf>,
        cwd: Option<String>,
        started_at: Option<f64>,
    ) -> Self {
        Session {
            provider: provider.to_string(),
            session_id,
            host: host.to_string(),
            path,
            cwd,
            started_at,
            name: None,
            name_raw: None,
            name_falsy: false,
            parent: None,
            link: None,
            native_parent: None,
            children: Vec::new(),
        }
    }

    /// `provider:session-id`, the identity in the tree.
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.session_id)
    }
}

/// One agent-manager registry record, live or archived.
#[derive(Clone)]
pub struct RegistryAgent {
    /// Host label the record was found under.
    pub host: String,
    /// Registered agent name.
    pub name: String,
    /// Harness the manager recorded for the agent.
    pub harness: String,
    /// Working directory the record names, when any.
    pub cwd: Option<String>,
    /// When the record was created (epoch seconds).
    pub created_at: f64,
    /// Native session id the record names, when any.
    pub native_session: Option<String>,
    /// Creator agent name the record names, when any.
    pub created_by: Option<String>,
}

/// Where to look, per host label.
#[derive(Default)]
pub struct Sources {
    /// `(host, Claude projects dir)` sources.
    pub claude_projects: Vec<(String, PathBuf)>,
    /// `(host, Codex sessions dir)` sources.
    pub codex_sessions: Vec<(String, PathBuf)>,
    /// `(host, Orc root dir)` sources.
    pub orc_roots: Vec<(String, PathBuf)>,
    /// `(host, Muse sessions dir)` sources.
    pub muse_sessions: Vec<(String, PathBuf)>,
    /// `(host, agent-manager registry dir)` sources.
    pub registries: Vec<(String, PathBuf)>,
}

/// The tree, the orphans, and what could not be resolved.
pub struct Discovery {
    /// Every session found, in scan order.
    pub sessions: Vec<Session>,
    /// Session key to position in `sessions`.
    pub index: HashMap<String, usize>,
    /// The requested coordinator session keys.
    pub roots: Vec<String>,
    /// Unreached sessions under a `--cwd-prefix`.
    pub orphans: Vec<String>,
    /// Launches of a registered name with no resolvable child.
    pub unresolved_launches: Vec<Value>,
    /// Launch-like text naming no registered agent.
    pub ignored_mentions: i64,
}

impl Discovery {
    fn session(&self, key: &str) -> &Session {
        &self.sessions[self.index[key]]
    }

    /// Every session below *key*, breadth first (oracle
    /// `descendants`, with its list-scan dedup).
    pub fn descendants(&self, key: &str) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut queue: VecDeque<String> = self.session(key).children.iter().cloned().collect();
        while let Some(child) = queue.pop_front() {
            if seen.contains(&child) {
                continue;
            }
            seen.push(child.clone());
            queue.extend(self.session(&child).children.iter().cloned());
        }
        seen
    }

    fn row(&self, key: &str) -> Value {
        let session = self.session(key);
        json!({
            "key": key,
            "provider": session.provider,
            "session_id": session.session_id,
            "host": session.host,
            "name": match &session.name_raw {
                Some(raw) => raw.clone(),
                None => session.name.clone().map_or(Value::Null, Value::String),
            },
            "cwd": session.cwd,
            "started_at": session.started_at.map(json_float),
            "parent": session.parent,
            "link": session.link,
            "path": session.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        })
    }

    /// The full report object (oracle `to_json_obj`).
    pub fn to_json_obj(&self) -> Value {
        let mut roots = Vec::new();
        for root in &self.roots {
            let mut row = self.row(root);
            let descendants = self.descendants(root);
            let obj = row.as_object_mut().expect("row object");
            obj.insert(
                "direct_children".into(),
                json!(self.session(root).children.len()),
            );
            obj.insert("descendants".into(), json!(descendants.len()));
            obj.insert("by_link".into(), count_links(self, &descendants));
            roots.push(row);
        }
        let mut tree = Vec::new();
        for root in &self.roots {
            for key in self.descendants(root) {
                tree.push(self.row(&key));
            }
        }
        json!({
            "schema_version": 1,
            "roots": roots,
            "tree": tree,
            "orphans": self.orphans.iter().map(|key| self.row(key)).collect::<Vec<_>>(),
            "unresolved_launches": self.unresolved_launches,
            "ignored_mentions": self.ignored_mentions,
        })
    }
}

fn count_links(discovery: &Discovery, keys: &[String]) -> Value {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for key in keys {
        let link = discovery
            .session(key)
            .link
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        *counts.entry(link).or_insert(0) += 1;
    }
    let mut obj = Map::new();
    for (link, count) in counts {
        obj.insert(link, json!(count));
    }
    Value::Object(obj)
}

/// A float the JSON writer must keep (CPython prints integral
/// floats with a trailing `.0`; the value below re-renders so).
fn json_float(value: f64) -> Value {
    if value.is_nan() {
        return Value::Null;
    }
    if value.is_infinite() {
        return Value::String(if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        });
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn as_str(value: &Value) -> Option<&str> {
    value.as_str()
}

/// The oracle's `_iso`: `fromisoformat` (with `Z` spelled out)
/// to an epoch timestamp; naive values read as local time.
fn iso(raw: &Value) -> Option<f64> {
    let text = raw.as_str()?;
    if text.is_empty() {
        return None;
    }
    let replaced = text.replace('Z', "+00:00");
    crate::responsiveness::parse_iso_instant(&replaced).map(|(at, _offset)| at)
}

fn parse_map(text: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// CPython `str()` of a JSON value, for scalar record fields
/// that arrive with the wrong type.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer.to_string()
            } else if let Some(unsigned) = number.as_u64() {
                unsigned.to_string()
            } else if let Some(float) = number.as_f64() {
                crate::json::py_float_repr(float)
            } else {
                number.as_str().to_string()
            }
        }
        Value::Bool(flag) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Logical paths (`.zst` counted as the plain name) below
/// *root*, sorted, for one suffix spelling.
fn logical_glob(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    collect_logical(root, suffix, &mut found);
    found.sort();
    found.dedup();
    found
}

fn collect_logical(dir: &Path, suffix: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if name.ends_with(suffix) && !name.ends_with(".zst") {
            found.push(path.clone());
        }
        let compressed_suffix = format!("{suffix}.zst");
        if name.ends_with(&compressed_suffix) {
            found.push(crate::compressed::logical_path(&path));
        }
        if is_dir {
            collect_logical(&path, suffix, found);
        }
    }
}

/// The head records of a log (oracle `_records`): at most
/// `limit` lines, JSON objects only, unreadable logs empty.
fn head_records(path: &Path, limit: usize) -> Vec<Map<String, Value>> {
    let Some(physical) = resolve_source(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if is_compressed(&physical) {
        let Ok(bytes) = read_source_bytes(path) else {
            return Vec::new();
        };
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if index >= limit {
                break;
            }
            if let Ok(text) = std::str::from_utf8(line) {
                if let Some(map) = parse_map(text) {
                    out.push(map);
                }
            }
        }
        return out;
    }
    let Ok(bytes) = std::fs::read(&physical) else {
        return Vec::new();
    };
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if index >= limit {
            break;
        }
        // A trailing split artifact after the final newline is
        // not a file line in the oracle's binary iteration.
        if line.is_empty() && index > 0 && bytes.ends_with(b"\n") {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(line) {
            if let Some(map) = parse_map(text) {
                out.push(map);
            }
        }
    }
    out
}

fn claude_head(path: &Path) -> (Option<String>, Option<f64>) {
    let mut cwd: Option<String> = None;
    let mut started: Option<f64> = None;
    for record in head_records(path, HEAD_RECORDS) {
        // The oracle stops when both are truthy, and its
        // `started or _iso(...)` discards an epoch-0 start.
        if cwd.as_deref().is_some_and(|value| !value.is_empty())
            && started.is_some_and(|value| value != 0.0)
        {
            break;
        }
        if cwd.is_none() {
            if let Some(value) = record.get("cwd").and_then(as_str) {
                cwd = Some(value.to_string());
            }
        }
        if !started.is_some_and(|value| value != 0.0) {
            if let Some(value) = record.get("timestamp").and_then(iso) {
                started = Some(value);
            }
        }
    }
    (cwd, started)
}

/// Top-level Claude Code sessions and their native subagents.
fn scan_claude(host: &str, projects: &Path) -> Vec<Session> {
    let mut sessions = Vec::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(projects) {
        for entry in entries.flatten() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    let mut tops: Vec<PathBuf> = Vec::new();
    for directory in &directories {
        if !directory.is_dir() {
            continue;
        }
        for path in logical_glob(directory, ".jsonl") {
            if path.parent() == Some(directory.as_path()) {
                tops.push(path);
            }
        }
    }
    tops.sort();
    for path in tops {
        let (cwd, started) = claude_head(&path);
        let root = Session::new(
            "claude",
            stem_of(&path),
            host,
            Some(path.clone()),
            cwd.clone(),
            started,
        );
        let root_key = root.key();
        sessions.push(root);
        let subagents = path
            .parent()
            .unwrap_or(Path::new(""))
            .join(stem_of(&path))
            .join("subagents");
        if subagents.is_dir() {
            for child in iter_logical_glob_agents(&subagents) {
                let agent_stem = stem_of(&child);
                let agent_id = agent_stem
                    .strip_prefix("agent-")
                    .unwrap_or(&agent_stem)
                    .to_string();
                let (_, child_started) = claude_head(&child);
                let mut session = Session::new(
                    "claude-subagent",
                    format!("{}/{agent_id}", stem_of(&path)),
                    host,
                    Some(child),
                    cwd.clone(),
                    child_started,
                );
                session.native_parent = Some(root_key.clone());
                session.link = Some("claude-subagent".to_string());
                sessions.push(session);
            }
        }
    }
    sessions
}

/// Codex rollouts, with each spawned child's parent thread.
fn scan_codex(host: &str, root: &Path) -> Vec<Session> {
    let mut sessions = Vec::new();
    for path in iter_logical_glob_rollouts(root) {
        let Ok(first) = read_first_line(&path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&first) else {
            continue;
        };
        let Some(record) = parse_map(text) else {
            continue;
        };
        let Some(meta) = record.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let Some(id) = meta.get("id").and_then(as_str) else {
            continue;
        };
        let source = meta.get("source");
        let subagent = source
            .and_then(Value::as_object)
            .and_then(|obj| obj.get("subagent"));
        let spawn = subagent
            .and_then(Value::as_object)
            .and_then(|obj| obj.get("thread_spawn"));
        // A truthy spawn parent shadows the meta parent even
        // when it is not a string; the link then dangles.
        let spawn_parent = spawn
            .and_then(Value::as_object)
            .and_then(|obj| obj.get("parent_thread_id"))
            .filter(|value| json_truthy(value));
        let parent_raw = spawn_parent.or_else(|| meta.get("parent_thread_id"));
        let mut session = Session::new(
            "codex",
            id.to_string(),
            host,
            Some(path),
            meta.get("cwd").and_then(as_str).map(str::to_string),
            meta.get("timestamp").and_then(iso),
        );
        if let Some(parent) = parent_raw {
            if json_truthy(parent) {
                session.link = Some("codex-spawn".to_string());
            }
            if let Some(parent) = parent.as_str() {
                if !parent.is_empty() {
                    session.native_parent = Some(format!("codex:{parent}"));
                }
            }
        }
        sessions.push(session);
    }
    sessions
}

/// Orc sessions from the session index, with recorded parents.
fn scan_orc(host: &str, root: &Path) -> Result<Vec<Session>, String> {
    let index = root.join(".orc").join("index.db");
    if !index.is_file() {
        return Ok(Vec::new());
    }
    let uri = format!("file:{}?mode=ro&immutable=1", index.display());
    let connection = Connection::open_with_flags(&uri, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    let columns: HashSet<String> = {
        let mut statement = connection
            .prepare("PRAGMA table_info(sessions)")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?;
        let mut set = HashSet::new();
        for row in rows {
            set.insert(row.map_err(|error| error.to_string())?);
        }
        set
    };
    let cwd_column = if columns.contains("cwd") {
        "cwd"
    } else {
        "NULL"
    };
    let sql = format!("SELECT id, parent_id, name, created_at, {cwd_column} FROM sessions");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let mut query = statement.query([]).map_err(|error| error.to_string())?;
    let mut sessions = Vec::new();
    while let Some(row) = query.next().map_err(|error| error.to_string())? {
        let id: rusqlite::types::Value = row.get(0).map_err(|error| error.to_string())?;
        let parent: rusqlite::types::Value = row.get(1).map_err(|error| error.to_string())?;
        let name: rusqlite::types::Value = row.get(2).map_err(|error| error.to_string())?;
        let created: rusqlite::types::Value = row.get(3).map_err(|error| error.to_string())?;
        let cwd: rusqlite::types::Value = row.get(4).map_err(|error| error.to_string())?;
        let id_text = sqlite_text(&id);
        let path = root
            .join(".orc")
            .join("sessions")
            .join(&id_text)
            .join("session.db");
        let mut session = Session::new(
            "orc",
            id_text,
            host,
            Some(path),
            match &cwd {
                rusqlite::types::Value::Text(text) => Some(text.clone()),
                _ => None,
            },
            match &created {
                rusqlite::types::Value::Text(text) => iso(&Value::String(text.clone())),
                _ => None,
            },
        );
        // The oracle keeps the recorded name raw: JSON rows
        // emit it as recorded, Markdown prints its `str()`.
        match &name {
            rusqlite::types::Value::Null => {}
            rusqlite::types::Value::Text(text) => {
                session.name = Some(text.clone());
                session.name_raw = Some(Value::String(text.clone()));
                session.name_falsy = text.is_empty();
            }
            rusqlite::types::Value::Integer(int) => {
                session.name = Some(int.to_string());
                session.name_raw = Some(json!(*int));
                session.name_falsy = *int == 0;
            }
            rusqlite::types::Value::Real(real) => {
                session.name = Some(crate::json::py_float_repr(*real));
                session.name_raw =
                    Some(serde_json::Number::from_f64(*real).map_or(Value::Null, Value::Number));
                session.name_falsy = *real == 0.0;
            }
            rusqlite::types::Value::Blob(bytes) => {
                session.name = Some(py_bytes_repr(bytes));
                session.name_raw = Some(Value::String(py_bytes_repr(bytes)));
                session.name_falsy = bytes.is_empty();
            }
        }
        let parent_text = match &parent {
            rusqlite::types::Value::Null => None,
            rusqlite::types::Value::Integer(0) => None,
            rusqlite::types::Value::Real(value) if *value == 0.0 => None,
            rusqlite::types::Value::Text(text) if text.is_empty() => None,
            other => Some(sqlite_text(other)),
        };
        if let Some(parent) = parent_text {
            if !parent.is_empty() {
                session.native_parent = Some(format!("orc:{parent}"));
                session.link = Some("orc-parent".to_string());
            }
        }
        sessions.push(session);
    }
    Ok(sessions)
}

/// CPython `repr` of a bytes value (`b'...'`), the `str()`
/// of a recorded Orc blob identity or name.
fn py_bytes_repr(bytes: &[u8]) -> String {
    let mut out = String::from("b'");
    for byte in bytes {
        match byte {
            b'\'' => out.push_str("\\'"),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(*byte as char),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('\'');
    out
}

fn sqlite_text(value: &rusqlite::types::Value) -> String {
    match value {
        rusqlite::types::Value::Null => String::new(),
        rusqlite::types::Value::Integer(int) => int.to_string(),
        rusqlite::types::Value::Real(real) => crate::json::py_float_repr(*real),
        rusqlite::types::Value::Text(text) => text.clone(),
        rusqlite::types::Value::Blob(bytes) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Muse sessions, by the working directory their route
/// facts record.
fn scan_muse(host: &str, root: &Path) -> Vec<Session> {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_muse(root, 0, &mut paths);
    paths.sort();
    let mut sessions = Vec::new();
    for path in paths {
        let mut cwd: Option<String> = None;
        let mut started: Option<f64> = None;
        for record in head_records(&path, HEAD_RECORDS) {
            let Some(payload) = record.get("payload").and_then(Value::as_object) else {
                continue;
            };
            if payload.get("kind").and_then(as_str) != Some("route_facts") {
                continue;
            }
            if let Some(facts) = payload.get("record").and_then(Value::as_object) {
                if let Some(value) = facts.get("cwd").and_then(as_str) {
                    cwd = Some(value.trim_end_matches('/').to_string());
                }
            }
            if let Some(number) = record.get("recorded_at") {
                if let Some(value) = py_number(number) {
                    started = Some(value / 1e6);
                }
            }
            break;
        }
        let id = path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        sessions.push(Session::new("muse", id, host, Some(path), cwd, started));
    }
    sessions
}

fn collect_muse(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if depth == 4 {
            if path
                .file_name()
                .map(|name| name == "session.jsonl")
                .unwrap_or(false)
            {
                found.push(path);
            }
            continue;
        }
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if is_dir {
            collect_muse(&path, depth + 1, found);
        }
    }
}

/// A JSON number as CPython would read it (`int`/`float`).
fn py_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|value| value != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `float(record.get("created_at") or 0.0)` and its queue twin.
fn float_or_zero(value: Option<&Value>) -> Result<f64, String> {
    let Some(value) = value else {
        return Ok(0.0);
    };
    if !json_truthy(value) {
        return Ok(0.0);
    }
    match value {
        Value::Number(number) => Ok(number.as_f64().unwrap_or(0.0)),
        Value::Bool(flag) => Ok(if *flag { 1.0 } else { 0.0 }),
        Value::String(text) => parse_py_float(text),
        Value::Array(_) => {
            Err("float() argument must be a string or a real number, not 'list'".to_string())
        }
        Value::Object(_) => {
            Err("float() argument must be a string or a real number, not 'dict'".to_string())
        }
        Value::Null => Ok(0.0),
    }
}

/// CPython `float(text)`: surrounding whitespace ignored,
/// single underscores allowed between digits, `inf`/`nan`
/// spelled out (Rust's parser already accepts those).
fn parse_py_float(text: &str) -> Result<f64, String> {
    let trimmed = text.trim();
    let mut cleaned = String::with_capacity(trimmed.len());
    let chars: Vec<char> = trimmed.chars().collect();
    for (at, ch) in chars.iter().enumerate() {
        if *ch == '_' {
            let between_digits = at > 0
                && at + 1 < chars.len()
                && chars[at - 1].is_ascii_digit()
                && chars[at + 1].is_ascii_digit();
            if !between_digits {
                return Err(format!(
                    "could not convert string to float: {}",
                    crate::json::py_repr_str(text)
                ));
            }
            continue;
        }
        cleaned.push(*ch);
    }
    cleaned
        .parse::<f64>()
        .map_err(|_| format!("could not convert string to float: '{}'", text))
}

fn registry_globs(registry: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    // `*/agent.json`
    if let Ok(entries) = std::fs::read_dir(registry) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("agent.json");
            if candidate.exists() {
                found.push(candidate);
            }
        }
    }
    // `agents/*/agent.json`
    let agents = registry.join("agents");
    if let Ok(entries) = std::fs::read_dir(&agents) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("agent.json");
            if candidate.exists() {
                found.push(candidate);
            }
        }
    }
    // `archive/**/agent.json`
    collect_named(&registry.join("archive"), "agent.json", &mut found);
    found.sort();
    found
}

fn collect_named(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().map(|item| item == name).unwrap_or(false) && path.is_file() {
            found.push(path.clone());
        }
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if is_dir {
            collect_named(&path, name, found);
        }
    }
}

/// Every agent record in a registry, live and archived.
fn load_registry(host: &str, registry: &Path) -> Result<Vec<RegistryAgent>, String> {
    let mut agents = Vec::new();
    for path in registry_globs(registry) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(record) = parse_map(&text) else {
            continue;
        };
        let Some(name) = record.get("name").and_then(as_str) else {
            continue;
        };
        let mut session: Option<String> = None;
        if let Some(native) = record.get("native_session").and_then(Value::as_object) {
            if let Some(value) = native.get("value").and_then(as_str) {
                session = Some(value.to_string());
            }
        }
        if let Some(arguments) = record.get("arguments").and_then(Value::as_array) {
            for pair in arguments.windows(2) {
                let flag = pair[0].as_str();
                if matches!(flag, Some("--session-id") | Some("--resume")) {
                    if let Some(value) = pair[1].as_str() {
                        // `session or value`: an empty recorded
                        // value does not block the arguments.
                        if session.as_deref().is_none_or(|text| text.is_empty()) {
                            session = Some(value.to_string());
                        }
                    }
                }
            }
        }
        let creator_raw = record
            .get("created_by")
            .filter(|value| json_truthy(value))
            .or_else(|| record.get("managed_by"));
        let created_by = match creator_raw {
            Some(Value::Object(obj)) => obj.get("agent").and_then(as_str).map(str::to_string),
            Some(Value::String(text)) => Some(text.clone()),
            _ => None,
        };
        let harness = match record.get("harness") {
            Some(Value::String(text)) if !text.is_empty() => text.clone(),
            Some(Value::Null) | None => "unknown".to_string(),
            Some(Value::Bool(false)) => "unknown".to_string(),
            Some(Value::Number(number)) if number.as_f64() == Some(0.0) => "unknown".to_string(),
            Some(other) => py_str(other),
        };
        let created_at = float_or_zero(record.get("created_at"))?;
        agents.push(RegistryAgent {
            host: host.to_string(),
            name: name.to_string(),
            harness,
            cwd: record.get("cwd").and_then(as_str).map(str::to_string),
            created_at,
            native_session: session,
            created_by,
        });
    }
    Ok(agents)
}

fn queue_globs(registry: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    // `*/queue/*/*.json` and `agents/*/queue/*/*.json`
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(registry) {
        for entry in entries.flatten() {
            bases.push(entry.path());
        }
    }
    let agents = registry.join("agents");
    if let Ok(entries) = std::fs::read_dir(&agents) {
        for entry in entries.flatten() {
            bases.push(entry.path());
        }
    }
    for base in &bases {
        collect_queue(&base.join("queue"), &mut found);
    }
    // `archive/**/queue/*/*.json`
    collect_archive_queues(&registry.join("archive"), &mut found);
    found.sort();
    found
}

fn collect_queue(queue: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(queue) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&dir) {
            for file in files.flatten() {
                let path = file.path();
                if path.extension().map(|ext| ext == "json").unwrap_or(false) && path.is_file() {
                    found.push(path);
                }
            }
        }
    }
}

fn collect_archive_queues(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = path.is_dir()
            && !std::fs::symlink_metadata(&path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        if !is_dir {
            continue;
        }
        if path
            .file_name()
            .map(|name| name == "queue")
            .unwrap_or(false)
        {
            collect_queue(&path, found);
        } else {
            collect_archive_queues(&path, found);
        }
    }
}

/// `(owner NAME, text, queued_at)` for every queued message.
fn load_queue(registry: &Path) -> Result<Vec<(String, String, f64)>, String> {
    let mut messages = Vec::new();
    for path in queue_globs(registry) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(record) = parse_map(&text) else {
            continue;
        };
        let Some(agent_path) = path.ancestors().nth(3).map(|dir| dir.join("agent.json")) else {
            continue;
        };
        let Ok(owner_text) = std::fs::read_to_string(&agent_path) else {
            continue;
        };
        let Some(owner) = parse_map(&owner_text) else {
            continue;
        };
        let (Some(text), Some(name)) = (
            record.get("text").and_then(as_str),
            owner.get("name").and_then(as_str),
        ) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let at = float_or_zero(record.get("queued_at"))?;
        messages.push((name.to_string(), text.to_string(), at));
    }
    Ok(messages)
}

fn prompt_key(text: &str) -> String {
    // CPython `text.split()` also breaks on \x1c-\x1f, which
    // Rust does not count as whitespace.
    text.split(|ch: char| ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch))
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// The first instruction a session received (oracle
/// `first_prompt`).
fn first_prompt(session: &Session) -> Option<String> {
    let path = session.path.as_ref()?;
    if session.provider != "claude" && session.provider != "codex" {
        return None;
    }
    for record in head_records(path, HEAD_RECORDS) {
        if session.provider == "codex" {
            let Some(payload) = record.get("payload").and_then(Value::as_object) else {
                continue;
            };
            if record.get("type").and_then(as_str) != Some("response_item")
                || payload.get("type").and_then(as_str) != Some("message")
                || payload.get("role").and_then(as_str) != Some("user")
            {
                continue;
            }
            let mut texts = Vec::new();
            if let Some(content) = payload.get("content").and_then(Value::as_array) {
                for block in content {
                    if let Some(block) = block.as_object() {
                        if let Some(text) = block.get("text").and_then(as_str) {
                            texts.push(text.to_string());
                        }
                    }
                }
            }
            let text = texts.join("\n").trim().to_string();
            if !text.is_empty() && !text.starts_with('<') && !text.starts_with("# AGENTS.md") {
                return Some(text);
            }
        } else {
            if record.get("type").and_then(as_str) != Some("user") {
                continue;
            }
            if record.get("isMeta").map(json_truthy).unwrap_or(false) {
                continue;
            }
            let message = record.get("message").and_then(Value::as_object);
            let content = message.and_then(|obj| obj.get("content"));
            if let Some(text) = content.and_then(as_str) {
                if !text.trim().is_empty() && !text.starts_with('<') {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn record_time(line: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(line).ok()?;
    let record = parse_map(text)?;
    if let Some(when) = record.get("timestamp").and_then(iso) {
        return Some(when);
    }
    record
        .get("recorded_at")
        .and_then(py_number)
        .map(|value| value / 1e6)
}

/// The logical lines of a log; empty for databases and
/// unreadable logs.
fn log_lines(path: &Path) -> Vec<Vec<u8>> {
    if path.extension().map(|ext| ext == "db").unwrap_or(false) {
        return Vec::new();
    }
    let Some(physical) = resolve_source(path) else {
        return Vec::new();
    };
    if is_compressed(&physical) {
        return match read_source_bytes(path) {
            Ok(bytes) => bytes
                .split(|byte| *byte == b'\n')
                .map(<[u8]>::to_vec)
                .collect(),
            Err(_) => Vec::new(),
        };
    }
    match std::fs::read(&physical) {
        Ok(bytes) => bytes
            .split_inclusive(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// `(when, NAME)` for every manager `... start NAME` command
/// in a log.
fn launches(path: &Path) -> Vec<(f64, String)> {
    let launch = launch_re();
    let variable = variable_launch_re();
    let mut found = Vec::new();
    for line in log_lines(path) {
        if !line.windows(8).any(|window| window == b"agentctl")
            || !line.windows(5).any(|window| window == b"start")
        {
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        for capture in launch.captures_iter(&line) {
            if let Some(name) = capture.get(1) {
                names.push(String::from_utf8_lossy(name.as_bytes()).into_owned());
            }
        }
        for capture in variable.captures_iter(&line) {
            if let Some(name) = capture.get(1) {
                names.push(String::from_utf8_lossy(name.as_bytes()).into_owned());
            }
        }
        if names.is_empty() {
            continue;
        }
        let Some(when) = record_time(&line) else {
            continue;
        };
        let mut seen: HashSet<String> = HashSet::new();
        for name in names {
            if seen.insert(name.clone()) {
                found.push((when, name));
            }
        }
    }
    found
}

/// When a log ran a harness CLI directly.
fn invocations(path: &Path) -> Vec<f64> {
    let exec = exec_re();
    let mut found = Vec::new();
    for line in log_lines(path) {
        let mentions = line.windows(4).any(|window| window == b"exec")
            || line.windows(6).any(|window| window == b"claude")
            || line.windows(4).any(|window| window == b"muse");
        if !mentions {
            continue;
        }
        if !exec.is_match(&line) {
            continue;
        }
        if let Some(when) = record_time(&line) {
            found.push(when);
        }
    }
    found
}

fn under(cwd: &Option<String>, prefixes: &[String]) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    if cwd.is_empty() {
        return false;
    }
    let normalized = format!("{}/", cwd.trim_end_matches('/'));
    prefixes
        .iter()
        .any(|prefix| normalized.starts_with(&format!("{}/", prefix.trim_end_matches('/'))))
}

fn recent(session: &Session, since: f64) -> bool {
    let Some(path) = &session.path else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() >= since)
        .unwrap_or(false)
}

fn attach(
    sessions: &mut [Session],
    index: &HashMap<String, usize>,
    child: &str,
    parent: &str,
    link: &str,
) {
    let child_at = index[child];
    if sessions[child_at].parent.is_some() || child == parent {
        return;
    }
    sessions[child_at].parent = Some(parent.to_string());
    sessions[child_at].link = Some(link.to_string());
    let parent_at = index[parent];
    sessions[parent_at].children.push(child.to_string());
}

fn reached_from_roots(
    sessions: &[Session],
    index: &HashMap<String, usize>,
    roots: &[String],
) -> HashSet<String> {
    let mut found: HashSet<String> = roots.iter().cloned().collect();
    let mut frontier: VecDeque<String> = roots.iter().cloned().collect();
    while let Some(key) = frontier.pop_front() {
        for child in &sessions[index[&key]].children {
            if found.insert(child.clone()) {
                frontier.push_back(child.clone());
            }
        }
    }
    found
}

/// Rebuild the tree under *roots* and sweep *cwd_prefixes*.
pub fn discover(
    sources: &Sources,
    roots: &[String],
    cwd_prefixes: &[String],
    since: Option<f64>,
) -> Result<Discovery, String> {
    let mut sessions: Vec<Session> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    fn add(sessions: &mut Vec<Session>, index: &mut HashMap<String, usize>, session: Session) {
        let key = session.key();
        index.entry(key).or_insert_with(|| {
            sessions.push(session);
            sessions.len() - 1
        });
    }
    for (host, path) in &sources.claude_projects {
        for session in scan_claude(host, path) {
            add(&mut sessions, &mut index, session);
        }
    }
    for (host, path) in &sources.codex_sessions {
        for session in scan_codex(host, path) {
            add(&mut sessions, &mut index, session);
        }
    }
    for (host, path) in &sources.orc_roots {
        for session in scan_orc(host, path)? {
            add(&mut sessions, &mut index, session);
        }
    }
    for (host, path) in &sources.muse_sessions {
        for session in scan_muse(host, path) {
            add(&mut sessions, &mut index, session);
        }
    }
    let mut registry: Vec<RegistryAgent> = Vec::new();
    for (host, path) in &sources.registries {
        registry.extend(load_registry(host, path)?);
    }
    let missing: Vec<&String> = roots
        .iter()
        .filter(|root| !index.contains_key(*root))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "root session(s) not found in any source: {}",
            missing
                .iter()
                .map(|root| root.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // Native links, in session insertion order.
    let native: Vec<(String, String, String)> = sessions
        .iter()
        .filter_map(|session| {
            session.native_parent.as_ref().map(|parent| {
                (
                    session.key(),
                    parent.clone(),
                    session.link.clone().unwrap_or_else(|| "native".to_string()),
                )
            })
        })
        .collect();
    for (child, parent, link) in native {
        if index.contains_key(&parent) {
            attach(&mut sessions, &index, &child, &parent, &link);
        }
    }

    let mut by_provider_host: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (at, session) in sessions.iter().enumerate() {
        let key = (session.provider.clone(), session.host.clone());
        by_provider_host.entry(key).or_default().push(at);
    }
    let mut claimed: HashSet<String> = HashSet::new();

    let resolve = |sessions: &[Session],
                   by_provider_host: &HashMap<(String, String), Vec<usize>>,
                   claimed: &HashSet<String>,
                   agent: &RegistryAgent|
     -> Option<String> {
        let provider = provider_by_harness(&agent.harness)?;
        if let Some(native) = &agent.native_session {
            let key = format!("{provider}:{native}");
            if sessions.iter().any(|session| session.key() == key) {
                return Some(key);
            }
        }
        let candidates = by_provider_host.get(&(provider.to_string(), agent.host.clone()))?;
        let mut best: Option<usize> = None;
        for &at in candidates {
            let candidate = &sessions[at];
            if claimed.contains(&candidate.key()) || candidate.started_at.is_none() {
                continue;
            }
            if let (Some(agent_cwd), Some(candidate_cwd)) = (&agent.cwd, &candidate.cwd) {
                if candidate_cwd.trim_end_matches('/') != agent_cwd.trim_end_matches('/') {
                    continue;
                }
            }
            let delay = candidate.started_at.unwrap() - agent.created_at;
            if (-5.0..=START_WINDOW_S).contains(&delay) {
                let better = match best {
                    None => true,
                    Some(best_at) => {
                        candidate.started_at.unwrap() < sessions[best_at].started_at.unwrap_or(0.0)
                    }
                };
                if better {
                    best = Some(at);
                }
            }
        }
        best.map(|at| sessions[at].key())
    };

    // The oracle keys `resolved` by the record value, so two
    // records with identical fields share one entry: this maps
    // every record to the first record with its value.
    // A record's identity for that collapse: every field.
    type RecordIdentity = (
        String,
        String,
        String,
        Option<String>,
        u64,
        Option<String>,
        Option<String>,
    );
    let mut canonical: Vec<usize> = Vec::with_capacity(registry.len());
    {
        let mut first_of: HashMap<RecordIdentity, usize> = HashMap::new();
        for (at, agent) in registry.iter().enumerate() {
            let identity = (
                agent.host.clone(),
                agent.name.clone(),
                agent.harness.clone(),
                agent.cwd.clone(),
                agent.created_at.to_bits(),
                agent.native_session.clone(),
                agent.created_by.clone(),
            );
            canonical.push(*first_of.entry(identity).or_insert(at));
        }
    }

    // Resolve in created_at order (stable).
    let mut order: Vec<usize> = (0..registry.len()).collect();
    order.sort_by(|&a, &b| {
        registry[a]
            .created_at
            .partial_cmp(&registry[b].created_at)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // `resolved` keeps one entry per distinct record value;
    // a duplicate resolves again and replaces its entry in
    // place, exactly as the oracle's dict assignment does.
    let mut resolved: Vec<(usize, String)> = Vec::new();
    let mut resolved_map: HashMap<usize, String> = HashMap::new();
    for agent_at in order {
        let agent = &registry[agent_at];
        if let Some(key) = resolve(&sessions, &by_provider_host, &claimed, agent) {
            claimed.insert(key.clone());
            let at = index[&key];
            if sessions[at].name.is_none() || sessions[at].name_falsy {
                sessions[at].name = Some(agent.name.clone());
                sessions[at].name_raw = None;
                sessions[at].name_falsy = false;
            }
            let canon = canonical[agent_at];
            match resolved.iter_mut().find(|(held, _)| *held == canon) {
                Some(entry) => entry.1 = key.clone(),
                None => resolved.push((canon, key.clone())),
            }
            resolved_map.insert(canon, key);
        }
    }
    let mut by_name: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (at, agent) in registry.iter().enumerate() {
        by_name
            .entry((agent.host.clone(), agent.name.clone()))
            .or_default()
            .push(at);
    }

    let mut unresolved: Vec<Value> = Vec::new();
    let mut mentions: i64 = 0;
    let mut scanned: HashSet<String> = HashSet::new();
    let mut queued: HashMap<(String, String), Vec<(String, f64)>> = HashMap::new();
    for (_host, path) in &sources.registries {
        for (name, text, at) in load_queue(path)? {
            queued
                .entry((_host.clone(), prompt_key(&text)))
                .or_default()
                .push((name, at));
        }
    }
    let mut sessions_by_agent: HashMap<(String, String), Vec<String>> = HashMap::new();
    for (agent_at, key) in &resolved {
        let agent = &registry[*agent_at];
        sessions_by_agent
            .entry((agent.host.clone(), agent.name.clone()))
            .or_default()
            .push(key.clone());
    }

    let mut prompts: HashMap<String, Option<String>> = HashMap::new();
    let mut exec_calls: Vec<(f64, String, String)> = Vec::new();
    let addressed = addressed_re();

    let mut changed = true;
    while changed {
        changed = false;
        // Launch scan over newly reached sessions, key-sorted.
        let mut todo: Vec<String> = reached_from_roots(&sessions, &index, roots)
            .difference(&scanned)
            .cloned()
            .collect();
        todo.sort();
        for key in todo {
            scanned.insert(key.clone());
            let before = sessions[index[&key]].children.len();
            // scan_launches(key)
            let (path, provider, host) = {
                let session = &sessions[index[&key]];
                (
                    session.path.clone(),
                    session.provider.clone(),
                    session.host.clone(),
                )
            };
            if let Some(path) = &path {
                if matches!(
                    provider.as_str(),
                    "claude" | "codex" | "muse" | "claude-subagent"
                ) {
                    for when in invocations(path) {
                        exec_calls.push((when, key.clone(), host.clone()));
                    }
                    for (when, name) in launches(path) {
                        let named = by_name
                            .get(&(host.clone(), name.clone()))
                            .cloned()
                            .unwrap_or_default();
                        if named.is_empty() {
                            mentions += 1;
                            continue;
                        }
                        let mut launched: Option<usize> = None;
                        for agent_at in &named {
                            let agent = &registry[*agent_at];
                            let by_creation = agent.created_at - when;
                            // `(started_at or -1e18) - when` in
                            // the oracle: a missing or epoch-0
                            // start never beats the window check.
                            let by_start =
                                resolved_map.get(&canonical[*agent_at]).and_then(|key| {
                                    sessions[index[key]].started_at.map(|started| {
                                        (if started == 0.0 { -1e18 } else { started }) - when
                                    })
                                });
                            let ok = (0.0..=LAUNCH_WINDOW_S).contains(&by_creation)
                                || by_start
                                    .map(|delay| (0.0..=LAUNCH_WINDOW_S).contains(&delay))
                                    .unwrap_or(false);
                            if ok {
                                launched = Some(match launched {
                                    None => *agent_at,
                                    Some(current) => {
                                        if agent.created_at < registry[current].created_at {
                                            *agent_at
                                        } else {
                                            current
                                        }
                                    }
                                });
                            }
                        }
                        match launched {
                            None => unresolved.push(json!({
                                "parent": key,
                                "name": name,
                                "at": when,
                                "reason": "no record created by this launch",
                            })),
                            Some(agent_at) => match resolved_map.get(&canonical[agent_at]) {
                                None => unresolved.push(json!({
                                    "parent": key,
                                    "name": name,
                                    "at": when,
                                    "reason": "no matching log",
                                })),
                                Some(child) => {
                                    let child = child.clone();
                                    attach(&mut sessions, &index, &child, &key, "manager-launch");
                                }
                            },
                        }
                    }
                }
            }
            if sessions[index[&key]].children.len() != before {
                changed = true;
            }
        }
        // Restart links from queued prompts.
        let keys: Vec<String> = sessions.iter().map(|session| session.key()).collect();
        for key in keys {
            let (parent, is_root, cwd, host, started_at) = {
                let session = &sessions[index[&key]];
                (
                    session.parent.clone(),
                    roots.contains(&key),
                    session.cwd.clone(),
                    session.host.clone(),
                    session.started_at,
                )
            };
            if parent.is_some() || is_root || !under(&cwd, cwd_prefixes) {
                continue;
            }
            if !prompts.contains_key(&key) {
                let prompt = first_prompt(&sessions[index[&key]]);
                prompts.insert(key.clone(), prompt);
            }
            let Some(prompt) = prompts[&key].clone() else {
                continue;
            };
            let mut owners: HashSet<String> = HashSet::new();
            if let Some(entries) = queued.get(&(host.clone(), prompt_key(&prompt))) {
                for (name, at) in entries {
                    if started_at.is_none() || *at <= started_at.unwrap() + 60.0 {
                        owners.insert(name.clone());
                    }
                }
            }
            if owners.is_empty() {
                if let Some(capture) = addressed.captures(&prompt) {
                    if let Some(name) = capture.get(1) {
                        let name = name.as_str().to_string();
                        if by_name.contains_key(&(host.clone(), name.clone())) {
                            owners.insert(name);
                        }
                    }
                }
            }
            if owners.len() != 1 {
                continue;
            }
            let owner = owners.into_iter().next().unwrap();
            let mut earlier: Vec<String> = Vec::new();
            if let Some(candidates) = sessions_by_agent.get(&(host.clone(), owner.clone())) {
                for candidate in candidates {
                    if candidate != &key
                        && sessions[index[candidate]].started_at.unwrap_or(0.0)
                            <= started_at.unwrap_or(0.0)
                    {
                        earlier.push(candidate.clone());
                    }
                }
            }
            if earlier.is_empty() {
                continue;
            }
            // CPython `max` keeps the first maximal candidate.
            let mut original: Option<String> = None;
            for candidate in earlier {
                let better = match &original {
                    None => true,
                    Some(current) => {
                        sessions[index[&candidate]].started_at.unwrap_or(0.0)
                            > sessions[index[current]].started_at.unwrap_or(0.0)
                    }
                };
                if better {
                    original = Some(candidate);
                }
            }
            let original = original.unwrap();
            attach(&mut sessions, &index, &key, &original, "manager-restart");
            let at = index[&key];
            if sessions[at].name.is_none() || sessions[at].name_falsy {
                sessions[at].name = Some(owner.clone());
                sessions[at].name_raw = None;
                sessions[at].name_falsy = false;
            }
            sessions_by_agent
                .entry((host.clone(), owner))
                .or_default()
                .push(key);
            changed = true;
        }
        // Shell-exec links.
        let keys: Vec<String> = sessions.iter().map(|session| session.key()).collect();
        for key in keys {
            let (parent, is_root, started_at, provider, cwd, host) = {
                let session = &sessions[index[&key]];
                (
                    session.parent.clone(),
                    roots.contains(&key),
                    session.started_at,
                    session.provider.clone(),
                    session.cwd.clone(),
                    session.host.clone(),
                )
            };
            let Some(started) = started_at else {
                continue;
            };
            if parent.is_some()
                || is_root
                || provider == "claude-subagent"
                || !under(&cwd, cwd_prefixes)
            {
                continue;
            }
            let mut prior: Vec<(f64, String)> = Vec::new();
            for (when, parent_key, exec_host) in &exec_calls {
                if exec_host == &host && parent_key != &key {
                    let delay = started - when;
                    if (0.0..=EXEC_WINDOW_S).contains(&delay) {
                        prior.push((delay, parent_key.clone()));
                    }
                }
            }
            if prior.is_empty() {
                continue;
            }
            prior.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            if prior.len() > 1 && prior[1].0 - prior[0].0 < 1.0 && prior[1].1 != prior[0].1 {
                continue;
            }
            let parent_key = prior[0].1.clone();
            attach(&mut sessions, &index, &key, &parent_key, "shell-exec");
            changed = true;
        }
    }

    // Creator links from registry records.
    let mut by_agent_name: HashMap<(String, String), String> = HashMap::new();
    for (agent_at, key) in &resolved {
        let agent = &registry[*agent_at];
        by_agent_name.insert((agent.host.clone(), agent.name.clone()), key.clone());
    }
    for (agent_at, key) in &resolved {
        let agent = &registry[*agent_at];
        if let Some(creator) = &agent.created_by {
            if let Some(parent) = by_agent_name.get(&(agent.host.clone(), creator.clone())) {
                let parent = parent.clone();
                attach(&mut sessions, &index, key, &parent, "manager-created-by");
            }
        }
    }

    let discovery_sessions = sessions;
    // Descendants per root over the finished tree.
    let mut probe = Discovery {
        sessions: discovery_sessions,
        index,
        roots: roots.to_vec(),
        orphans: Vec::new(),
        unresolved_launches: Vec::new(),
        ignored_mentions: 0,
    };
    let mut reached: HashSet<String> = roots.iter().cloned().collect();
    for root in roots {
        for key in probe.descendants(root) {
            reached.insert(key);
        }
    }
    let mut orphans: Vec<String> = Vec::new();
    for session in &probe.sessions {
        let key = session.key();
        if reached.contains(&key) || session.provider == "claude-subagent" {
            continue;
        }
        if !under(&session.cwd, cwd_prefixes) {
            continue;
        }
        if let Some(since) = since {
            let started_ok = session.started_at.unwrap_or(0.0) >= since;
            if !started_ok && !recent(session, since) {
                continue;
            }
        }
        orphans.push(key);
    }
    orphans.sort_by(|a, b| {
        let sa = probe.session(a).started_at.unwrap_or(0.0);
        let sb = probe.session(b).started_at.unwrap_or(0.0);
        sa.partial_cmp(&sb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
    probe.orphans = orphans;
    probe.unresolved_launches = unresolved;
    probe.ignored_mentions = mentions;
    Ok(probe)
}

/// The short human report (oracle `format_markdown`).
pub fn format_markdown(discovery: &Discovery) -> String {
    let mut lines: Vec<String> = vec![
        "| root | name | direct | descendants | by link |".to_string(),
        "|---|---|---|---|---|".to_string(),
    ];
    for root in &discovery.roots {
        let descendants = discovery.descendants(root);
        let links_obj = count_links(discovery, &descendants);
        let mut parts: Vec<String> = Vec::new();
        if let Value::Object(map) = &links_obj {
            for (link, count) in map {
                parts.push(format!("{link} {}", count.as_i64().unwrap_or(0)));
            }
        }
        let session = discovery.session(root);
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            root,
            session.name.clone().unwrap_or_default(),
            session.children.len(),
            descendants.len(),
            parts.join(", ")
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "orphans (under a --cwd-prefix, reached by no root): {}",
        discovery.orphans.len()
    ));
    for key in discovery.orphans.iter().take(200) {
        let session = discovery.session(key);
        let started = match session.started_at {
            // The oracle prints `?` when the start is unknown
            // or epoch 0 (its truthiness rule again).
            Some(at) if at != 0.0 => {
                let seconds = at.trunc() as i64;
                let nanos = ((at - seconds as f64) * 1e9).round() as u32;
                chrono::DateTime::from_timestamp(seconds, nanos)
                    .map(|when| when.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "?".to_string())
            }
            _ => "?".to_string(),
        };
        lines.push(format!(
            "- {} {} {} cwd={}",
            started,
            key,
            session.name.clone().unwrap_or_default(),
            session.cwd.clone().unwrap_or_else(|| "None".to_string())
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "launches of a registered name with no resolvable child: {}; launch-like text naming no registered agent (ignored): {}",
        discovery.unresolved_launches.len(),
        discovery.ignored_mentions
    ));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// CPython `json.dumps(value, indent=2)` with ASCII escaping,
/// for the report (floats in CPython spelling via
/// `json_float` at build time).
pub fn py_dumps2(value: &Value) -> String {
    let mut out = String::new();
    dumps2_into(value, 0, &mut out);
    out
}

fn dumps2_into(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                out.push_str(&integer.to_string());
            } else if let Some(unsigned) = number.as_u64() {
                out.push_str(&unsigned.to_string());
            } else if let Some(float) = number.as_f64() {
                out.push_str(&crate::json::py_float_repr(float));
            } else {
                out.push_str(number.as_str());
            }
        }
        Value::String(text) => py_json_string(text, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(level + 1));
                dumps2_into(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(level));
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(level + 1));
                py_json_string(key, out);
                out.push_str(": ");
                dumps2_into(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&"  ".repeat(level));
            out.push('}');
        }
    }
}

fn py_json_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 || (ch as u32) >= 0x7f => {
                let code = ch as u32;
                if code > 0xffff {
                    let shifted = code - 0x1_0000;
                    out.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (shifted >> 10),
                        0xdc00 + (shifted & 0x3ff)
                    ));
                } else {
                    out.push_str(&format!("\\u{code:04x}"));
                }
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// `LABEL=DIR` pairs (oracle `parse_labelled`).
pub fn parse_labelled(values: &[String], flag: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let mut pairs = Vec::new();
    for value in values {
        let Some((label, directory)) = value.split_once('=') else {
            return Err(format!(
                "{flag} expects LABEL=DIR, got {}",
                shlex_quote(value)
            ));
        };
        if label.is_empty() || directory.is_empty() {
            return Err(format!(
                "{flag} expects LABEL=DIR, got {}",
                shlex_quote(value)
            ));
        }
        pairs.push((label.to_string(), expand_user(directory)));
    }
    Ok(pairs)
}

/// `Path.expanduser` for `~` and `~/...`.
pub fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "_@%+=:,./-".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
