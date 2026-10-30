//! Muse's session journal.
//!
//! Muse writes `<data>/muse/sessions/YYYY/MM/DD/<session>/session.jsonl`, `<data>` being
//! `$XDG_DATA_HOME` or `~/.local/share`. It is an event-sourced journal: each line is a record
//! (with `recorded_at` in microseconds, `payload_type` and `payload`) or a transaction frame whose
//! `children[].record_json` hold records as JSON text. This decoder reads:
//!
//! * `runtime.user_intent.accepted`: a prompt (`payload.model_messages[].content[]` text items);
//! * `runtime.session` run events (`payload.kind` `run`): `assistant_message_committed` (the
//!   model's text, `event.text`), `assistant_tool_calls_committed` (tool calls), `started` and
//!   `terminal` (a turn starting and ending).
//!
//! The runtime holds its session's `.session.lock` open while it runs, so the lock names the
//! session even when no id is known. Sessions under `subagent/` are sub-agents', not the agent's.

use std::path::PathBuf;

use serde_json::Value;

use crate::locate::{find_in_dated_tree, newest};
use crate::{Decoder, Event, Hints, Located, Transcript, Via};

/// Muse.
pub struct Muse;

/// The [`Transcript`] of Muse.
pub static MUSE: Muse = Muse;

