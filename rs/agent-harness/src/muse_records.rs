//! Muse session-journal records at the format level.
//!
//! The event decoder (`muse.rs`) reduces a journal to the
//! shared event model, unwrapping transaction frames. An
//! archivist reads the journal the way its Python oracle
//! does: line by line, each line one JSON value, and only
//! object lines are records. Frames are bookkeeping lines
//! whose `payload_type` is absent, so they drop out of the
//! record stream without unwrapping.

use serde_json::Value;

use crate::rollout::complete_prefix;

/// One journal record line.
#[derive(Clone, Debug)]
pub struct JournalRecord {
    /// 1-based line number.
    pub line: i64,
    /// The parsed JSON object.
    pub value: Value,
}

/// Why a journal's records do not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum JournalError {
    /// A line is not valid JSON (or not UTF-8 JSON text).
    InvalidJson(i64),
}

/// Parse the records of *data* (callers pass the
/// newline-complete prefix when that is the unit). Blank
/// lines and non-object lines are not records.
pub fn parse_journal_records(data: &[u8]) -> Result<Vec<JournalRecord>, JournalError> {
    let mut records = Vec::new();
    for (index, raw) in split_lines(data).into_iter().enumerate() {
        let line = index as i64 + 1;
        if raw
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        {
            continue;
        }
        let text = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
        let value: Value =
            crate::rollout::parse_lenient(text).ok_or(JournalError::InvalidJson(line))?;
        if value.is_object() {
            records.push(JournalRecord { line, value });
        }
    }
    Ok(records)
}

/// The newline-complete prefix of a journal file.
#[must_use]
pub fn complete_journal(data: &[u8]) -> &[u8] {
    complete_prefix(data)
}

/// A record's `recorded_at` (microseconds) in whole
/// milliseconds: only a positive integer reads.
#[must_use]
pub fn recorded_at_ms(record: &Value) -> Option<i64> {
    match record.get("recorded_at") {
        Some(Value::Number(number)) if !number.is_f64() => {
            let held = number
                .as_i64()
                .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))?;
            if held > 0 {
                Some(held / 1000)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The lines of *data*, split the way
/// `parse_journal_records` splits them.
pub fn journal_lines(data: &[u8]) -> Vec<&[u8]> {
    split_lines(data)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_skip_blanks_and_non_objects() {
        let data = b"\n{\"recorded_at\":1500,\"payload_type\":\"x\"}\n[1]\n{\"a\":1}";
        let records = parse_journal_records(data).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].line, 2);
        assert_eq!(recorded_at_ms(&records[0].value), Some(1));
        assert_eq!(recorded_at_ms(&records[1].value), None);
    }

    #[test]
    fn malformed_records_refuse_by_line() {
        assert!(matches!(
            parse_journal_records(b"{\"a\":1}\n{bad\n"),
            Err(JournalError::InvalidJson(2))
        ));
    }
}
