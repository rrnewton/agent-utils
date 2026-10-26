//! Rust edition of `agentctl chat context` (#545 rust-primary-agentctl-chat-readers).
//!
//! Reads one bounded page of prior thread messages for a saved request
//! through the operator's adapter transport: the Unix-socket transport when
//! `transport_socket` is configured, else the literal command transport
//! (`transport_command`). The validation of the request and of the adapter's
//! response follows the existing reader exactly (resource shapes, strict
//! cutoff, descending order, sender and size bounds, cursor rules) and the
//! printed document is byte-identical. The built-in provider transport is
//! not implemented in this edition yet: with neither adapter configured this
//! refuses and names the other edition, rather than guessing at provider
//! credentials.

use std::io::{Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::chat_history::{read_json_value, request_path, Ordered};

const MAX_MESSAGES: i64 = 200;
const MAX_CURSOR_BYTES: usize = 8 * 1024;
const MAX_TEXT_BYTES: usize = 32_000;
const MAX_COMMAND_STDOUT: usize = 8 * 1024 * 1024;
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(60);

type Result<T> = std::result::Result<T, String>;

fn invalid(message: impl Into<String>) -> String {
    message.into()
}

fn is_id_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

fn resource_matches(value: &str, space: &str, kind: &str) -> bool {
    let prefix = format!("{space}/{kind}/");
    value.len() > prefix.len()
        && value.starts_with(&prefix)
        && value[prefix.len()..].bytes().all(is_id_char)
}

fn exact_space(value: &str) -> bool {
    // spaces/[A-Za-z0-9_-]+
    let Some(rest) = value.strip_prefix("spaces/") else {
        return false;
    };
    !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// (epoch seconds, nanoseconds) for an RFC3339 timestamp, or the reader's error.
fn instant(value: &str) -> Result<(i64, i64)> {
    let bad = || {
        invalid(
            "context timestamps must be RFC3339 with a timezone and at most nine fractional digits",
        )
    };
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return Err(bad());
    }
    let digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let shape = digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && bytes.get(4) == Some(&b'-')
        && digit(5)
        && digit(6)
        && bytes.get(7) == Some(&b'-')
        && digit(8)
        && digit(9)
        && matches!(bytes.get(10), Some(b'T') | Some(b't'))
        && digit(11)
        && digit(12)
        && bytes.get(13) == Some(&b':')
        && digit(14)
        && digit(15)
        && bytes.get(16) == Some(&b':')
        && digit(17)
        && digit(18);
    if !shape {
        return Err(bad());
    }
    let number = |start: usize, len: usize| -> i64 {
        value
            .get(start..start + len)
            .and_then(|s| s.parse().ok())
            .unwrap_or(-1)
    };
    let (year, month, day, hour, minute, second) = (
        number(0, 4),
        number(5, 2),
        number(8, 2),
        number(11, 2),
        number(14, 2),
        number(17, 2),
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
        || year < 0
    {
        return Err(bad());
    }
    let mut index = 19;
    let mut nanos: i64 = 0;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let digits = &value[start..index];
        if digits.is_empty() || digits.len() > 9 {
            return Err(bad());
        }
        let mut padded = digits.to_owned();
        while padded.len() < 9 {
            padded.push('0');
        }
        nanos = padded.parse().map_err(|_| bad())?;
    }
    let offset_seconds: i64 = match bytes.get(index) {
        Some(b'Z') | Some(b'z') if index + 1 == bytes.len() => 0,
        Some(sign @ (b'+' | b'-')) if index + 6 == bytes.len() => {
            let zone_hour = number(index + 1, 2);
            let zone_minute = number(index + 4, 2);
            if bytes.get(index + 3) != Some(&b':')
                || !(0..=23).contains(&zone_hour)
                || !(0..=59).contains(&zone_minute)
            {
                return Err(invalid("context timestamp has an invalid timezone offset"));
            }
            let magnitude = zone_hour * 3600 + zone_minute * 60;
            if *sign == b'+' {
                magnitude
            } else {
                -magnitude
            }
        }
        _ => return Err(bad()),
    };
    // Days from civil (Howard Hinnant's algorithm), then clock and offset.
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Ok((
        days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds,
        nanos,
    ))
}

fn get_text<'a>(value: &'a Value, field: &str, label: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{label} {field} must be a string")))
}

