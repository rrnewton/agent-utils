//! Per-agent turn-runner dispatch core (Track B slice B3-6).
//!
//! Pure, independently testable pieces of `py/agentctl/foreign/agent_runner.py`:
//! harness argv builders, Codex JSON event observation, Antigravity log
//! parsing/outcome classification, and the bounded Muse JSONL validator.
//! The daemon loop (inbox intake, transcript/event writing, registry status
//! updates, watchdog threads) wires these to [`super::ownership`] spawns in
//! the CLI slices; nothing here spawns or signals a process.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::registry::{agent_dir_at, AgentRecord, Message};

/// Harness executable env overrides, read per call in Rust (Python reads
/// them once at module import).
pub fn codex_bin() -> String {
    std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_owned())
}

/// Antigravity executable (`$AGY_BIN`).
pub fn agy_bin() -> String {
    std::env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_owned())
}

/// Muse executable (`$MUSE_BIN`).
pub fn muse_bin() -> String {
    std::env::var("MUSE_BIN").unwrap_or_else(|_| "muse".to_owned())
}

/// Per-turn timeout seconds (`$SUBAGENTS_TURN_TIMEOUT`, default 3600).
pub fn turn_timeout_s() -> i64 {
    turn_timeout_s_from(std::env::var("SUBAGENTS_TURN_TIMEOUT").ok().as_deref())
}

fn turn_timeout_s_from(value: Option<&str>) -> i64 {
    value
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(3600)
}

/// The per-turn Codex answer file (`-o` target) for one turn.
pub fn codex_answer_path(base: &Path, name: &str, seq: i64) -> PathBuf {
    agent_dir_at(base, name)
        .unwrap_or_else(|_| base.join("state").join(name))
        .join(format!("codex-answer-{seq:012}.txt"))
}

/// Build one Codex exec/resume argv (Python `_build_codex_argv`).
/// `effort_env` is `$SUBAGENT_EFFORT`; the per-turn effort wins.
pub fn build_codex_argv(
    base: &Path,
    record: &AgentRecord,
    msg: &Message,
    effort_env: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![codex_bin(), "exec".to_owned()];
    if let Some(session_id) = &record.session_id {
        argv.push("resume".to_owned());
        argv.push(session_id.clone());
    }
    if let Some(model) = msg.model.as_deref().or(record.model.as_deref()) {
        argv.push("-m".to_owned());
        argv.push(model.to_owned());
    }
    if let Some(effort) = msg.effort.as_deref().or(effort_env) {
        argv.push("-c".to_owned());
        argv.push(format!(
            "model_reasoning_effort={}",
            serde_json::to_string(effort).expect("string serializes")
        ));
    }
    if record.codex_bypass_permissions {
        argv.push("--dangerously-bypass-approvals-and-sandbox".to_owned());
    }
    argv.push("--skip-git-repo-check".to_owned());
    argv.push("--json".to_owned());
    argv.push("-o".to_owned());
    argv.push(
        codex_answer_path(base, &record.name, msg.seq)
            .to_string_lossy()
            .into_owned(),
    );
    argv.push("-".to_owned());
    argv
}

/// The per-turn Antigravity log path.
pub fn agy_turn_log_path(base: &Path, name: &str, seq: i64) -> PathBuf {
    agent_dir_at(base, name)
        .unwrap_or_else(|_| base.join("state").join(name))
        .join(format!("agy-turn-{seq:012}.log"))
}

/// Build one Antigravity print argv (Python `_build_agy_argv`).
pub fn build_agy_argv(
    record: &AgentRecord,
    msg: &Message,
    log_path: &Path,
    turn_timeout_s: i64,
) -> Vec<String> {
    let mut argv = vec![agy_bin()];
    if let Some(model) = msg.model.as_deref().or(record.model.as_deref()) {
        argv.push("--model".to_owned());
        argv.push(model.to_owned());
    }
    if let Some(session_id) = &record.session_id {
        argv.push("--conversation".to_owned());
        argv.push(session_id.clone());
    }
    argv.push("--add-dir".to_owned());
    argv.push(record.cwd.clone());
    argv.push("--log-file".to_owned());
    argv.push(log_path.to_string_lossy().into_owned());
    argv.push("--print-timeout".to_owned());
    argv.push(format!("{turn_timeout_s}s"));
    argv.push("--print".to_owned());
    argv.push(msg.text.clone());
    argv
}

const MUSE_RUNNER_OWNED_OPTIONS: [&str; 6] = [
    "--",
    "--api-key-stdin",
    "--json",
    "--model",
    "--prompt-file",
    "--session-id",
];

