//! Parent-child responsiveness of a persistent-agent team,
//! measured from its own records: `py/wrkviz/responsiveness.py`
//! at parity.
//!
//! Rebuilds the coordinator/child loop from an agentctl registry,
//! the coordinator's Claude Code transcript, and the children's
//! own Claude Code or Muse logs: for every child turn end the
//! parent had to answer, when it was told, when it first looked,
//! and when it first acted; and, for every idle stretch of every
//! child, what the child was waiting for. Reads only; calls no
//! model.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// Queue message id prefix the agent manager writes for watch
/// notices.
pub const WATCH_ID_PREFIX: &str = "agent-watch-";
/// Queue message id prefix for chat-bridge prompts.
pub const CHAT_ID_PREFIX: &str = "chat-";

const SCHEDULED_FIRE: &str = "scheduled_task_fire";
const TURN_DURATION: &str = "turn_duration";
const TASK_NOTIFICATION: &str = "<task-notification>";
/// Verbs in a coordinator tool call that count as acting on a
/// child, as opposed to looking.
pub const ACTING_VERBS: &[&str] = &["stop", "send", "answer", "start"];
/// A queued prompt the child picks up within this many seconds of
/// going idle was already waiting for it.
const PRE_QUEUED_SECONDS: f64 = 180.0;
/// A child's own message counts as notice of a turn end if queued
/// at most this long before it.
const CHILD_MESSAGE_LEAD_SECONDS: f64 = 900.0;
/// Records read from the head of a Muse session log while looking
/// for its working directory.
const MUSE_HEADER_RECORDS: usize = 200;
/// A Muse session log belongs to a child registered at most this
/// long before the log started.
const MUSE_START_WINDOW: f64 = 600.0;
/// How long after a turn end to look for the parent's response
/// when the child never resumed.
const OPEN_HORIZON_SECONDS: f64 = 6.0 * 3600.0;
/// Longest tool-call command scanned; longer commands are mostly
/// embedded documents.
const COMMAND_SCAN_LIMIT: usize = 16_384;

