//! Claude Code session records at the format level.
//!
//! The event decoder (`claude.rs`) reduces a session to the
//! shared event model. An archivist needs the records
//! themselves: UUIDs, content blocks, tool uses and results,
//! sub-agent sidecars. This module is that layer: how a
//! session file's newline-complete prefix splits into records
//! and how a record's timestamp reads. Deduplication, fork
//! handling, and team assembly remain the caller's.

use serde_json::Value;

use crate::rollout::complete_prefix;

/// One newline-complete session record.
#[derive(Clone, Debug)]
pub struct SessionRecord {
    /// 1-based line number in the complete prefix.
    pub line: i64,
    /// The raw line bytes (no line boundary).
    pub raw: Vec<u8>,
    /// The parsed JSON object.
    pub value: Value,
}

/// Why a session's records do not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionError {
    /// A line is not valid JSON (or not UTF-8 JSON text).
    InvalidJson(i64),
    /// A line parses but is not a JSON object.
    NotAnObject(i64),
}

/// The lines of a session file's newline-complete prefix,
/// split the way CPython's `bytes.splitlines` splits.
pub fn session_lines(data: &[u8]) -> Vec<&[u8]> {
    split_lines(complete_prefix(data))
}

/// Parse one session line into its JSON object value.
/// CPython's `json.loads` also accepts the `NaN` and
/// `Infinity` constants; they read as `null` here, the
/// same lenient spelling the rollout reader uses.
pub fn parse_session_line(raw: &[u8], line: i64) -> Result<Value, SessionError> {
    let text = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    let value: Value =
        crate::rollout::parse_lenient(text).ok_or(SessionError::InvalidJson(line))?;
    if !value.is_object() {
        return Err(SessionError::NotAnObject(line));
    }
    Ok(value)
}

/// Parse every record of a session file's bytes (the
/// newline-complete prefix is taken first). A record must be
/// a JSON object; an empty object is a record.
pub fn parse_session_records(data: &[u8]) -> Result<Vec<SessionRecord>, SessionError> {
    let mut records = Vec::new();
    for (index, raw) in session_lines(data).into_iter().enumerate() {
        let line = index as i64 + 1;
        let value = parse_session_line(raw, line)?;
        records.push(SessionRecord {
            line,
            raw: raw.to_vec(),
            value,
        });
    }
    Ok(records)
}

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    // CPython `bytes.splitlines` (see rollout.rs).
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

/// Why a record timestamp does not read.
#[derive(Clone, Debug, PartialEq)]
pub enum TimestampError {
    /// Not an ISO-8601 timestamp.
    Invalid(String),
    /// A timestamp with no timezone.
    NoTimezone,
}

/// A record's `timestamp` field in epoch milliseconds.
/// `None` when the field is absent. Unlike the Codex
/// reader, a present-but-unreadable timestamp is an error,
/// and a timezone is required. The accepted shapes are
/// CPython `datetime.fromisoformat`'s: seconds optional,
/// fractional seconds of any width, and offsets
/// `±HH:MM[:SS]` or `±HHMM` under 24 hours. The result
/// truncates `timestamp() * 1000` toward zero, including
/// before the epoch.
pub fn timestamp_ms(value: &Value) -> Result<Option<i64>, TimestampError> {
    if value.is_null() {
        return Ok(None);
    }
    let Some(text) = value.as_str().filter(|text| !text.is_empty()) else {
        return Err(TimestampError::Invalid(value.to_string()));
    };
    parse_claude_timestamp(text).map(Some)
}