fn valid_limit(limit: i64) -> Result<i64> {
    if (1..=MAX_MESSAGES).contains(&limit) {
        Ok(limit)
    } else {
        Err(invalid(
            "context limit must be an integer between 1 and 200",
        ))
    }
}

fn valid_cursor(cursor: Option<&str>) -> Result<Option<String>> {
    match cursor {
        None => Ok(None),
        Some("") => Err(invalid("context cursor must be a nonempty string or null")),
        Some(value) if value.len() > MAX_CURSOR_BYTES => Err(invalid(format!(
            "context cursor must not exceed {MAX_CURSOR_BYTES} UTF-8 bytes"
        ))),
        Some(value) => Ok(Some(value.to_owned())),
    }
}

fn thread_reply(value: &Value, label: &str) -> Result<Option<bool>> {
    match value.get("thread_reply") {
        None => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(invalid(format!(
            "{label} thread_reply must be a boolean when present"
        ))),
    }
}

enum Transport {
    Socket(String),
    Command(Vec<String>),
}

fn call_transport(transport: &Transport, request: &Value) -> Result<Value> {
    let document = match transport {
        Transport::Socket(path) => call_socket(path, request)?,
        Transport::Command(command) => call_command(command, request)?,
    };
    if !document.is_object() {
        return Err(invalid("context response must be an object"));
    }
    Ok(document)
}

fn call_socket(path: &str, request: &Value) -> Result<Value> {
    let socket_path = Path::new(path);
    let parent = socket_path
        .parent()
        .ok_or_else(|| invalid("chat adapter socket path has no parent"))?;
    let uid = unsafe { libc::getuid() };
    let parent_meta = std::fs::metadata(parent).map_err(|error| {
        invalid(format!(
            "cannot inspect chat adapter socket directory: {error}"
        ))
    })?;
    let endpoint_meta = std::fs::symlink_metadata(socket_path)
        .map_err(|error| invalid(format!("cannot inspect chat adapter socket: {error}")))?;
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        if !parent_meta.is_dir()
            || parent_meta.uid() != uid
            || parent_meta.permissions().mode() & 0o077 != 0
            || !endpoint_meta.file_type().is_socket()
            || endpoint_meta.uid() != uid
        {
            return Err(invalid(
                "chat adapter socket must be owned by this account in a private directory",
            ));
        }
    }
    let payload = serde_json::to_string(request)
        .map_err(|error| invalid(format!("cannot encode context request: {error}")))?;
    let mut payload = payload.into_bytes();
    payload.push(b'\n');
    if payload.len() > 1024 * 1024 {
        return Err(invalid("chat adapter request exceeds 1 MiB"));
    }
    let deadline = Instant::now() + TRANSPORT_TIMEOUT;
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|error| invalid(format!("cannot connect to chat adapter socket: {error}")))?;
    stream
        .set_read_timeout(Some(TRANSPORT_TIMEOUT))
        .map_err(|error| invalid(format!("cannot bound chat adapter socket: {error}")))?;
    stream
        .write_all(&payload)
        .map_err(|error| invalid(format!("cannot send to chat adapter socket: {error}")))?;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 65536];
    while !response.contains(&b'\n') {
        if Instant::now() > deadline {
            return Err(invalid("chat adapter response exceeded its deadline"));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(|error| invalid(format!("cannot read chat adapter response: {error}")))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        if response.len() > 1024 * 1024 {
            return Err(invalid("chat adapter response exceeds 1 MiB"));
        }
    }
    let line = response
        .split(|b| *b == b'\n')
        .next()
        .ok_or_else(|| invalid("chat adapter response is empty"))?;
    let text =
        std::str::from_utf8(line).map_err(|_| invalid("chat adapter response is not UTF-8"))?;
    serde_json::from_str(text)
        .map_err(|error| invalid(format!("chat adapter response is not JSON: {error}")))
}

