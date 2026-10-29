//! Claude Code: its composer, launch arguments, screen hints, and transcript.

use std::path::PathBuf;

use super::{FollowupPolicy, HarnessAdapter, ScreenMarkers};
use crate::submission::{composer_from, is_labelled_rule, is_rule, join_rows, ComposerView};

/// Claude Code.
pub struct ClaudeCode;

static MARKERS: ScreenMarkers = ScreenMarkers {
    // Drawn in the empty input box while messages wait behind the running turn.
    queue_hints: &["Press up to edit queued messages"],
    // The permission dialog and the folder-trust screen.
    approval: &["do you want to proceed", "yes, i trust this folder"],
    // `⏺` in older builds.
    reply_bullets: &['●', '⏺'],
    // `> text` in older builds.
    user_turn_glyphs: &['❯', '>'],
    tool_output_glyphs: &['⎿'],
    // Spinner frames that open the running-turn row.
    running_glyphs: &['✻', '✽', '✶', '✳', '✢', '*'],
    composer_placeholders: &["Press up to edit queued messages"],
};

/// The hint Claude Code draws in place of the status row under its input box for a while after
/// a paste, so that row's `esc to interrupt` is missing even while a turn runs.
pub const PASTE_HINT: &str = "paste again to expand";

/// The two phrases of Claude Code's folder-trust screen.
const TRUST_QUESTION: &str = "Quick safety check: Is this a project you created or one you trust?";
const TRUST_ACCEPT: &str = "Yes, I trust this folder";
/// The refusal on the same screen, which a managed send also requires before it refuses.
pub const TRUST_REFUSE: &str = "No, exit";

impl HarnessAdapter for ClaudeCode {
    fn transient_recovery(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
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
        let mut arguments = Vec::new();
        if let Some(resume) = resume.filter(|value| !value.is_empty()) {
            arguments.extend(["--resume".to_owned(), resume.to_owned()]);
        }
        if let Some(model) = model.filter(|value| !value.is_empty()) {
            arguments.extend(["--model".to_owned(), model.to_owned()]);
        }
        (arguments, Vec::new())
    }

    fn reasoning_arguments(&self, effort: &str) -> Option<Vec<String>> {
        Some(vec!["--effort".to_owned(), effort.to_owned()])
    }

    fn split_conversation(&self, argv: &[String]) -> Option<super::Conversations> {
        Some(split(argv))
    }

    fn fresh_arguments(&self, rest: Vec<String>, conversation: &str) -> Option<Vec<String>> {
        let mut arguments = vec!["--session-id".to_owned(), conversation.to_owned()];
        arguments.extend(rest);
        Some(arguments)
    }

    fn replay_arguments(&self, rest: Vec<String>, conversation: &str) -> Vec<String> {
        let mut arguments = vec!["--resume".to_owned(), conversation.to_owned()];
        arguments.extend(rest);
        arguments
    }

    fn assigns_session_id(&self) -> bool {
        true
    }