/// Build one exact Muse exec turn around the caller-owned session id.
///
/// Muse parses options before `exec` as TUI options, so the subcommand
/// comes immediately after the executable on every turn. Errors carry the
/// Python `ValueError` messages verbatim.
pub fn build_muse_argv(record: &AgentRecord, msg: &Message) -> Result<Vec<String>, String> {
    let Some(session_id) = &record.session_id else {
        return Err("Muse worker has no caller-owned session id".to_owned());
    };
    for item in &record.harness_args {
        let key = item.split('=').next().unwrap_or(item);
        if MUSE_RUNNER_OWNED_OPTIONS.contains(&key) || item == "exec" || item == "resume" {
            return Err(format!(
                "Muse harness arguments cannot override runner-owned argument '{key}'"
            ));
        }
    }
    let mut argv = vec![muse_bin(), "exec".to_owned()];
    argv.extend(record.harness_args.iter().cloned());
    if let Some(model) = msg.model.as_deref().or(record.model.as_deref()) {
        argv.push("--model".to_owned());
        argv.push(model.to_owned());
    }
    argv.push("--json".to_owned());
    argv.push("--session-id".to_owned());
    argv.push(session_id.clone());
    argv.push("--".to_owned());
    argv.push(msg.text.clone());
    Ok(argv)
}

fn take_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// One-line live summary lines for a Codex `--json` event (Python
/// `_pane_echo`, returned instead of printed).
pub fn pane_echo_lines(stripped: &str) -> Vec<String> {
    if stripped.is_empty() {
        return Vec::new();
    }
    let Ok(event) = serde_json::from_str::<Value>(stripped) else {
        return vec![take_chars(stripped, 200)];
    };
    let Some(event) = event.as_object() else {
        return Vec::new();
    };
    let item = event.get("item").and_then(Value::as_object);
    let item_type = item
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match (event.get("type").and_then(Value::as_str), item_type) {
        (Some("item.started"), "command_execution") => {
            let command = item
                .and_then(|item| item.get("command"))
                .map(scalar_text)
                .unwrap_or_default();
            vec![format!("$ {}", take_chars(&command, 160))]
        }
        (Some("item.completed"), "agent_message") => {
            let text = item
                .and_then(|item| item.get("text"))
                .map(scalar_text)
                .unwrap_or_default();
            let lines: Vec<&str> = text.trim().lines().collect();
            let mut out: Vec<String> = lines
                .iter()
                .take(12)
                .map(|line| format!("| {}", take_chars(line, 200)))
                .collect();
            if lines.len() > 12 {
                out.push(format!(
                    "| ... ({} more lines in transcript)",
                    lines.len() - 12
                ));
            }
            out
        }
        (Some("thread.started"), _) => {
            let thread_id = event
                .get("thread_id")
                .map(scalar_text)
                .unwrap_or_else(|| "?".to_owned());
            vec![format!("[session {thread_id}]")]
        }
        _ => Vec::new(),
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// Session/error capture from Codex `--json` event lines.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexTurnObserver {
    /// `thread.started` session id, when seen.
    pub captured_session: Option<String>,
    /// Last `error`/`turn.failed` message, when seen.
    pub captured_error: Option<String>,
}

impl CodexTurnObserver {
    /// Observe one event line (non-JSON lines are ignored, as in Python).
    pub fn push_line(&mut self, stripped: &str) {
        if stripped.is_empty() {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(stripped) else {
            return;
        };
        let Some(event) = event.as_object() else {
            return;
        };
        if event.get("type").and_then(Value::as_str) == Some("thread.started") {
            if let Some(thread_id) = event.get("thread_id").and_then(Value::as_str) {
                self.captured_session = Some(thread_id.to_owned());
            }
        }
        if matches!(
            event.get("type").and_then(Value::as_str),
            Some("error") | Some("turn.failed")
        ) {
            let message = event
                .get("message")
                .filter(|message| !matches!(message, Value::Null | Value::Bool(false)))
                .or_else(|| {
                    event
                        .get("error")
                        .and_then(Value::as_object)
                        .and_then(|error| error.get("message"))
                });
            if let Some(message) = message.and_then(Value::as_str).filter(|m| !m.is_empty()) {
                self.captured_error = Some(message.to_owned());
            }
        }
    }
}

fn agy_session_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Stream completed for ([0-9a-fA-F-]{36})").expect("static regex"))
}

fn agy_resolved_model_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"Propagating selected model override to backend: label="([^"]+)""#)
            .expect("static regex")
    })
}

fn agy_quota_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::RegexBuilder::new(r"\bRESOURCE_EXHAUSTED\b|\bcode 429\b|Individual quota reached")
            .case_insensitive(true)
            .build()
            .expect("static regex")
    })
}