impl Transcript for Muse {
    fn name(&self) -> &'static str {
        "muse"
    }

    fn locate(&self, hints: &Hints) -> Option<Located> {
        let journals = |files: Vec<PathBuf>| -> Vec<PathBuf> {
            files
                .into_iter()
                .filter(|path| {
                    let text = path.to_string_lossy();
                    path.file_name().is_some_and(|name| name == ".session.lock")
                        && text.contains("/muse/sessions/")
                        && !text.contains("/subagent/")
                })
                .filter_map(|lock| Some(lock.parent()?.join("session.jsonl")))
                .filter(|path| path.is_file())
                .collect()
        };
        let held = journals(hints.open_files());
        if let Some(session) = hints.session_id() {
            if let Some(path) = held.iter().find(|path| {
                path.parent()
                    .and_then(|dir| dir.file_name())
                    .is_some_and(|name| name == session)
            }) {
                return Some(Located {
                    path: path.clone(),
                    evidence: "the process holds the lock of the session given".into(),
                });
            }
        }
        // The nearest process holding a session lock is the runtime: a descendant's lock can be
        // another Muse session run from its shell.
        for (pid, files) in hints.open_files_by_process() {
            if let Some(path) = newest(journals(files)) {
                return Some(Located {
                    path,
                    evidence: format!("process {pid} holds its session lock"),
                });
            }
        }
        let session = hints.session_id()?.to_owned();
        let data = hints
            .environment("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| hints.home_dir().map(|home| home.join(".local/share")))?;
        let path = find_in_dated_tree(&data.join("muse/sessions"), 60, &|entry| {
            let journal = entry.join("session.jsonl");
            (entry.file_name().is_some_and(|name| *name == *session) && journal.is_file())
                .then_some(journal)
        })?;
        Some(Located {
            path,
            evidence: "the session id given".into(),
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(MuseDecoder { running: false })
    }
}

/// Every record a journal line holds: the records its transaction frame wraps, then the line
/// itself when it is a record.
fn records(line: &[u8]) -> Vec<Value> {
    let Ok(frame) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = frame["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|child| child["record_json"].as_str())
        .filter_map(|text| serde_json::from_str(text).ok())
        .collect();
    if frame.get("payload_type").is_some() {
        out.push(frame);
    }
    out
}

struct MuseDecoder {
    running: bool,
}

impl Decoder for MuseDecoder {
    fn feed(&mut self, line: &[u8], out: &mut Vec<Event>) {
        // Most lines are hook and task bookkeeping; only these can hold an event.
        const NEEDLES: [&[u8]; 4] = [
            b"assistant_",
            b"user_intent.accepted",
            b"erminal",
            b"started",
        ];
        if !NEEDLES
            .iter()
            .any(|needle| line.windows(needle.len()).any(|window| window == *needle))
        {
            return;
        }
        for record in records(line) {
            let at = record["recorded_at"].as_f64().map(|micros| micros / 1e6);
            let payload = &record["payload"];
            match record["payload_type"].as_str().unwrap_or_default() {
                "runtime.user_intent.accepted" => {
                    let texts: Vec<&str> = payload["model_messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|message| message["content"].as_array().into_iter().flatten())
                        .filter(|item| item["kind"] == "text")
                        .filter_map(|item| item["text"].as_str())
                        .collect();
                    if !texts.is_empty() {
                        out.push(Event::Prompt {
                            at,
                            text: texts.join("\n"),
                            via: if self.running {
                                Via::Queued
                            } else {
                                Via::Typed
                            },
                        });
                    }
                }
                "runtime.session" if payload["kind"] == "run" => {
                    let event = &payload["event"];
                    match event["kind"].as_str().unwrap_or_default() {
                        "assistant_message_committed" => {
                            if let Some(text) = event["text"].as_str() {
                                out.push(Event::Text {
                                    at,
                                    text: text.to_owned(),
                                });
                            }
                        }
                        "assistant_tool_calls_committed" => {
                            for call in event["tool_calls"].as_array().into_iter().flatten() {
                                out.push(Event::ToolCall {
                                    at,
                                    id: call["call_id"]
                                        .as_str()
                                        .or_else(|| call["id"].as_str())
                                        .unwrap_or_default()
                                        .to_owned(),
                                    name: call["name"].as_str().unwrap_or("tool").to_owned(),
                                });
                            }
                        }
                        "started" => self.running = true,
                        "terminal" => {
                            self.running = false;
                            out.push(Event::TurnEnd { at });
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fake::{process, scratch};
    use std::fs;

    fn run(kind: &str, extra: &str, micros: u64) -> String {
        format!(
            r#"{{"recorded_at":{micros},"payload_type":"runtime.session","payload":{{"kind":"run","event":{{"kind":"{kind}"{extra}}}}}}}"#
        )
    }

    #[test]
    fn records_and_framed_records_decode_to_events() {
        let intent = |micros: u64, text: &str| {
            format!(
                r#"{{"recorded_at":{micros},"payload_type":"runtime.user_intent.accepted","payload":{{"model_messages":[{{"content":[{{"kind":"text","text":"{text}"}}]}}]}}}}"#
            )
        };
        let framed = serde_json::json!({
            "children": [
                {"record_json": run("assistant_message_committed", r#","text":"Done.""#, 3_000_000)},
                {"record_json": run("assistant_tool_calls_committed", r#","tool_calls":[{"call_id":"c1","name":"bash"}]"#, 3_000_001)},
            ]
        })
        .to_string();
        let lines = [
            intent(1_000_000, "first"),
            run("started", "", 2_000_000),
            intent(2_500_000, "steer"),
            framed,
            run("terminal", "", 4_000_000),
            r#"{"recorded_at":5,"payload_type":"runtime.session","payload":{"kind":"run","event":{"kind":"hook_run_started"}}}"#.to_owned(),
        ];
        let events = crate::decode_all(&MUSE, lines.join("\n").as_bytes());
        assert_eq!(
            events,
            [
                Event::Prompt {
                    at: Some(1.0),
                    text: "first".into(),
                    via: Via::Typed
                },
                Event::Prompt {
                    at: Some(2.5),
                    text: "steer".into(),
                    via: Via::Queued
                },
                Event::Text {
                    at: Some(3.0),
                    text: "Done.".into()
                },
                Event::ToolCall {
                    at: Some(3.000001),
                    id: "c1".into(),
                    name: "bash".into()
                },
                Event::TurnEnd { at: Some(4.0) },
            ]
        );
    }

    #[test]
    fn the_held_lock_names_the_journal_even_when_deleted_and_sub_agents_do_not() {
        let root = scratch("muse-locate");
        let sessions = root.join("home/.local/share/muse/sessions/2026/10/28");
        let own = sessions.join("s-own");
        let child = own.join("subagent/s-child");
        fs::create_dir_all(&child).unwrap();
        fs::write(own.join("session.jsonl"), "").unwrap();
        fs::write(child.join("session.jsonl"), "").unwrap();
        fs::write(child.join(".session.lock"), "").unwrap();
        // The runtime holds a lock file that was deleted and recreated: the kernel names it
        // `.session.lock (deleted)`.
        let gone = own.join(".session.lock");
        fs::write(&gone, "").unwrap();
        let proc_root = root.join("proc");
        process(&proc_root, 70, 1, 1, &[], &[]);
        let deleted = PathBuf::from(format!("{} (deleted)", gone.display()));
        process(
            &proc_root,
            71,
            70,
            2,
            &[],
            &[child.join(".session.lock"), deleted],
        );
        fs::remove_file(&gone).unwrap();
        let hints = Hints {
            pid: Some(70),
            proc_root: proc_root.clone(),
            ..Hints::default()
        };
        assert_eq!(MUSE.locate(&hints).unwrap().path, own.join("session.jsonl"));
        // Another Muse session run from the runtime's shell, newer, belongs to a descendant.
        let other = sessions.join("s-other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("session.jsonl"), "").unwrap();
        fs::write(other.join(".session.lock"), "").unwrap();
        process(&proc_root, 72, 71, 3, &[], &[other.join(".session.lock")]);
        assert_eq!(MUSE.locate(&hints).unwrap().path, own.join("session.jsonl"));
        let hints = Hints {
            pid: None,
            home: Some(root.join("home")),
            session: Some("s-own".into()),
            proc_root,
            ..Hints::default()
        };
        assert_eq!(MUSE.locate(&hints).unwrap().path, own.join("session.jsonl"));
        fs::remove_dir_all(root).ok();
    }
}