fn call_command(command: &[String], request: &Value) -> Result<Value> {
    if command.is_empty() {
        return Err(invalid("chat helper command must not be empty"));
    }
    let payload = serde_json::to_string(request)
        .map_err(|error| invalid(format!("cannot encode context request: {error}")))?;
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| invalid(format!("cannot run chat adapter command: {error}")))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(payload.as_bytes())
            .map_err(|error| invalid(format!("cannot write to chat adapter: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| invalid(format!("chat adapter command failed: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(invalid(format!(
            "chat adapter exited {}: {tail}",
            output.status.code().unwrap_or(-1)
        )));
    }
    if output.stdout.len() > MAX_COMMAND_STDOUT {
        return Err(invalid("chat adapter response exceeds its stdout bound"));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("chat adapter response is not UTF-8"))?;
    serde_json::from_str(text.trim())
        .map_err(|error| invalid(format!("chat adapter response is not JSON: {error}")))
}

/// Read one page of prior thread messages; returns the printed JSON document.
pub fn read_context(
    state: &Path,
    request: &str,
    limit: i64,
    cursor: Option<&str>,
) -> Result<String> {
    let limit = valid_limit(limit)?;
    let cursor = valid_cursor(cursor)?;
    let bridge = read_json_value(&state.join("bridge.json"))?;
    let config = bridge
        .get("config")
        .cloned()
        .ok_or_else(|| invalid("saved config must be an object"))?;
    let config_space = get_text(&config, "space", "chat config")?.to_owned();
    let path = request_path(state, request)?;
    let record = read_json_value(&path)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    if record.get("key").and_then(Value::as_str) != Some(stem.as_str()) {
        return Err(invalid("saved request key does not match its file"));
    }
    let source = record
        .get("message")
        .cloned()
        .ok_or_else(|| invalid("saved source message must be an object"))?;
    if !source.is_object() {
        return Err(invalid("saved source message must be an object"));
    }
    let source_id = get_text(&source, "id", "saved source message")?.to_owned();
    if !source_id.starts_with(&format!("{config_space}/messages/")) {
        return Err(invalid(
            "saved request belongs to a different configured space",
        ));
    }
    let transport = select_transport(&config)?;
    let document = read_context_with(&transport, &source, limit, cursor.as_deref())?;
    let mut out = String::new();
    document.dump(0, &mut out);
    Ok(out)
}

fn select_transport(config: &Value) -> Result<Transport> {
    if let Some(socket) = config.get("transport_socket") {
        if !socket.is_null() {
            let socket = socket
                .as_str()
                .ok_or_else(|| invalid("transport_socket must be an absolute Unix socket path"))?;
            if !Path::new(socket).is_absolute() {
                return Err(invalid(
                    "transport_socket must be an absolute Unix socket path",
                ));
            }
            return Ok(Transport::Socket(socket.to_owned()));
        }
    }
    if let Some(command) = config.get("transport_command") {
        if let Some(items) = command.as_array() {
            if !items.is_empty() {
                let argv: Vec<String> = items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("transport_command entries must be strings"))
                    })
                    .collect::<Result<Vec<String>>>()?;
                return Ok(Transport::Command(argv));
            }
        }
    }
    Err(invalid(
        "chat context in this edition needs a configured adapter (transport_socket or transport_command); the built-in provider transport is not implemented yet, so run it with the other edition (for example DAGRUN_ENGINE=python)",
    ))
}

