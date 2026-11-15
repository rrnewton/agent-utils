//! Muse's MSP protocol, as the headless wrapper speaks it (`#1234 headless-muse`).
//!
//! The wrapper runs one persistent `muse serve` process (MSP over stdio:
//! newline-delimited JSON-RPC 2.0). After the `initialize` handshake it
//! opens the recorded session (`session/start` with the recorded id, or
//! `session/resume` when the session journal already proves it), and each
//! queue prompt becomes one `turn/start` whose `commandId` is the wrapper's
//! own input id, so MSP notifications settle queue items exactly as Claude
//! Code's `command_lifecycle` does. MSP events are translated into the
//! wrapper's [`Signal`]s, so the typed event log, `tail`, `console`,
//! `status` and the watcher work unchanged.
//!
//! Approvals: the session runs with approval mode `onRequest` on the wire.
//! A prompt arrives as an `approval/request` server request; the wrapper
//! holds it for the owner through the shared approval path, and the answer
//! travels back as `approval/decide` naming the prompt's
//! `currentRequirementId` and one of its offered `availableChoices` — the
//! wrapper never invents a choice and never auto-resolves a prompt.
//! `request_user_input` is withheld at launch (`--disable-request-user-input`);
//! a user-input prompt that still arrives is refused with a JSON-RPC error
//! the event log records, never auto-cancelled silently.
//!
//! MSP is a young surface: this translation is pinned to Muse
//! [`TESTED_VERSION`] and its exported schema fingerprint
//! ([`SCHEMA_FINGERPRINT`], from `muse schema generate-json-schema`).
//! A harness that answers with a different version yields
//! [`Signal::VersionMismatch`]: the wrapper logs it and carries on.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::adapter::HarnessAdapter;
use super::claude::{self, InputState, PermissionRequest, Signal};
use super::events;

/// Muse build this translation was snapshot-tested against.
pub const TESTED_VERSION: &str = "1.4.23";
/// Fingerprint of the stable MSP schema that build exports
/// (`muse schema generate-json-schema`, `msp.schema.json`).
pub const SCHEMA_FINGERPRINT: &str =
    "sha256:e766d8e6a5f19fe1e5140617959474db830200dbdc1d5ea842eb97666e948780";

/// Client name in the MSP handshake; the host requires `^[a-z0-9_]+$`.
const CLIENT_NAME: &str = "agentctl_headless";
/// JSON-RPC id of the wrapper's `initialize` request.
const HELLO_INIT_ID: &str = "agentctl-hello-init";
/// JSON-RPC id of the wrapper's session-open request.
const HELLO_SESSION_ID: &str = "agentctl-hello-session";

/// Which Muse transport a headless Muse runs. Selected explicitly at
/// `start` and recorded; the wrapper never switches transports at runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    /// One persistent `muse serve` MSP host (this module).
    Serve,
    /// One `muse exec --json` process per turn (the fallback driver).
    Exec,
}

impl Transport {
    /// The recorded spelling of this transport.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Serve => "serve",
            Transport::Exec => "exec",
        }
    }

    /// Parse a recorded transport spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "serve" => Some(Transport::Serve),
            "exec" => Some(Transport::Exec),
            _ => None,
        }
    }
}

/// Muse as a [`HarnessAdapter`], speaking MSP to `muse serve`.
#[derive(Debug)]
pub struct MuseHarness {
    /// The transport this instance drives.
    transport: Transport,
    /// Server version from the handshake, for [`Signal::Init`].
    server_version: Mutex<Option<String>>,
    /// The session the handshake was asked to open.
    session: Mutex<Option<String>>,
    /// The recorded session is known to exist on the host: open it with
    /// `session/resume`, never `session/start` (which refuses a retained
    /// id). Learned from every successful open and from a conflict.
    must_resume: Mutex<bool>,
}

impl MuseHarness {
    /// A harness driving `muse serve`.
    #[must_use]
    pub fn serve() -> Self {
        Self {
            transport: Transport::Serve,
            server_version: Mutex::new(None),
            session: Mutex::new(None),
            must_resume: Mutex::new(false),
        }
    }

    /// A harness driving the per-turn `muse exec` fallback.
    #[must_use]
    pub fn exec() -> Self {
        Self {
            transport: Transport::Exec,
            server_version: Mutex::new(None),
            session: Mutex::new(None),
            must_resume: Mutex::new(false),
        }
    }
}

