//! Claude Code's session transcript.
//!
//! Claude Code writes `<config>/projects/<project>/<session>.jsonl`, where `<config>` is
//! `$CLAUDE_CONFIG_DIR` or `~/.claude` and `<project>` is the working directory with every
//! character other than an ASCII letter or digit replaced by `-`. Each line is one entry:
//!
//! * `user`: a prompt (`message.content` a string, or text blocks) or tool results;
//! * `assistant`: one content block of the model's message: `text`, `thinking` or `tool_use`;
//! * `attachment` with `attachment.type` `queued_command`: a prompt that arrived while a turn
//!   ran (`attachment.prompt`);
//! * `system` with `subtype` `turn_duration`: the turn ended.
//!
//! Entries with `isSidechain` belong to a sub-agent and are skipped. Every other entry is
//! bookkeeping.
//!
//! The running process names its session in `<config>/sessions/<pid>.json` (`sessionId`, with
//! `procStart`, the process's start time in clock ticks, to tell a reused process id apart). The
//! command that starts Claude Code may be a wrapper, so the process and its descendants are
//! searched.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::parse_utc;
use crate::{Decoder, Event, Hints, Located, Transcript, Via};

/// Claude Code.
pub struct Claude;

/// The [`Transcript`] of Claude Code.
pub static CLAUDE: Claude = Claude;

impl Transcript for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn locate(&self, hints: &Hints) -> Option<Located> {
        let config = hints
            .environment("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| hints.home_dir().map(|home| home.join(".claude")))?;
        let (session, evidence, cwd) = match hints.session_id() {
            Some(session) => (session.to_owned(), "the session id given".to_owned(), None),
            None => running_session(hints, &config)?,
        };
        let projects = config.join("projects");
        let cwd = cwd.or_else(|| hints.cwd.clone());
        if let Some(cwd) = cwd {
            let path = projects
                .join(project_name(&cwd))
                .join(format!("{session}.jsonl"));
            if path.is_file() {
                return Some(Located { path, evidence });
            }
        }
        let path = fs::read_dir(&projects)
            .ok()?
            .flatten()
            .map(|project| project.path().join(format!("{session}.jsonl")))
            .find(|path| path.is_file())?;
        Some(Located { path, evidence })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(ClaudeDecoder)
    }
}

/// The directory name Claude Code gives the project at `cwd`.
#[must_use]
pub fn project_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The session the harness process (or a descendant) names in `<config>/sessions/<pid>.json`,
/// with the evidence and the working directory the record gives.
fn running_session(hints: &Hints, config: &Path) -> Option<(String, String, Option<PathBuf>)> {
    let root = hints.proc_root();
    for pid in crate::proc::tree(root, hints.pid?) {
        let Ok(text) = fs::read_to_string(config.join("sessions").join(format!("{pid}.json")))
        else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(session) = record["sessionId"]
            .as_str()
            .filter(|value| !value.is_empty() && !value.contains(['/', '\0']))
        else {
            continue;
        };
        // A record left by an earlier process that had this id names another session.
        let started = record["procStart"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| record["procStart"].as_u64());
        if let (Some(started), Some(actual)) = (started, crate::proc::start_ticks(root, pid)) {
            if started != actual {
                continue;
            }
        }
        let cwd = record["cwd"].as_str().map(PathBuf::from);
        return Some((
            session.to_owned(),
            format!("process {pid} names it in sessions/{pid}.json"),
            cwd,
        ));
    }
    None
}