    fn skill_directory(&self) -> Option<&'static str> {
        Some(".claude")
    }

    fn verified_submission(&self) -> bool {
        true
    }

    fn composer_view(&self, rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
        // A labelled top border is tried only when two plain rules do not already
        // frame a composer, so a draft row that happens to look like a labelled
        // rule cannot change how an unlabelled screen is split.
        view_framed(rows, is_rule).or_else(|| view_framed(rows, is_labelled_rule))
    }

    fn trust_prompt(&self, screen: &str) -> bool {
        screen.contains(TRUST_QUESTION) && screen.contains(TRUST_ACCEPT)
    }

    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::OptIn
    }

    fn tui(&self) -> Option<&'static dyn crate::harness_tui::HarnessTui> {
        crate::harness_tui::harness_tui(Some("claude"))
    }

    fn transcript_path(&self, hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
        transcript_path(hints)
    }

    fn read_transcript(&self, lines: &[&[u8]], activity: &mut super::transcript::Activity) {
        read_transcript(lines, activity);
    }

    /// Claude Code writes each sub-agent's conversation to `<session>/subagents/*.jsonl` beside
    /// the session's own transcript.
    fn related_transcripts(
        &self,
        _hints: &super::transcript::Hints<'_>,
        main: &std::path::Path,
    ) -> Vec<PathBuf> {
        let directory = main.with_extension("").join("subagents");
        std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Whether `screen` shows the folder-trust screen with its refusal, as a managed send checks
/// before it types into an idle Claude Code.
#[must_use]
pub fn trust_prompt_with_refusal(screen: &str) -> bool {
    screen.contains(TRUST_QUESTION)
        && screen.contains(TRUST_REFUSE)
        && screen.contains(TRUST_ACCEPT)
}

/// Split `rows` at the last plain rule and the nearest row above it accepted by `is_top`.
fn view_framed(
    rows: &[(Vec<char>, Vec<char>)],
    is_top: fn(&[char]) -> bool,
) -> Option<ComposerView> {
    let bottom = rows.iter().rposition(|(plain, _)| is_rule(plain))?;
    let top = rows[..bottom]
        .iter()
        .rposition(|(plain, _)| is_top(plain))?;
    let body = &rows[top + 1..bottom];
    let first = body.first()?;
    let offset = first.0.iter().take_while(|c| c.is_whitespace()).count();
    if first.0.get(offset) != Some(&'❯') {
        return None;
    }
    let (composer, composer_solid) = composer_from(body, offset + 1);
    Some(ComposerView {
        transcript: join_rows(&rows[..top]),
        composer,
        composer_solid,
        footer: join_rows(&rows[bottom + 1..]),
    })
}

/// Whether `argument` selects a conversation (`--session-id`, `--resume`, `--continue`,
/// `--fork-session`, `-r`, `-c`, inline or not).
#[must_use]
pub fn conversation_selector(argument: &str) -> bool {
    let key = argument.split_once('=').map_or(argument, |(key, _)| key);
    matches!(
        key,
        "--session-id" | "--resume" | "--continue" | "--fork-session" | "-r" | "-c"
    ) || (!argument.starts_with("--") && (argument.starts_with("-r") || argument.starts_with("-c")))
}

enum Selector {
    Flag,
    Value(&'static str, Option<String>),
}

fn selector(argument: &str) -> Result<Option<Selector>, String> {
    let (key, inline) = match argument.split_once('=') {
        Some((key, value)) => (key, Some(value.to_owned())),
        None => (argument, None),
    };
    if argument.starts_with("--") {
        return Ok(match key {
            "--resume" => Some(Selector::Value("--resume", inline)),
            "--session-id" => Some(Selector::Value("--session-id", inline)),
            "--continue" | "--fork-session" => Some(Selector::Flag),
            _ => None,
        });
    }
    if argument == "-c" {
        return Ok(Some(Selector::Flag));
    }
    if argument == "-r" {
        return Ok(Some(Selector::Value("-r", None)));
    }
    if let Some(value) = argument.strip_prefix("-r") {
        return Ok(Some(Selector::Value("-r", Some(value.to_owned()))));
    }
    if argument.starts_with("-c") {
        return Err(format!(
            "recorded Claude argument {argument:?} combines a conversation flag and cannot be replayed"
        ));
    }
    Ok(None)
}

fn split(argv: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let mut values = Vec::new();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < argv.len() {
        match selector(&argv[index])? {
            None => {
                rest.push(argv[index].clone());
                index += 1;
            }
            Some(Selector::Flag) => index += 1,
            Some(Selector::Value(option, inline)) => {
                let value = match inline {
                    Some(value) => {
                        index += 1;
                        value
                    }
                    None => {
                        if index + 1 >= argv.len() || argv[index + 1].starts_with('-') {
                            return Err(format!(
                                "recorded Claude {option} has no conversation value; it opens a picker and cannot be replayed"
                            ));
                        }
                        index += 2;
                        argv[index - 1].clone()
                    }
                };
                if value.is_empty() {
                    return Err(format!(
                        "recorded Claude {option} has an empty conversation value"
                    ));
                }
                values.push(value);
            }
        }
    }
    Ok((values, rest))
}

/// Bottom screen lines scanned for background work and the scrolled-view pill.
const BACKGROUND_WINDOW_LINES: usize = 12;

/// The part of a status row near the bottom of `screen` that says work is still running, when
/// one shows: a running turn, or background work the agent started. Claude Code counts its background
/// shells and sub-agents in the footer under its input box, as in
/// `⏵⏵ auto mode on · 2 shells · ← 1 agent · ↓ to manage`, and in the row that closes a
/// turn, as in `✻ Sautéed for 5m 51s · done 12:44 AM · 2 shells still running`. The turn's
/// row is history: it still says `still running` after the work ends. So when the input box
/// (a `❯` row) is in view, only the footer rows under it are read, and the turn's row only
/// when no input box is. An agent that went idle waiting on such work, such as a validation
/// gate it runs in a background shell, has not stopped without a verdict; only one whose
/// screen then stops changing for the stall threshold is reported, as stalled.
#[must_use]
pub fn background_work(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let mut start = lines.len().saturating_sub(BACKGROUND_WINDOW_LINES);
    if let Some(input) = lines[start..]
        .iter()
        .rposition(|line| line.trim_start().starts_with('❯'))
    {
        start += input + 1;
    }
    lines[start..].iter().rev().find_map(|line| {
        if !line.contains('·') {
            return None;
        }
        line.split('·').find_map(|segment| {
            let segment = segment.trim().trim_start_matches('←').trim_start();
            background_count(segment).then(|| segment.to_owned())
        })
    })
}

/// Whether a footer segment shows work running: Claude Code's `esc to interrupt`, which it
/// draws while a turn runs, or a count of background shells or agents: `1 shell`, `2 shells`,
/// `1 agent`, `3 agents`, each optionally followed by `still running`. Herdr's status can say
/// `idle` while the footer says `esc to interrupt` and output streams (live finding
/// 2026-10-20), so the screen wins.
fn background_count(segment: &str) -> bool {
    if segment == super::WORKING_MARKER {
        return true;
    }
    let segment = segment.strip_suffix(" still running").unwrap_or(segment);
    let Some((count, noun)) = segment.split_once(' ') else {
        return false;
    };
    !count.is_empty()
        && count.bytes().all(|byte| byte.is_ascii_digit())
        && count.parse::<u64>().is_ok_and(|count| count > 0)
        && matches!(noun, "shell" | "shells" | "agent" | "agents")
}

/// How many new messages Claude Code says wait below its view, when the view is scrolled up.
/// Scrolled up, Claude Code freezes the transcript where the reader left it and ends the last
/// row above its input box with `16 new messages (ctrl+End) ↓`, raising the count as messages
/// arrive below (live panes, 2026-10-20). Its pane keeps no terminal scrollback, so nothing
/// below the view can be read: not the running turn's timer, not the newest output, not a
/// closing verdict. A growing count changes the screen, which is progress like any change; a
/// count that holds still proves nothing, since one long tool call or sub-agent run is a
/// single message. The pill is read only at the end of that one row, between the input box's
/// rule and any blank rows, so a transcript that quotes it is not a scrolled view.
#[must_use]
pub fn scrolled_below(screen: &str) -> Option<u64> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(BACKGROUND_WINDOW_LINES);
    let input = start
        + lines[start..]
            .iter()
            .rposition(|line| line.trim_start().starts_with('❯'))?;
    let row = lines[..input].iter().rev().find(|line| {
        let line = line.trim();
        !line.is_empty() && !line.chars().all(|character| character == '─')
    })?;
    let row = row.trim_end().strip_suffix('↓')?.trim_end();
    let row = match row.strip_suffix(')') {
        Some(hinted) => hinted.rsplit_once('(')?.0.trim_end(),
        None => row,
    };
    let row = row
        .strip_suffix(" new messages")
        .or_else(|| row.strip_suffix(" new message"))?;
    // The pill can be drawn flush against the transcript row it covers, so the count is the
    // row's trailing digits, not its last word.
    let count = &row[row.len() - row.bytes().rev().take_while(u8::is_ascii_digit).count()..];
    count.parse::<u64>().ok().filter(|count| *count > 0)
}

/// Claude Code's transcript for `hints`: `<projects>/<project>/<session>.jsonl`, where
/// `<projects>` is `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`) and `<project>`
/// is the working directory with every character other than an ASCII letter or digit replaced by
/// `-`. Claude Code keeps no transcript open between writes, so the session id is required; when
/// the encoded directory does not hold it, every project directory is searched, as
/// `agentctl inbox watch` does.
fn transcript_path(hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
    let session = hints.session?;
    if session.is_empty() || session.contains(['/', '\0']) {
        return None;
    }
    let projects = hints
        .environment("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| hints.home.map(|home| home.join(".claude")))?
        .join("projects");
    if let Some(cwd) = hints.cwd {
        let encoded: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = projects.join(encoded).join(format!("{session}.jsonl"));
        if path.is_file() {
            return Some(path);
        }
    }
    crate::inbox::watch::transcript_of_session(&projects, session)
}

/// Read Claude Code's JSONL entries. An `assistant` entry is model output (text, thinking or a
/// tool call); a `user` entry carries tool results or a prompt; an `api-request` entry is a model
/// request sent, whose reply has not been written until the next `assistant` entry. A `tool_use`
/// block without its `tool_result` is a call in flight. Every other entry (attachments, titles,
/// modes, snapshots) is bookkeeping: it moves the last-event clock only.
fn read_transcript(lines: &[&[u8]], activity: &mut super::transcript::Activity) {
    use super::transcript::{parse_utc, ToolCall, ToolLimit};
    use serde_json::Value;
    let mut open: Vec<ToolCall> = Vec::new();
    for line in lines {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let at = entry["timestamp"].as_str().and_then(parse_utc);
        let kind = entry["type"].as_str().unwrap_or_default();
        let blocks = entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match kind {
            "assistant" => {
                let Some(at) = at else { continue };
                let mut label = "assistant";
                for block in &blocks {
                    match block["type"].as_str() {
                        Some("tool_use") => {
                            label = "tool call";
                            let input = &block["input"];
                            let name = block["name"].as_str().unwrap_or("tool").to_owned();
                            let background = input["run_in_background"] == true;
                            let limit = if background {
                                ToolLimit::Background
                            } else if let Some(ms) = input["timeout"].as_f64() {
                                ToolLimit::Explicit(ms / 1000.0)
                            } else if name == "Bash" {
                                // Bash's own default; BASH_DEFAULT_TIMEOUT_MS can change it.
                                ToolLimit::HarnessDefault(120.0)
                            } else {
                                ToolLimit::None
                            };
                            open.push(ToolCall {
                                id: block["id"].as_str().unwrap_or_default().to_owned(),
                                command: input["command"]
                                    .as_str()
                                    .or_else(|| input["description"].as_str())
                                    .or_else(|| input["prompt"].as_str())
                                    .or_else(|| input["url"].as_str())
                                    .map(str::to_owned),
                                name,
                                started_at: at,
                                limit,
                            });
                        }
                        Some("text") if label == "assistant" => label = "assistant text",
                        Some("thinking") if label == "assistant" => label = "thinking",
                        _ => {}
                    }
                }
                activity.output(at, label);
            }
            // Skill bodies, hook output and compaction summaries are written as user entries the
            // harness made itself.
            "user" if entry["isMeta"] == true || entry["isCompactSummary"] == true => {
                if let Some(at) = at {
                    activity.bookkeeping(at, "meta");
                }
            }
            "user" => {
                let Some(at) = at else { continue };
                let mut results = 0;
                for block in &blocks {
                    if block["type"] == "tool_result" {
                        results += 1;
                        let id = block["tool_use_id"].as_str().unwrap_or_default();
                        open.retain(|call| call.id != id);
                    }
                }
                if results > 0 {
                    activity.event(at, "tool result");
                } else {
                    activity.prompt(at);
                    // A prompt starts a new turn; anything still open was abandoned.
                    open.clear();
                }
            }
            "api-request" => {
                if let Some(at) = at {
                    activity.request(at);
                }
            }
            _ => {
                if let Some(at) = at {
                    activity.bookkeeping(at, kind);
                }
            }
        }
    }
    activity.set_open_tools(open);
}

#[cfg(test)]
mod tests {
    use super::super::transcript::{Activity, Hints, ToolLimit};
    use super::*;

    const FIXTURE: &str = include_str!("../../testdata/transcripts/claude.jsonl");
    /// 2026-10-27T10:00:00Z, the fixture's first entry.
    const T0: f64 = 1_793_095_200.0;

    fn read(lines: usize) -> Activity {
        let all: Vec<&[u8]> = FIXTURE.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all[..lines.min(all.len())], &mut activity);
        activity
    }

    #[test]
    fn a_long_subagent_call_is_in_flight_untimed_and_later_bookkeeping_is_not_output() {
        let activity = read(usize::MAX);
        let tool = activity.tool.clone().expect("a call in flight");
        assert_eq!(tool.name, "Agent");
        assert_eq!(tool.limit, ToolLimit::None);
        assert!(tool.untimed());
        assert_eq!(tool.started_at, T0 + 305.0);
        assert_eq!(tool.command.as_deref(), Some("Review the change"));
        // The background shell is in flight too, but does not hold the turn.
        assert_eq!(activity.open_tools, 2);
        assert_eq!(activity.last_output_at, Some(T0 + 306.0));
        assert_eq!(activity.last_event.as_deref(), Some("queue-operation"));
        assert_eq!(activity.last_event_at, Some(T0 + 1200.0));
        assert_eq!(activity.request_since, None);
        assert_eq!(activity.prompt_at, Some(T0));
    }

    #[test]
    fn a_request_with_no_reply_is_open_and_a_bash_call_carries_its_timeout() {
        let activity = read(6);
        assert_eq!(activity.request_since, Some(T0 + 301.0));
        assert!(activity.tool.is_none());
        let activity = read(4);
        let tool = activity.tool.expect("bash in flight");
        assert_eq!(tool.limit, ToolLimit::Explicit(600.0));
        assert_eq!(tool.describe(), "Bash `make validate`");
        assert_eq!(activity.last_output.as_deref(), Some("tool call"));
    }

    #[test]
    fn harness_made_user_entries_are_bookkeeping_and_keep_calls_in_flight() {
        let mut text = FIXTURE.to_owned();
        text.push_str(
            "{\"type\":\"user\",\"isMeta\":true,\"timestamp\":\"2026-10-27T10:30:00.000Z\",\"message\":{\"role\":\"user\",\"content\":\"skill body\"}}\n",
        );
        let all: Vec<&[u8]> = text.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all, &mut activity);
        assert_eq!(activity.tool.expect("still in flight").name, "Agent");
        assert_eq!(activity.prompt_at, Some(T0));
        assert_eq!(activity.last_event.as_deref(), Some("meta"));
        assert_eq!(activity.last_progress_at, Some(T0 + 306.0));
    }

    #[test]
    fn sub_agent_transcripts_beside_the_session_are_related() {
        let home = crate::turn_state::tests::scratch();
        let main = home.join("s1.jsonl");
        std::fs::write(&main, FIXTURE).unwrap();
        std::fs::create_dir_all(home.join("s1/subagents")).unwrap();
        std::fs::write(home.join("s1/subagents/agent-a.jsonl"), "{}\n").unwrap();
        let hints = Hints {
            session: None,
            cwd: None,
            home: None,
            pid: None,
            proc: None,
        };
        assert_eq!(
            ClaudeCode.related_transcripts(&hints, &main),
            [home.join("s1/subagents/agent-a.jsonl")]
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn the_transcript_is_found_in_the_encoded_project_directory_by_session_id() {
        let home = crate::turn_state::tests::scratch();
        let project = home.join(".claude/projects/-work-my-project");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("abc-123.jsonl");
        std::fs::write(&path, FIXTURE).unwrap();
        let hints = Hints {
            session: Some("abc-123"),
            cwd: Some(std::path::Path::new("/work/my.project")),
            home: Some(&home),
            pid: None,
            proc: None,
        };
        assert_eq!(transcript_path(&hints), Some(path.clone()));
        // A working directory that encodes elsewhere still finds it by searching.
        let hints = Hints {
            cwd: Some(std::path::Path::new("/elsewhere")),
            ..hints
        };
        assert_eq!(transcript_path(&hints), Some(path));
        let hints = Hints {
            session: None,
            ..hints
        };
        assert_eq!(
            transcript_path(&hints),
            None,
            "no session id, no transcript"
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn conversation_selectors_are_recognised_inline_and_split() {
        for argument in [
            "--resume",
            "--resume=x",
            "--session-id",
            "-r",
            "-rabc",
            "-c",
            "--continue",
        ] {
            assert!(conversation_selector(argument), "{argument}");
        }
        for argument in ["--model", "--effort", "-m"] {
            assert!(!conversation_selector(argument), "{argument}");
        }
    }
}