fn agy_reset_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::RegexBuilder::new(r"Resets in\s+([0-9]+(?:d|h|m|s)(?:[0-9]+(?:d|h|m|s))*)")
            .case_insensitive(true)
            .build()
            .expect("static regex")
    })
}

fn agy_auth_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::RegexBuilder::new(r"error getting token source|not logged into Antigravity")
            .case_insensitive(true)
            .build()
            .expect("static regex")
    })
}

/// The last Antigravity session id in a turn log, lower-cased.
pub fn capture_agy_session_id_from_text(text: &str) -> Option<String> {
    agy_session_re()
        .captures_iter(text)
        .last()
        .map(|captures| captures[1].to_lowercase())
}

/// [`capture_agy_session_id_from_text`] for a log file, when it exists.
pub fn capture_agy_session_id(log_path: &Path) -> Option<String> {
    if !log_path.exists() {
        return None;
    }
    let bytes = std::fs::read(log_path).ok()?;
    capture_agy_session_id_from_text(&String::from_utf8_lossy(&bytes))
}

/// The `===AGY MODEL …===` transcript note. `log_text` is `None` when the
/// turn log file is missing.
pub fn agy_model_note_from_text(
    requested: Option<&str>,
    log_text: Option<&str>,
    seq: i64,
) -> String {
    let requested_note = requested.unwrap_or("<agy-default>");
    let Some(log_text) = log_text else {
        return format!(
            "===AGY MODEL WARNING seq={seq} requested={requested_note} resolved=<unknown> reason=missing-log==="
        );
    };
    let resolved = agy_resolved_model_re()
        .captures_iter(log_text)
        .last()
        .map(|captures| captures[1].to_owned())
        .unwrap_or_else(|| "<agy-default>".to_owned());
    let mut reasons = Vec::new();
    if requested.is_some_and(|requested| resolved != requested) {
        reasons.push("requested-name-did-not-match-resolved-name");
        if log_text.contains("not in local config") || log_text.contains("defaulting to") {
            reasons.push("agy-reported-defaulting");
        }
    }
    if reasons.is_empty() {
        format!("===AGY MODEL seq={seq} requested={requested_note} resolved={resolved}===")
    } else {
        format!(
            "===AGY MODEL WARNING seq={seq} requested={requested_note} resolved={resolved} reason={}===",
            reasons.join(",")
        )
    }
}

/// Read a turn log lossily; the second element is the warning line Python
/// appends when the read fails.
pub fn read_agy_turn_log(log_path: &Path) -> (String, Option<String>) {
    if !log_path.exists() {
        return (String::new(), None);
    }
    match std::fs::read(log_path) {
        Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), None),
        Err(error) => (
            String::new(),
            Some(format!(
                "===AGY LOG WARNING log={} reason={}: {error}===",
                log_path.display(),
                python_os_error_name(&error)
            )),
        ),
    }
}

fn python_os_error_name(error: &std::io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(code) if code == libc::EISDIR => "IsADirectoryError",
        Some(code) if code == libc::EACCES || code == libc::EPERM => "PermissionError",
        Some(code) if code == libc::ENOENT => "FileNotFoundError",
        _ => "OSError",
    }
}

/// A classified Antigravity turn outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgyOutcome {
    /// Sentinel `rc=` label.
    pub rc_label: String,
    /// Registry status to record.
    pub status: String,
    /// Extra `===TURN-DONE …===` fields (for example `reset_in=…`).
    pub done_fields: Vec<String>,
    /// Message to publish when the turn produced no output.
    pub message_if_empty: String,
}

/// Classify an Antigravity outcome from its combined output corpus.
pub fn classify_agy_outcome(
    raw_rc_label: &str,
    stdout_text: &str,
    stderr_text: &str,
    turn_log_text: &str,
) -> AgyOutcome {
    let corpus = format!("{stdout_text}\n{stderr_text}\n{turn_log_text}");
    if agy_quota_re().is_match(&corpus) {
        let reset = agy_reset_re()
            .captures_iter(&corpus)
            .last()
            .map(|captures| captures[1].to_owned());
        let (done_fields, reset_note) = match &reset {
            Some(reset_in) => (
                vec![format!("reset_in={reset_in}")],
                format!(" reset_in={reset_in}"),
            ),
            None => (Vec::new(), String::new()),
        };
        return AgyOutcome {
            rc_label: "quota_exhausted".to_owned(),
            status: "quota_exhausted".to_owned(),
            done_fields,
            message_if_empty: format!("[AGY ERROR] RESOURCE_EXHAUSTED (code 429).{reset_note}")
                .trim()
                .to_owned(),
        };
    }
    if agy_auth_re().is_match(&corpus) {
        return AgyOutcome {
            rc_label: "auth_error".to_owned(),
            status: "auth_error".to_owned(),
            done_fields: Vec::new(),
            message_if_empty:
                "[AGY ERROR] Antigravity auth error: not logged in; human re-login required."
                    .to_owned(),
        };
    }
    AgyOutcome {
        rc_label: raw_rc_label.to_owned(),
        status: if raw_rc_label == "0" { "idle" } else { "error" }.to_owned(),
        done_fields: Vec::new(),
        message_if_empty: String::new(),
    }
}

