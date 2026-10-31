//! Claude Code's prompt history file (`~/.claude/history.jsonl`).
//!
//! Claude Code keeps two records of a session. The transcript is the
//! complete one, and it is also the one Claude Code deletes after its
//! cleanup window. The history file is the other record: one JSON
//! object per prompt the user submitted, carrying the verbatim text,
//! any pasted attachments, a millisecond timestamp, the working
//! directory it was typed in, and the session it belonged to. It is
//! never cleaned up.
//!
//! This module is the format fact: how one history line reads. What
//! a caller selects from the history (a project, a set of covered
//! sessions) and what it builds from the prompts is the caller's
//! policy, not this crate's.

use serde_json::Value;

/// One prompt-history record: the fields one history line carries.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPrompt {
    /// The prompt text as submitted, with text attachments appended
    /// (see [`HistoryPrompt::text`]).
    pub display: String,
    /// When the prompt was submitted, in milliseconds since the
    /// epoch, as recorded.
    pub timestamp_ms: i64,
    /// The working directory the prompt was typed in.
    pub project: String,
    /// The session the prompt belonged to, when the line names one.
    /// The oldest lines carry none.
    pub session_id: Option<String>,
    /// The rendered text attachments, in attachment order.
    pub pasted: Vec<(String, String)>,
}

impl HistoryPrompt {
    /// The prompt's full text: the submitted text, then each text
    /// attachment as a `[Pasted text #<key>]` block.
    #[must_use]
    pub fn text(&self) -> String {
        if self.pasted.is_empty() {
            return self.display.clone();
        }
        let blocks: Vec<String> = self
            .pasted
            .iter()
            .map(|(key, content)| format!("[Pasted text #{key}]\n{content}"))
            .collect();
        format!("{}\n\n{}", self.display, blocks.join("\n\n"))
    }
}

/// Why one history line does not read as a prompt record.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryError {
    /// The line is not a JSON object (or not JSON at all).
    NotAnObject,
    /// A named field is missing or has the wrong shape.
    Field(&'static str),
    /// `pastedContents` holds a non-object attachment.
    PastedNotObject(String),
}

fn py_space(c: char) -> bool {
    // CPython `str.isspace` includes the file/group/record/unit
    // separators alongside the Unicode White_Space set.
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

fn py_trim(text: &str) -> &str {
    text.trim_matches(py_space)
}

fn py_trim_end(text: &str) -> &str {
    text.trim_end_matches(py_space)
}

/// Decode one newline-complete history line (without its newline).
///
/// The line must be a JSON object with a non-empty `display` string,
/// a non-negative integer `timestamp`, and a non-empty `project`
/// string. `sessionId` may be absent. `pastedContents`, when an
/// object, contributes its `text` attachments in attachment order:
/// keys order by length and then lexicographically, which is
/// numeric order for the decimal keys the writer emits.
pub fn parse_history_line(line: &[u8]) -> Result<HistoryPrompt, HistoryError> {
    let value: Value = serde_json::from_slice(line).map_err(|_| HistoryError::NotAnObject)?;
    let object = value.as_object().ok_or(HistoryError::NotAnObject)?;
    let display = object
        .get("display")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(HistoryError::Field("display"))?
        .trim()
        .to_owned();
    let timestamp_ms = object
        .get("timestamp")
        .and_then(Value::as_i64)
        .filter(|stamp| *stamp >= 0)
        .ok_or(HistoryError::Field("timestamp"))?;
    let project = object
        .get("project")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(HistoryError::Field("project"))?
        .to_owned();
    let session_id = match object.get("sessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(_) => return Err(HistoryError::Field("sessionId")),
    };
    let mut pasted: Vec<(String, String)> = Vec::new();
    if let Some(contents) = object.get("pastedContents") {
        if let Some(map) = contents.as_object() {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| {
                (a.chars().count(), a.as_str()).cmp(&(b.chars().count(), b.as_str()))
            });
            for key in keys {
                let item = &map[key];
                let item = item
                    .as_object()
                    .ok_or_else(|| HistoryError::PastedNotObject(key.clone()))?;
                if item.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                let Some(content) = item.get("content").and_then(Value::as_str) else {
                    continue;
                };
                if py_trim(content).is_empty() {
                    continue;
                }
                pasted.push((key.clone(), py_trim_end(content).to_owned()));
            }
        }
    }
    Ok(HistoryPrompt {
        display,
        timestamp_ms,
        project,
        session_id,
        pasted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_reads_with_its_attachments_in_order() {
        let line = br#"{"display":"  fix it  ","timestamp":1760000000000,"project":"/work/widget","sessionId":"s-1","pastedContents":{"10":{"type":"text","content":"ten  "},"2":{"type":"text","content":"two"},"3":{"type":"image","content":"x"},"4":{"type":"text","content":"   "}}}"#;
        let prompt = parse_history_line(line).unwrap();
        assert_eq!(prompt.display, "fix it");
        assert_eq!(prompt.timestamp_ms, 1_760_000_000_000);
        assert_eq!(prompt.session_id.as_deref(), Some("s-1"));
        assert_eq!(
            prompt.text(),
            "fix it\n\n[Pasted text #2]\ntwo\n\n[Pasted text #10]\nten"
        );
    }

    #[test]
    fn a_prompt_without_a_session_reads_with_none() {
        let line = br#"{"display":"hi","timestamp":0,"project":"/w"}"#;
        let prompt = parse_history_line(line).unwrap();
        assert_eq!(prompt.session_id, None);
        assert_eq!(prompt.text(), "hi");
    }

    #[test]
    fn malformed_lines_refuse_by_field() {
        assert_eq!(
            parse_history_line(b"{\"display\":1}"),
            Err(HistoryError::Field("display"))
        );
        assert_eq!(parse_history_line(b"[1]"), Err(HistoryError::NotAnObject));
        assert_eq!(
            parse_history_line(br#"{"display":"x","timestamp":-1,"project":"/w"}"#),
            Err(HistoryError::Field("timestamp"))
        );
        assert_eq!(
            parse_history_line(
                br#"{"display":"x","timestamp":1,"project":"/w","pastedContents":{"1":2}}"#
            ),
            Err(HistoryError::PastedNotObject("1".to_string()))
        );
    }
}
