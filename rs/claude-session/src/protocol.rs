//! Claude Code's stream-json protocol framing, as the headless wrapper speaks it.
//!
//! Moved verbatim from `agentctl::headless::claude` (`#1331 claude-session-summarizer`);
//! agentctl re-exports these names, so its behaviour is unchanged. This module only frames:
//! it holds no state and does no I/O except reading `/dev/urandom` for [`random_uuid`].

use std::fs::File;

use serde_json::{json, Value};

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
/// asks the owner; without, `--permission-prompts none` is added, so no tool
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_arguments_are_the_stream_json_set() {
        assert_eq!(
            protocol_arguments(),
            vec![
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose"
            ]
        );
    }

    #[test]
    fn effective_arguments_default_to_host_prompts() {
        assert_eq!(
            effective_arguments(&[], true),
            vec![
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio"
            ]
        );
        assert_eq!(
            effective_arguments(&[], false),
            vec!["--permission-prompts", "none"]
        );
        let own = vec!["--permission-prompts".to_owned(), "none".to_owned()];
        assert_eq!(effective_arguments(&own, true), own);
    }

    #[test]
    fn reserved_arguments_are_refused() {
        assert!(validate_arguments(&["--model".to_owned(), "x".to_owned()]).is_ok());
        assert!(validate_arguments(&["-p".to_owned()]).is_err());
        assert!(validate_arguments(&[String::new()]).is_err());
    }

    #[test]
    fn uuid_helpers_agree() {
        let uuid = random_uuid().unwrap();
        assert!(is_uuid(&uuid));
        assert!(!is_uuid("not-a-uuid"));
    }

    #[test]
    fn input_line_frames_a_user_turn() {
        let line = input_line("u-1", "hello", "s-1");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["uuid"], "u-1");
        assert_eq!(value["message"]["content"], "hello");
        assert!(line.ends_with('\n'));
    }
}