static ASK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\?\s*$|\bmay i\b|\bshall i\b|\bshould i\b|\bdo you want\b").unwrap()
});
static AWAIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"waiting (?:for|on) (?:your|the coordinator|their|an? )|all[- ]clear|\bholding\b|on hold|hold acknowledged|your (?:decision|answer|go|call)|landing (?:turn|queue)|coordinator (?:now )?(?:has|knows)|until you|when you (?:say|reply)").unwrap()
});
static WATCH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^- ([a-z][a-z0-9-]*):").unwrap());
static ASSIGNMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|[;&|\s])([A-Za-z_]\w*)=([^\s;&|'"]+)"#).unwrap());
static VARIABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{?([A-Za-z_]\w*)\}?").unwrap());
static FOR_LOOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bfor\s+(\w+)\s+in\s+([^;]{0,2000}?)\s*;?\s*do\b").unwrap());
static HEREDOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?(\w+)['"]?"#).unwrap());
static SEGMENT_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&&|\|\||[;|&]|\$\(|`").unwrap());
static HEAD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:timeout\s+\S+|nice|nohup|exec|time|command)\s+)*(?:\S+=\S*\s+)*(?:env(?:\s+-C\s+\S+)?\s+(?:\S+=\S*\s+)*)?(?:\S*/)?(agentctl|wrkslots)\S*(?:\s+--?[A-Za-z][\w-]*(?:[= ][^\s-]\S*)?){0,6}\s+([a-z][a-z-]*)((?:\s+\S+){0,4})").unwrap()
});

/// Where one team's records live.
#[derive(Clone, Debug)]
pub struct Sources {
    /// Agent manager registry directory.
    pub registry: PathBuf,
    /// The coordinator's Claude Code transcript.
    pub coordinator_transcript: PathBuf,
    /// The coordinator's registered name.
    pub coordinator: String,
    /// Claude Code projects directory, when searched.
    pub claude_projects: Option<PathBuf>,
    /// Muse sessions directory, when searched.
    pub muse_sessions: Option<PathBuf>,
    /// Text marking the coordinator's periodic sweep prompt.
    pub sweep_marker: Option<String>,
    /// Shell-style name patterns to leave out.
    pub exclude: Vec<String>,
}

/// One child as the registry describes it.
#[derive(Clone, Debug)]
pub struct AgentRecord {
    /// Registered name.
    pub name: String,
    /// Harness (`claude`, `muse`, ...).
    pub harness: String,
    /// Registration instant (seconds).
    pub created: f64,
    /// Working directory, when recorded.
    pub cwd: Option<String>,
    /// Slot name, when recorded.
    pub slot: Option<String>,
    /// Native harness session id, when recorded.
    pub native_session: Option<String>,
    /// Located child log.
    pub log: Option<PathBuf>,
    /// When the coordinator stopped the child, when seen.
    pub stopped: Option<f64>,
}

/// One message from any agent queue in the registry.
#[derive(Clone, Debug)]
pub struct QueueMessage {
    /// Queue owner (agent name).
    pub owner: String,
    /// Message id.
    pub message_id: String,
    /// Message text.
    pub text: String,
    /// When the manager queued it.
    pub queued: Option<f64>,
    /// When delivery was confirmed typed in.
    pub confirmed: Option<f64>,
}

/// A prompt that reached the coordinator.
#[derive(Clone, Debug)]
pub struct CoordinatorInput {
    /// Arrival instant.
    pub at: f64,
    /// Input kind.
    pub kind: String,
    /// The child it concerns, when one is named.
    pub agent: Option<String>,
    /// Whether it arrived mid-turn (queued command attachment).
    pub mid_turn: bool,
}

/// A coordinator tool call (or queued message) addressed to one
/// child.
#[derive(Clone, Debug)]
pub struct Action {
    /// Instant.
    pub at: f64,
    /// Verb (`read` for any non-acting agentctl verb).
    pub verb: String,
    /// Child name.
    pub agent: String,
}

/// One turn of a child.
#[derive(Clone, Debug)]
pub struct ChildTurn {
    /// Start instant.
    pub start: f64,
    /// What started it: `first`, `parent`, or `self`.
    pub trigger: String,
    /// End instant, when the turn closed.
    pub end: Option<f64>,
    /// The child's last reply text.
    pub reply: String,
}

/// A child turn end that the parent had to answer.
#[derive(Clone, Debug)]
pub struct Episode {
    /// Child name.
    pub agent: String,
    /// Child harness.
    pub harness: String,
    /// Turn end instant.
    pub turn_end: f64,
    /// What the last reply asked of the parent.
    pub verdict: String,
    /// What started the next turn (`stop`/`open` when none did).
    pub next_trigger: String,
    /// When the child resumed, when it did.
    pub resumed: Option<f64>,
    /// How the parent came to act.
    pub path: String,
    /// The first notice kind, when any arrived.
    pub first_notice: Option<String>,
    /// Whether the coordinator was mid-turn at the turn end.
    pub parent_busy: bool,
    /// Earliest notice queueing instant.
    pub notified: Option<f64>,
    /// First look instant.
    pub observed: Option<f64>,
    /// First action instant.
    pub acted: Option<f64>,
    /// The acting verb.
    pub act_verb: Option<String>,
    /// Reply excerpt.
    pub reply: String,
}

impl Episode {
    /// Seconds from the turn end to `at`.
    pub fn latency(&self, at: Option<f64>) -> Option<f64> {
        at.map(|held| held - self.turn_end)
    }
}

/// One idle stretch of one child.
#[derive(Clone, Debug)]
pub struct Gap {
    /// Child name.
    pub agent: String,
    /// Child harness.
    pub harness: String,
    /// Stretch start (a turn end).
    pub start: f64,
    /// Stretch end (next turn start or stop).
    pub end: f64,
    /// What the child waited for: `parent`, `self`, or `stop`.
    pub waiting_for: String,
    /// The turn's verdict.
    pub verdict: String,
}

/// Everything the command prints.
#[derive(Clone, Debug)]
pub struct Report {
    /// Coordinator name.
    pub coordinator: String,
    /// Window start instant.
    pub window_start: f64,
    /// Window end instant.
    pub window_end: f64,
    /// Coordinator turns as (start, end).
    pub coordinator_turns: Vec<(f64, f64)>,
    /// Prompts the coordinator received.
    pub inputs: Vec<CoordinatorInput>,
    /// Registered children.
    pub agents: Vec<AgentRecord>,
    /// Episodes, by turn end.
    pub episodes: Vec<Episode>,
    /// Idle stretches, by start.
    pub gaps: Vec<Gap>,
    /// Every queue message.
    pub messages: Vec<QueueMessage>,
}

// ---------------------------------------------------------------- reading

fn str_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn number_of(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        _ => None,
    }
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

/// Parse an ISO-8601 instant to epoch seconds; the second element
/// says whether it carried a UTC offset. Mirrors CPython
/// `datetime.fromisoformat` (3.11+): extended or basic date, any
/// single-character separator, extended or basic time, comma or
/// dot fractions, and `Z` or `±HH[:MM[:SS]]`/`±HHMM[SS]` offsets.
/// Naive values read as local time, as CPython's
/// `datetime.timestamp()` reads them.
pub fn parse_iso_instant(text: &str) -> Option<(f64, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let digits = |from: usize, count: usize| -> Option<u32> {
        if from + count > chars.len() {
            return None;
        }
        let mut value: u32 = 0;
        for ch in &chars[from..from + count] {
            if !ch.is_ascii_digit() {
                return None;
            }
            value = value * 10 + ch.to_digit(10)?;
        }
        Some(value)
    };
    let (year, month, day, mut pos) = if chars.len() >= 10 && chars.get(4) == Some(&'-') {
        (
            digits(0, 4)? as i32,
            digits(5, 2)?,
            digits(8, 2).filter(|_| chars.get(7) == Some(&'-'))?,
            10,
        )
    } else {
        (digits(0, 4)? as i32, digits(4, 2)?, digits(6, 2)?, 8)
    };
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    if pos == chars.len() {
        let naive = date.and_hms_opt(0, 0, 0)?;
        return local_epoch(naive).map(|at| (at, false));
    }
    pos += 1; // the single separator character
    let hour = digits(pos, 2)?;
    pos += 2;
    let (mut minute, mut second, mut nanos) = (0u32, 0u32, 0u32);
    if chars.get(pos) == Some(&':') {
        minute = digits(pos + 1, 2)?;
        pos += 3;
        if chars.get(pos) == Some(&':') {
            second = digits(pos + 1, 2)?;
            pos += 3;
        }
    } else if chars.get(pos).is_some_and(|ch| ch.is_ascii_digit()) {
        minute = digits(pos, 2)?;
        pos += 2;
        if chars.get(pos).is_some_and(|ch| ch.is_ascii_digit()) {
            second = digits(pos, 2)?;
            pos += 2;
        }
    }
    if matches!(chars.get(pos), Some('.' | ',')) {
        let start = pos + 1;
        let mut end = start;
        while chars.get(end).is_some_and(|ch| ch.is_ascii_digit()) {
            end += 1;
        }
        if end == start {
            return None;
        }
        let fraction: String = chars[start..end.min(start + 9)].iter().collect();
        let padded = format!("{fraction:0<9}");
        nanos = padded.parse().ok()?;
        pos = end;
    }
    let time = chrono::NaiveTime::from_hms_nano_opt(hour, minute, second, nanos)?;
    let naive = date.and_time(time);
    if pos == chars.len() {
        return local_epoch(naive).map(|at| (at, false));
    }
    if chars.get(pos) == Some(&'Z') {
        if pos + 1 != chars.len() {
            return None;
        }
        return Some((epoch_seconds(&naive.and_utc()), true));
    }
    let sign: i64 = match chars.get(pos) {
        Some('+') => 1,
        Some('-') => -1,
        _ => return None,
    };
    pos += 1;
    let rest: String = chars[pos..].iter().collect();
    let offset_seconds: i64 = if rest.contains(':') {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() < 2 || parts.len() > 3 {
            return None;
        }
        let mut total = 0i64;
        for (weight, part) in [
            (3600i64, parts[0]),
            (60, parts[1]),
            (1, parts.get(2).copied().unwrap_or("00")),
        ] {
            if part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            total += weight * part.parse::<i64>().ok()?;
        }
        total
    } else {
        if !rest.bytes().all(|byte| byte.is_ascii_digit()) || ![2, 4, 6].contains(&rest.len()) {
            return None;
        }
        let hours: i64 = rest[..2].parse().ok()?;
        let minutes: i64 = if rest.len() >= 4 {
            rest[2..4].parse().ok()?
        } else {
            0
        };
        let seconds: i64 = if rest.len() == 6 {
            rest[4..6].parse().ok()?
        } else {
            0
        };
        hours * 3600 + minutes * 60 + seconds
    };
    if offset_seconds >= 86_400 {
        return None;
    }
    Some((
        epoch_seconds(&naive.and_utc()) - sign as f64 * offset_seconds as f64,
        true,
    ))
}

fn local_epoch(naive: chrono::NaiveDateTime) -> Option<f64> {
    naive
        .and_local_timezone(chrono::Local)
        .single()
        .or_else(|| naive.and_local_timezone(chrono::Local).earliest())
        .map(|parsed| epoch_seconds(&parsed))
}

fn epoch_seconds<T: chrono::TimeZone>(at: &chrono::DateTime<T>) -> f64 {
    at.timestamp() as f64 + at.timestamp_subsec_micros() as f64 / 1e6
}

fn iso_seconds(value: Option<&Value>) -> Option<f64> {
    let text = value?.as_str()?;
    parse_iso_instant(text).map(|(at, _)| at)
}

/// Rewrite CPython's non-standard JSON constants (`NaN`,
/// `Infinity`, `-Infinity`) to `null` outside string literals, so
/// a record carrying one in an unused field still parses.
fn lenient_constants(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            let width = if byte < 0x80 {
                1
            } else if byte >= 0xf0 {
                4
            } else if byte >= 0xe0 {
                3
            } else {
                2
            };
            let end = (index + width).min(text.len());
            out.push_str(&text[index..end]);
            index = end;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push('"');
            index += 1;
            continue;
        }
        let rest = &text[index..];
        let mut replaced = false;
        for token in ["-Infinity", "Infinity", "NaN"] {
            if let Some(after_text) = rest.strip_prefix(token) {
                let before_ok = index == 0
                    || matches!(
                        bytes[index - 1],
                        b' ' | b'\t' | b'\n' | b'\r' | b':' | b',' | b'[' | b'{'
                    );
                let after = after_text.chars().next();
                let after_ok = after.is_none()
                    || matches!(after, Some(' ' | '\t' | '\n' | '\r' | ',' | ']' | '}'));
                if before_ok && after_ok {
                    out.push_str("null");
                    index += token.len();
                    replaced = true;
                    break;
                }
            }
        }
        if !replaced {
            out.push(bytes[index] as char);
            index += 1;
        }
    }
    out
}

fn parse_lenient_line(line: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) => Some(map),
        Ok(_) => None,
        Err(_) => match serde_json::from_str::<Value>(&lenient_constants(line)) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        },
    }
}

fn json_lines(path: &Path) -> Result<Vec<Map<String, Value>>, String> {
    let bytes = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut records = Vec::new();
    for line in normalized.split('\n') {
        if let Some(map) = parse_lenient_line(line) {
            records.push(map);
        }
    }
    Ok(records)
}

fn read_json(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str::<Value>(&text)
        .unwrap_or_else(|_| serde_json::from_str(&lenient_constants(&text)).unwrap_or(Value::Null));
    match parsed {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn sorted_children(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

fn record_dirs(registry: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for parent in [
        registry.join("agents"),
        registry.to_path_buf(),
        registry.join("archive"),
    ] {
        if !parent.is_dir() {
            continue;
        }
        for child in sorted_children(&parent) {
            if parent == registry
                && matches!(
                    child.file_name().and_then(|name| name.to_str()),
                    Some("agents" | "locks" | "archive")
                )
            {
                continue;
            }
            if child.join("agent.json").is_file() {
                dirs.push(child);
            }
        }
    }
    dirs
}

/// Every registered child, live or archived, oldest first.
pub fn load_agents(registry: &Path, coordinator: &str) -> Vec<AgentRecord> {
    let mut agents = Vec::new();
    for directory in record_dirs(registry) {
        let Some(record) = read_json(&directory.join("agent.json")) else {
            continue;
        };
        let Some(name) = str_of(record.get("name")).filter(|held| !held.is_empty()) else {
            continue;
        };
        if name == coordinator {
            continue;
        }
        let session = match record.get("native_session") {
            Some(Value::Object(native)) => str_of(native.get("value")),
            _ => None,
        };
        agents.push(AgentRecord {
            name,
            harness: str_of(record.get("harness"))
                .filter(|held| !held.is_empty())
                .unwrap_or_else(|| "unknown".to_string()),
            created: number_of(record.get("created_at")).unwrap_or(0.0),
            cwd: str_of(record.get("cwd")),
            slot: str_of(record.get("slot")),
            native_session: session,
            log: None,
            stopped: None,
        });
    }
    agents.sort_by(|left, right| left.created.total_cmp(&right.created));
    agents
}

/// Every queue message in every state, attributed to the queue's
/// owner.
pub fn load_messages(registry: &Path) -> Vec<QueueMessage> {
    let mut messages = Vec::new();
    for directory in record_dirs(registry) {
        let owner = read_json(&directory.join("agent.json"))
            .and_then(|record| str_of(record.get("name")))
            .filter(|held| !held.is_empty())
            .unwrap_or_else(|| {
                directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let queue = directory.join("queue");
        let mut paths: Vec<PathBuf> = Vec::new();
        for state in sorted_children(&queue) {
            if !state.is_dir() {
                continue;
            }
            for path in sorted_children(&state) {
                if path.extension().is_some_and(|ext| ext == "json") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        for path in paths {
            let Some(record) = read_json(&path) else {
                continue;
            };
            messages.push(QueueMessage {
                owner: owner.clone(),
                message_id: str_of(record.get("id"))
                    .filter(|held| !held.is_empty())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    }),
                text: str_of(record.get("text")).unwrap_or_default(),
                queued: number_of(record.get("queued_at")),
                confirmed: number_of(record.get("confirmed_at")),
            });
        }
    }
    messages
}

/// The child a coordinator message came from, by the `NAME:`
/// convention: the message starts with its registered name
/// followed by a colon or a space, optionally after `LANDED (`.
pub fn message_author(text: &str, names: &[String]) -> Option<String> {
    let mut head = text.trim_start();
    if let Some(rest) = head.strip_prefix("LANDED (") {
        head = rest;
    }
    let mut best: Option<&str> = None;
    for name in names {
        if let Some(rest) = head.strip_prefix(name.as_str()) {
            if matches!(rest.chars().next(), Some(':' | ' ' | ')' | ','))
                && best.is_none_or(|held| name.len() > held.len())
            {
                best = Some(name.as_str());
            }
        }
    }
    best.map(str::to_owned)
}

fn prompt_text(record: &Map<String, Value>) -> Option<String> {
    let message = record.get("message")?.as_object()?;
    match message.get("content") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(block) = block.as_object() else {
                    continue;
                };
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    return Some(str_of(block.get("text")).unwrap_or_default());
                }
            }
            None
        }
        _ => None,
    }
}

/// Every agent a watch-agents notice lists, one `- NAME:
/// condition` line each.
pub fn watch_agents(text: &str) -> Vec<String> {
    WATCH_RE
        .captures_iter(text)
        .map(|captured| captured[1].to_string())
        .collect()
}

fn marker_in(marker: &Option<String>, text: &str) -> bool {
    marker
        .as_ref()
        .is_some_and(|held| !held.is_empty() && text.contains(held.as_str()))
}

/// The kind of a coordinator prompt and the first child it
/// concerns, when one is named.
pub fn classify_input(
    text: &str,
    names: &[String],
    sweep_marker: &Option<String>,
) -> (String, Option<String>) {
    let head: String = text.chars().take(300).collect();
    if marker_in(sweep_marker, &head) {
        return ("sweep".to_string(), None);
    }
    if head.starts_with(TASK_NOTIFICATION) {
        return ("task-notification".to_string(), None);
    }
    if head.starts_with("[agentctl watch-agents]") {
        let listed = watch_agents(text);
        return ("watch".to_string(), listed.into_iter().next());
    }
    if let Some(author) = message_author(text, names) {
        return ("child-message".to_string(), Some(author));
    }
    if head.contains("chat bridge") {
        return ("chat".to_string(), None);
    }
    ("other".to_string(), None)
}

/// Agent names matched as whole words: not preceded or followed
/// by `[A-Za-z0-9-]`. Mirrors the oracle's lookaround pattern with
/// alternatives ordered longest-first.
pub struct NameMatcher {
    names: Vec<String>,
}

impl NameMatcher {
    /// Build from registered names (longest first, stable).
    pub fn new(names: &[String]) -> NameMatcher {
        let mut ordered: Vec<String> = names
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect();
        ordered.sort_by_key(|name| std::cmp::Reverse(name.chars().count()));
        NameMatcher { names: ordered }
    }

    fn boundary(chars: &[char], index: usize) -> bool {
        index >= chars.len() || !matches!(chars[index], 'A'..='Z' | 'a'..='z' | '0'..='9' | '-')
    }

    /// Every name occurrence in `text`, left to right.
    pub fn findall(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut found = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            if index > 0 && matches!(chars[index - 1], 'A'..='Z' | 'a'..='z' | '0'..='9' | '-') {
                index += 1;
                continue;
            }
            let mut matched = false;
            for name in &self.names {
                let name_chars: Vec<char> = name.chars().collect();
                if chars[index..].starts_with(&name_chars)
                    && Self::boundary(&chars, index + name_chars.len())
                {
                    found.push(name.clone());
                    index += name_chars.len();
                    matched = true;
                    break;
                }
            }
            if !matched {
                index += 1;
            }
        }
        found
    }

    /// Whether `operand` is exactly one registered name.
    pub fn is_name(&self, operand: &str) -> bool {
        self.names.iter().any(|name| name == operand)
    }
}

fn expand(command: &str) -> String {
    let mut values: Vec<(String, String)> = Vec::new();
    for captured in ASSIGNMENT_RE.captures_iter(command) {
        let name = captured[1].to_string();
        if !values.iter().any(|(held, _)| *held == name) {
            values.push((name, captured[2].to_string()));
        }
    }
    if values.is_empty() {
        return command.to_string();
    }
    let substitute = |text: &str, values: &Vec<(String, String)>| -> String {
        VARIABLE_RE
            .replace_all(text, |captured: &regex::Captures<'_>| {
                let name = &captured[1];
                values
                    .iter()
                    .find(|(held, _)| held == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| captured[0].to_string())
            })
            .into_owned()
    };
    for index in 0..values.len() {
        let once = substitute(&values[index].1.clone(), &values);
        values[index].1 = substitute(&once, &values);
    }
    substitute(command, &values)
}

fn after_first_word(segment: &str) -> String {
    match segment.find(char::is_whitespace) {
        Some(index) => segment[index..].trim_start().to_string(),
        None => String::new(),
    }
}

/// CPython `str.splitlines`: breaks at `\n`, `\r`, `\r\n`, `\x0b`,
/// `\x0c`, `\x1c`-`\x1e`, `\x85`, ` ` and ` `.
fn split_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                lines.push(std::mem::take(&mut current));
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => {
                lines.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The simple commands of a shell command line, skipping heredoc
/// bodies.
fn segments(command: &str) -> Vec<String> {
    let truncated: String = command.chars().take(COMMAND_SCAN_LIMIT).collect();
    let expanded = expand(&truncated);
    let mut out = Vec::new();
    let mut terminator: Option<String> = None;
    for line in split_lines(&expanded).iter().map(String::as_str) {
        if let Some(end) = &terminator {
            if line.trim() == end {
                terminator = None;
            }
            continue;
        }
        if let Some(captured) = HEREDOC_RE.captures(line) {
            terminator = Some(captured[1].to_string());
        }
        for piece in SEGMENT_SPLIT_RE.split(line) {
            let mut segment = piece
                .trim()
                .trim_start_matches(['(', '{', ' '])
                .trim()
                .to_string();
            if segment.starts_with("do ")
                || segment.starts_with("then ")
                || segment.starts_with("else ")
            {
                segment = after_first_word(&segment);
            }
            if !segment.is_empty() {
                out.push(segment);
            }
        }
    }
    out
}

/// `(verb, child)` pairs one coordinator Bash command addresses.
/// Only a simple command starting with `agentctl` counts; a `for
/// VAR in NAMES; do agentctl VERB $VAR` loop applies the verb to
/// each listed name.
pub fn command_targets(command: &str, pattern: &NameMatcher) -> Vec<(String, String)> {
    let mut loops: Vec<(String, Vec<String>)> = Vec::new();
    for captured in FOR_LOOP_RE.captures_iter(command) {
        let names = pattern.findall(&captured[2]);
        let variable = captured[1].to_string();
        if let Some(entry) = loops.iter_mut().find(|(held, _)| *held == variable) {
            entry.1 = names;
        } else {
            loops.push((variable, names));
        }
    }
    let mut pairs = Vec::new();
    for segment in segments(command) {
        let Some(captured) = HEAD_RE.captures(&segment) else {
            continue;
        };
        if &captured[1] != "agentctl" {
            continue;
        }
        let verb = if ACTING_VERBS.contains(&&captured[2]) {
            captured[2].to_string()
        } else {
            "read".to_string()
        };
        let operands: Vec<&str> = captured[3].split_whitespace().collect();
        let operand = operands
            .first()
            .map(|held| held.trim_matches(|ch| matches!(ch, '\'' | '"')))
            .unwrap_or("");
        if let Some(variable) = VARIABLE_RE.captures(operand) {
            if variable
                .get(0)
                .is_some_and(|whole| whole.as_str() == operand)
            {
                if let Some((_, agents)) = loops.iter().find(|(held, _)| held == &variable[1]) {
                    pairs.extend(agents.iter().map(|agent| (verb.clone(), agent.clone())));
                    continue;
                }
            }
        }
        if pattern.is_name(operand) {
            pairs.push((verb, operand.to_string()));
        }
    }
    pairs
}

/// Slots a Bash command reads the handoff of, or removes, with
/// wrkslots.
pub fn slot_targets(command: &str) -> Vec<String> {
    let mut slots = Vec::new();
    for segment in segments(command) {
        let Some(captured) = HEAD_RE.captures(&segment) else {
            continue;
        };
        if &captured[1] != "wrkslots" || !matches!(&captured[2], "read-handoff" | "remove") {
            continue;
        }
        let operands: Vec<&str> = captured[3]
            .split_whitespace()
            .filter(|token| !token.starts_with('-'))
            .collect();
        if let Some(first) = operands.first() {
            let mut chars = first.chars();
            let valid = matches!(chars.next(), Some('a'..='z' | '0'..='9'))
                && chars.all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '-'));
            if valid {
                slots.push(first.to_string());
            }
        }
    }
    slots
}

fn bash_command(block: &Map<String, Value>) -> Option<String> {
    if block.get("name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    let input = block.get("input")?.as_object()?;
    str_of(input.get("command"))
}

/// Turns, prompts and per-child actions from one coordinator
/// transcript.
pub type CoordinatorScan = (Vec<(f64, f64)>, Vec<CoordinatorInput>, Vec<Action>);

/// The coordinator's turns, the prompts it received, and its Bash
/// commands per child. A turn ends at its `turn_duration` record;
/// a turn the harness never closed ends at its last record when
/// the next prompt starts a new one.
pub fn load_coordinator(
    transcript: &Path,
    names: &[String],
    slots: &HashMap<String, Vec<AgentRecord>>,
    sweep_marker: &Option<String>,
) -> Result<CoordinatorScan, String> {
    let mut turns: Vec<(f64, f64)> = Vec::new();
    let mut inputs: Vec<CoordinatorInput> = Vec::new();
    let mut actions: Vec<Action> = Vec::new();
    let mut started: Option<f64> = None;
    let mut last: Option<f64> = None;
    let pattern = if names.is_empty() {
        None
    } else {
        Some(NameMatcher::new(names))
    };

    for record in json_lines(transcript)? {
        let Some(at) = iso_seconds(record.get("timestamp")) else {
            continue;
        };
        let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "user" && !truthy(record.get("isMeta")) {
            match prompt_text(&record) {
                None => {
                    last = Some(at);
                }
                Some(text) => {
                    let (input_kind, agent) = classify_input(&text, names, sweep_marker);
                    inputs.push(CoordinatorInput {
                        at,
                        kind: input_kind.clone(),
                        agent,
                        mid_turn: false,
                    });
                    if input_kind == "watch" {
                        for other in watch_agents(&text).into_iter().skip(1) {
                            inputs.push(CoordinatorInput {
                                at,
                                kind: "watch".to_string(),
                                agent: Some(other),
                                mid_turn: false,
                            });
                        }
                    }
                    if let (Some(was), Some(held)) = (started, last) {
                        turns.push((was, held));
                    }
                    started = Some(at);
                }
            }
        } else if kind == "attachment" {
            if let Some(attachment) = record.get("attachment").and_then(Value::as_object) {
                if attachment.get("type").and_then(Value::as_str) == Some("queued_command") {
                    let text = str_of(attachment.get("prompt")).unwrap_or_default();
                    let (input_kind, agent) = classify_input(&text, names, sweep_marker);
                    inputs.push(CoordinatorInput {
                        at,
                        kind: input_kind.clone(),
                        agent,
                        mid_turn: true,
                    });
                    if input_kind == "watch" {
                        for other in watch_agents(&text).into_iter().skip(1) {
                            inputs.push(CoordinatorInput {
                                at,
                                kind: "watch".to_string(),
                                agent: Some(other),
                                mid_turn: true,
                            });
                        }
                    }
                }
            }
        } else if kind == "system"
            && record.get("subtype").and_then(Value::as_str) == Some(SCHEDULED_FIRE)
        {
            let prompt = str_of(record.get("prompt")).unwrap_or_default();
            let sweep = sweep_marker.is_none()
                || sweep_marker
                    .as_ref()
                    .is_some_and(|held| prompt.contains(held.as_str()));
            inputs.push(CoordinatorInput {
                at,
                kind: if sweep { "sweep" } else { "timer" }.to_string(),
                agent: None,
                mid_turn: false,
            });
            if let (Some(was), Some(held)) = (started, last) {
                turns.push((was, held));
            }
            started = Some(at);
        } else if kind == "system"
            && record.get("subtype").and_then(Value::as_str) == Some(TURN_DURATION)
        {
            if let Some(was) = started {
                turns.push((was, at));
            }
            started = None;
        } else if kind == "assistant" {
            if started.is_none() {
                started = Some(at);
            }
            let content = match record.get("message") {
                Some(Value::Object(message)) => message.get("content"),
                _ => None,
            };
            if let Some(Value::Array(blocks)) = content {
                for block in blocks {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let Some(command) = bash_command(block) else {
                        continue;
                    };
                    if command.contains("agentctl") {
                        if let Some(matcher) = &pattern {
                            for (verb, agent) in command_targets(&command, matcher) {
                                actions.push(Action { at, verb, agent });
                            }
                        }
                    }
                    if command.contains("wrkslots") {
                        for slot in slot_targets(&command) {
                            let owners: Vec<&AgentRecord> = slots
                                .get(&slot)
                                .map(|held| {
                                    held.iter().filter(|agent| agent.created <= at).collect()
                                })
                                .unwrap_or_default();
                            let mut best: Option<&AgentRecord> = None;
                            for owner in owners {
                                if best.is_none_or(|held| owner.created > held.created) {
                                    best = Some(owner);
                                }
                            }
                            if let Some(owner) = best {
                                actions.push(Action {
                                    at,
                                    verb: "remove".to_string(),
                                    agent: owner.name.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        if (kind == "user" || kind == "assistant") && started.is_some() {
            last = Some(at);
        }
    }
    actions.sort_by(|left, right| left.at.total_cmp(&right.at));
    Ok((turns, inputs, actions))
}

/// Turns of a Claude Code transcript; `turn_duration` closes
/// each one.
pub fn claude_turns(path: &Path) -> Result<Vec<ChildTurn>, String> {
    let mut turns: Vec<ChildTurn> = Vec::new();
    let mut current: Option<ChildTurn> = None;
    let mut last_reply = String::new();
    let mut last: Option<f64> = None;
    for record in json_lines(path)? {
        let Some(at) = iso_seconds(record.get("timestamp")) else {
            continue;
        };
        let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "user" && !truthy(record.get("isMeta")) {
            match prompt_text(&record) {
                None => {
                    last = Some(at);
                }
                Some(text) => {
                    if let (Some(mut done), Some(held)) = (current.take(), last) {
                        done.end = Some(held);
                        done.reply = last_reply.clone();
                        turns.push(done);
                    }
                    let trigger = if text.starts_with(TASK_NOTIFICATION) {
                        "self"
                    } else if turns.is_empty() {
                        "first"
                    } else {
                        "parent"
                    };
                    current = Some(ChildTurn {
                        start: at,
                        trigger: trigger.to_string(),
                        end: None,
                        reply: String::new(),
                    });
                    last = Some(at);
                }
            }
        } else if kind == "assistant" {
            last = Some(at);
            let content = match record.get("message") {
                Some(Value::Object(message)) => message.get("content"),
                _ => None,
            };
            if let Some(Value::Array(blocks)) = content {
                for block in blocks {
                    let Some(block) = block.as_object() else {
                        continue;
                    };
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        let text = str_of(block.get("text")).unwrap_or_default();
                        if !text.trim().is_empty() {
                            last_reply = text;
                        }
                    }
                }
            }
            if current.is_none() {
                current = Some(ChildTurn {
                    start: at,
                    trigger: "self".to_string(),
                    end: None,
                    reply: String::new(),
                });
            }
        } else if kind == "system"
            && record.get("subtype").and_then(Value::as_str) == Some(TURN_DURATION)
        {
            if let Some(mut done) = current.take() {
                done.end = Some(at);
                done.reply = last_reply.clone();
                turns.push(done);
            }
        }
    }
    if let Some(done) = current {
        turns.push(done);
    }
    Ok(turns)
}

/// Turns of a Muse session log: a `started` run event to its
/// `terminal` event. A turn started with an empty prompt was
/// started by the agent's own background work.
pub fn muse_turns(path: &Path) -> Result<Vec<ChildTurn>, String> {
    let mut turns: Vec<ChildTurn> = Vec::new();
    let mut current: Option<ChildTurn> = None;
    let mut last_reply = String::new();
    for record in json_lines(path)? {
        let Some(payload) = record.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let Some(recorded) = number_of(record.get("recorded_at")) else {
            continue;
        };
        if payload.get("kind").and_then(Value::as_str) != Some("run") {
            continue;
        }
        let Some(event) = payload.get("event").and_then(Value::as_object) else {
            continue;
        };
        let at = recorded / 1e6;
        match event.get("kind").and_then(Value::as_str) {
            Some("started") => {
                let prompt = str_of(event.get("prompt")).unwrap_or_default();
                let trigger = if turns.is_empty() {
                    "first"
                } else if prompt.is_empty() {
                    "self"
                } else {
                    "parent"
                };
                current = Some(ChildTurn {
                    start: at,
                    trigger: trigger.to_string(),
                    end: None,
                    reply: String::new(),
                });
            }
            Some("assistant_message_committed") => {
                last_reply = str_of(event.get("text")).unwrap_or_default();
            }
            Some("terminal") => {
                if let Some(mut done) = current.take() {
                    done.end = Some(at);
                    done.reply = last_reply.clone();
                    turns.push(done);
                }
            }
            _ => {}
        }
    }
    if let Some(done) = current {
        turns.push(done);
    }
    Ok(turns)
}

/// When a Muse session started and its working directory, from
/// its `route_facts` record; `None` when it records neither in
/// its first records.
pub fn muse_session_cwd(path: &Path) -> Result<Option<(f64, String)>, String> {
    for (count, record) in json_lines(path)?.into_iter().enumerate() {
        if count > MUSE_HEADER_RECORDS {
            return Ok(None);
        }
        let Some(payload) = record.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let Some(recorded) = number_of(record.get("recorded_at")) else {
            continue;
        };
        if payload.get("kind").and_then(Value::as_str) != Some("route_facts") {
            continue;
        }
        let cwd = match payload.get("record") {
            Some(Value::Object(facts)) => str_of(facts.get("cwd")),
            _ => None,
        };
        return Ok(match cwd {
            Some(held) if !held.is_empty() => {
                Some((recorded / 1e6, held.trim_end_matches('/').to_string()))
            }
            _ => None,
        });
    }
    Ok(None)
}

fn muse_logs(sessions: &Path) -> Result<BTreeMap<String, Vec<(f64, PathBuf)>>, String> {
    let mut found: BTreeMap<String, Vec<(f64, PathBuf)>> = BTreeMap::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_session_logs(sessions, 0, &mut paths);
    paths.sort();
    for path in paths {
        if let Some((started, cwd)) = muse_session_cwd(&path)? {
            found.entry(cwd).or_default().push((started, path));
        }
    }
    Ok(found)
}

fn collect_session_logs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 4 {
        let candidate = dir.join("session.jsonl");
        if candidate.is_file() {
            out.push(candidate);
        }
        return;
    }
    for child in sorted_children(dir) {
        if child.is_dir() {
            collect_session_logs(&child, depth + 1, out);
        }
    }
}

/// What a child's last reply asks of its parent: DONE, BLOCKED,
/// ask, await, or progress.
pub fn verdict(reply: &str) -> String {
    let lines: Vec<&str> = reply
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail: Vec<&&str> = lines.iter().rev().take(30).collect();
    for marker in ["DONE:", "BLOCKED:"] {
        if tail.iter().any(|line| line.starts_with(marker)) {
            return marker[..marker.len() - 1].to_string();
        }
    }
    let lowered = reply.to_lowercase();
    if ASK_RE.is_match(&lowered) {
        return "ask".to_string();
    }
    if AWAIT_RE.is_match(&lowered) {
        return "await".to_string();
    }
    "progress".to_string()
}

/// Shell-style pattern match (the oracle's `fnmatchcase`).
pub fn fnmatchcase(name: &str, pattern: &str) -> bool {
    let mut regex = String::from("(?s)^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' => {
                let mut cursor = index + 1;
                if cursor < chars.len() && chars[cursor] == '!' {
                    cursor += 1;
                }
                if cursor < chars.len() && chars[cursor] == ']' {
                    cursor += 1;
                }
                while cursor < chars.len() && chars[cursor] != ']' {
                    cursor += 1;
                }
                if cursor >= chars.len() {
                    regex.push_str("\\[");
                } else {
                    let body: String = chars[index + 1..cursor].iter().collect();
                    let class = if let Some(rest) = body.strip_prefix('!') {
                        format!("^{rest}")
                    } else if let Some(rest) = body.strip_prefix('^') {
                        format!("\\^{rest}")
                    } else {
                        body
                    };
                    regex.push('[');
                    regex.push_str(&class.replace('\\', "\\\\"));
                    regex.push(']');
                    index = cursor;
                }
            }
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
        index += 1;
    }
    regex.push('$');
    Regex::new(&regex).is_ok_and(|compiled| compiled.is_match(name))
}

fn locate_logs(agents: &mut [AgentRecord], sources: &Sources) -> Result<(), String> {
    let muse = match &sources.muse_sessions {
        Some(sessions) => muse_logs(sessions)?,
        None => BTreeMap::new(),
    };
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut order: Vec<usize> = (0..agents.len()).collect();
    order.sort_by(|left, right| agents[*left].created.total_cmp(&agents[*right].created));
    for index in order {
        let agent = &agents[index];
        if agent.harness == "claude" {
            if let (Some(native), Some(projects)) = (
                agent
                    .native_session
                    .as_ref()
                    .filter(|held| !held.is_empty()),
                &sources.claude_projects,
            ) {
                // The oracle interpolates the session id into a
                // glob pattern; match child names against it the
                // same way, whatever type the hit is.
                let pattern = format!("{native}.jsonl");
                let mut hits: Vec<PathBuf> = Vec::new();
                for project in sorted_children(projects) {
                    if !project.is_dir() {
                        continue;
                    }
                    for child in sorted_children(&project) {
                        let name = child
                            .file_name()
                            .map(|held| held.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if fnmatchcase(&name, &pattern) {
                            hits.push(child);
                        }
                    }
                }
                hits.sort();
                agents[index].log = hits.into_iter().next();
            }
        } else if agent.harness == "muse" {
            if let Some(cwd) = &agent.cwd {
                let key = cwd.trim_end_matches('/');
                let created = agent.created;
                let mut candidates: Vec<(f64, PathBuf)> = muse
                    .get(key)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(started, path)| {
                        created - 120.0 <= *started
                            && *started <= created + MUSE_START_WINDOW
                            && !claimed.contains(path)
                    })
                    .collect();
                candidates.sort_by(|left, right| {
                    left.0
                        .total_cmp(&right.0)
                        .then_with(|| left.1.cmp(&right.1))
                });
                if let Some((_, path)) = candidates.into_iter().next() {
                    claimed.insert(path.clone());
                    agents[index].log = Some(path);
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- analysis

fn bisect_right(values: &[f64], target: f64) -> usize {
    let mut low = 0;
    let mut high = values.len();
    while low < high {
        let middle = (low + high) / 2;
        if values[middle] <= target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// Rebuild every child's idle stretches and the parent's response
/// to each turn end.
pub fn analyze(
    sources: &Sources,
    since: Option<f64>,
    until: Option<f64>,
) -> Result<Report, String> {
    if !sources.coordinator_transcript.is_file() {
        return Err(format!(
            "coordinator transcript not found: {}",
            sources.coordinator_transcript.display()
        ));
    }
    if !sources.registry.is_dir() {
        return Err(format!(
            "agent registry not found: {}",
            sources.registry.display()
        ));
    }
    let mut agents: Vec<AgentRecord> = load_agents(&sources.registry, &sources.coordinator)
        .into_iter()
        .filter(|agent| {
            !sources
                .exclude
                .iter()
                .any(|pattern| fnmatchcase(&agent.name, pattern))
        })
        .collect();
    let names: Vec<String> = agents.iter().map(|agent| agent.name.clone()).collect();
    let mut slots: HashMap<String, Vec<AgentRecord>> = HashMap::new();
    for agent in &agents {
        if let Some(slot) = &agent.slot {
            slots.entry(slot.clone()).or_default().push(agent.clone());
        }
    }
    let (turns, inputs, actions) = load_coordinator(
        &sources.coordinator_transcript,
        &names,
        &slots,
        &sources.sweep_marker,
    )?;
    if turns.is_empty() {
        return Err("the coordinator transcript holds no completed turn".to_string());
    }
    let window_start = since.unwrap_or(turns[0].0);
    let window_end = until.unwrap_or(turns[turns.len() - 1].1);
    let messages = load_messages(&sources.registry);
    let mut by_agent: HashMap<String, Vec<Action>> = names
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    for action in actions {
        by_agent
            .entry(action.agent.clone())
            .or_default()
            .push(action);
    }
    for message in &messages {
        if let (Some(list), Some(queued)) = (by_agent.get_mut(&message.owner), message.queued) {
            list.push(Action {
                at: queued,
                verb: "send".to_string(),
                agent: message.owner.clone(),
            });
        }
    }
    for list in by_agent.values_mut() {
        list.sort_by(|left, right| left.at.total_cmp(&right.at));
    }
    for agent in &mut agents {
        agent.stopped = by_agent.get(&agent.name).and_then(|list| {
            list.iter()
                .find(|action| action.verb == "stop" && action.at > agent.created)
                .map(|action| action.at)
        });
    }
    locate_logs(&mut agents, sources)?;

    let starts: Vec<f64> = turns.iter().map(|(start, _)| *start).collect();
    let busy = |at: f64| -> bool {
        let index = bisect_right(&starts, at);
        index > 0 && turns[index - 1].0 <= at && at <= turns[index - 1].1
    };
    let coordinator_queue: Vec<&QueueMessage> = messages
        .iter()
        .filter(|message| message.owner == sources.coordinator)
        .collect();
    let child_messages: Vec<(&QueueMessage, Option<String>)> = coordinator_queue
        .iter()
        .filter(|message| {
            !message.message_id.starts_with(WATCH_ID_PREFIX)
                && !message.message_id.starts_with(CHAT_ID_PREFIX)
        })
        .map(|message| (*message, message_author(&message.text, &names)))
        .collect();
    let watch: Vec<(&QueueMessage, Vec<String>)> = coordinator_queue
        .iter()
        .filter(|message| message.message_id.starts_with(WATCH_ID_PREFIX))
        .map(|message| (*message, watch_agents(&message.text)))
        .collect();
    let mut child_queues: HashMap<String, Vec<&QueueMessage>> = HashMap::new();
    for message in &messages {
        if message.owner != sources.coordinator {
            child_queues
                .entry(message.owner.clone())
                .or_default()
                .push(message);
        }
    }
    let sweeps: Vec<f64> = {
        let mut held: Vec<f64> = inputs
            .iter()
            .filter(|entry| entry.kind == "sweep")
            .map(|entry| entry.at)
            .collect();
        held.sort_by(|left, right| left.total_cmp(right));
        held
    };
    let chats: Vec<f64> = {
        let mut held: Vec<f64> = inputs
            .iter()
            .filter(|entry| entry.kind == "chat")
            .map(|entry| entry.at)
            .collect();
        held.sort_by(|left, right| left.total_cmp(right));
        held
    };

    let mut episodes: Vec<Episode> = Vec::new();
    let mut gaps: Vec<Gap> = Vec::new();
    for agent in &agents {
        let Some(log) = &agent.log else {
            continue;
        };
        if !log.is_file() {
            continue;
        }
        let child = if agent.harness == "claude" {
            claude_turns(log)?
        } else {
            muse_turns(log)?
        };
        let agent_actions = by_agent.get(&agent.name).cloned().unwrap_or_default();
        for (index, turn) in child.iter().enumerate() {
            let Some(turn_end) = turn.end else {
                continue;
            };
            if !(window_start <= turn_end && turn_end <= window_end) {
                continue;
            }
            if agent.stopped.is_some_and(|stopped| turn_end > stopped) {
                continue;
            }
            let following = child.get(index + 1);
            let resumed = following.map(|next| next.start);
            let trigger = match following {
                Some(next) => next.trigger.clone(),
                None => {
                    if agent.stopped.is_some() {
                        "stop".to_string()
                    } else {
                        "open".to_string()
                    }
                }
            };
            let kind = verdict(&turn.reply);
            let gap_end = resumed.or(agent.stopped);
            if let Some(gap_end) = gap_end {
                let waiting = match trigger.as_str() {
                    "self" => "self",
                    "stop" => "stop",
                    _ => "parent",
                };
                gaps.push(Gap {
                    agent: agent.name.clone(),
                    harness: agent.harness.clone(),
                    start: turn_end,
                    end: gap_end.min(window_end),
                    waiting_for: waiting.to_string(),
                    verdict: kind.clone(),
                });
            }
            if trigger == "self" {
                continue;
            }
            let horizon = gap_end.unwrap_or(turn_end + OPEN_HORIZON_SECONDS);
            let looked: Vec<&Action> = agent_actions
                .iter()
                .filter(|action| turn_end < action.at && action.at <= horizon + 5.0)
                .collect();
            let mut observed = looked.first().map(|action| action.at);
            let acting = looked.iter().find(|action| {
                ACTING_VERBS.contains(&action.verb.as_str()) || action.verb == "remove"
            });
            let mut acted = acting.map(|action| action.at);
            let mut act_verb = acting.map(|action| action.verb.clone());
            let own: Vec<&QueueMessage> = child_messages
                .iter()
                .filter(|(_, author)| author.as_deref() == Some(agent.name.as_str()))
                .map(|(message, _)| *message)
                .filter(|message| {
                    message.queued.is_some_and(|queued| {
                        (turn.start - 1.0).max(turn_end - CHILD_MESSAGE_LEAD_SECONDS) <= queued
                            && queued <= horizon
                    })
                })
                .collect();
            let notices: Vec<&QueueMessage> = watch
                .iter()
                .filter(|(_, listed)| listed.iter().any(|held| held == &agent.name))
                .map(|(message, _)| *message)
                .filter(|message| {
                    message
                        .queued
                        .is_some_and(|queued| turn_end < queued && queued <= horizon)
                })
                .collect();
            let mut delivered: Vec<(f64, String)> = Vec::new();
            for (message, label, input_kind) in own
                .iter()
                .map(|message| (*message, "child-message", "child-message"))
                .chain(
                    notices
                        .iter()
                        .map(|message| (*message, "watch-notice", "watch")),
                )
            {
                let mut arrival = message.confirmed;
                if arrival.is_none() {
                    if let Some(queued_at) = message.queued {
                        arrival = inputs
                            .iter()
                            .filter(|entry| {
                                entry.kind == input_kind
                                    && entry.agent.as_deref() == Some(agent.name.as_str())
                                    && queued_at - 1.0 <= entry.at
                                    && entry.at <= horizon
                            })
                            .map(|entry| entry.at)
                            .next();
                    }
                }
                if let Some(at) = arrival {
                    delivered.push((at, label.to_string()));
                }
            }
            for at in &sweeps {
                if turn_end < *at && *at <= horizon {
                    delivered.push((*at, "sweep".to_string()));
                }
            }
            let sweep = sweeps
                .iter()
                .find(|at| turn_end < **at && **at <= horizon)
                .copied();
            let chat = chats
                .iter()
                .find(|at| turn_end < **at && **at <= horizon)
                .copied();
            let first_notice = delivered
                .iter()
                .min_by(|left, right| {
                    left.0
                        .total_cmp(&right.0)
                        .then_with(|| left.1.cmp(&right.1))
                })
                .map(|(_, label)| label.clone());
            let reference = observed.or(acted);
            let mut path = "none".to_string();
            if let Some(reference) = reference {
                let before: Vec<&(f64, String)> = delivered
                    .iter()
                    .filter(|(at, _)| *at <= reference + 1.0)
                    .collect();
                if !before.is_empty() {
                    path = before
                        .iter()
                        .max_by(|left, right| {
                            left.0
                                .total_cmp(&right.0)
                                .then_with(|| left.1.cmp(&right.1))
                        })
                        .map(|(_, label)| label.clone())
                        .unwrap_or_default();
                } else if chat.is_some_and(|held| held <= reference) {
                    path = "chat-turn".to_string();
                } else {
                    path = "coordinator-own".to_string();
                }
            } else if trigger == "parent" {
                if let Some(resumed_at) = resumed {
                    if resumed_at - turn_end <= PRE_QUEUED_SECONDS {
                        let waiting_message = child_queues
                            .get(&agent.name)
                            .map(|queue| {
                                queue.iter().any(|message| {
                                    message.queued.is_some_and(|queued| {
                                        queued <= turn_end + 1.0
                                            && (message.confirmed.is_none()
                                                || message.confirmed.is_some_and(|confirmed| {
                                                    (confirmed - resumed_at).abs()
                                                        <= PRE_QUEUED_SECONDS
                                                }))
                                    })
                                })
                            })
                            .unwrap_or(false);
                        if waiting_message {
                            path = "pre-queued".to_string();
                            observed = Some(resumed_at);
                            acted = Some(resumed_at);
                            act_verb = Some("send".to_string());
                        }
                    }
                }
            }
            let mut queued: Vec<f64> = own
                .iter()
                .chain(notices.iter())
                .filter_map(|message| message.queued)
                .collect();
            if let Some(at) = sweep {
                queued.push(at);
            }
            let notified = queued.iter().copied().reduce(f64::min);
            episodes.push(Episode {
                agent: agent.name.clone(),
                harness: agent.harness.clone(),
                turn_end,
                verdict: kind,
                next_trigger: trigger,
                resumed,
                path,
                first_notice,
                parent_busy: busy(turn_end),
                notified,
                observed,
                acted,
                act_verb,
                reply: turn
                    .reply
                    .chars()
                    .take(240)
                    .collect::<String>()
                    .replace('\n', " "),
            });
        }
    }
    episodes.sort_by(|left, right| left.turn_end.total_cmp(&right.turn_end));
    gaps.sort_by(|left, right| left.start.total_cmp(&right.start));
    Ok(Report {
        coordinator: sources.coordinator.clone(),
        window_start,
        window_end,
        coordinator_turns: turns,
        inputs,
        agents,
        episodes,
        gaps,
        messages,
    })
}

// ---------------------------------------------------------------- reporting

/// CPython `round(value, ndigits)`: the correctly rounded
/// decimal rounding of the exact binary value, ties to even.
/// Rust's precision formatting does exactly that rounding, so
/// round-trip through it (scaling first would shift ties).
pub fn py_round(value: f64, ndigits: i32) -> f64 {
    if !value.is_finite() || ndigits < 0 {
        return value;
    }
    format!("{:.*}", ndigits as usize, value)
        .parse::<f64>()
        .unwrap_or(value)
}

fn py_round_int(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// CPython 3.12+ `sum` over floats: Neumaier compensated
/// summation (a plain left fold can land one ulp away).
pub fn py_sum(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0f64;
    let mut compensation = 0.0f64;
    for value in values {
        let total = sum + value;
        if sum.abs() >= value.abs() {
            compensation += (sum - total) + value;
        } else {
            compensation += (value - total) + sum;
        }
        sum = total;
    }
    sum + compensation
}

/// The sample at the rounded index `fraction * (n - 1)`; `None`
/// for no values.
pub fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(|left, right| left.total_cmp(right));
    let index = py_round_int(fraction * (ordered.len() as f64 - 1.0));
    let index = index.clamp(0, ordered.len() as i64 - 1) as usize;
    Some(ordered[index])
}

fn minutes(value: Option<f64>) -> Value {
    match value {
        None => Value::Null,
        Some(held) => float_value(py_round(held / 60.0, 1)),
    }
}

/// Count, p50, p90 and max of a latency sample, in minutes.
pub fn distribution(values: impl Iterator<Item = Option<f64>>) -> Value {
    let present: Vec<f64> = values.flatten().collect();
    let mut out = Map::new();
    out.insert("n".into(), Value::from(present.len() as i64));
    out.insert("p50_min".into(), minutes(percentile(&present, 0.5)));
    out.insert("p90_min".into(), minutes(percentile(&present, 0.9)));
    out.insert(
        "max_min".into(),
        minutes(present.iter().copied().reduce(f64::max)),
    );
    Value::Object(out)
}

fn float_value(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn split_by(episodes: &[Episode], key: &str) -> Value {
    let mut groups: BTreeMap<String, Vec<&Episode>> = BTreeMap::new();
    for episode in episodes {
        let value = match key {
            "path" => episode.path.clone(),
            "first_notice" => episode
                .first_notice
                .clone()
                .unwrap_or_else(|| "None".to_string()),
            "harness" => episode.harness.clone(),
            "verdict" => episode.verdict.clone(),
            "parent_busy" => episode.parent_busy.to_string().to_lowercase(),
            _ => String::new(),
        };
        groups.entry(value).or_default().push(episode);
    }
    let mut out = Map::new();
    for (name, group) in groups {
        out.insert(
            name,
            distribution(group.iter().map(|episode| episode.latency(episode.acted))),
        );
    }
    Value::Object(out)
}

fn iso_at(at: f64) -> String {
    use chrono::{Datelike as _, Timelike as _};
    let micros = (at * 1e6).round() as i64;
    chrono::DateTime::from_timestamp_micros(micros)
        .map(|held| {
            // CPython strftime("%Y") is unpadded on this platform.
            format!(
                "{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                held.year(),
                held.month(),
                held.day(),
                held.hour(),
                held.minute(),
                held.second()
            )
        })
        .unwrap_or_default()
}

/// The report's figures as one JSON-ready object.
pub fn summary(report: &Report) -> Value {
    let episodes = &report.episodes;
    let mut idle: BTreeMap<String, f64> = BTreeMap::from([
        ("parent".to_string(), 0.0),
        ("self".to_string(), 0.0),
        ("stop".to_string(), 0.0),
    ]);
    for gap in &report.gaps {
        *idle.get_mut(&gap.waiting_for).unwrap() += gap.end - gap.start;
    }
    let span = report.window_end - report.window_start;
    let busy_seconds: f64 = report
        .coordinator_turns
        .iter()
        .filter(|(start, end)| *end > report.window_start && *start < report.window_end)
        .map(|(start, end)| end.min(report.window_end) - start.max(report.window_start))
        .sum();
    let starts: Vec<f64> = report
        .coordinator_turns
        .iter()
        .map(|(start, _)| *start)
        .collect();
    let mut wake: Vec<f64> = Vec::new();
    for episode in episodes {
        if !episode.parent_busy {
            let index = bisect_right(&starts, episode.turn_end);
            if index < starts.len() {
                wake.push(starts[index] - episode.turn_end);
            }
        }
    }
    let delivered: Vec<&QueueMessage> = report
        .messages
        .iter()
        .filter(|message| {
            message.owner == report.coordinator
                && message.queued.is_some()
                && message.confirmed.is_some()
        })
        .collect();
    let delivery = |watch_kind: u8| {
        distribution(delivered.iter().map(|message| {
            let is_watch = message.message_id.starts_with(WATCH_ID_PREFIX);
            let is_chat = message.message_id.starts_with(CHAT_ID_PREFIX);
            let keep = match watch_kind {
                0 => !is_watch && !is_chat,
                1 => is_watch,
                _ => is_chat,
            };
            if keep {
                Some(message.confirmed.unwrap() - message.queued.unwrap())
            } else {
                None
            }
        }))
    };
    let mut coordinator = Map::new();
    coordinator.insert(
        "turns".into(),
        Value::from(report.coordinator_turns.len() as i64),
    );
    coordinator.insert(
        "busy_fraction".into(),
        if span > 0.0 {
            float_value(py_round(busy_seconds / span, 3))
        } else {
            Value::Null
        },
    );
    let mut input_counts: BTreeMap<String, i64> = BTreeMap::new();
    for entry in &report.inputs {
        if report.window_start <= entry.at && entry.at <= report.window_end {
            let key = if entry.mid_turn {
                format!("{}/mid-turn", entry.kind)
            } else {
                entry.kind.clone()
            };
            *input_counts.entry(key).or_insert(0) += 1;
        }
    }
    let mut inputs_obj = Map::new();
    for (key, count) in input_counts {
        inputs_obj.insert(key, Value::from(count));
    }
    coordinator.insert("inputs".into(), Value::Object(inputs_obj));
    coordinator.insert(
        "next_turn_after_child_end_when_idle".into(),
        distribution(wake.iter().map(|held| Some(*held))),
    );
    let mut queue_delivery = Map::new();
    queue_delivery.insert("from_children".into(), delivery(0));
    queue_delivery.insert("watch_notices".into(), delivery(1));
    queue_delivery.insert("chat".into(), delivery(2));
    let idle_total: f64 = py_sum(idle.values().copied());
    let mut idle_hours = Map::new();
    for (key, value) in &idle {
        idle_hours.insert(key.clone(), float_value(py_round(value / 3600.0, 1)));
    }
    let mut out = Map::new();
    out.insert(
        "window".into(),
        Value::Array(vec![
            Value::String(iso_at(report.window_start)),
            Value::String(iso_at(report.window_end)),
        ]),
    );
    out.insert(
        "children_with_logs".into(),
        Value::from(
            report
                .agents
                .iter()
                .filter(|agent| agent.log.is_some())
                .count() as i64,
        ),
    );
    out.insert("episodes".into(), Value::from(episodes.len() as i64));
    out.insert("coordinator".into(), Value::Object(coordinator));
    out.insert(
        "act_latency".into(),
        distribution(
            episodes
                .iter()
                .map(|episode| episode.latency(episode.acted)),
        ),
    );
    out.insert(
        "observe_latency".into(),
        distribution(
            episodes
                .iter()
                .map(|episode| episode.latency(episode.observed)),
        ),
    );
    out.insert(
        "notify_latency".into(),
        distribution(episodes.iter().map(|episode| {
            episode
                .notified
                .map(|notified| (notified - episode.turn_end).max(0.0))
        })),
    );
    out.insert("act_latency_by_path".into(), split_by(episodes, "path"));
    out.insert(
        "act_latency_by_first_notice".into(),
        split_by(episodes, "first_notice"),
    );
    out.insert(
        "act_latency_by_harness".into(),
        split_by(episodes, "harness"),
    );
    out.insert(
        "act_latency_by_verdict".into(),
        split_by(episodes, "verdict"),
    );
    out.insert(
        "act_latency_by_parent_busy".into(),
        split_by(episodes, "parent_busy"),
    );
    out.insert(
        "queue_delivery_to_coordinator".into(),
        Value::Object(queue_delivery),
    );
    out.insert("child_idle_hours".into(), Value::Object(idle_hours));
    out.insert(
        "child_idle_waiting_on_parent_fraction".into(),
        if idle_total > 0.0 {
            float_value(py_round((idle["parent"] + idle["stop"]) / idle_total, 3))
        } else {
            Value::Null
        },
    );
    out.insert(
        "parent_gap".into(),
        distribution(
            report
                .gaps
                .iter()
                .filter(|gap| gap.waiting_for == "parent")
                .map(|gap| Some(gap.end - gap.start)),
        ),
    );
    out.insert(
        "done_until_stop".into(),
        distribution(
            report
                .gaps
                .iter()
                .filter(|gap| gap.waiting_for == "stop" && gap.verdict == "DONE")
                .map(|gap| Some(gap.end - gap.start)),
        ),
    );
    Value::Object(out)
}

/// One JSON-ready row per episode.
pub fn episode_rows(report: &Report) -> Value {
    let mut rows = Vec::new();
    for episode in &report.episodes {
        let mut row = Map::new();
        row.insert("agent".into(), Value::String(episode.agent.clone()));
        row.insert("harness".into(), Value::String(episode.harness.clone()));
        row.insert("turn_end".into(), Value::String(iso_at(episode.turn_end)));
        row.insert("verdict".into(), Value::String(episode.verdict.clone()));
        row.insert("next".into(), Value::String(episode.next_trigger.clone()));
        row.insert("path".into(), Value::String(episode.path.clone()));
        row.insert(
            "first_notice".into(),
            episode
                .first_notice
                .clone()
                .map_or(Value::Null, Value::String),
        );
        row.insert("parent_busy".into(), Value::Bool(episode.parent_busy));
        row.insert(
            "notify_min".into(),
            minutes(
                episode
                    .notified
                    .map(|notified| (notified - episode.turn_end).max(0.0)),
            ),
        );
        row.insert(
            "observe_min".into(),
            minutes(episode.latency(episode.observed)),
        );
        row.insert("act_min".into(), minutes(episode.latency(episode.acted)));
        row.insert(
            "act_verb".into(),
            episode.act_verb.clone().map_or(Value::Null, Value::String),
        );
        row.insert("reply".into(), Value::String(episode.reply.clone()));
        rows.push(Value::Object(row));
    }
    Value::Array(rows)
}

fn py_float(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let rendered = format!("{value:?}");
    match rendered.split_once('e') {
        Some((mantissa, exponent)) => {
            let exponent: i64 = exponent.parse().unwrap_or(0);
            format!("{mantissa}e{exponent:+03}")
        }
        None => rendered,
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

fn py_dumps_into(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                out.push_str(&integer.to_string());
            } else if let Some(unsigned) = number.as_u64() {
                out.push_str(&unsigned.to_string());
            } else if let Some(float) = number.as_f64() {
                out.push_str(&py_float(float));
            } else {
                out.push('0');
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
                out.push_str(&" ".repeat(level + 1));
                py_dumps_into(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(level));
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
                out.push_str(&" ".repeat(level + 1));
                py_json_string(key, out);
                out.push_str(": ");
                py_dumps_into(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(level));
            out.push('}');
        }
    }
}

/// CPython `json.dumps(value, indent=1)` with ASCII escaping.
pub fn py_dumps(value: &Value) -> String {
    let mut out = String::new();
    py_dumps_into(value, 0, &mut out);
    out
}

fn stats_cells(stats: &Value) -> (String, String, String, String) {
    let get = |key: &str| -> String {
        match stats.get(key) {
            Some(Value::Null) | None => "None".to_string(),
            Some(Value::Number(number)) => {
                if let Some(integer) = number.as_i64() {
                    integer.to_string()
                } else if let Some(float) = number.as_f64() {
                    py_float(float)
                } else {
                    "None".to_string()
                }
            }
            _ => "None".to_string(),
        }
    };
    (get("n"), get("p50_min"), get("p90_min"), get("max_min"))
}

/// A short human-readable report.
pub fn format_markdown(report: &Report) -> String {
    let figures = summary(report);
    let window = figures.get("window").cloned().unwrap_or(Value::Null);
    let window_text: Vec<String> = match &window {
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => vec![String::new(), String::new()],
    };
    let count = |key: &str| -> String {
        figures
            .get(key)
            .and_then(Value::as_i64)
            .map_or_else(|| "None".to_string(), |held| held.to_string())
    };
    let mut lines: Vec<String> = vec![
        "# Parent-child responsiveness".to_string(),
        String::new(),
        format!(
            "Window {} to {}; {} children with logs; {} episodes.",
            window_text.first().cloned().unwrap_or_default(),
            window_text.get(1).cloned().unwrap_or_default(),
            count("children_with_logs"),
            count("episodes")
        ),
        String::new(),
        "| Split | Group | n | p50 min | p90 min | max min |".to_string(),
        "|---|---|---:|---:|---:|---:|".to_string(),
    ];
    for split in [
        "act_latency_by_path",
        "act_latency_by_first_notice",
        "act_latency_by_harness",
        "act_latency_by_verdict",
        "act_latency_by_parent_busy",
    ] {
        if let Some(Value::Object(groups)) = figures.get(split) {
            for (group, stats) in groups {
                let (n, p50, p90, max) = stats_cells(stats);
                lines.push(format!(
                    "| {} | {group} | {n} | {p50} | {p90} | {max} |",
                    split.strip_prefix("act_latency_by_").unwrap_or(split)
                ));
            }
        }
    }
    let mut subset = Map::new();
    for key in [
        "coordinator",
        "act_latency",
        "notify_latency",
        "queue_delivery_to_coordinator",
        "child_idle_hours",
        "child_idle_waiting_on_parent_fraction",
        "parent_gap",
        "done_until_stop",
    ] {
        subset.insert(
            key.to_string(),
            figures.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    lines.push(String::new());
    lines.push("```json".to_string());
    lines.push(py_dumps(&Value::Object(subset)));
    lines.push("```".to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// Analyze, optionally write per-episode rows, and return the
/// formatted report.
pub fn run(
    sources: &Sources,
    since: Option<f64>,
    until: Option<f64>,
    output_format: &str,
    episodes_file: Option<&Path>,
) -> Result<String, String> {
    if output_format != "json" && output_format != "markdown" {
        return Err(format!(
            "unsupported format {}",
            crate::json::py_repr_str(output_format)
        ));
    }
    let report = analyze(sources, since, until)?;
    if let Some(path) = episodes_file {
        let text = format!("{}\n", py_dumps(&episode_rows(&report)));
        std::fs::write(path, text).map_err(|error| crate::json::os_error(&error, path))?;
    }
    if output_format == "json" {
        return Ok(format!("{}\n", py_dumps(&summary(&report))));
    }
    Ok(format_markdown(&report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_round_matches_cpython() {
        // Values verified against CPython round().
        for (value, ndigits, expected) in [
            (0.05, 1, 0.1),
            (0.25, 1, 0.2),
            (0.35, 1, 0.3),
            (0.45, 1, 0.5),
            (2.675, 2, 2.67),
            (1.005, 2, 1.0),
            (0.125, 1, 0.1),
            (0.15, 1, 0.1),
            (8.25, 1, 8.2),
            (2.5, 1, 2.5),
        ] {
            assert_eq!(py_round(value, ndigits), expected, "{value} @{ndigits}");
        }
        assert_eq!(py_round_int(1.5), 2);
        assert_eq!(py_round_int(2.5), 2);
        assert_eq!(py_round_int(0.5), 0);
    }

    #[test]
    fn py_sum_matches_cpython_neumaier() {
        // Verified against CPython 3.12 sum().
        assert_eq!(py_sum([1e16, 1.0, -1e16, 1.0].into_iter()), 2.0);
        let cost = |input: f64, output: f64| (input + output * 5.0) / 1_000_000.0;
        let costs = [
            cost(357000.0, 42500.0),
            cost(261000.0, 8100.0),
            cost(24000.0, 1200.0),
            cost(60000.0, 3800.0),
            cost(50000.0, 2900.0),
            cost(60000.0, 3800.0),
            cost(50000.0, 2900.0),
            cost(60000.0, 3800.0),
            cost(50000.0, 2900.0),
            cost(60000.0, 3800.0),
            cost(50000.0, 2900.0),
        ];
        assert_eq!(format!("{:.2}", py_sum(costs.into_iter())), "1.48");
    }

    #[test]
    fn iso_instants_match_fromisoformat_acceptance() {
        for accepted in [
            "2026-01-05T12:00:00Z",
            "2026-01-05 12:00:00+00:00",
            "2026-01-05_12:00:00.5+00",
            "20260105T120000+00:00:00",
            "2026-01-05T12:00:00,500Z",
            "2026-01-05T12:00+0000",
        ] {
            assert!(
                parse_iso_instant(accepted).is_some_and(|(_, offset)| offset),
                "{accepted}"
            );
        }
        for rejected in [
            "2026-1-5T12:00:00Z",
            "2026-01-05T12:00:00+24:00",
            "not-an-instant",
            "2026-13-05T12:00:00Z",
        ] {
            assert!(parse_iso_instant(rejected).is_none(), "{rejected}");
        }
        let (at, offset) = parse_iso_instant("2026-01-05T12:00:00").unwrap();
        assert!(!offset && at > 0.0);
        let (zulu, _) = parse_iso_instant("2026-01-05T12:00:00Z").unwrap();
        let (plus, _) = parse_iso_instant("2026-01-05T14:00:00+02:00").unwrap();
        assert_eq!(zulu, plus);
    }

    #[test]
    fn split_lines_uses_cpython_boundaries() {
        assert_eq!(split_lines("a\rb\r\nc\x0bd"), vec!["a", "b", "c", "d"]);
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines(""), Vec::<String>::new());
    }
}