fn parse_claude_timestamp(text: &str) -> Result<i64, TimestampError> {
    let invalid = || TimestampError::Invalid(text.to_owned());
    let bytes = text.as_bytes();
    let digits = |start: usize, count: usize| -> Result<i64, TimestampError> {
        let mut value = 0i64;
        for offset in 0..count {
            let byte = *bytes.get(start + offset).ok_or_else(invalid)?;
            if !byte.is_ascii_digit() {
                return Err(invalid());
            }
            value = value * 10 + i64::from(byte - b'0');
        }
        Ok(value)
    };
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid());
    }
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    if !valid_date(year, month, day) {
        return Err(invalid());
    }
    let days = days_from_civil(year, month, day);
    if bytes.len() == 10 {
        return Err(TimestampError::NoTimezone);
    }
    if !matches!(bytes[10], b'T' | b' ') {
        return Err(invalid());
    }
    let hour = digits(11, 2)?;
    if bytes.get(13) != Some(&b':') {
        return Err(invalid());
    }
    let minute = digits(14, 2)?;
    if hour > 23 || minute > 59 {
        return Err(invalid());
    }
    let mut index = 16;
    let mut second = 0i64;
    if bytes.get(index) == Some(&b':') {
        second = digits(17, 2)?;
        if second > 59 {
            return Err(invalid());
        }
        index = 19;
    }
    let mut micros = 0i64;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let mut count = 0usize;
        let mut value = 0i64;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            if count < 6 {
                value = value * 10 + i64::from(bytes[index] - b'0');
                count += 1;
            }
            index += 1;
        }
        if count == 0 {
            return Err(invalid());
        }
        while count < 6 {
            value *= 10;
            count += 1;
        }
        micros = value;
    }
    let zone = &text[index..];
    if zone.is_empty() {
        // A well-formed naive timestamp is refused for
        // lacking a timezone, not for being unreadable.
        return Err(TimestampError::NoTimezone);
    }
    let offset_seconds: i64 = if zone == "Z" {
        0
    } else {
        let zone_bytes = zone.as_bytes();
        let sign: i64 = match zone_bytes.first() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Err(invalid()),
        };
        let body = &zone[1..];
        let (oh, om, os) = if body.len() == 5 && body.as_bytes()[2] == b':' {
            (
                digits_at(body, 0, 2, text)?,
                digits_at(body, 3, 2, text)?,
                0,
            )
        } else if body.len() == 8 && body.as_bytes()[2] == b':' && body.as_bytes()[5] == b':' {
            (
                digits_at(body, 0, 2, text)?,
                digits_at(body, 3, 2, text)?,
                digits_at(body, 6, 2, text)?,
            )
        } else if body.len() == 4 && body.bytes().all(|b| b.is_ascii_digit()) {
            (
                digits_at(body, 0, 2, text)?,
                digits_at(body, 2, 2, text)?,
                0,
            )
        } else {
            return Err(invalid());
        };
        if om > 59 || os > 59 {
            return Err(invalid());
        }
        let magnitude = oh * 3600 + om * 60 + os;
        if magnitude >= 24 * 3600 {
            return Err(invalid());
        }
        sign * magnitude
    };
    let local_micros = (days * 86_400 + hour * 3600 + minute * 60 + second) * 1_000_000 + micros;
    let epoch_micros = local_micros - offset_seconds * 1_000_000;
    // CPython `int(timestamp() * 1000)`: truncation toward
    // zero, which Rust integer division also does.
    Ok(epoch_micros / 1000)
}

fn digits_at(text: &str, start: usize, count: usize, whole: &str) -> Result<i64, TimestampError> {
    let bytes = text.as_bytes();
    let mut value = 0i64;
    for offset in 0..count {
        let byte = *bytes
            .get(start + offset)
            .ok_or_else(|| TimestampError::Invalid(whole.to_owned()))?;
        if !byte.is_ascii_digit() {
            return Err(TimestampError::Invalid(whole.to_owned()));
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Ok(value)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_read_and_refuse() {
        assert_eq!(
            timestamp_ms(&Value::String("2026-08-05T00:16:40Z".into())),
            Ok(Some(1_785_889_000_000))
        );
        assert_eq!(timestamp_ms(&Value::Null), Ok(None));
        assert_eq!(
            timestamp_ms(&Value::String("2026-08-05".into())),
            Err(TimestampError::NoTimezone)
        );
        assert!(matches!(
            timestamp_ms(&Value::String("soon".into())),
            Err(TimestampError::Invalid(_))
        ));
        // Seconds are optional, offsets may carry seconds,
        // and an offset over 24 hours refuses.
        assert_eq!(
            timestamp_ms(&Value::String("2026-08-05T00:16Z".into())),
            Ok(Some(1_785_888_960_000))
        );
        assert_eq!(
            timestamp_ms(&Value::String("1970-01-01T01:00:00+01:00".into())),
            Ok(Some(0))
        );
        assert!(matches!(
            timestamp_ms(&Value::String("2026-08-05T00:16:40+24:00".into())),
            Err(TimestampError::Invalid(_))
        ));
        assert!(matches!(
            timestamp_ms(&Value::String("2026-13-01T00:00:00".into())),
            Err(TimestampError::Invalid(_))
        ));
        // Truncation toward zero before the epoch.
        assert_eq!(
            timestamp_ms(&Value::String("1969-12-31T23:59:59.9999Z".into())),
            Ok(Some(0))
        );
    }

    #[test]
    fn records_parse_from_bytes() {
        let data = b"{\"type\":\"user\"}\n{\"partial\"";
        let records = parse_session_records(data).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].value["type"], "user");
        assert!(matches!(
            parse_session_records(b"[1]\n"),
            Err(SessionError::NotAnObject(1))
        ));
        assert!(parse_session_records(b"{}\n").is_ok());
    }
}
