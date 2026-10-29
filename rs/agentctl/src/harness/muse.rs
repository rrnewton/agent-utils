//! Muse: launch arguments, screen phrases, and its session journal.
//!
//! Muse's ruled editor is read by [`crate::client::muse_editor`] and its forms by
//! [`crate::harness_tui::muse`]; this module names them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::{FollowupPolicy, HarnessAdapter, ScreenMarkers};

/// Muse.
pub struct Muse;

static MARKERS: ScreenMarkers = ScreenMarkers {
    // Muse draws its queued inputs as a block (`◆ Queued input`), read by `turn_state`.
    queue_hints: &[],
    // Its approval and workspace-trust prompts.
    approval: &[
        "approval required",
        "requires approval",
        "approve this",
        "trust this workspace",
        "workspace trust",
    ],
    reply_bullets: &['◆'],
    user_turn_glyphs: &['❯'],
    tool_output_glyphs: &['└'],
    // `◆`/`◇`/`◈` open its running row (`◇ Calling tools (30s · esc to interrupt)`), which
    // animates through all three.
    running_glyphs: &['◆', '◇', '◈'],
    // Attribution not recorded where the placeholder was added; kept with the editor it most
    // resembles.
    composer_placeholders: &["Type your message"],
};

impl HarnessAdapter for Muse {
    fn name(&self) -> &'static str {
        "muse"
    }

    fn display_name(&self) -> &'static str {
        "Muse"
    }

    fn markers(&self) -> &'static ScreenMarkers {
        &MARKERS
    }

    fn structured_launch(&self) -> bool {
        true
    }

    fn launch_arguments(
        &self,
        model: Option<&str>,
        resume: Option<&str>,
    ) -> (Vec<String>, Vec<String>) {
        let mut before = Vec::new();
        if let Some(model) = model.filter(|value| !value.is_empty()) {
            before.extend(["--model".to_owned(), model.to_owned()]);
        }
        // `resume ID` is a subcommand that must come after every option.
        let mut after = Vec::new();
        if let Some(resume) = resume.filter(|value| !value.is_empty()) {
            after.extend(["resume".to_owned(), resume.to_owned()]);
        }
        (before, after)
    }

    fn reasoning_arguments(&self, effort: &str) -> Option<Vec<String>> {
        Some(vec!["--reasoning-effort".to_owned(), effort.to_owned()])
    }

    fn split_conversation(&self, argv: &[String]) -> Option<super::Conversations> {
        let mut values = Vec::new();
        let mut rest = argv;
        if rest.len() >= 2 && rest[rest.len() - 2] == "resume" {
            values.push(rest[rest.len() - 1].clone());
            rest = &rest[..rest.len() - 2];
        }
        if rest.iter().any(|item| item == "resume") {
            return Some(Err(
                "recorded Muse arguments name resume in an unsupported position".to_owned(),
            ));
        }
        Some(Ok((values, rest.to_vec())))
    }

    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::WhenVerified
    }

    fn followup_tui(&self) -> Option<&'static dyn crate::harness_tui::HarnessTui> {
        crate::harness_tui::followup_tui(Some("muse"))
    }

    fn transcript_path(&self, hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
        transcript_path(hints)
    }

    fn read_transcript(&self, lines: &[&[u8]], activity: &mut super::transcript::Activity) {
        read_transcript(lines, activity);
    }

    /// The sub-agent sessions the runtime holds locked.
    fn related_transcripts(
        &self,
        hints: &super::transcript::Hints<'_>,
        _main: &std::path::Path,
    ) -> Vec<PathBuf> {
        hints
            .open_files()
            .into_iter()
            .filter(|path| {
                let text = path.to_string_lossy();
                path.file_name().is_some_and(|name| name == ".session.lock")
                    && text.contains("/muse/sessions/")
                    && text.contains("/subagent/")
            })
            .filter_map(|lock| Some(lock.parent()?.join("session.jsonl")))
            .collect()
    }
}

