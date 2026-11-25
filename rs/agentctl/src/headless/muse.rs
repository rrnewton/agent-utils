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
//! Launch (#2247): `muse serve` refuses exec and TUI options (`--yolo`,
//! `--approval-mode`, `--reasoning-effort`, `--json`, ...) with exit 2, so
//! its argv is built from an allowlist ([`serve_launch`]): the options
//! `muse serve --help` lists plus the `muse` launcher's own, each kept
//! once. Approval flags become the session's approval mode on the wire,
//! and the declared effort travels on the wire too: the session's standing
//! default (`session/setReasoningEffort`, once the session opens) and every
//! `turn/start`'s `reasoningEffort`. Anything else is left off, and the
//! wrapper logs what was left off at spawn.
//!
//! Approvals: the session runs with approval mode `onRequest` on the wire
//! (`denyUnmatched` when nobody answers prompts), unless the operator
//! arguments select one (`--yolo`, `--disable-approval`, `--approval-mode`).
//! `session/start` carries it; a `session/resume` cannot, so a resumed
//! session gets it as `session/setApprovalMode` once it opens.
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
/// JSON-RPC id of the standing-effort request sent once the session opens.
const SESSION_EFFORT_ID: &str = "agentctl-session-effort";
/// JSON-RPC id of the approval-mode request sent after a resume opens.
const SESSION_APPROVAL_ID: &str = "agentctl-session-approval";

/// `muse serve` options without a value (`muse serve --help`, Muse 1.4.27).
/// Left out on purpose: the socket-daemon options (`--listen`, `--join` and
/// the rest), which stop serving the wrapper over stdio, and
/// `--per-session`, which refuses `--trust-workspace`.
const SERVE_FLAGS: &[&str] = &[
    "--no-session-log",
    "--multi-tenant",
    "--disable-request-user-input",
    "--disable-sandbox",
    "--disable-write",
    "--disable-shell",
    "--trust-workspace",
    "--enable-attach",
];
/// `muse serve` options that take one value and may be given once (serve
/// refuses a repeated `--model`): the last one given wins.
const SERVE_SINGLE: &[&str] = &[
    "--preset",
    "--provider",
    "--model",
    "--contributor-mode",
    "--sandbox-network",
];
/// The `muse` launcher's own options without a value: it reads them before
/// it runs the binary, whatever the subcommand (`muse --help`).
const LAUNCHER_FLAGS: &[&str] = &[
    "--dangerously-enable-internet-mode",
    "--secure-internet-mode",
    "--dangerously-disable-linux-sandbox",
    "--disable-muse-llm-rules",
    "--meta-sensitive-mode",
    "--meta-jail-audit-mode",
];
/// The `muse` launcher's own options that take a value; some repeat.
const LAUNCHER_VALUED: &[&str] = &[
    "--with-proxy",
    "--meta-agent-role",
    "--startup-context-mode",
    "--meta-add-writable-dir",
    "--meta-force-enable-features",
    "--meta-tag",
];
/// Effort options: `muse serve` takes none, so the declared effort (`--effort`,
/// which agentctl writes from the record) travels on the wire instead.
const EFFORT_OPTIONS: &[&str] = &["--effort", "--reasoning-effort"];

/// What one `muse serve` launch runs (#2247): the argv after the binary,
/// the approval mode the operator arguments select, and what they asked
/// for that serve cannot take.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServeLaunch {
    /// `serve`, its options and the launcher's, each option once.
    pub argv: Vec<String>,
    /// The MSP approval mode an approval flag selected; `None` keeps the
    /// wrapper's default (`onRequest`, or `denyUnmatched` without approvals).
    pub approval_mode: Option<&'static str>,
    /// Operator arguments left off the serve argv, as given.
    pub dropped: Vec<String>,
}

/// The MSP approval mode for an exec `--approval-mode` value. `never`
/// (never ask) is Muse's permission bypass beside `--yolo`, as agentctl's
/// own bypass rule reads it (`permission_bypass_flag`), so it allows all.
fn exec_approval_mode(value: &str) -> Option<&'static str> {
    match value {
        "on-request" => Some("onRequest"),
        "untrusted" => Some("promptUnmatched"),
        "never" => Some("allowAll"),
        _ => None,
    }
}

/// The approval mode a serve session selects: the one the operator
/// arguments chose, else `onRequest` when the wrapper answers prompts and
/// `denyUnmatched` when nobody does.
fn approval_mode(arguments: &[String], approvals: bool) -> &'static str {
    serve_launch(arguments)
        .approval_mode
        .unwrap_or(if approvals {
            "onRequest"
        } else {
            "denyUnmatched"
        })
}