/// A UUIDv7: MSP `commandId`s must be UUIDv7 (the fresh turn's id derives
/// from it). Time-ordered like the host's own ids.
#[must_use]
pub fn uuid_v7() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    bytes[0..6].copy_from_slice(&millis.to_be_bytes()[2..8]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn line(value: Value) -> String {
    let mut text = value.to_string();
    text.push('\n');
    text
}

fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Bounded, masked one-line summary of a tool call's arguments.
fn tool_summary(tool: &str, args: &str) -> String {
    let parsed: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    claude::tool_summary(tool, &parsed)
}

/// The tool result preview, mirroring the Claude translation's bounds.
fn output_preview(text: &str) -> Vec<String> {
    text.lines()
        .take(claude::PREVIEW_LINES)
        .map(|row| events::excerpt(&claude::mask_secrets(row), claude::PREVIEW_LINE_BYTES))
        .collect()
}

impl MuseHarness {
    fn session_open_line(&self, resume_hint: bool, session: &str, approvals: bool) -> String {
        // A session this instance already opened (or collided with) is
        // resumed; the transcript hint covers a fresh wrapper process.
        let resume = resume_hint || self.must_resume.lock().map(|must| *must).unwrap_or(false);
        let command_id = uuid_v7();
        if resume {
            line(json!({
                "jsonrpc": "2.0", "id": HELLO_SESSION_ID, "method": "session/resume",
                "params": {"commandId": command_id, "sessionId": session},
            }))
        } else {
            line(json!({
                "jsonrpc": "2.0", "id": HELLO_SESSION_ID, "method": "session/start",
                "params": {
                    "commandId": command_id,
                    "sessionId": session,
                    "approvalMode": if approvals { "onRequest" } else { "denyUnmatched" },
                },
            }))
        }
    }

    fn parse_response(&self, id: &str, value: &Value) -> Vec<Signal> {
        if id == HELLO_INIT_ID {
            if let Some(result) = value.get("result") {
                let version = text_of(&result["serverInfo"], "version");
                if let Ok(mut cached) = self.server_version.lock() {
                    *cached = Some(version.clone());
                }
                if !version.is_empty() && version != TESTED_VERSION {
                    return vec![Signal::VersionMismatch {
                        expected: TESTED_VERSION.to_owned(),
                        actual: version,
                    }];
                }
            }
            return Vec::new();
        }
        if id == HELLO_SESSION_ID {
            if let Some(error) = value.get("error") {
                let reason = error["data"]["reason"].as_str().unwrap_or_default();
                let message = error["message"].as_str().unwrap_or("session open failed");
                // Correct course on the next launch: a retained id wants
                // `session/resume`; a missing one wants `session/start`.
                if let Ok(mut must) = self.must_resume.lock() {
                    *must = reason == "session_id_conflict";
                }
                return vec![Signal::SessionOpenFailed {
                    message: format!("{message} ({reason})"),
                }];
            }
            if let Ok(mut must) = self.must_resume.lock() {
                *must = true;
            }
            let session = value
                .get("result")
                .map(|result| result["session"].clone())
                .unwrap_or(Value::Null);
            let session_id = if session.is_object() {
                text_of(&session, "sessionId")
            } else {
                self.session
                    .lock()
                    .ok()
                    .and_then(|cached| cached.clone())
                    .unwrap_or_default()
            };
            let version = self
                .server_version
                .lock()
                .ok()
                .and_then(|cached| cached.clone())
                .unwrap_or_else(|| TESTED_VERSION.to_owned());
            return vec![Signal::Init {
                version,
                model: text_of(&session, "modelId"),
                permission_mode: session
                    .get("approvalMode")
                    .map(|mode| text_of(mode, "mode"))
                    .unwrap_or_default(),
                session: session_id,
                tools: 0,
            }];
        }
        if let Some(disposition) = value
            .get("result")
            .and_then(|result| result.get("disposition"))
            .and_then(Value::as_str)
        {
            // A `turn/start` answer: accepted means the host holds the
            // input; `turn/started` settles it as started.
            let _ = disposition;
            return vec![Signal::Input {
                uuid: id.to_owned(),
                state: InputState::Queued,
            }];
        }
        if value.get("error").is_some() {
            if id.starts_with("agentctl-") {
                return vec![Signal::ControlResponse {
                    request_id: id.to_owned(),
                    body: Value::Null,
                }];
            }
            // The host refused the input outright (an unloaded session, a
            // malformed turn): it was never seen, so it returns to its lane.
            return vec![Signal::Input {
                uuid: id.to_owned(),
                state: InputState::Cancelled,
            }];
        }
        if value.get("result").is_some() {
            if id.starts_with("agentctl-") {
                return vec![Signal::ControlResponse {
                    request_id: id.to_owned(),
                    body: value.get("result").cloned().unwrap_or(Value::Null),
                }];
            }
            // A `session/compact` admission settles its queue item.
            return vec![Signal::Input {
                uuid: id.to_owned(),
                state: InputState::Started,
            }];
        }
        Vec::new()
    }

    fn parse_notification(&self, method: &str, params: &Value) -> Vec<Signal> {
        match method {
            "turn/started" => vec![Signal::Input {
                uuid: text_of(params, "turnId"),
                state: InputState::Started,
            }],
            "turn/unqueued" | "turn/retracted" => vec![Signal::Input {
                uuid: text_of(params, "turnId"),
                state: InputState::Cancelled,
            }],
            "turn/completed" => {
                let terminal = text_of(params, "terminal");
                let mut signals = Vec::new();
                if terminal == "cancelled" {
                    signals.push(Signal::Input {
                        uuid: text_of(params, "turnId"),
                        state: InputState::Cancelled,
                    });
                }
                signals.push(Signal::Result {
                    subtype: match terminal.as_str() {
                        "completed" => "success".to_owned(),
                        "cancelled" => "error_during_execution".to_owned(),
                        other => other.to_owned(),
                    },
                    duration_ms: params.get("durationMs").and_then(Value::as_u64),
                    usage: params.get("usage").cloned().unwrap_or(json!({})),
                    cost_usd: None,
                    denials: Vec::new(),
                });
                signals
            }
            "item/started" | "item/completed" => self.parse_item(method, params),
            _ => Vec::new(),
        }
    }

    fn parse_item(&self, method: &str, params: &Value) -> Vec<Signal> {
        let item = &params["item"];
        let kind = text_of(item, "kind");
        let id = text_of(item, "itemId");
        match (method, kind.as_str()) {
            ("item/started", "toolCall") => {
                let tool = text_of(item, "tool");
                let args = text_of(item, "args");
                let input: Value = serde_json::from_str(&args).unwrap_or(Value::Null);
                let reply = input
                    .get("command")
                    .and_then(Value::as_str)
                    .and_then(super::reply::detect);
                vec![Signal::ToolStarted {
                    id,
                    summary: tool_summary(&tool, &args),
                    reply,
                    tool,
                }]
            }
            ("item/completed", "toolCall") => {
                let output = text_of(item, "visibleOutput");
                let status = text_of(item, "status");
                vec![Signal::ToolCompleted {
                    id,
                    ok: status != "failed" && item.get("error").is_none(),
                    bytes: output.len(),
                    lines: output.lines().count(),
                    preview: output_preview(&output),
                    denial: None,
                }]
            }
            ("item/completed", "agentMessage") => {
                let phase = text_of(item, "messagePhase");
                let text = text_of(item, "text");
                if text.is_empty() || (!phase.is_empty() && phase != "finalAnswer") {
                    Vec::new()
                } else {
                    vec![Signal::Text(text)]
                }
            }
            ("item/started", "compaction") => vec![Signal::CompactStarted],
            ("item/completed", "compaction") => vec![
                Signal::CompactCompleted {
                    trigger: "manual".to_owned(),
                    pre_tokens: None,
                    post_tokens: None,
                },
                Signal::Result {
                    subtype: "success".to_owned(),
                    duration_ms: None,
                    usage: json!({}),
                    cost_usd: None,
                    denials: Vec::new(),
                },
            ],
            _ => Vec::new(),
        }
    }

    fn parse_approval_request(&self, id: &Value, params: &Value) -> Vec<Signal> {
        let raw_args = text_of(params, "rawArgs");
        let input: Value = serde_json::from_str(&raw_args).unwrap_or_else(|_| json!(raw_args));
        let tool = {
            let name = text_of(params, "toolName");
            if name.is_empty() {
                params["subject"]["kind"]
                    .as_str()
                    .unwrap_or("tool")
                    .to_owned()
            } else {
                name
            }
        };
        let choices = params["availableChoices"]
            .as_array()
            .map(|choices| {
                choices
                    .iter()
                    .map(|choice| {
                        (
                            text_of(choice, "choiceId"),
                            text_of(choice, "decision"),
                            text_of(choice, "scope"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let detail = claude::mask_secrets(&raw_args);
        vec![Signal::PermissionRequest(Box::new(PermissionRequest {
            request_id: text_of(params, "approvalId"),
            tool: tool.clone(),
            summary: tool_summary(&tool, &raw_args),
            detail: events::excerpt(&detail, claude::DETAIL_BYTES),
            complete: detail.len() <= claude::DETAIL_BYTES,
            input,
            reason: text_of(params, "reviewerReason"),
            reason_type: if params["judgeEscalated"].as_bool() == Some(true) {
                "classifier".to_owned()
            } else {
                "rule".to_owned()
            },
            blocked_path: None,
            tool_use_id: text_of(params, "itemId"),
            requirement: params.get("currentRequirementId").cloned(),
            choices,
            wire_id: Some(id.to_string()),
        }))]
    }
}

impl HarnessAdapter for MuseHarness {
    fn name(&self) -> &'static str {
        "muse"
    }

    fn capabilities(&self) -> Value {
        json!({
            "schema": "agentctl-harness-capabilities/v1",
            "harness": "muse",
            "tested_harness_version": TESTED_VERSION,
            "msp_schema_fingerprint": SCHEMA_FINGERPRINT,
            "transport": self.transport.as_str(),
            "transports": {
                "serve": "muse serve (MSP over stdio), persistent",
                "exec": "muse exec --json per turn, explicit selection only",
            },
            "persistent": self.transport == Transport::Serve,
            "mid_turn_input": self.transport == Transport::Serve,
            "input_lifecycle": "by-caller-id",
            "receiver_record": "session-journal",
            "interrupt": self.transport == Transport::Serve,
            "cancel_queued": self.transport == Transport::Serve,
            "compact": "command",
            "resume": "session-id",
            "self_started_turns": false,
            "send_boundary": "acceptance",
            "settle_on": "started",
            "streams_text": "per-message",
            "approvals": true,
            "user_input": false,
        })
    }

    fn default_binary(&self) -> &'static str {
        "muse"
    }

    fn launch_arguments(
        &self,
        _resume: bool,
        _session: &str,
        arguments: &[String],
        _approvals: bool,
    ) -> Vec<String> {
        // `muse serve` takes its model (and provider) at launch; the
        // declared effort is per-turn on the wire, never a serve flag,
        // and approval mode is selected per session in the handshake.
        let mut argv = vec![
            "serve".to_owned(),
            "--trust-workspace".to_owned(),
            "--disable-request-user-input".to_owned(),
        ];
        let mut iter = arguments.iter();
        while let Some(argument) = iter.next() {
            match argument.as_str() {
                "--effort" => {
                    let _ = iter.next();
                }
                "--model" => {
                    argv.push("--model".to_owned());
                    if let Some(model) = iter.next() {
                        argv.push(model.clone());
                    }
                }
                other if other.starts_with("--effort=") => {}
                other if other.starts_with("--model=") => argv.push(other.to_owned()),
                other => argv.push(other.to_owned()),
            }
        }
        argv
    }

    fn hello_lines(
        &self,
        resume: bool,
        session: &str,
        _cwd: &Path,
        _effort: Option<&str>,
        approvals: bool,
    ) -> Vec<String> {
        if let Ok(mut cached) = self.session.lock() {
            *cached = Some(session.to_owned());
        }
        vec![
            line(json!({
                "jsonrpc": "2.0", "id": HELLO_INIT_ID, "method": "initialize",
                "params": {"clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")}},
            })),
            line(json!({"jsonrpc": "2.0", "method": "initialized"})),
            self.session_open_line(resume, session, approvals),
        ]
    }

    fn needs_init_before_input(&self) -> bool {
        true
    }

    fn init_begins_turn(&self) -> bool {
        // MSP reports the session once at open; turns begin at
        // `turn/started`, not at the handshake.
        false
    }

    fn new_input_id(&self) -> std::io::Result<String> {
        Ok(uuid_v7())
    }

    fn input_line(&self, uuid: &str, text: &str, session: &str, effort: Option<&str>) -> String {
        if text == "/compact" {
            return line(json!({
                "jsonrpc": "2.0", "id": uuid, "method": "session/compact",
                "params": {"commandId": uuid_v7(), "sessionId": session},
            }));
        }
        let mut params = json!({
            "commandId": uuid,
            "sessionId": session,
            "input": [{"type": "text", "text": text}],
        });
        if let Some(effort) = effort {
            params["reasoningEffort"] = json!(effort);
        }
        line(json!({
            "jsonrpc": "2.0", "id": uuid, "method": "turn/start", "params": params,
        }))
    }

    fn parse(&self, value: &Value) -> Vec<Signal> {
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            if method == "approval/request" {
                if let Some(id) = value.get("id") {
                    return self.parse_approval_request(id, &params);
                }
                return Vec::new();
            }
            if method == "userInput/request" {
                return vec![Signal::UnknownControlRequest {
                    request_id: value.get("id").map(ToString::to_string).unwrap_or_default(),
                    subtype: method.to_owned(),
                }];
            }
            return self.parse_notification(method, &params);
        }
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            return self.parse_response(id, value);
        }
        Vec::new()
    }

    fn interrupt_line(&self, request_id: &str, session: &str, turn: Option<&str>) -> String {
        let mut params = json!({"commandId": uuid_v7(), "sessionId": session});
        if let Some(turn) = turn {
            params["turnId"] = json!(turn);
        }
        line(json!({
            "jsonrpc": "2.0", "id": request_id, "method": "turn/interrupt",
            "params": params,
        }))
    }

    fn control_error_line(&self, request_id: &str, error: &str) -> String {
        line(json!({
            "jsonrpc": "2.0", "id": request_id,
            "error": {"code": -32601, "message": error},
        }))
    }

    fn permission_answer_line(
        &self,
        request_id: &str,
        allow: bool,
        _input: &Value,
        message: &str,
        requirement: Option<&Value>,
        choices: &[(String, String, String)],
        session: &str,
    ) -> String {
        let wanted = if allow { "approved" } else { "denied" };
        let choice = choices
            .iter()
            .find(|(_, decision, scope)| decision == wanted && scope == "once")
            .or_else(|| choices.iter().find(|(_, decision, _)| decision == wanted));
        let Some((choice_id, _, _)) = choice else {
            return String::new();
        };
        let mut params = json!({
            "commandId": uuid_v7(),
            "sessionId": session,
            "approvalId": request_id,
            "choiceId": choice_id,
        });
        if let Some(requirement) = requirement {
            params["requirementId"] = requirement.clone();
        }
        if !allow && !message.is_empty() {
            params["feedback"] = json!(message);
        }
        line(json!({
            "jsonrpc": "2.0", "id": uuid_v7(), "method": "approval/decide",
            "params": params,
        }))
    }

    fn permission_receipt_line(&self, request: &PermissionRequest) -> Option<String> {
        // MSP answers a server request with its presentation receipt; the
        // decision itself travels as `approval/decide`.
        request
            .wire_id
            .as_ref()
            .map(|id| line(json!({"jsonrpc": "2.0", "id": id, "result": {}})))
    }

    fn asks_wrapper(&self, _arguments: &[String], approvals: bool) -> bool {
        approvals
    }

    fn mask_secrets(&self, text: &str) -> String {
        claude::mask_secrets(text)
    }

    fn denial_text(&self, text: &str) -> String {
        claude::denial_text(text)
    }

    fn config_directory(&self) -> Option<PathBuf> {
        if let Some(home) = std::env::var_os("MUSE_HOME") {
            return Some(PathBuf::from(home));
        }
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })?;
        Some(base.join("muse"))
    }

    fn transcript_path(&self, _cwd: &Path, session: &str, config: &Path) -> PathBuf {
        // Muse journals live under `<muse home>/sessions/<date>/<id>/session.jsonl`.
        let root = config.join("sessions");
        if let Ok(dates) = std::fs::read_dir(&root) {
            for date in dates.flatten() {
                let candidate = date.path().join(session).join("session.jsonl");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        root.join(session).join("session.jsonl")
    }

    fn transcript_received(&self, transcript: &Path, uuid: &str) -> bool {
        let Ok(text) = std::fs::read_to_string(transcript) else {
            return false;
        };
        text.lines().any(|row| {
            let Ok(record) = serde_json::from_str::<Value>(row) else {
                return false;
            };
            record["payload"]["command_id"].as_str() == Some(uuid)
                || record["payload"]["commandId"].as_str() == Some(uuid)
        })
    }

    fn preview_lines(&self) -> usize {
        claude::PREVIEW_LINES
    }

    fn valid_session_id(&self, session: &str) -> bool {
        claude::is_uuid(session)
    }

    fn validate_arguments(&self, arguments: &[String]) -> Result<(), String> {
        for argument in arguments {
            if argument == "--permission-prompts"
                || argument == "--permission-mode"
                || argument.starts_with("--permission-mode=")
            {
                return Err(format!(
                    "{argument:?} is a Claude Code option; a headless Muse takes its approval mode on the wire (onRequest) and its options via `muse serve` flags passed with --harness-arg"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness() -> MuseHarness {
        MuseHarness::serve()
    }

    #[test]
    fn uuid_v7_has_the_v7_shape() {
        let id = uuid_v7();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "7");
        assert!(claude::is_uuid(&id));
    }

    #[test]
    fn launch_strips_effort_and_keeps_the_model() {
        let argv = harness().launch_arguments(
            false,
            "s",
            &[
                "--model".to_owned(),
                "m1".to_owned(),
                "--effort".to_owned(),
                "high".to_owned(),
            ],
            true,
        );
        assert_eq!(argv[0], "serve");
        assert!(argv.iter().any(|arg| arg == "--model"));
        assert!(!argv.iter().any(|arg| arg.contains("effort")));
    }

    #[test]
    fn hello_opens_the_recorded_session_on_request() {
        let lines = harness().hello_lines(false, "sess-1", Path::new("/w"), Some("high"), true);
        assert_eq!(lines.len(), 3);
        let open: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(open["method"], "session/start");
        assert_eq!(open["params"]["sessionId"], "sess-1");
        assert_eq!(open["params"]["approvalMode"], "onRequest");
    }

    #[test]
    fn a_turn_is_one_msp_turn_with_explicit_effort() {
        let parsed: Value =
            serde_json::from_str(&harness().input_line("u1", "hello", "s1", Some("max"))).unwrap();
        assert_eq!(parsed["method"], "turn/start");
        assert_eq!(parsed["params"]["commandId"], "u1");
        assert_eq!(parsed["params"]["reasoningEffort"], "max");
        assert_eq!(parsed["params"]["input"][0]["text"], "hello");
    }

    #[test]
    fn turn_notifications_settle_the_input() {
        let h = harness();
        let started: Value =
            serde_json::from_str(r#"{"method":"turn/started","params":{"turnId":"u1"}}"#).unwrap();
        assert_eq!(
            h.parse(&started),
            vec![Signal::Input {
                uuid: "u1".into(),
                state: InputState::Started
            }]
        );
        let done: Value = serde_json::from_str(
            r#"{"method":"turn/completed","params":{"turnId":"u1","terminal":"completed","durationMs":5}}"#,
        )
        .unwrap();
        match &h.parse(&done)[..] {
            [Signal::Result {
                subtype,
                duration_ms,
                ..
            }] => {
                assert_eq!(subtype, "success");
                assert_eq!(*duration_ms, Some(5));
            }
            other => panic!("unexpected signals: {other:?}"),
        }
    }

    #[test]
    fn an_unpinned_harness_warns_instead_of_refusing() {
        let h = harness();
        let init: Value = serde_json::from_str(
            r#"{"id":"agentctl-hello-init","result":{"serverInfo":{"version":"9.9.9"}}}"#,
        )
        .unwrap();
        assert_eq!(
            h.parse(&init),
            vec![Signal::VersionMismatch {
                expected: TESTED_VERSION.into(),
                actual: "9.9.9".into(),
            }]
        );
    }
}