/// Muse's journal for `hints`: `<sessions>/YYYY/MM/DD/<session>/session.jsonl`, `<sessions>`
/// being `$XDG_DATA_HOME/muse/sessions` (default `~/.local/share/muse/sessions`). Muse's runtime
/// process holds its session's `.session.lock` open for as long as it runs, so the lock it holds
/// names the session even when the record has no session id (Muse reports none to Herdr); a
/// sub-agent's session, under `subagent/`, is not the agent's own. With no process to ask, the
/// session id names the directory.
fn transcript_path(hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
    let held: Vec<PathBuf> = hints
        .open_files()
        .into_iter()
        .filter(|path| {
            let text = path.to_string_lossy();
            path.file_name().is_some_and(|name| name == ".session.lock")
                && text.contains("/muse/sessions/")
                && !text.contains("/subagent/")
        })
        .filter_map(|lock| Some(lock.parent()?.join("session.jsonl")))
        .filter(|path| path.is_file())
        .collect();
    if let Some(session) = hints.session {
        if let Some(path) = held.iter().find(|path| {
            path.parent()
                .and_then(|dir| dir.file_name())
                .is_some_and(|name| name == session)
        }) {
            return Some(path.clone());
        }
    }
    if let Some(path) = super::transcript::newest(held) {
        return Some(path);
    }
    let session = hints
        .session
        .filter(|value| !value.is_empty() && !value.contains('/'))?;
    let data = hints
        .environment("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| hints.home.map(|home| home.join(".local/share")))?;
    super::transcript::find_in_dated_tree(&data.join("muse/sessions"), 60, &|entry| {
        let journal = entry.join("session.jsonl");
        (entry.file_name().is_some_and(|name| name == session) && journal.is_file())
            .then_some(journal)
    })
}

/// Every record a journal line holds: the line itself when it is a record, and the records its
/// transaction frame wraps (as JSON text under `children[].record_json`).
fn records(line: &[u8]) -> Vec<serde_json::Value> {
    let Ok(frame) = serde_json::from_slice::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(children) = frame["children"].as_array() {
        for child in children {
            if let Some(record) = child["record_json"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
            {
                out.push(record);
            }
        }
    }
    if frame.get("payload_type").is_some() {
        out.push(frame);
    }
    out
}

/// Read Muse's journal records (`recorded_at` in microseconds). Run events say what the model
/// does: a provider request configured is a request sent; a response created, reasoning, an
/// assistant message, and committed tool calls are output. `tool_batch.effect.started` and
/// `.terminal` bracket each tool call; a call whose task was linked as a background stream does not
/// hold the turn. A user intent accepted, or a top-level turn started, opens a turn.
fn read_transcript(lines: &[&[u8]], activity: &mut super::transcript::Activity) {
    use super::transcript::{ToolCall, ToolLimit};
    use serde_json::Value;
    let mut calls: BTreeMap<String, (String, Value)> = BTreeMap::new();
    let mut background: BTreeSet<String> = BTreeSet::new();
    let mut open: Vec<(ToolCall, String)> = Vec::new();
    for line in lines {
        for record in records(line) {
            let Some(at) = record["recorded_at"].as_f64().map(|micros| micros / 1e6) else {
                continue;
            };
            let payload_type = record["payload_type"].as_str().unwrap_or_default();
            let payload = &record["payload"];
            match payload_type {
                "runtime.session" if payload["kind"] == "run" => {
                    let event = &payload["event"];
                    let kind = event["kind"].as_str().unwrap_or_default();
                    match kind {
                        "provider_request_options_configured" | "model_request_configured" => {
                            activity.request(at);
                        }
                        "model_response_created" | "model_completed" => activity.output(at, kind),
                        "reasoning_summary_delta"
                        | "reasoning_committed"
                        | "reasoning_summary_committed" => activity.output(at, "reasoning"),
                        "assistant_message_committed" => activity.output(at, "assistant text"),
                        "assistant_tool_calls_committed" => {
                            for call in event["tool_calls"].as_array().into_iter().flatten() {
                                let name = call["name"].as_str().unwrap_or("tool").to_owned();
                                let arguments = call["args"]
                                    .as_str()
                                    .and_then(|text| serde_json::from_str(text).ok())
                                    .unwrap_or(Value::Null);
                                for key in ["call_id", "id"] {
                                    if let Some(id) = call[key].as_str() {
                                        calls.insert(
                                            id.to_owned(),
                                            (name.clone(), arguments.clone()),
                                        );
                                    }
                                }
                            }
                            activity.output(at, "tool call");
                        }
                        "task_stream_linked" if event["execution_mode"] == "background" => {
                            if let Some(task) = event["task_id"].as_str() {
                                background.insert(task.to_owned());
                            }
                            activity.bookkeeping(at, kind);
                        }
                        "top_level_turn_started" => activity.prompt(at),
                        _ => activity.bookkeeping(at, if kind.is_empty() { "run" } else { kind }),
                    }
                }
                "runtime.user_intent.accepted" => activity.prompt(at),
                "tool_batch.effect.started" => {
                    let effect = &payload["record"];
                    let id = effect["call_id"].as_str().unwrap_or_default().to_owned();
                    let task = effect["task_id"].as_str().unwrap_or_default().to_owned();
                    let (name, arguments) = calls.get(&id).cloned().unwrap_or_else(|| {
                        (
                            effect["tool_name"].as_str().unwrap_or("tool").to_owned(),
                            Value::Null,
                        )
                    });
                    let limit = ["timeout_ms", "yield_time_ms"]
                        .iter()
                        .filter_map(|key| arguments[*key].as_f64())
                        .fold(None, |most: Option<f64>, ms| {
                            Some(most.map_or(ms, |most| most.max(ms)))
                        })
                        .map_or(ToolLimit::None, |ms| ToolLimit::Explicit(ms / 1000.0));
                    let command = arguments["command"]
                        .as_str()
                        .or_else(|| arguments["description"].as_str())
                        .or_else(|| arguments["path"].as_str())
                        .or_else(|| arguments["objective"].as_str())
                        .map(str::to_owned);
                    open.push((
                        ToolCall {
                            id,
                            name,
                            command,
                            started_at: at,
                            limit,
                        },
                        task,
                    ));
                    activity.event(at, "tool started");
                }
                "tool_batch.effect.terminal" => {
                    let id = payload["record"]["call_id"].as_str().unwrap_or_default();
                    open.retain(|(call, _)| call.id != id);
                    activity.event(at, "tool result");
                    if open.is_empty() {
                        activity.request(at);
                    }
                }
                _ => activity.bookkeeping(at, payload_type),
            }
        }
    }
    activity.set_open_tools(
        open.into_iter()
            .map(|(mut call, task)| {
                if background.contains(&task) {
                    call.limit = ToolLimit::Background;
                }
                call
            })
            .collect(),
    );
}

/// Background shells Muse lists as running in its terminals panel, drawn between the editor's
/// bottom rule and the footer while any exist: `main · ↓ to select`, then one row per terminal,
/// `├ ◈ Sleep 20 between steps  running   19s`, and `ran 18s, exit 0` once it finished. A Muse
/// that ended its turn waiting on one, as a goal turn often does between steps, has not stopped
/// without a verdict (#678 muse-bridge-latency). `None` when no terminal runs.
#[must_use]
pub fn background_work(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let bottom = lines.iter().rposition(|line| {
        let line = line.trim();
        line.chars().count() >= 8 && line.chars().all(|c| matches!(c, '─' | '━' | '═'))
    })?;
    let running = lines[bottom + 1..]
        .iter()
        .skip_while(|line| !line.trim_start().starts_with("main · "))
        .skip(1)
        .take_while(|line| {
            let line = line.trim_start();
            line.starts_with('├') || line.starts_with('└')
        })
        .filter(|line| {
            line.split("  ")
                .map(str::trim)
                .any(|field| field == "running")
        })
        .count();
    (running > 0).then(|| {
        format!(
            "{running} background shell{}",
            if running == 1 { "" } else { "s" }
        )
    })
}

/// Muse's journal, read forward from a point fixed before a prompt is submitted, for the proof
/// that Muse accepted that prompt (#678 muse-bridge-latency).
///
/// Muse writes `runtime.user_intent.accepted`, with the prompt's exact text, the moment it takes
/// a submitted or steered prompt, while it may draw the user turn only seconds later, after the
/// tool call in flight. A steer Muse refuses (`! steer failed: ...`, the prompt put back in the
/// editor) writes no such record.
#[derive(Debug)]
pub(crate) struct JournalWatch {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
}

/// The most journal bytes one [`JournalWatch::accepted`] call reads; a turn writes far less
/// between two polls.
const JOURNAL_READ_LIMIT: u64 = 16 * 1024 * 1024;

impl JournalWatch {
    /// Start watching the journal of the Muse runtime under process `pid` (the launcher or the
    /// runtime itself) at its current end. `None` when no journal can be found.
    pub(crate) fn start(pid: u32) -> Option<Self> {
        let home = super::home();
        let path = transcript_path(&super::transcript::Hints {
            session: None,
            cwd: None,
            home: home.as_deref(),
            pid: Some(pid),
            proc: Some(&crate::work_state::LinuxProc),
        })?;
        Self::at_end(path)
    }

    /// Start watching `path` at its current end.
    pub(crate) fn at_end(path: PathBuf) -> Option<Self> {
        let offset = std::fs::metadata(&path).ok()?.len();
        Some(Self {
            path,
            offset,
            carry: Vec::new(),
        })
    }

    /// Whether the journal, since the watch started, records `prompt` as a user intent Muse
    /// accepted. Text is compared with runs of whitespace collapsed, as the screen proofs do.
    pub(crate) fn accepted(&mut self, prompt: &str) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return false;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut chunk = Vec::new();
        let Ok(read) = file.take(JOURNAL_READ_LIMIT).read_to_end(&mut chunk) else {
            return false;
        };
        self.offset += read as u64;
        self.carry.extend_from_slice(&chunk);
        let Some(end) = self.carry.iter().rposition(|byte| *byte == b'\n') else {
            return false;
        };
        let complete: Vec<u8> = self.carry.drain(..=end).collect();
        let wanted = collapse(prompt);
        complete
            .split(|byte| *byte == b'\n')
            .filter(|line| {
                line.windows(ACCEPTED.len())
                    .any(|window| window == ACCEPTED.as_bytes())
            })
            .flat_map(records)
            .any(|record| {
                record["payload_type"] == ACCEPTED && accepted_text(&record) == Some(wanted.clone())
            })
    }
}

/// The journal record of a user intent Muse accepted.
const ACCEPTED: &str = "runtime.user_intent.accepted";

/// The text of an accepted user intent, its text blocks joined, whitespace collapsed.
fn accepted_text(record: &serde_json::Value) -> Option<String> {
    let messages = record["payload"]["model_messages"].as_array()?;
    let text: Vec<&str> = messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["kind"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| collapse(&text.join("\n")))
}

/// Words separated by single spaces.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::transcript::{Activity, Hints, ToolLimit};
    use super::*;

    const FIXTURE: &str = include_str!("../../testdata/transcripts/muse.jsonl");
    /// 2026-10-27T10:00:00Z, the fixture's first entry.
    const T0: f64 = 1_793_095_200.0;

    fn read_text(text: &str, lines: usize) -> Activity {
        let all: Vec<&[u8]> = text.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all[..lines.min(all.len())], &mut activity);
        activity
    }

    #[test]
    fn framed_records_bracket_each_tool_call_and_name_its_command() {
        let tool = read_text(FIXTURE, 5).tool.expect("first bash in flight");
        assert_eq!(tool.limit, ToolLimit::Explicit(30.0));
        assert_eq!(tool.command.as_deref(), Some("cargo test -p agentctl"));
        let activity = read_text(FIXTURE, 6);
        assert!(activity.tool.is_none());
        assert_eq!(activity.request_since, Some(T0 + 40.0));
        let activity = read_text(FIXTURE, usize::MAX);
        let tool = activity.tool.clone().expect("second bash in flight");
        assert_eq!(tool.describe(), "bash `python3 scripts/long_check.py`");
        assert!(tool.untimed());
        assert_eq!(tool.started_at, T0 + 45.0);
        assert_eq!(activity.last_output_at, Some(T0 + 44.0));
        assert_eq!(
            activity.last_event.as_deref(),
            Some("resource_usage_sampled")
        );
        assert_eq!(activity.last_event_at, Some(T0 + 600.0));
        assert_eq!(activity.prompt_at, Some(T0));
    }

    #[test]
    fn a_call_on_a_background_stream_does_not_hold_the_turn() {
        let mut text = FIXTURE.to_owned();
        let linked = serde_json::json!({
            "recorded_at": (T0 as u64 + 46) * 1_000_000,
            "payload_type": "runtime.session",
            "payload": {"kind": "run", "event": {
                "kind": "task_stream_linked", "execution_mode": "background", "task_id": "task_2"
            }},
        });
        text.push_str(&format!("{linked}\n"));
        let activity = read_text(&text, usize::MAX);
        assert!(activity.tool.is_none());
        assert_eq!(activity.open_tools, 1);
    }

    #[test]
    fn the_session_the_runtime_holds_locked_is_the_agents_own() {
        let home = crate::turn_state::tests::scratch();
        let root = home.join(".local/share/muse/sessions/2026/10/27");
        let own = root.join("session-own");
        let child = own.join("subagent/session-child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(own.join("session.jsonl"), FIXTURE).unwrap();
        std::fs::write(child.join("session.jsonl"), FIXTURE).unwrap();
        let mut proc = crate::work_state::fake::FakeProc::default();
        proc.spawn(200, 1, 'S', "muse")
            .spawn(201, 200, 'S', "muse.real")
            .open(
                201,
                15,
                &format!("{} (deleted)", own.join(".session.lock").display()),
                None,
            )
            .open(
                201,
                16,
                &child.join(".session.lock").display().to_string(),
                None,
            );
        let hints = Hints {
            session: None,
            cwd: None,
            home: Some(&home),
            pid: Some(200),
            proc: Some(&proc),
        };
        assert_eq!(transcript_path(&hints), Some(own.join("session.jsonl")));
        let hints = Hints {
            session: Some("session-own"),
            pid: None,
            proc: None,
            ..hints
        };
        assert_eq!(transcript_path(&hints), Some(own.join("session.jsonl")));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn the_journal_watch_sees_only_an_accepted_intent_with_the_prompt_written_after_it_started() {
        let dir =
            std::env::temp_dir().join(format!("agentctl-muse-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let accepted = |text: &str| {
            serde_json::json!({
                "payload_type": "runtime.user_intent.accepted",
                "payload": {"model_messages": [{"content": [{"kind": "text", "text": text}]}]},
            })
            .to_string()
        };
        let prompt = "hold: stop after the current step\nand report";
        // Accepted before the watch started: not this submission.
        std::fs::write(&path, format!("{}\n", accepted(prompt))).unwrap();
        let mut watch = JournalWatch::at_end(path.clone()).unwrap();
        assert!(!watch.accepted(prompt));
        let append = |line: String| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(line.as_bytes()).unwrap();
        };
        // Another prompt, and a steer queued (not an accepted intent) with this one's text.
        append(format!("{}\n", accepted("something else")));
        append(format!(
            "{}\n",
            serde_json::json!({"payload_type": "runtime.session", "payload": {"event": {"kind": "inbox_item_queued", "body": prompt}}})
        ));
        assert!(!watch.accepted(prompt));
        // Half a line is kept until its newline arrives; a transaction frame's children count,
        // and whitespace is compared collapsed.
        let frame = serde_json::json!({"children": [{"record_json": accepted("hold:  stop after the current step and report")}]}).to_string();
        let (head, tail) = frame.split_at(frame.len() / 2);
        append(head.to_owned());
        assert!(!watch.accepted(prompt));
        append(format!("{tail}\n"));
        assert!(watch.accepted(prompt));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn running_terminals_in_the_panel_are_background_work() {
        let rule = "─".repeat(40);
        let footer = "  watermelon-full-preview · xhigh · ~/work/project · Launch overrides";
        let screen = |rows: &str| {
            format!("◆ Finished · Sleep · ✓ · ctrl+o\n{rule}\n❯\n{rule}\n{rows}{footer}\n")
        };
        assert_eq!(
            background_work(&screen(
                "main · ↓ to select\n├ ◈ Sleep ninety seconds  running      1m 21s\n└ ◆ Sleep sixty seconds  running   18s\n"
            ))
            .as_deref(),
            Some("2 background shells")
        );
        assert_eq!(
            background_work(&screen(
                "main · ↓ to select\n└ ◆ Sleep between steps  ran 18s, exit 0      20s\n"
            )),
            None
        );
        assert_eq!(background_work(&screen("")), None);
        // A transcript row that says running is not the panel.
        assert_eq!(
            background_work(&format!(
                "└ ◆ Sleep  running   3s\n{rule}\n❯\n{rule}\n{footer}\n"
            )),
            None
        );
    }
}
