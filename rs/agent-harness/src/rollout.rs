//! Codex rollout records at the format level.
//!
//! The event decoder (`codex.rs`) reduces a rollout to the
//! shared event model, which is all a reply reader needs. A
//! transcript archivist needs the records themselves: session
//! metadata, turn lifecycle, tool inputs and outputs, sub-agent
//! lineage. This module is that layer of the format: how the
//! newline-complete prefix of a rollout file splits into
//! records, how a record's timestamp reads, and how message
//! content parts flatten. Selection, snapshotting, and team
//! assembly remain the caller's.

use serde_json::{Map, Value};

/// One newline-complete rollout record.
#[derive(Clone, Debug)]
pub struct RolloutRecord {
    /// 1-based line number in the complete prefix.
    pub line: i64,
    /// The raw line bytes (no line boundary).
    pub raw: Vec<u8>,
    /// The parsed JSON object.
    pub value: Value,
}

impl RolloutRecord {
    /// The record's `type` field, when a string.
    #[must_use]
    pub fn kind(&self) -> Option<&str> {
        self.value.get("type").and_then(Value::as_str)
    }

    /// The record's payload object (empty when absent).
    #[must_use]
    pub fn payload(&self) -> Map<String, Value> {
        self.value
            .get("payload")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// The payload's `type` field, when a string.
    #[must_use]
    pub fn payload_kind(&self) -> Option<String> {
        self.payload()
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// The record timestamp in milliseconds (0 when unreadable).
    #[must_use]
    pub fn timestamp_ms(&self) -> i64 {
        iso_ms(self.value.get("timestamp").unwrap_or(&Value::Null))
    }
}

/// Why a rollout's records do not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum RolloutError {
    /// A line is not valid JSON (or not UTF-8 JSON text).
    InvalidJson(i64),
    /// A line parses but is not a non-empty JSON object.
    NotAnObject(i64),
}

/// The newline-complete prefix of a rollout file: everything
/// through the last newline, or nothing.
#[must_use]
pub fn complete_prefix(data: &[u8]) -> &[u8] {
    if data.last() == Some(&b'\n') {
        return data;
    }
    match data.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => &data[..=index],
        None => &[],
    }
}

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    // CPython `bytes.splitlines`: boundaries are \n, \r and
    // \r\n only (the wide set belongs to `str.splitlines`).
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < data.len() {
        let byte = data[index];
        if matches!(byte, b'\n' | b'\r') {
            lines.push(&data[start..index]);
            if byte == b'\r' && data.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            index += 1;
            start = index;
        } else {
            index += 1;
        }
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// Parse every record of a newline-complete prefix.
pub fn parse_rollout_records(complete: &[u8]) -> Result<Vec<RolloutRecord>, RolloutError> {
    let mut records = Vec::new();
    for (index, raw) in split_lines(complete).into_iter().enumerate() {
        let line = index as i64 + 1;
        let text = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
        let value: Value = parse_lenient(text).ok_or(RolloutError::InvalidJson(line))?;
        match &value {
            Value::Object(map) if !map.is_empty() => records.push(RolloutRecord {
                line,
                raw: raw.to_vec(),
                value,
            }),
            _ => return Err(RolloutError::NotAnObject(line)),
        }
    }
    Ok(records)
}

/// serde_json with CPython's non-standard constants (`NaN`,
/// `Infinity`, `-Infinity`) rewritten to `null` outside
/// string literals, which preserves downstream verdicts.
pub(crate) fn parse_lenient(raw: &[u8]) -> Option<Value> {
    if let Ok(value) = serde_json::from_slice::<Value>(raw) {
        return Some(value);
    }
    let text = std::str::from_utf8(raw).ok()?;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            // Push the full UTF-8 scalar, not one byte.
            let ch_len = utf8_len(byte);
            out.push_str(&text[index..(index + ch_len).min(text.len())]);
            index += ch_len;
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
            if let Some(tail) = rest.strip_prefix(token) {
                let _ = tail;
                let before_ok = index == 0
                    || matches!(
                        bytes[index - 1],
                        b' ' | b'\t' | b'\n' | b'\r' | b':' | b',' | b'[' | b'{'
                    );
                let after = rest[token.len()..].chars().next();
                let _ = &after;
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
            // Push the full UTF-8 scalar, not one byte.
            let ch_len = utf8_len(byte);
            out.push_str(&text[index..(index + ch_len).min(text.len())]);
            index += ch_len;
        }
    }
    serde_json::from_str(&out).ok()
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// The session-metadata payload of a rollout's first record,
/// when it is a `session_meta` record.
#[must_use]
pub fn session_metadata(records: &[RolloutRecord]) -> Option<Map<String, Value>> {
    let first = records.first()?;
    if first.kind() != Some("session_meta") {
        return None;
    }
    first
        .value
        .get("payload")
        .and_then(Value::as_object)
        .cloned()
}

/// An ISO-8601 timestamp to epoch milliseconds, truncating
/// toward zero; 0 when the value is not a readable timestamp.
/// A naive timestamp reads as UTC.
#[must_use]
pub fn iso_ms(value: &Value) -> i64 {
    let Some(text) = value.as_str() else {
        return 0;
    };
    parse_iso_ms(text).unwrap_or(0)
}

fn parse4(bytes: &[u8], at: usize) -> Option<i64> {
    let mut value = 0i64;
    for offset in 0..4 {
        let byte = *bytes.get(at + offset)?;
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

fn parse2(bytes: &[u8], at: usize) -> Option<i64> {
    let mut value = 0i64;
    for offset in 0..2 {
        let byte = *bytes.get(at + offset)?;
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

fn valid_date(year: i64, month: i64, day: i64) -> bool {
    if !(1..=12).contains(&month) || day < 1 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    day <= days
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_iso_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    // Date-only form reads as midnight UTC.
    if bytes.len() == 10 {
        let year = parse4(bytes, 0)?;
        let month = parse2(bytes, 5)?;
        let day = parse2(bytes, 8)?;
        if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
            return None;
        }
        if !valid_date(year, month, day) {
            return None;
        }
        return Some(days_from_civil(year, month, day) * 86_400_000);
    }
    if bytes.len() < 19 {
        return None;
    }
    let digit = |at: usize| -> Option<i64> {
        bytes
            .get(at)
            .filter(|b| b.is_ascii_digit())
            .map(|b| i64::from(b - b'0'))
    };
    let num = |at: usize, len: usize| -> Option<i64> {
        let mut value = 0i64;
        for offset in 0..len {
            value = value * 10 + digit(at + offset)?;
        }
        Some(value)
    };
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let year = num(0, 4)?;
    let month = num(5, 2)?;
    let day = num(8, 2)?;
    if !valid_date(year, month, day) {
        return None;
    }
    let separator = bytes.get(10).copied()?;
    if separator != b'T' && separator != b' ' {
        return None;
    }
    if bytes.get(13) != Some(&b':') || bytes.get(16) != Some(&b':') {
        return None;
    }
    let hour = num(11, 2)?;
    let minute = num(14, 2)?;
    let second = num(17, 2)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let mut index = 19;
    let mut micros = 0i64;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let mut digits = 0;
        let mut value = 0i64;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            if digits < 7 {
                value = value * 10 + i64::from(bytes[index] - b'0');
                digits += 1;
            }
            index += 1;
        }
        if digits == 0 {
            return None;
        }
        while digits < 7 {
            value *= 10;
            digits += 1;
        }
        // value holds 7 digits (100ns units); keep microseconds.
        micros = value / 10;
    }
    let zone = &text[index..];
    let offset_seconds: i64 = if zone.is_empty() || zone == "Z" {
        0
    } else if zone.len() == 6
        && (zone.starts_with('+') || zone.starts_with('-'))
        && zone.as_bytes().get(3) == Some(&b':')
    {
        let sign: i64 = if zone.starts_with('-') { -1 } else { 1 };
        let zh: i64 = zone[1..3].parse().ok()?;
        let zm: i64 = zone[4..6].parse().ok()?;
        sign * (zh * 3600 + zm * 60)
    } else if zone.len() == 5
        && (zone.starts_with('+') || zone.starts_with('-'))
        && zone[1..].bytes().all(|b| b.is_ascii_digit())
    {
        let sign: i64 = if zone.starts_with('-') { -1 } else { 1 };
        let zh: i64 = zone[1..3].parse().ok()?;
        let zm: i64 = zone[3..5].parse().ok()?;
        sign * (zh * 3600 + zm * 60)
    } else {
        return None;
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1000 + micros / 1000)
}

/// A numeric epoch that may be seconds or milliseconds:
/// values past 10^10 read as milliseconds already.
#[must_use]
pub fn numeric_epoch_ms(value: &Value, fallback_ms: i64) -> i64 {
    if value.is_boolean() {
        return fallback_ms;
    }
    let Some(numeric) = value.as_f64() else {
        return fallback_ms;
    };
    if numeric > 10_000_000_000.0 {
        numeric.trunc() as i64
    } else {
        (numeric * 1000.0).trunc() as i64
    }
}

/// Flatten a message payload's content parts into plain text,
/// encrypted text, and an availability label (`plaintext`,
/// `encrypted`, or `none`).
#[must_use]
pub fn content_parts(
    payload: &Map<String, Value>,
) -> (Option<String>, Option<String>, &'static str) {
    let Some(parts) = payload.get("content").and_then(Value::as_array) else {
        return (None, None, "none");
    };
    let mut plain: Vec<&str> = Vec::new();
    let mut encrypted: Vec<&str> = Vec::new();
    for part in parts {
        let Some(part) = part.as_object() else {
            continue;
        };
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    plain.push(text);
                }
            }
            Some("encrypted_content") => {
                if let Some(text) = part.get("encrypted_content").and_then(Value::as_str) {
                    encrypted.push(text);
                }
            }
            _ => {}
        }
    }
    let plain_text = plain.concat();
    let plain_text = if plain_text.is_empty() {
        None
    } else {
        Some(plain_text)
    };
    let encrypted_text = encrypted.join("\n");
    let encrypted_text = if encrypted_text.is_empty() {
        None
    } else {
        Some(encrypted_text)
    };
    if encrypted_text.is_some() {
        (plain_text, encrypted_text, "encrypted")
    } else if plain_text.is_some() {
        (plain_text, None, "plaintext")
    } else {
        (None, None, "none")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_parse_with_complete_prefix_and_metadata() {
        let data = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"t1\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n{\"partial\"";
        let records = parse_rollout_records(complete_prefix(data)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].payload_kind().as_deref(), Some("task_complete"));
        let metadata = session_metadata(&records).unwrap();
        assert_eq!(metadata["id"], "t1");
    }

    #[test]
    fn content_and_times_read() {
        let payload: Map<String, Value> = serde_json::from_str(
            r#"{"content":[{"type":"input_text","text":"a"},{"type":"encrypted_content","encrypted_content":"g1"},{"type":"output_text","text":"b"}]}"#,
        )
        .unwrap();
        let (plain, encrypted, availability) = content_parts(&payload);
        assert_eq!(plain.as_deref(), Some("ab"));
        assert_eq!(encrypted.as_deref(), Some("g1"));
        assert_eq!(availability, "encrypted");
        assert_eq!(
            iso_ms(&Value::String("2026-08-05T00:16:40Z".into())),
            1_785_889_000_000
        );
        assert_eq!(numeric_epoch_ms(&Value::from(1010), 7), 1_010_000);
        assert_eq!(
            numeric_epoch_ms(&Value::from(1_015_200_000i64), 7),
            1_015_200_000_000
        );
        assert_eq!(numeric_epoch_ms(&Value::Bool(true), 7), 7);
    }

    #[test]
    fn malformed_records_refuse_by_line() {
        assert!(matches!(
            parse_rollout_records(b"[1]\n"),
            Err(RolloutError::NotAnObject(1))
        ));
        assert!(matches!(
            parse_rollout_records(b"{bad\n"),
            Err(RolloutError::InvalidJson(1))
        ));
    }
}