/// Add bare flag `name` to `options` unless it is there already.
fn flag(options: &mut Vec<(String, Option<String>)>, name: &str) {
    if !options.iter().any(|(option, _)| option == name) {
        options.push((name.to_owned(), None));
    }
}

/// Build the `muse serve` launch for operator `arguments` (#2247). Every
/// kept option is in `muse serve --help`'s set or is a launcher option;
/// approval flags become [`ServeLaunch::approval_mode`] (`--yolo` also
/// disables serve's sandbox, as it does exec's); effort options leave the
/// argv; a bare flag is kept once, a single-valued option keeps its last
/// value, and a valued option that repeats keeps its distinct values.
#[must_use]
pub fn serve_launch(arguments: &[String]) -> ServeLaunch {
    // (option, value): the argv is rendered from these once deduplicated.
    let mut options: Vec<(String, Option<String>)> = vec![
        ("--trust-workspace".to_owned(), None),
        ("--disable-request-user-input".to_owned(), None),
    ];
    let mut launch = ServeLaunch::default();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (argument.as_str(), None),
        };
        let name = if name == "-c" { "--config" } else { name };
        let single = SERVE_SINGLE.contains(&name);
        let valued = single
            || name == "--config"
            || LAUNCHER_VALUED.contains(&name)
            || EFFORT_OPTIONS.contains(&name)
            || name == "--approval-mode"
            || name == "--approval-judge";
        if valued {
            let value = match inline {
                Some(value) => Some(value),
                None if index < arguments.len() && !arguments[index].starts_with("--") => {
                    index += 1;
                    Some(arguments[index - 1].clone())
                }
                None => None,
            };
            let Some(value) = value else {
                launch.dropped.push(argument.clone());
                continue;
            };
            if EFFORT_OPTIONS.contains(&name) {
                if name != "--effort" {
                    launch.dropped.push(format!("{name} {value}"));
                }
                continue;
            }
            if name == "--approval-mode" {
                match exec_approval_mode(&value) {
                    Some(mode) => launch.approval_mode = Some(mode),
                    None => launch.dropped.push(format!("{name} {value}")),
                }
                continue;
            }
            if name == "--approval-judge" {
                launch.dropped.push(format!("{name} {value}"));
                continue;
            }
            if single {
                if let Some(slot) = options.iter_mut().find(|(option, _)| option == name) {
                    slot.1 = Some(value);
                    continue;
                }
            } else if options
                .iter()
                .any(|(option, kept)| option == name && kept.as_deref() == Some(value.as_str()))
            {
                continue;
            }
            options.push((name.to_owned(), Some(value)));
            continue;
        }
        if inline.is_some() {
            // A bare flag never takes `=value`; serve would refuse it.
            launch.dropped.push(argument.clone());
            continue;
        }
        match name {
            "--yolo" => {
                launch.approval_mode = Some("allowAll");
                flag(&mut options, "--disable-sandbox");
            }
            "--disable-approval" => launch.approval_mode = Some("allowAll"),
            name if SERVE_FLAGS.contains(&name) || LAUNCHER_FLAGS.contains(&name) => {
                flag(&mut options, name);
            }
            _ => {
                // Not a serve option: leave it off, with the value it may carry
                // (serve takes no positional arguments either).
                let mut dropped = argument.clone();
                if argument.starts_with('-')
                    && index < arguments.len()
                    && !arguments[index].starts_with('-')
                {
                    dropped.push(' ');
                    dropped.push_str(&arguments[index]);
                    index += 1;
                }
                launch.dropped.push(dropped);
            }
        }
    }
    launch.argv.push("serve".to_owned());
    for (option, value) in options {
        launch.argv.push(option);
        launch.argv.extend(value);
    }
    launch
}

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
    /// The last session open was a `session/resume`, which carries no
    /// approval mode: the mode follows once the session opens.
    resumed: Mutex<bool>,
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
            resumed: Mutex::new(false),
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
            resumed: Mutex::new(false),
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
    fn session_open_line(&self, resume_hint: bool, session: &str, mode: &str) -> String {
        // A session this instance already opened (or collided with) is
        // resumed; the transcript hint covers a fresh wrapper process.
        let resume = resume_hint || self.must_resume.lock().map(|must| *must).unwrap_or(false);
        if let Ok(mut resumed) = self.resumed.lock() {
            *resumed = resume;
        }
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
                    "approvalMode": mode,
                },
            }))
        }
    }

    fn parse_response(&self, id: &str, value: &Value) -> Vec<Signal> {
        if id == SESSION_EFFORT_ID || id == SESSION_APPROVAL_ID {
            // A setting applied once the session opened: a refusal is logged,
            // never a restart (the per-turn effort still rides every turn).
            let Some(error) = value.get("error") else {
                return Vec::new();
            };
            let reason = error["data"]["reason"].as_str().unwrap_or_default();
            let message = error["message"].as_str().unwrap_or("refused");
            return vec![Signal::SessionConfigFailed {
                setting: if id == SESSION_EFFORT_ID {
                    "reasoning_effort"
                } else {
                    "approval_mode"
                }
                .to_owned(),
                message: if reason.is_empty() {
                    message.to_owned()
                } else {
                    format!("{message} ({reason})")
                },
            }];
        }
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
            "approval/resolved" => vec![Signal::PermissionResolved {
                request_id: text_of(params, "approvalId"),
                decision: text_of(params, "decision"),
            }],
            "approval/updated" => vec![Signal::PermissionUpdated {
                request_id: text_of(params, "approvalId"),
                requirement: params.get("currentRequirementId").cloned(),
                choices: params["availableChoices"]
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
                    .unwrap_or_default(),
            }],
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
                // Every completed message is committed prose, whatever its phase
                // (`commentary`, `finalAnswer`, ...): a CHAT_REPLY block in a non-final
                // message must reach the event log or the chat bridge cannot capture it
                // (#1974). Only the harness's final text was logged before, which silently
                // dropped such blocks.
                let text = text_of(item, "text");
                if text.is_empty() {
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
            "interrupt": true,
            "cancel_queued": self.transport == Transport::Serve,
            "compact": if self.transport == Transport::Serve { "command" } else { "prompt" },
            "resume": "session-id",
            "self_started_turns": false,
            "send_boundary": "acceptance",
            "settle_on": "started",
            "streams_text": "per-message",
            "tool_events": self.transport == Transport::Serve,
            "approvals": self.transport == Transport::Serve,
            "user_input": false,
        })
    }

    fn default_binary(&self) -> &'static str {
        "muse"
    }

    fn launch_arguments(
        &self,
        _resume: bool,
        session: &str,
        arguments: &[String],
        _approvals: bool,
    ) -> Vec<String> {
        if self.transport == Transport::Exec {
            // The wrapper's harness process is the exec relay (this
            // binary's internal `headless-muse-exec-relay`); it takes
            // the recorded session and the model, and the effort per turn.
            let mut argv = vec!["--session".to_owned(), session.to_owned()];
            let mut iter = arguments.iter();
            while let Some(argument) = iter.next() {
                match argument.as_str() {
                    "--effort" => {
                        let _ = iter.next();
                    }
                    "--model" | "--provider" => {
                        argv.push(argument.clone());
                        if let Some(value) = iter.next() {
                            argv.push(value.clone());
                        }
                    }
                    other if other.starts_with("--effort=") => {}
                    other if other.starts_with("--model=") => argv.push(other.to_owned()),
                    other if other.starts_with("--provider=") => argv.push(other.to_owned()),
                    other if other.starts_with("--provider") => argv.push(other.to_owned()),
                    _ => {}
                }
            }
            return argv;
        }
        // `muse serve` takes its model, provider and sandbox posture at
        // launch; the declared effort and the approval mode travel on the
        // wire, never as serve flags (#2247).
        serve_launch(arguments).argv
    }

    fn hello_lines(
        &self,
        resume: bool,
        session: &str,
        _cwd: &Path,
        _effort: Option<&str>,
        arguments: &[String],
        approvals: bool,
    ) -> Vec<String> {
        if self.transport == Transport::Exec {
            // The relay opens the session itself and reports it as the
            // same `Init` the serve handshake produces.
            return Vec::new();
        }
        if let Ok(mut cached) = self.session.lock() {
            *cached = Some(session.to_owned());
        }
        vec![
            line(json!({
                "jsonrpc": "2.0", "id": HELLO_INIT_ID, "method": "initialize",
                "params": {"clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")}},
            })),
            line(json!({"jsonrpc": "2.0", "method": "initialized"})),
            self.session_open_line(resume, session, approval_mode(arguments, approvals)),
        ]
    }

    fn session_opened_lines(
        &self,
        session: &str,
        effort: Option<&str>,
        arguments: &[String],
        approvals: bool,
    ) -> Vec<String> {
        if self.transport == Transport::Exec {
            // `muse exec` takes the effort per run; it has no approval wire.
            return Vec::new();
        }
        let mut lines = Vec::new();
        // `muse serve` has no effort option: the declared effort becomes
        // the session's standing default, so a turn the host starts on its
        // own runs at it too (every `turn/start` also carries it).
        if let Some(effort) = effort {
            lines.push(line(json!({
                "jsonrpc": "2.0", "id": SESSION_EFFORT_ID, "method": "session/setReasoningEffort",
                "params": {"commandId": uuid_v7(), "sessionId": session, "reasoningEffort": effort},
            })));
        }
        // `session/resume` carries no approval mode: select it now.
        if self.resumed.lock().map(|resumed| *resumed).unwrap_or(false) {
            lines.push(line(json!({
                "jsonrpc": "2.0", "id": SESSION_APPROVAL_ID, "method": "session/setApprovalMode",
                "params": {"commandId": uuid_v7(), "sessionId": session,
                           "mode": approval_mode(arguments, approvals)},
            })));
        }
        lines
    }

    fn launch_warnings(&self, arguments: &[String]) -> Vec<String> {
        if self.transport == Transport::Exec {
            return Vec::new();
        }
        let dropped = serve_launch(arguments).dropped;
        if dropped.is_empty() {
            return Vec::new();
        }
        vec![format!(
            "muse serve does not take {}; left off the serve command line (approval mode and the declared --effort travel on the wire)",
            dropped.join(", ")
        )]
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
        if self.transport == Transport::Exec {
            // One relay instruction; the relay runs one `muse exec`.
            return line(json!({
                "relay": "turn", "id": uuid, "text": text, "effort": effort,
            }));
        }
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
        if self.transport == Transport::Exec {
            // The relay ends the running `muse exec` process group.
            return line(json!({"relay": "interrupt", "id": request_id}));
        }
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

    fn asks_wrapper(&self, arguments: &[String], approvals: bool) -> bool {
        // `muse exec` has no approval wire; its harness decides alone. A
        // session that allows or denies every unmatched call never asks.
        approvals
            && self.transport == Transport::Serve
            && matches!(
                approval_mode(arguments, approvals),
                "onRequest" | "promptUnmatched"
            )
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

    fn transcript_received_for(&self, transcript: &Path, uuid: &str, text: &str) -> bool {
        if self.transcript_received(transcript, uuid) {
            // An exec journal records the turn's command id outright.
            return true;
        }
        if text.is_empty() {
            return false;
        }
        // A serve journal records the submitted prompt, not the client
        // command id: exactly one turn intake carrying this prompt
        // proves the model received it. Zero or several matches prove
        // nothing (a duplicated prompt stays quarantined, never dropped
        // on a guess and never replayed either).
        let Ok(journal) = std::fs::read_to_string(transcript) else {
            return false;
        };
        let matches = journal
            .lines()
            .filter(|row| {
                let Ok(record) = serde_json::from_str::<Value>(row) else {
                    return false;
                };
                record["payload"]["record"]["command"]["prompt"].as_str() == Some(text)
            })
            .count();
        matches == 1
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

/// The exec transport's persistent relay (`agentctl headless-muse-exec-relay`,
/// an internal subcommand): the wrapper's harness process for a Muse
/// recorded on the exec transport. It reads one relay instruction per
/// queue prompt from stdin, runs one `muse exec --json` for it, and
/// translates the session journal into the same MSP-shaped lines the
/// serve translation parses — so the wrapper cannot tell the transports
/// apart except through the declared capabilities. An interrupt
/// instruction ends the running exec's process group.
pub fn run_exec_relay(
    session: &str,
    muse_bin: &str,
    model: Option<&str>,
    provider: Option<&str>,
) -> i32 {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    let stdout = std::io::stdout();
    let emit = |value: Value| {
        let mut handle = stdout.lock();
        let _ = writeln!(handle, "{}", value);
        let _ = handle.flush();
    };
    emit(json!({
        "id": HELLO_SESSION_ID,
        "result": {"session": {
            "sessionId": session,
            "modelId": model.unwrap_or_default(),
            "approvalMode": {"mode": "onRequest"},
        }},
    }));

    let running: std::sync::Arc<Mutex<Option<u32>>> = std::sync::Arc::new(Mutex::new(None));
    let interrupted: std::sync::Arc<Mutex<bool>> = std::sync::Arc::new(Mutex::new(false));
    let (sender, receiver) = mpsc::channel::<String>();
    {
        let running = running.clone();
        let interrupted = interrupted.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for row in stdin.lock().lines() {
                let Ok(row) = row else { break };
                // Interrupts land at once, even mid-turn: end the
                // running exec's process group from here.
                if let Ok(instruction) = serde_json::from_str::<Value>(&row) {
                    if instruction["relay"].as_str() == Some("interrupt") {
                        if let Some(pid) = running.lock().ok().and_then(|slot| *slot) {
                            if let Ok(mut flag) = interrupted.lock() {
                                *flag = true;
                            }
                            // SAFETY: pid is the relay's own child,
                            // spawned in its own process group below.
                            unsafe {
                                libc::killpg(pid as i32, libc::SIGTERM);
                            }
                        }
                        let mut handle = std::io::stdout().lock();
                        let _ = writeln!(
                            handle,
                            "{}",
                            json!({
                                "id": instruction["id"].as_str().unwrap_or_default(),
                                "result": {},
                            })
                        );
                        let _ = handle.flush();
                        continue;
                    }
                }
                if sender.send(row).is_err() {
                    return;
                }
            }
        });
    }

    while let Ok(row) = receiver.recv() {
        let Ok(instruction) = serde_json::from_str::<Value>(&row) else {
            continue;
        };
        if instruction["relay"].as_str() == Some("turn") {
            {
                let id = instruction["id"].as_str().unwrap_or_default().to_owned();
                let text = instruction["text"].as_str().unwrap_or_default().to_owned();
                emit(json!({
                    "id": id,
                    "result": {"disposition": "queued", "turnId": id},
                }));
                let mut command = Command::new(muse_bin);
                command
                    .args(["exec", "--json", "--session-id", session])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit());
                if let Some(effort) = instruction["effort"].as_str() {
                    command.args(["--reasoning-effort", effort]);
                }
                if let Some(model) = model {
                    command.args(["--model", model]);
                }
                if let Some(provider) = provider {
                    command.args(["--provider", provider]);
                }
                command.arg(&text);
                std::os::unix::process::CommandExt::process_group(&mut command, 0);
                let started = Instant::now();
                let Ok(mut child) = command.spawn() else {
                    emit(json!({
                        "method": "turn/completed",
                        "params": {"turnId": id, "terminal": "failed"},
                    }));
                    continue;
                };
                if let Ok(mut slot) = running.lock() {
                    *slot = Some(child.id());
                }
                let mut saw_start = false;
                let mut final_text = String::new();
                let mut terminal = "failed".to_owned();
                let mut usage = json!({});
                if let Some(out) = child.stdout.take() {
                    for row in BufReader::new(out).lines() {
                        let Ok(row) = row else { break };
                        let Ok(record) = serde_json::from_str::<Value>(&row) else {
                            continue;
                        };
                        let payload = &record["payload"];
                        match record["payload_type"].as_str() {
                            Some("run.lifecycle.started") if !saw_start => {
                                saw_start = true;
                                emit(json!({
                                    "method": "turn/started",
                                    "params": {"turnId": id},
                                }));
                            }
                            Some("run.usage") => {
                                if let Some(first) = payload["models"]
                                    .as_array()
                                    .and_then(|models| models.first())
                                {
                                    usage = json!({
                                        "input_tokens": first["input_tokens"],
                                        "output_tokens": first["output_tokens"],
                                        "cache_read_input_tokens": first["cached_tokens"],
                                        "cache_creation_input_tokens": 0,
                                    });
                                }
                            }
                            Some("run.terminal.completed") => {
                                terminal = payload["terminal"]
                                    .as_str()
                                    .unwrap_or("completed")
                                    .to_owned();
                                final_text =
                                    payload["text"].as_str().unwrap_or_default().to_owned();
                            }
                            _ => {}
                        }
                    }
                }
                let status = child.wait().ok();
                if let Ok(mut slot) = running.lock() {
                    *slot = None;
                }
                let _ = status;
                if interrupted.lock().map(|flag| *flag).unwrap_or(false) {
                    terminal = "cancelled".to_owned();
                    if let Ok(mut flag) = interrupted.lock() {
                        *flag = false;
                    }
                }
                if !saw_start {
                    emit(json!({
                        "method": "turn/started",
                        "params": {"turnId": id},
                    }));
                }
                if !final_text.is_empty() {
                    emit(json!({
                        "method": "item/completed",
                        "params": {"item": {
                            "itemId": format!("{id}-msg"),
                            "kind": "agentMessage",
                            "messagePhase": "finalAnswer",
                            "status": "completed",
                            "text": final_text,
                            "turnId": id,
                        }, "sessionId": session},
                    }));
                }
                emit(json!({
                    "method": "turn/completed",
                    "params": {
                        "turnId": id,
                        "terminal": terminal,
                        "durationMs": started.elapsed().as_millis() as u64,
                        "usage": usage,
                    },
                }));
            }
        }
    }
    0
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
        let lines =
            harness().hello_lines(false, "sess-1", Path::new("/w"), Some("high"), &[], true);
        assert_eq!(lines.len(), 3);
        let open: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(open["method"], "session/start");
        assert_eq!(open["params"]["sessionId"], "sess-1");
        assert_eq!(open["params"]["approvalMode"], "onRequest");
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    /// The operator arguments of the #2247 report: a Muse profile carrying exec and TUI flags,
    /// as `declared_arguments` hands them to the wrapper (effort and model appended).
    fn reported_profile() -> Vec<String> {
        strings(&[
            "--yolo",
            "--dangerously-enable-internet-mode",
            "--disable-request-user-input",
            "--effort",
            "max",
            "--model",
            "m1",
        ])
    }

    #[test]
    fn the_reported_profile_launches_a_serve_argv_serve_accepts() {
        // #2247: serve refuses --yolo with exit 2; it becomes allowAll on the wire, and
        // --yolo's sandbox half becomes serve's own --disable-sandbox.
        let argv = harness().launch_arguments(false, "s", &reported_profile(), true);
        assert_eq!(
            argv,
            strings(&[
                "serve",
                "--trust-workspace",
                "--disable-request-user-input",
                "--disable-sandbox",
                "--dangerously-enable-internet-mode",
                "--model",
                "m1",
            ])
        );
        let launch = serve_launch(&reported_profile());
        assert_eq!(launch.approval_mode, Some("allowAll"));
        assert!(launch.dropped.is_empty(), "{:?}", launch.dropped);
        assert!(harness().launch_warnings(&reported_profile()).is_empty());
        // Nothing is repeated, and every kept option is serve's or the launcher's.
        let options: Vec<&String> = argv[1..].iter().filter(|a| a.starts_with("--")).collect();
        let mut unique = options.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), options.len(), "{argv:?}");
        for option in options {
            assert!(
                SERVE_FLAGS.contains(&option.as_str())
                    || SERVE_SINGLE.contains(&option.as_str())
                    || LAUNCHER_FLAGS.contains(&option.as_str()),
                "{option} is not a serve or launcher option"
            );
        }
    }

    #[test]
    fn the_reported_profile_selects_allow_all_and_carries_its_effort_on_the_wire() {
        let h = harness();
        let arguments = reported_profile();
        let effort = crate::headless::adapter::declared_effort(&arguments);
        assert_eq!(effort, Some("max"));
        let hello = h.hello_lines(false, "sess-1", Path::new("/w"), effort, &arguments, true);
        let open: Value = serde_json::from_str(&hello[2]).unwrap();
        assert_eq!(open["method"], "session/start");
        assert_eq!(open["params"]["approvalMode"], "allowAll");
        // Nobody is asked: an allow-all session raises no prompts.
        assert!(!h.asks_wrapper(&arguments, true));
        // Once the session opens: the standing effort, and no approval change after a start.
        let opened = h.session_opened_lines("sess-1", effort, &arguments, true);
        assert_eq!(opened.len(), 1, "{opened:?}");
        let set: Value = serde_json::from_str(&opened[0]).unwrap();
        assert_eq!(set["method"], "session/setReasoningEffort");
        assert_eq!(set["params"]["sessionId"], "sess-1");
        assert_eq!(set["params"]["reasoningEffort"], "max");
        assert!(claude::is_uuid(
            set["params"]["commandId"].as_str().unwrap()
        ));
        // Every turn still carries it.
        let turn: Value =
            serde_json::from_str(&h.input_line("u1", "hi", "sess-1", effort)).unwrap();
        assert_eq!(turn["params"]["reasoningEffort"], "max");
    }

    #[test]
    fn a_resumed_session_gets_its_approval_mode_once_it_opens() {
        let h = harness();
        let arguments = reported_profile();
        let hello = h.hello_lines(
            true,
            "sess-1",
            Path::new("/w"),
            Some("max"),
            &arguments,
            true,
        );
        let open: Value = serde_json::from_str(&hello[2]).unwrap();
        assert_eq!(open["method"], "session/resume");
        assert!(open["params"].get("approvalMode").is_none());
        let opened = h.session_opened_lines("sess-1", Some("max"), &arguments, true);
        let methods: Vec<Value> = opened
            .iter()
            .map(|row| serde_json::from_str::<Value>(row).unwrap())
            .collect();
        assert_eq!(methods[0]["method"], "session/setReasoningEffort");
        assert_eq!(methods[1]["method"], "session/setApprovalMode");
        assert_eq!(methods[1]["params"]["mode"], "allowAll");
        assert_eq!(methods[1]["params"]["sessionId"], "sess-1");
        // Without an approval flag the default stands: onRequest with approvals.
        let plain = strings(&["--model", "m1"]);
        let _ = h.hello_lines(true, "sess-1", Path::new("/w"), None, &plain, true);
        let opened = h.session_opened_lines("sess-1", None, &plain, true);
        let set: Value = serde_json::from_str(&opened[0]).unwrap();
        assert_eq!(set["params"]["mode"], "onRequest");
        assert!(h.asks_wrapper(&plain, true));
    }

    #[test]
    fn a_refused_session_setting_is_logged_not_fatal() {
        let h = harness();
        let refused: Value = serde_json::from_str(
            r#"{"id":"agentctl-session-effort","error":{"code":-32602,"message":"invalid params","data":{"reason":"unknown_tier"}}}"#,
        )
        .unwrap();
        assert_eq!(
            h.parse(&refused),
            vec![Signal::SessionConfigFailed {
                setting: "reasoning_effort".into(),
                message: "invalid params (unknown_tier)".into(),
            }]
        );
        let accepted: Value = serde_json::from_str(
            r#"{"id":"agentctl-session-approval","result":{"status":"accepted","applyOutcome":"noop"}}"#,
        )
        .unwrap();
        assert!(h.parse(&accepted).is_empty());
    }

    #[test]
    fn serve_translates_approval_flags_and_leaves_exec_options_off() {
        let launch = serve_launch(&strings(&[
            "--approval-mode",
            "untrusted",
            "--json",
            "--session-id",
            "abc",
            "--approval-judge=off",
            "--reasoning-effort",
            "high",
            "--trust-workspace",
            "--trust-workspace",
            "-c",
            "a=1",
            "--config=a=1",
            "--config",
            "b=2",
            "--model=m0",
            "--model",
            "m1",
            "--listen",
            "stray",
            "--disable-sandbox=yes",
            "--meta-tag",
            "t1",
            "--meta-tag=t1",
        ]));
        assert_eq!(launch.approval_mode, Some("promptUnmatched"));
        assert_eq!(
            launch.argv,
            strings(&[
                "serve",
                "--trust-workspace",
                "--disable-request-user-input",
                "--config",
                "a=1",
                "--config",
                "b=2",
                "--model",
                "m1",
                "--meta-tag",
                "t1",
            ])
        );
        assert_eq!(
            launch.dropped,
            strings(&[
                "--json",
                "--session-id abc",
                "--approval-judge off",
                "--reasoning-effort high",
                "--listen stray",
                "--disable-sandbox=yes",
            ])
        );
        let warnings = harness().launch_warnings(&strings(&["--json"]));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("--json"), "{warnings:?}");
        // The last approval flag wins; `never` is the bypass, like --yolo.
        let last = serve_launch(&strings(&["--yolo", "--approval-mode=on-request"]));
        assert_eq!(last.approval_mode, Some("onRequest"));
        assert!(last.argv.contains(&"--disable-sandbox".to_owned()));
        let never = serve_launch(&strings(&["--approval-mode", "never"]));
        assert_eq!(never.approval_mode, Some("allowAll"));
        assert!(!never.argv.contains(&"--disable-sandbox".to_owned()));
        assert!(!harness().asks_wrapper(&strings(&["--approval-mode", "never"]), true));
        let disabled = serve_launch(&strings(&["--disable-approval"]));
        assert_eq!(disabled.approval_mode, Some("allowAll"));
        assert!(!disabled.argv.contains(&"--disable-sandbox".to_owned()));
        // No approvals and no flag: nobody answers, so unmatched calls are denied.
        assert_eq!(approval_mode(&[], false), "denyUnmatched");
        assert_eq!(approval_mode(&[], true), "onRequest");
    }

    #[test]
    fn the_exec_transport_launch_is_unchanged() {
        let h = MuseHarness::exec();
        let argv = h.launch_arguments(false, "sess-1", &reported_profile(), true);
        assert_eq!(argv, strings(&["--session", "sess-1", "--model", "m1"]));
        assert!(h
            .hello_lines(
                false,
                "sess-1",
                Path::new("/w"),
                Some("max"),
                &reported_profile(),
                true
            )
            .is_empty());
        assert!(h
            .session_opened_lines("sess-1", Some("max"), &reported_profile(), true)
            .is_empty());
        assert!(h.launch_warnings(&reported_profile()).is_empty());
        assert!(!h.asks_wrapper(&reported_profile(), true));
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
    fn a_completed_agent_message_is_text_whatever_its_phase() {
        // #1974: a CHAT_REPLY block in a non-final (e.g. commentary) completed message must
        // become assistant text in the event log, or the chat bridge cannot capture it.
        let h = harness();
        for phase in ["commentary", "finalAnswer", ""] {
            let message: Value = serde_json::json!({
                "method": "item/completed",
                "params": {"item": {
                    "itemId": "m1", "kind": "agentMessage", "messagePhase": phase,
                    "text": "<CHAT_REPLY_001>\nhi\n</CHAT_REPLY_001>", "turnId": "u1",
                }, "sessionId": "s1"},
            });
            assert_eq!(
                h.parse(&message),
                vec![Signal::Text(
                    "<CHAT_REPLY_001>\nhi\n</CHAT_REPLY_001>".into()
                )],
                "phase {phase:?}"
            );
        }
        let empty: Value = serde_json::json!({
            "method": "item/completed",
            "params": {"item": {
                "itemId": "m2", "kind": "agentMessage", "messagePhase": "commentary",
                "text": "", "turnId": "u1",
            }, "sessionId": "s1"},
        });
        assert!(h.parse(&empty).is_empty());
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
    fn recovery_proves_by_command_id_or_unique_prompt() {
        let dir = std::env::temp_dir().join(format!(
            "agentctl-muse-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("session.jsonl");
        std::fs::write(
            &journal,
            concat!(
                "{\"payload\":{\"command_id\":\"u-exec\",\"kind\":\"turn_input_user\"}}\n",
                "{\"payload\":{\"record\":{\"command\":{\"kind\":\"turn_submit\",\"prompt\":\"hello once\"}}}}\n",
                "{\"payload\":{\"record\":{\"command\":{\"kind\":\"turn_submit\",\"prompt\":\"twice\"}}}}\n",
                "{\"payload\":{\"record\":{\"command\":{\"kind\":\"turn_submit\",\"prompt\":\"twice\"}}}}\n",
            ),
        )
        .unwrap();
        let h = harness();
        assert!(h.transcript_received_for(&journal, "u-exec", "anything"));
        assert!(h.transcript_received_for(&journal, "u-other", "hello once"));
        assert!(!h.transcript_received_for(&journal, "u-other", "twice"));
        assert!(!h.transcript_received_for(&journal, "u-other", "never sent"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_approval_request_names_its_stage_and_choices() {
        let h = harness();
        let request: Value = serde_json::from_str(
            r#"{"id": 7, "method": "approval/request", "params": {
                "approvalId": "ap-1", "toolName": "shell",
                "rawArgs": "{\"command\": \"git push\"}", "itemId": "t1",
                "currentRequirementId": {"id": "r1"},
                "availableChoices": [
                    {"choiceId": "allow-once", "decision": "approved", "scope": "once", "label": "Allow"},
                    {"choiceId": "deny-once", "decision": "denied", "scope": "once", "label": "Deny"}
                ]}}"#,
        )
        .unwrap();
        let [Signal::PermissionRequest(pending)] = &h.parse(&request)[..] else {
            panic!("expected one permission request");
        };
        assert_eq!(pending.request_id, "ap-1");
        assert_eq!(pending.tool, "shell");
        assert_eq!(pending.requirement, Some(json!({"id": "r1"})));
        assert_eq!(pending.choices.len(), 2);
        // The answer names the stage and picks the once-scoped choice.
        let line = h.permission_answer_line(
            "ap-1",
            true,
            &json!({}),
            "",
            pending.requirement.as_ref(),
            &pending.choices,
            "sess-1",
        );
        let decide: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(decide["method"], "approval/decide");
        assert_eq!(decide["params"]["approvalId"], "ap-1");
        assert_eq!(decide["params"]["choiceId"], "allow-once");
        assert_eq!(decide["params"]["requirementId"], json!({"id": "r1"}));
        assert_eq!(decide["params"]["sessionId"], "sess-1");
        let denied = h.permission_answer_line(
            "ap-1",
            false,
            &json!({}),
            "not now",
            pending.requirement.as_ref(),
            &pending.choices,
            "sess-1",
        );
        let decide: Value = serde_json::from_str(&denied).unwrap();
        assert_eq!(decide["params"]["choiceId"], "deny-once");
        assert_eq!(decide["params"]["feedback"], "not now");
        // Presenting is receipted at once; the decision travels separately.
        let receipt = h.permission_receipt_line(pending).unwrap();
        let receipt: Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(receipt["result"], json!({}));
    }

    #[test]
    fn approvals_decided_elsewhere_and_stage_changes_parse() {
        let h = harness();
        let resolved: Value = serde_json::from_str(
            r#"{"method":"approval/resolved","params":{"approvalId":"ap-1","decision":"approvedForSession"}}"#,
        )
        .unwrap();
        assert_eq!(
            h.parse(&resolved),
            vec![Signal::PermissionResolved {
                request_id: "ap-1".into(),
                decision: "approvedForSession".into(),
            }]
        );
        let updated: Value = serde_json::from_str(
            r#"{"method":"approval/updated","params":{"approvalId":"ap-1","currentRequirementId":{"id":"r2"},"availableChoices":[{"choiceId":"allow-2","decision":"approved","scope":"once","label":"Allow"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            h.parse(&updated),
            vec![Signal::PermissionUpdated {
                request_id: "ap-1".into(),
                requirement: Some(json!({"id": "r2"})),
                choices: vec![("allow-2".into(), "approved".into(), "once".into())],
            }]
        );
    }

    #[test]
    fn exec_lines_drive_the_relay() {
        let h = MuseHarness::exec();
        let turn: Value =
            serde_json::from_str(&h.input_line("u1", "hello", "s1", Some("max"))).unwrap();
        assert_eq!(turn["relay"], "turn");
        assert_eq!(turn["effort"], "max");
        let interrupt: Value =
            serde_json::from_str(&h.interrupt_line("agentctl-interrupt-1", "s1", Some("u1")))
                .unwrap();
        assert_eq!(interrupt["relay"], "interrupt");
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