/// Maximum Muse JSONL events accepted in one turn.
pub const MUSE_MAX_EVENTS: usize = 100_000;
/// Maximum bytes of one Muse JSONL event line.
pub const MUSE_MAX_EVENT_BYTES: usize = 1_048_576;
/// Maximum total Muse JSONL bytes in one turn.
pub const MUSE_MAX_TRANSCRIPT_BYTES: usize = 64 * 1_048_576;
/// Maximum bytes of the Muse final answer text.
pub const MUSE_MAX_ANSWER_BYTES: usize = 4 * 1_048_576;

const MUSE_WORKSPACE_RECORD_KEYS: [&str; 6] = [
    "command_id",
    "commit",
    "dirty",
    "reference",
    "vcs",
    "workspace_root",
];

/// Recognize Muse's sole observed post-terminal bookkeeping event.
/// Accepted only once by the caller and never changes the outcome; its
/// version-1 shape and command binding stay strict.
pub fn is_muse_workspace_epilogue(
    event: &serde_json::Map<String, Value>,
    payload: &serde_json::Map<String, Value>,
    command_id: Option<&str>,
) -> bool {
    let Some(command_id) = command_id else {
        return false;
    };
    if event.get("payload_type").and_then(Value::as_str)
        != Some("session.workspace_branch.observed")
        || event.get("payload_schema_version").and_then(Value::as_i64) != Some(1)
        || event.get("record_type").and_then(Value::as_str) != Some("event")
        || event.get("durability").and_then(Value::as_str) != Some("durable")
        || event.get("causation_id").and_then(Value::as_str) != Some(command_id)
        || payload.len() != 3
        || payload.get("command_id").and_then(Value::as_str) != Some(command_id)
        || payload.get("kind").and_then(Value::as_str) != Some("workspace_branch_observed")
    {
        return false;
    }
    let Some(record) = payload.get("record").and_then(Value::as_object) else {
        return false;
    };
    let keys: std::collections::HashSet<&str> = record.keys().map(String::as_str).collect();
    let required: std::collections::HashSet<&str> =
        ["command_id", "commit", "reference", "vcs", "workspace_root"]
            .into_iter()
            .collect();
    let allowed: std::collections::HashSet<&str> = MUSE_WORKSPACE_RECORD_KEYS.into_iter().collect();
    if !required.is_subset(&keys)
        || !keys.is_subset(&allowed)
        || record.get("command_id").and_then(Value::as_str) != Some(command_id)
        || !record.get("commit").is_some_and(Value::is_string)
        || record.get("vcs").and_then(Value::as_str) != Some("git")
        || !record.get("workspace_root").is_some_and(Value::is_string)
        || record.get("dirty").is_some_and(|dirty| !dirty.is_boolean())
    {
        return false;
    }
    let Some(reference) = record.get("reference").and_then(Value::as_object) else {
        return false;
    };
    reference.len() == 2
        && reference.get("kind").and_then(Value::as_str) == Some("branch")
        && reference.get("name").is_some_and(Value::is_string)
}

/// What one fed Muse JSONL line did to the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuseLineOutcome {
    /// Counted and valid so far; the caller writes it to the transcript.
    Written,
    /// Protocol failure. `written` says whether Python writes the line to
    /// the transcript before failing (it writes before JSON parsing).
    /// `kill` marks the failures Python stops immediately (size bounds).
    ProtocolError {
        /// Human-readable protocol failure.
        message: String,
        /// Whether the line reaches the transcript.
        written: bool,
        /// Whether Python kills the harness at this point in the loop.
        kill: bool,
    },
}

/// Bounded Muse JSONL turn validator with a stable session id.
#[derive(Debug, Clone)]
pub struct MuseTurnValidator {
    session_id: String,
    event_count: usize,
    transcript_bytes: usize,
    previous_sequence: Option<i128>,
    accepted_command_id: Option<String>,
    terminal: Option<String>,
    saw_workspace_epilogue: bool,
    answer: String,
    protocol_error: Option<String>,
}