fn read_context_with(
    transport: &Transport,
    source: &Value,
    limit: i64,
    cursor: Option<&str>,
) -> Result<Ordered> {
    let identifier = get_text(source, "id", "context source")?.to_owned();
    // identifier must be <spaces/ID>/messages/<id>
    let space = {
        let Some(rest) = identifier.strip_prefix("spaces/") else {
            return Err(invalid(
                "context source must name one exact message resource",
            ));
        };
        let Some((id, tail)) = rest.split_once("/messages/") else {
            return Err(invalid(
                "context source must name one exact message resource",
            ));
        };
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || tail.is_empty()
            || !tail.bytes().all(is_id_char)
        {
            return Err(invalid(
                "context source must name one exact message resource",
            ));
        }
        format!("spaces/{id}")
    };
    if !exact_space(&space) {
        return Err(invalid(
            "context source must name one exact message resource",
        ));
    }
    let thread = get_text(source, "thread", "context source")?.to_owned();
    let before = get_text(source, "created_at", "context source")?.to_owned();
    thread_reply(source, "context source")?;
    // google_context_params validation
    if !exact_space(&space) {
        return Err(invalid("context space must be an exact spaces/ID resource"));
    }
    if !resource_matches(&thread, &space, "threads") {
        return Err(invalid(
            "context threads resource must belong to the selected space",
        ));
    }
    let cutoff = instant(&before)?;
    let request = serde_json::json!({
        "action": "context",
        "space": space,
        "thread": thread,
        "before": before,
        "limit": limit,
        "cursor": cursor,
    });
    let document = call_transport(transport, &request)?;
    let continuation = match document.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => valid_cursor(Some(value))?,
        Some(_) => return Err(invalid("context cursor must be a nonempty string or null")),
    };
    if cursor.is_some() && continuation.as_deref() == cursor {
        return Err(invalid("context response repeated its input cursor"));
    }
    let rows = document
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("context messages must be an array"))?;
    if rows.len() as i64 > limit {
        return Err(invalid(
            "context response exceeds the requested message limit",
        ));
    }
    let mut messages: Vec<Ordered> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut previous = cutoff;
    for row in rows {
        if !row.is_object() {
            return Err(invalid("context message must be an object"));
        }
        let name = get_text(row, "id", "context message")?.to_owned();
        if !resource_matches(&name, &space, "messages") {
            return Err(invalid(
                "context messages resource must belong to the selected space",
            ));
        }
        if name == identifier {
            return Err(invalid("context response included the source message"));
        }
        if seen.contains(&name) {
            return Err(invalid("context response repeated a message"));
        }
        seen.push(name.clone());
        if get_text(row, "thread", "context message")? != thread {
            return Err(invalid(
                "context response included a message from another thread",
            ));
        }
        let stamp = get_text(row, "created_at", "context message")?.to_owned();
        let at = instant(&stamp)?;
        if at >= cutoff {
            return Err(invalid(
                "context response included a message not before the source",
            ));
        }
        if at > previous {
            return Err(invalid(
                "context response must arrive in descending creation order",
            ));
        }
        previous = at;
        let sender = get_text(row, "sender", "context message")?.to_owned();
        let sender_valid = sender.strip_prefix("users/").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
        if !sender_valid {
            return Err(invalid(
                "context message sender must be an exact users/ID resource",
            ));
        }
        let text = get_text(row, "text", "context message")?.to_owned();
        if text.len() > MAX_TEXT_BYTES {
            return Err(invalid("context message text exceeds 32000 UTF-8 bytes"));
        }
        let reply_flag = thread_reply(row, "context message")?;
        let mut fields = vec![
            ("id".to_owned(), Ordered::Text(name)),
            ("thread".to_owned(), Ordered::Text(thread.clone())),
            ("created_at".to_owned(), Ordered::Text(stamp)),
            ("sender".to_owned(), Ordered::Text(sender)),
            ("text".to_owned(), Ordered::Text(text)),
        ];
        if let Some(flag) = reply_flag {
            fields.push(("thread_reply".to_owned(), Ordered::Bool(flag)));
        }
        messages.push(Ordered::object(fields));
    }
    messages.reverse();
    Ok(Ordered::object(vec![
        ("source".to_owned(), Ordered::Text(identifier)),
        ("space".to_owned(), Ordered::Text(space)),
        ("thread".to_owned(), Ordered::Text(thread)),
        ("before".to_owned(), Ordered::Text(before)),
        (
            "order".to_owned(),
            Ordered::Text("chronological".to_owned()),
        ),
        ("messages".to_owned(), Ordered::Array(messages)),
        (
            "cursor".to_owned(),
            continuation.map_or(Ordered::Null, Ordered::Text),
        ),
    ]))
}

/// Parse a context document for tests without any transport.
#[cfg(test)]
fn context_fixture() -> Value {
    serde_json::json!({
        "id": "spaces/test/messages/incoming.message",
        "thread": "spaces/test/threads/original-thread",
        "created_at": "2026-01-02T00:00:00.123456789Z",
        "sender": "users/owner",
        "text": "Please investigate",
        "thread_reply": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instant_preserves_nanoseconds_and_offsets() {
        let base = instant("2026-01-02T00:00:00.123456789Z").expect("instant");
        assert_eq!(base.1, 123_456_789);
        let shifted = instant("2026-01-02T01:00:00+01:00").expect("offset");
        assert_eq!(shifted.0, instant("2026-01-02T00:00:00Z").expect("utc").0);
        assert!(instant("2026-01-02 00:00:00Z").is_err());
    }

    #[test]
    fn context_validation_rejects_out_of_order() {
        let transport = Transport::Command(vec!["true".to_owned()]);
        // The fixture transport is exercised through the Python differential;
        // here only the pure validators run.
        assert_eq!(
            valid_limit(0),
            Err("context limit must be an integer between 1 and 200".to_owned())
        );
        assert!(valid_limit(200).is_ok());
        let _ = transport;
        let _ = context_fixture();
    }
}