fn text_of(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

struct ClaudeDecoder;

impl Decoder for ClaudeDecoder {
    fn feed(&mut self, line: &[u8], out: &mut Vec<Event>) {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if entry["isSidechain"] == true {
            return;
        }
        let at = entry["timestamp"].as_str().and_then(parse_utc);
        match entry["type"].as_str().unwrap_or_default() {
            "assistant" => {
                for block in entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"].as_str() {
                                out.push(Event::Text {
                                    at,
                                    text: text.to_owned(),
                                });
                            }
                        }
                        Some("tool_use") => out.push(Event::ToolCall {
                            at,
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: block["name"].as_str().unwrap_or("tool").to_owned(),
                        }),
                        _ => {}
                    }
                }
            }
            "user" => {
                let content = &entry["message"]["content"];
                let results = content.as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| block["type"] == "tool_result")
                });
                if !results {
                    if let Some(text) = text_of(content) {
                        out.push(Event::Prompt {
                            at,
                            text,
                            via: Via::Typed,
                        });
                    }
                }
            }
            "attachment" if entry["attachment"]["type"] == "queued_command" => {
                if let Some(text) = text_of(&entry["attachment"]["prompt"]) {
                    out.push(Event::Prompt {
                        at,
                        text,
                        via: Via::Queued,
                    });
                }
            }
            "system" if entry["subtype"] == "turn_duration" => out.push(Event::TurnEnd { at }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fake::{process, scratch};

    #[test]
    fn entries_decode_to_prompts_text_calls_and_turn_ends() {
        let lines = [
            r#"{"type":"user","timestamp":"2026-10-30T12:00:00Z","message":{"role":"user","content":"hello"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-30T12:00:01Z","message":{"content":[{"type":"thinking","thinking":"hm"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-30T12:00:02Z","message":{"content":[{"type":"text","text":"**Hi**"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-30T12:00:03Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-30T12:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-30T12:00:05Z","attachment":{"type":"queued_command","prompt":"later"}}"#,
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-30T12:00:06Z","message":{"content":[{"type":"text","text":"sub-agent"}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-30T12:00:07Z"}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
            "not json",
        ];
        let events = crate::decode_all(&CLAUDE, lines.join("\n").as_bytes());
        let base = parse_utc("2026-10-30T12:00:00Z").unwrap();
        assert_eq!(
            events,
            [
                Event::Prompt {
                    at: Some(base),
                    text: "hello".into(),
                    via: Via::Typed
                },
                Event::Text {
                    at: Some(base + 2.0),
                    text: "**Hi**".into()
                },
                Event::ToolCall {
                    at: Some(base + 3.0),
                    id: "t1".into(),
                    name: "Bash".into()
                },
                Event::Prompt {
                    at: Some(base + 5.0),
                    text: "later".into(),
                    via: Via::Queued
                },
                Event::TurnEnd {
                    at: Some(base + 7.0)
                },
                Event::Prompt {
                    at: None,
                    text: "a\nb".into(),
                    via: Via::Typed
                },
            ]
        );
    }

    #[test]
    fn the_running_process_names_its_session_and_a_stale_record_does_not() {
        let root = scratch("claude-locate");
        let proc_root = root.join("proc");
        let config = root.join("config");
        let cwd = PathBuf::from("/work/my.repo");
        let project = config.join("projects").join(project_name(&cwd));
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(config.join("sessions")).unwrap();
        fs::write(project.join("s-live.jsonl"), "").unwrap();
        // A wrapper (100) starts the harness (101), which writes the session record.
        process(
            &proc_root,
            100,
            1,
            7,
            &[("CLAUDE_CONFIG_DIR", config.to_str().unwrap())],
            &[],
        );
        process(&proc_root, 101, 100, 8, &[], &[]);
        fs::write(
            config.join("sessions/101.json"),
            r#"{"pid":101,"sessionId":"s-live","cwd":"/work/my.repo","procStart":"8"}"#,
        )
        .unwrap();
        let hints = Hints {
            pid: Some(100),
            proc_root: proc_root.clone(),
            ..Hints::default()
        };
        let found = CLAUDE.locate(&hints).unwrap();
        assert_eq!(found.path, project.join("s-live.jsonl"));
        assert!(found.evidence.contains("101"), "{}", found.evidence);
        assert_eq!(project_name(&cwd), "-work-my-repo");

        // A record whose start time is another process's is not believed.
        fs::write(
            config.join("sessions/101.json"),
            r#"{"pid":101,"sessionId":"s-live","procStart":"9"}"#,
        )
        .unwrap();
        assert_eq!(CLAUDE.locate(&hints), None);

        // A session id given directly is found in any project.
        let hints = Hints {
            session: Some("s-live".into()),
            home: Some(root.clone()),
            ..hints
        };
        assert_eq!(
            CLAUDE.locate(&hints).unwrap().path,
            project.join("s-live.jsonl")
        );
        fs::remove_dir_all(root).ok();
    }
}