impl MuseTurnValidator {
    /// Start validating one turn for `session_id`.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            event_count: 0,
            transcript_bytes: 0,
            previous_sequence: None,
            accepted_command_id: None,
            terminal: None,
            saw_workspace_epilogue: false,
            answer: String::new(),
            protocol_error: None,
        }
    }

    /// The accepted command id, once `runtime.command.accepted` was seen.
    pub fn accepted_command_id(&self) -> Option<&str> {
        self.accepted_command_id.as_deref()
    }

    /// The terminal outcome (`completed`/`failed`/`cancelled`), once seen.
    pub fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    /// The final answer text from the terminal event.
    pub fn answer(&self) -> &str {
        &self.answer
    }

    /// The first protocol failure, when one was recorded.
    pub fn protocol_error(&self) -> Option<&str> {
        self.protocol_error.as_deref()
    }

    fn fail(&mut self, message: String, written: bool, kill: bool) -> MuseLineOutcome {
        self.protocol_error = Some(message.clone());
        MuseLineOutcome::ProtocolError {
            message,
            written,
            kill,
        }
    }

    /// Feed one raw stdout line (including its newline, as read).
    pub fn push_line(&mut self, encoded: &[u8]) -> MuseLineOutcome {
        let Ok(line) = std::str::from_utf8(encoded) else {
            return self.fail(
                "Muse JSONL Unicode decode failure: invalid utf-8 sequence".to_owned(),
                false,
                false,
            );
        };
        if encoded.len() > MUSE_MAX_EVENT_BYTES {
            return self.fail(
                format!("Muse JSONL event exceeds {MUSE_MAX_EVENT_BYTES} bytes"),
                false,
                true,
            );
        }
        self.transcript_bytes += encoded.len();
        if self.transcript_bytes > MUSE_MAX_TRANSCRIPT_BYTES {
            return self.fail(
                format!("Muse turn transcript exceeds {MUSE_MAX_TRANSCRIPT_BYTES} bytes"),
                false,
                true,
            );
        }
        self.event_count += 1;
        if self.event_count > MUSE_MAX_EVENTS {
            return self.fail(
                format!("Muse turn exceeds {MUSE_MAX_EVENTS} events"),
                false,
                true,
            );
        }
        // Python writes the line to the transcript before parsing it.
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(error) => {
                return self.fail(format!("Muse emitted invalid JSONL: {error}"), true, false);
            }
        };
        let Some(event) = event
            .as_object()
            .filter(|event| event.get("schema_version").and_then(Value::as_i64) == Some(1))
        else {
            return self.fail(
                "Muse emitted an unsupported event envelope".to_owned(),
                true,
                false,
            );
        };
        let stream_ok = event
            .get("stream")
            .and_then(Value::as_object)
            .is_some_and(|stream| {
                stream.get("kind").and_then(Value::as_str) == Some("session")
                    && stream.get("id").and_then(Value::as_str) == Some(self.session_id.as_str())
            });
        if !stream_ok {
            return self.fail(
                "Muse event session identity changed".to_owned(),
                true,
                false,
            );
        }
        let sequence = event.get("sequence").and_then(|value| match value {
            Value::Number(number) => number
                .as_i64()
                .map(i128::from)
                .or_else(|| number.as_u64().map(i128::from)),
            _ => None,
        });
        let Some(sequence) = sequence.filter(|sequence| {
            *sequence >= 0
                && self
                    .previous_sequence
                    .is_none_or(|previous| *sequence > previous)
        }) else {
            return self.fail(
                "Muse event sequence is invalid or non-monotonic".to_owned(),
                true,
                false,
            );
        };
        self.previous_sequence = Some(sequence);
        let empty = serde_json::Map::new();
        let payload = event
            .get("payload")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        if self.terminal.is_some() {
            if !self.saw_workspace_epilogue
                && is_muse_workspace_epilogue(event, payload, self.accepted_command_id.as_deref())
            {
                self.saw_workspace_epilogue = true;
                return MuseLineOutcome::Written;
            }
            return self.fail(
                "Muse emitted an unexpected event after its terminal outcome".to_owned(),
                true,
                false,
            );
        }
        if event.get("payload_type").and_then(Value::as_str) == Some("runtime.command.accepted") {
            let command_id = payload.get("command_id").and_then(Value::as_str);
            if self.accepted_command_id.is_some()
                || payload.get("command_kind").and_then(Value::as_str) != Some("turn.submit")
                || !command_id.is_some_and(|id| !id.is_empty())
            {
                return self.fail(
                    "Muse emitted an invalid or duplicate command acceptance".to_owned(),
                    true,
                    false,
                );
            }
            self.accepted_command_id = command_id.map(str::to_owned);
        }
        if event.get("payload_type").and_then(Value::as_str) == Some("run.terminal.completed") {
            if self.accepted_command_id.is_none()
                || payload.get("command_id").and_then(Value::as_str)
                    != self.accepted_command_id.as_deref()
            {
                return self.fail(
                    "Muse terminal outcome does not match the accepted command".to_owned(),
                    true,
                    false,
                );
            }
            let terminal = payload.get("terminal").and_then(Value::as_str);
            let text = payload.get("text").and_then(Value::as_str);
            let (Some(terminal), Some(text)) = (terminal, text) else {
                return self.fail("Muse terminal outcome is malformed".to_owned(), true, false);
            };
            if !matches!(terminal, "completed" | "failed" | "cancelled") {
                return self.fail("Muse terminal outcome is malformed".to_owned(), true, false);
            }
            if text.len() > MUSE_MAX_ANSWER_BYTES {
                return self.fail(
                    format!("Muse final answer exceeds {MUSE_MAX_ANSWER_BYTES} bytes"),
                    true,
                    false,
                );
            }
            self.terminal = Some(terminal.to_owned());
            self.answer = text.to_owned();
        }
        MuseLineOutcome::Written
    }

    /// Classify the finished stream (Python outcome selection after wait).
    pub fn finish(&mut self, rc: i64, timed_out: bool, stderr_tail: &str) -> MuseTurnOutcome {
        let label = if timed_out {
            "timeout".to_owned()
        } else if self.protocol_error.is_some() {
            "protocol_error".to_owned()
        } else if rc != 0 {
            rc.to_string()
        } else if self.accepted_command_id.is_none() || self.terminal.is_none() {
            self.protocol_error =
                Some("Muse exited without a complete accepted terminal outcome".to_owned());
            "protocol_error".to_owned()
        } else if self.terminal.as_deref() != Some("completed") {
            self.terminal.clone().expect("terminal checked")
        } else {
            "0".to_owned()
        };
        let answer = if label == "protocol_error" {
            let detail = self
                .protocol_error
                .as_deref()
                .unwrap_or("Muse emitted an invalid event stream");
            if self.answer.is_empty() {
                format!("[MUSE ERROR] {detail}")
            } else {
                format!(
                    "[MUSE ERROR] {detail}\n[MUSE UNVERIFIED OUTPUT]\n{}",
                    self.answer
                )
            }
        } else if self.answer.is_empty() {
            let detail = self
                .protocol_error
                .as_deref()
                .filter(|detail| !detail.is_empty())
                .unwrap_or(stderr_tail);
            format!(
                "[MUSE ERROR] {}",
                if detail.is_empty() {
                    format!("process exited {rc}")
                } else {
                    detail.to_owned()
                }
            )
        } else {
            self.answer.clone()
        };
        MuseTurnOutcome {
            status: if label == "0" { "idle" } else { "error" }.to_owned(),
            rc_label: label,
            answer,
        }
    }
}

