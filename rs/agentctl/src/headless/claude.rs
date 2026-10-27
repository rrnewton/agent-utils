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
//! This module only translates. It holds no state and does no I/O except reading a transcript
//! during recovery.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::events::{excerpt, SUMMARY_BYTES};

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
    },
    /// A tool call returned.
    ToolCompleted {
        /// Tool call identifier.
        id: String,
        /// False when the result is marked as an error.
        ok: bool,
        /// Size of the result text, in bytes.
        bytes: usize,
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
        /// Refused tool calls: `(tool, summary)`.
        denials: Vec<(String, String)>,
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

/// The operator arguments as the harness gets them: `--permission-prompts none` is added
/// unless given, so no tool call can wait for a human; anything refused is denied and shows as
/// `permission.denied` in the event log.
#[must_use]
pub fn effective_arguments(arguments: &[String]) -> Vec<String> {
    let mut effective = arguments.to_vec();
    let given = arguments
        .iter()
        .any(|argument| argument.split('=').next() == Some("--permission-prompts"));
    if !given {
        effective.extend(["--permission-prompts".to_owned(), "none".to_owned()]);
    }
    effective
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

/// The harness input ID of queue item `queue_id`: the ID itself when it is a UUID, else a
/// version-8 UUID (RFC 9562) built from SHA-256 of a fixed namespace and the ID, so the same
/// item always maps to the same input.
#[must_use]
pub fn input_uuid(queue_id: &str) -> String {
    if is_uuid(queue_id) {
        return queue_id.to_ascii_lowercase();
    }
    let digest = Sha256::digest(format!("agentctl-headless-input/v1\0{queue_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let text = hex(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &text[..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..]
    )
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
                            Some(Signal::ToolStarted {
                                id: block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned(),
                                summary: tool_summary(
                                    &tool,
                                    block.get("input").unwrap_or(&Value::Null),
                                ),
                                tool,
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
                    .map(|block| Signal::ToolCompleted {
                        id: block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        ok: block.get("is_error").and_then(Value::as_bool) != Some(true),
                        bytes: content_bytes(block.get("content").unwrap_or(&Value::Null)),
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
                            (tool, summary)
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
                    summary: "git push --token=*** origin".to_owned()
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
                bytes: 3
            }]
        );
        let result = json!({"type": "result", "subtype": "success", "duration_ms": 1795,
            "usage": {"input_tokens": 2}, "model_usage_since_last_result": {"m": {"costUSD": 0.25}, "n": {"costUSD": 0.5}},
            "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "touch x"}}]});
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
        assert_eq!(denials, &vec![("Bash".to_owned(), "touch x".to_owned())]);
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
    fn input_ids_are_uuids_and_stable() {
        let id = "01793116308769000000-4242";
        let uuid = input_uuid(id);
        assert!(is_uuid(&uuid), "{uuid}");
        assert_eq!(uuid, input_uuid(id));
        assert_eq!(&uuid[14..15], "8");
        let given = "6B4A1C1E-2F6E-4F7E-9A51-1D0C3E2A9B10";
        assert_eq!(input_uuid(given), given.to_ascii_lowercase());
        let line: Value = serde_json::from_str(&input_line(&uuid, "hi", "s")).unwrap();
        assert_eq!(line["uuid"], uuid.as_str());
        assert_eq!(line["message"]["content"], "hi");
    }

    #[test]
    fn permission_prompts_default_to_none() {
        assert_eq!(
            effective_arguments(&["--model".to_owned(), "m".to_owned()]),
            vec!["--model", "m", "--permission-prompts", "none"]
        );
        let given = vec!["--permission-prompts=host".to_owned()];
        assert_eq!(effective_arguments(&given), given);
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
}
