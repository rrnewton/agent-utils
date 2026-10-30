//! Codex's rollout transcript.
//!
//! Codex writes `<home>/sessions/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, `<home>` being
//! `$CODEX_HOME` or `~/.codex`. Each line has a `type` and a `payload`:
//!
//! * `response_item` with payload `type` `message`: a message of the conversation; `role`
//!   `assistant` holds the model's text in `content[]` items of type `output_text`, `role` `user`
//!   a prompt in items of type `input_text`;
//! * `response_item` with payload `type` `function_call` or `custom_tool_call`: a tool call;
//! * `event_msg` with payload `type` `task_complete`: the turn ended.
//!
//! The running process holds its rollout open, which names it even when no thread id is known.
//! A sub-agent's rollout names a parent thread in its first entry and is not the agent's own.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::parse_utc;
use crate::locate::{find_in_dated_tree, newest};
use crate::{Decoder, Event, Hints, Located, Transcript, Via};

/// Codex.
pub struct Codex;

/// The [`Transcript`] of Codex.
pub static CODEX: Codex = Codex;

impl Transcript for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn locate(&self, hints: &Hints) -> Option<Located> {
        let session = hints.session_id();
        let open: Vec<PathBuf> = hints
            .open_files()
            .into_iter()
            .filter(|path| rollout_name(path) && path.to_string_lossy().contains("/sessions/"))
            .collect();
        if let Some(session) = session {
            let suffix = format!("-{session}.jsonl");
            if let Some(path) = open
                .iter()
                .find(|path| path.to_string_lossy().ends_with(&suffix))
            {
                return Some(Located {
                    path: path.clone(),
                    evidence: "the process holds the rollout of the thread given open".into(),
                });
            }
        }
        let main: Vec<PathBuf> = open
            .iter()
            .filter(|path| main_thread(path))
            .cloned()
            .collect();
        if let Some(path) = newest(if main.is_empty() { open } else { main }) {
            return Some(Located {
                path,
                evidence: "the process holds it open".into(),
            });
        }
        let session = session?;
        let sessions = hints
            .environment("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| hints.home_dir().map(|home| home.join(".codex")))?
            .join("sessions");
        let suffix = format!("-{session}.jsonl");
        let path = find_in_dated_tree(&sessions, 60, &|entry| {
            (rollout_name(entry) && entry.to_string_lossy().ends_with(&suffix))
                .then(|| entry.to_owned())
        })?;
        Some(Located {
            path,
            evidence: "the thread id given".into(),
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(CodexDecoder)
    }
}

fn rollout_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

/// Whether the rollout's first entry names no parent thread.
fn main_thread(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    if BufReader::new(file).read_line(&mut first).is_err() {
        return false;
    }
    let Ok(entry) = serde_json::from_str::<Value>(&first) else {
        return true;
    };
    entry
        .pointer("/payload/source/subagent")
        .or_else(|| entry.pointer("/payload/parent_thread_id"))
        .is_none_or(Value::is_null)
}

fn item_texts(content: &Value, kind: &str) -> Option<String> {
    let texts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|item| item["type"] == kind)
        .filter_map(|item| item["text"].as_str())
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

struct CodexDecoder;

impl Decoder for CodexDecoder {
    fn feed(&mut self, line: &[u8], out: &mut Vec<Event>) {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let at = entry["timestamp"].as_str().and_then(parse_utc);
        let payload = &entry["payload"];
        match (
            entry["type"].as_str().unwrap_or_default(),
            payload["type"].as_str().unwrap_or_default(),
        ) {
            ("response_item", "message") => match payload["role"].as_str() {
                Some("assistant") => {
                    if let Some(text) = item_texts(&payload["content"], "output_text") {
                        out.push(Event::Text { at, text });
                    }
                }
                Some("user") => {
                    if let Some(text) = item_texts(&payload["content"], "input_text") {
                        out.push(Event::Prompt {
                            at,
                            text,
                            via: Via::Typed,
                        });
                    }
                }
                _ => {}
            },
            ("response_item", "function_call" | "custom_tool_call") => out.push(Event::ToolCall {
                at,
                id: payload["call_id"].as_str().unwrap_or_default().to_owned(),
                name: payload["name"].as_str().unwrap_or("tool").to_owned(),
            }),
            ("event_msg", "task_complete") => out.push(Event::TurnEnd { at }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fake::{process, scratch};

    #[test]
    fn rollout_lines_decode_to_events() {
        let lines = [
            r#"{"timestamp":"2026-10-30T12:00:00Z","type":"session_meta","payload":{"id":"t"}}"#,
            r#"{"timestamp":"2026-10-30T12:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}"#,
            r#"{"timestamp":"2026-10-30T12:00:02Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
            r#"{"timestamp":"2026-10-30T12:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec","call_id":"c1"}}"#,
            r#"{"timestamp":"2026-10-30T12:00:04Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"| a | b |"}]}}"#,
            r#"{"timestamp":"2026-10-30T12:00:05Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
        ];
        let events = crate::decode_all(&CODEX, lines.join("\n").as_bytes());
        let base = parse_utc("2026-10-30T12:00:00Z").unwrap();
        assert_eq!(
            events,
            [
                Event::Prompt {
                    at: Some(base + 1.0),
                    text: "go".into(),
                    via: Via::Typed
                },
                Event::ToolCall {
                    at: Some(base + 3.0),
                    id: "c1".into(),
                    name: "exec".into()
                },
                Event::Text {
                    at: Some(base + 4.0),
                    text: "| a | b |".into()
                },
                Event::TurnEnd {
                    at: Some(base + 5.0)
                },
            ]
        );
    }

    #[test]
    fn the_open_main_rollout_is_preferred_to_a_sub_agents() {
        let root = scratch("codex-locate");
        let day = root.join("home/.codex/sessions/2026/10/30");
        fs::create_dir_all(&day).unwrap();
        let main = day.join("rollout-2026-10-30T01-00-00-main.jsonl");
        let child = day.join("rollout-2026-10-30T01-00-01-child.jsonl");
        fs::write(&main, "{\"type\":\"session_meta\",\"payload\":{}}\n").unwrap();
        fs::write(
            &child,
            "{\"type\":\"session_meta\",\"payload\":{\"parent_thread_id\":\"main\"}}\n",
        )
        .unwrap();
        let proc_root = root.join("proc");
        process(&proc_root, 50, 1, 1, &[], &[main.clone(), child.clone()]);
        let hints = Hints {
            pid: Some(50),
            proc_root: proc_root.clone(),
            ..Hints::default()
        };
        assert_eq!(CODEX.locate(&hints).unwrap().path, main);
        let hints = Hints {
            session: Some("child".into()),
            ..hints
        };
        assert_eq!(CODEX.locate(&hints).unwrap().path, child);
        // With no process, the thread id finds the rollout in the dated tree.
        let hints = Hints {
            pid: None,
            home: Some(root.join("home")),
            session: Some("main".into()),
            proc_root,
            ..Hints::default()
        };
        assert_eq!(CODEX.locate(&hints).unwrap().path, main);
        fs::remove_dir_all(root).ok();
    }
}