/// A finished Muse turn's classified outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuseTurnOutcome {
    /// Sentinel `rc=` label.
    pub rc_label: String,
    /// Registry status to record.
    pub status: String,
    /// Answer (or `[MUSE ERROR] …` text) to publish.
    pub answer: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> AgentRecord {
        AgentRecord {
            name: "worker-1".to_owned(),
            harness: "codex".to_owned(),
            backend: "tmux".to_owned(),
            tmux_target: "subagents:worker".to_owned(),
            cwd: "/tmp/work".to_owned(),
            model: Some("record-model".to_owned()),
            session_id: None,
            status: "idle".to_owned(),
            next_seq: 3,
            created_at: "2026-11-07T00:00:00-05:00".to_owned(),
            ..AgentRecord::default()
        }
    }

    fn msg(text: &str) -> Message {
        Message {
            seq: 7,
            text: text.to_owned(),
            model: None,
            queued_at: "2026-11-07T00:00:00-05:00".to_owned(),
            effort: None,
        }
    }

    #[test]
    fn codex_argv_exec_resume_model_effort_and_bypass() {
        let base = Path::new("/base");
        let rec = record();
        let argv = build_codex_argv(base, &rec, &msg("hi"), Some("high"));
        assert_eq!(argv[0], codex_bin());
        assert_eq!(
            &argv[1..],
            [
                "exec",
                "-m",
                "record-model",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--skip-git-repo-check",
                "--json",
                "-o",
                "/base/state/worker-1/codex-answer-000000000007.txt",
                "-",
            ]
        );
        let mut rec = record();
        rec.session_id = Some("sess-1".to_owned());
        rec.codex_bypass_permissions = true;
        let mut turn = msg("again");
        turn.model = Some("turn-model".to_owned());
        turn.effort = Some("low".to_owned());
        let argv = build_codex_argv(base, &rec, &turn, Some("high"));
        assert_eq!(
            &argv[1..6],
            ["exec", "resume", "sess-1", "-m", "turn-model"]
        );
        assert!(argv.contains(&"model_reasoning_effort=\"low\"".to_owned()));
        assert!(argv.contains(&"--dangerously-bypass-approvals-and-sandbox".to_owned()));
    }

    #[test]
    fn agy_argv_and_outcome_classification() {
        let mut rec = record();
        rec.session_id = Some("conv-1".to_owned());
        let log = Path::new("/tmp/agy.log");
        let argv = build_agy_argv(
            &rec,
            &msg("hello agy"),
            log,
            turn_timeout_s_from(Some("42")),
        );
        assert_eq!(argv[0], agy_bin());
        assert_eq!(
            &argv[1..],
            [
                "--model",
                "record-model",
                "--conversation",
                "conv-1",
                "--add-dir",
                "/tmp/work",
                "--log-file",
                "/tmp/agy.log",
                "--print-timeout",
                "42s",
                "--print",
                "hello agy",
            ]
        );
        assert_eq!(turn_timeout_s_from(None), 3600);
        assert_eq!(turn_timeout_s_from(Some("junk")), 3600);

        let quota = classify_agy_outcome("1", "", "RESOURCE_EXHAUSTED Resets in 2h15m", "");
        assert_eq!(quota.rc_label, "quota_exhausted");
        assert_eq!(quota.done_fields, ["reset_in=2h15m"]);
        assert!(quota.message_if_empty.contains("reset_in=2h15m"));
        let auth = classify_agy_outcome("1", "error getting token source", "", "");
        assert_eq!(auth.status, "auth_error");
        let ok = classify_agy_outcome("0", "done", "", "");
        assert_eq!((ok.rc_label.as_str(), ok.status.as_str()), ("0", "idle"));

        let log_text = "Stream completed for 123E4567-E89B-42D3-A456-426614174000\n\
                        Propagating selected model override to backend: label=\"Other Model\"\n\
                        requested model not in local config; defaulting to it\n";
        assert_eq!(
            capture_agy_session_id_from_text(log_text).as_deref(),
            Some("123e4567-e89b-42d3-a456-426614174000")
        );
        let note = agy_model_note_from_text(Some("Wanted"), Some(log_text), 7);
        assert!(note.contains("AGY MODEL WARNING"), "{note}");
        assert!(note.contains("agy-reported-defaulting"), "{note}");
        assert!(agy_model_note_from_text(None, None, 7).contains("reason=missing-log"));
    }

    #[test]
    fn muse_argv_is_exact_and_refuses_overrides() {
        let mut rec = record();
        rec.harness = "muse".to_owned();
        rec.session_id = Some("sess-9".to_owned());
        rec.harness_args = vec!["--reasoning-effort".to_owned(), "high".to_owned()];
        let argv = build_muse_argv(&rec, &msg("hello muse")).unwrap();
        assert_eq!(
            argv,
            [
                muse_bin(),
                "exec".to_owned(),
                "--reasoning-effort".to_owned(),
                "high".to_owned(),
                "--model".to_owned(),
                "record-model".to_owned(),
                "--json".to_owned(),
                "--session-id".to_owned(),
                "sess-9".to_owned(),
                "--".to_owned(),
                "hello muse".to_owned(),
            ]
        );
        rec.session_id = None;
        assert_eq!(
            build_muse_argv(&rec, &msg("x")).unwrap_err(),
            "Muse worker has no caller-owned session id"
        );
        rec.session_id = Some("sess-9".to_owned());
        rec.harness_args = vec!["--model=other".to_owned()];
        assert!(build_muse_argv(&rec, &msg("x"))
            .unwrap_err()
            .contains("cannot override runner-owned argument '--model'"));
    }

    #[test]
    fn pane_echo_and_codex_observer() {
        assert_eq!(
            pane_echo_lines("{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\",\"command\":\"ls -la\"}}"),
            ["$ ls -la"]
        );
        assert_eq!(pane_echo_lines("not json"), ["not json"]);
        let mut observer = CodexTurnObserver::default();
        observer.push_line("{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}");
        observer.push_line("{\"type\":\"turn.failed\",\"error\":{\"message\":\"boom\"}}");
        assert_eq!(observer.captured_session.as_deref(), Some("thread-1"));
        assert_eq!(observer.captured_error.as_deref(), Some("boom"));
    }

    fn muse_event(sequence: i64, payload_type: &str, payload: Value) -> Vec<u8> {
        let mut line = json!({
            "schema_version": 1,
            "stream": {"kind": "session", "id": "sess-9"},
            "sequence": sequence,
            "payload_type": payload_type,
            "payload": payload,
        })
        .to_string();
        line.push('\n');
        line.into_bytes()
    }

    fn muse_accepted(sequence: i64) -> Vec<u8> {
        muse_event(
            sequence,
            "runtime.command.accepted",
            json!({"command_id": "cmd-1", "command_kind": "turn.submit"}),
        )
    }

    fn muse_terminal(sequence: i64, terminal: &str, text: &str) -> Vec<u8> {
        muse_event(
            sequence,
            "run.terminal.completed",
            json!({"command_id": "cmd-1", "terminal": terminal, "text": text}),
        )
    }

    #[test]
    fn muse_validator_accepts_a_complete_turn_with_one_epilogue() {
        let mut validator = MuseTurnValidator::new("sess-9");
        assert_eq!(
            validator.push_line(&muse_accepted(0)),
            MuseLineOutcome::Written
        );
        assert_eq!(
            validator.push_line(&muse_terminal(1, "completed", "the answer")),
            MuseLineOutcome::Written
        );
        let epilogue = {
            let mut line = json!({
                "schema_version": 1,
                "stream": {"kind": "session", "id": "sess-9"},
                "sequence": 2,
                "payload_type": "session.workspace_branch.observed",
                "payload_schema_version": 1,
                "record_type": "event",
                "durability": "durable",
                "causation_id": "cmd-1",
                "payload": {
                    "command_id": "cmd-1",
                    "kind": "workspace_branch_observed",
                    "record": {
                        "command_id": "cmd-1",
                        "commit": "abc123",
                        "dirty": false,
                        "reference": {"kind": "branch", "name": "main"},
                        "vcs": "git",
                        "workspace_root": "/tmp/work"
                    }
                }
            })
            .to_string();
            line.push('\n');
            line.into_bytes()
        };
        assert_eq!(validator.push_line(&epilogue), MuseLineOutcome::Written);
        // A second post-terminal event is a protocol error.
        match validator.push_line(&muse_event(3, "other", json!({}))) {
            MuseLineOutcome::ProtocolError { written, .. } => assert!(written),
            other => panic!("expected protocol error, got {other:?}"),
        }

        let mut clean = MuseTurnValidator::new("sess-9");
        clean.push_line(&muse_accepted(0));
        clean.push_line(&muse_terminal(1, "completed", "the answer"));
        let outcome = clean.finish(0, false, "");
        assert_eq!(
            (
                outcome.rc_label.as_str(),
                outcome.status.as_str(),
                outcome.answer.as_str()
            ),
            ("0", "idle", "the answer")
        );
    }

    #[test]
    fn muse_validator_rejects_bad_streams() {
        // Non-monotonic sequence.
        let mut validator = MuseTurnValidator::new("sess-9");
        validator.push_line(&muse_accepted(3));
        match validator.push_line(&muse_terminal(3, "completed", "x")) {
            MuseLineOutcome::ProtocolError { message, .. } => {
                assert!(message.contains("non-monotonic"), "{message}")
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
        // Session identity change.
        let mut validator = MuseTurnValidator::new("sess-9");
        let mut foreign = muse_accepted(0);
        let text = String::from_utf8(foreign.clone())
            .unwrap()
            .replace("sess-9", "sess-x");
        foreign = text.into_bytes();
        assert!(matches!(
            validator.push_line(&foreign),
            MuseLineOutcome::ProtocolError { .. }
        ));
        // Terminal without acceptance.
        let mut validator = MuseTurnValidator::new("sess-9");
        assert!(matches!(
            validator.push_line(&muse_terminal(0, "completed", "x")),
            MuseLineOutcome::ProtocolError { .. }
        ));
        // Oversize line: killed at once and never written.
        let mut validator = MuseTurnValidator::new("sess-9");
        match validator.push_line(&vec![b'x'; MUSE_MAX_EVENT_BYTES + 1]) {
            MuseLineOutcome::ProtocolError { written, kill, .. } => {
                assert!(!written && kill);
            }
            other => panic!("expected oversize protocol error, got {other:?}"),
        }
        // Missing terminal outcome at clean exit.
        let mut validator = MuseTurnValidator::new("sess-9");
        validator.push_line(&muse_accepted(0));
        assert_eq!(validator.finish(0, false, "").rc_label, "protocol_error");
        // Timeout label wins over everything else.
        let mut validator = MuseTurnValidator::new("sess-9");
        assert_eq!(validator.finish(9, true, "tail").rc_label, "timeout");
    }
}
