//! Codex: its composer, idle screen, held messages, launch arguments, and rollout transcript.

use std::path::{Path, PathBuf};

use super::{FollowupPolicy, HarnessAdapter, ScreenMarkers};
use crate::submission::{
    compact, composer_from, composer_view, join_rows, render_screen, text_of, ComposerView,
    RUNNING_TURN_ROWS,
};

/// Codex.
pub struct Codex;

static MARKERS: ScreenMarkers = ScreenMarkers {
    // The edit hint under its queued block, and the block's heading.
    queue_hints: &["edit last queued message", "Queued follow-up inputs"],
    // Its command approval and its folder-trust question (`Do you trust the contents of this
    // directory?`).
    approval: &["would you like to run", "allow command?", "do you trust"],
    reply_bullets: &['•'],
    user_turn_glyphs: &['›'],
    tool_output_glyphs: &[],
    // It alternates the two while a turn runs.
    running_glyphs: &['•', '◦'],
    // Its rotating composer placeholders.
    composer_placeholders: &[
        "Ask Codex to do anything",
        "Explain this codebase",
        "Summarize recent commits",
        "Implement {feature}",
        "Find and fix a bug in @filename",
        "Write tests for @filename",
        "Improve documentation in @filename",
        "Run /review on my current changes",
        "Use /skills to list available skills",
    ],
};

/// The two phrases of Codex's folder-trust dialog.
const TRUST_QUESTION: &str = "Trust this folder?";
const TRUST_ACCEPT: &str = "Trust and continue";

impl HarnessAdapter for Codex {
    fn native_group_leader(&self, executable: &Path) -> bool {
        executable.file_name().is_some_and(|name| name == "codex")
    }
    fn transient_recovery(&self) -> bool {
        true
    }
    fn stalled_goal_resume(&self, screen: &str) -> Option<&'static str> {
        screen
            .lines()
            .any(|line| line.trim().starts_with("Goal stalled"))
            .then_some("/goal resume")
    }
    fn name(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex"
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
            arguments.extend(["resume".to_owned(), resume.to_owned()]);
        }
        // Inline mode keeps the conversation in the terminal's own rows, where agentctl reads it.
        arguments.push("--no-alt-screen".to_owned());
        if let Some(model) = model.filter(|value| !value.is_empty()) {
            arguments.extend(["--model".to_owned(), model.to_owned()]);
        }
        (arguments, Vec::new())
    }

    fn reasoning_arguments(&self, effort: &str) -> Option<Vec<String>> {
        Some(vec![
            "--config".to_owned(),
            format!("model_reasoning_effort={effort}"),
        ])
    }

    fn split_conversation(&self, argv: &[String]) -> Option<super::Conversations> {
        let mut values = Vec::new();
        let mut rest = argv;
        if rest.first().map(String::as_str) == Some("resume") {
            if rest.len() < 2 || rest[1].starts_with('-') {
                return Some(Err(
                    "recorded Codex resume has no conversation id (for example --last) and cannot be replayed"
                        .to_owned(),
                ));
            }
            values.push(rest[1].clone());
            rest = &rest[2..];
        }
        if rest.iter().any(|item| item == "resume") {
            return Some(Err(
                "recorded Codex arguments name resume in an unsupported position".to_owned(),
            ));
        }
        Some(Ok((values, rest.to_vec())))
    }

    fn replay_arguments(&self, rest: Vec<String>, conversation: &str) -> Vec<String> {
        let mut arguments = vec!["resume".to_owned(), conversation.to_owned()];
        arguments.extend(rest);
        arguments
    }

    fn skill_directory(&self) -> Option<&'static str> {
        Some(".codex")
    }

    fn verified_submission(&self) -> bool {
        true
    }

    fn composer_view(&self, rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
        view(rows)
    }

    fn submit_key(&self, view: &ComposerView) -> &'static str {
        // A busy Codex steers the active turn on Enter and queues on Tab. It shows
        // the Tab hint only while it is busy and holds staged text.
        if view.footer.contains(QUEUE_HINT) {
            "Tab"
        } else {
            "Enter"
        }
    }

    fn trust_prompt(&self, screen: &str) -> bool {
        screen.contains(TRUST_QUESTION) && screen.contains(TRUST_ACCEPT)
    }

    /// Herdr's bundled Codex rules recognise only working and blocked screens. An idle Codex
    /// matches no rule, and `agent explain` reports `unknown` with `fallback_reason:
    /// codex_state_ambiguous`, which never becomes `idle`. An empty composer with a recognised
    /// footer and no running turn, dialog or queued message is idle.
    fn settle_herdr_state(&self, herdr_state: &str, screen: &str) -> Option<&'static str> {
        (herdr_state == "unknown" && idle_composer(screen)).then_some("idle")
    }

    fn followup_policy(&self) -> FollowupPolicy {
        FollowupPolicy::Always
    }

    fn tui(&self) -> Option<&'static dyn crate::harness_tui::HarnessTui> {
        crate::harness_tui::harness_tui(Some("codex"))
    }

    fn transcript_path(&self, hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
        transcript_path(hints)
    }

    fn read_transcript(&self, lines: &[&[u8]], activity: &mut super::transcript::Activity) {
        read_transcript(lines, activity);
    }

    /// The other rollouts the process holds open: its sub-agent threads.
    fn related_transcripts(
        &self,
        hints: &super::transcript::Hints<'_>,
        main: &Path,
    ) -> Vec<PathBuf> {
        hints
            .open_files()
            .into_iter()
            .filter(|path| rollout_name(path) && path != main)
            .collect()
    }
}

/// The hint a busy Codex shows under staged text: Tab queues it for the end of the turn.
pub const QUEUE_HINT: &str = "tab to queue message";
/// Glyphs Codex draws in column 0 of its composer's first row. Earlier releases
/// draw `›`; v0.159.1 draws `»`, and keeps `›` for selection lists such as its
/// folder-trust prompt.
const COMPOSER_MARKERS: [char; 2] = ['›', '»'];
/// Headings, lowercased, under which Codex lists the messages it holds while a turn runs: a
/// prompt submitted with Enter mid-turn waits for the next tool call, one submitted with Tab for
/// the end of the turn.
pub const PENDING_HEADINGS: [&str; 4] = [
    "messages to be submitted after next tool call",
    "messages to be submitted at end of turn",
    "queued follow-up inputs",
    "queued follow-up messages",
];
/// Shortest pending-message preview, without blanks, that counts as the start of a longer
/// prompt. A shorter preview counts only when it is the whole prompt.
const PREVIEW_MIN_CHARS: usize = 12;

fn view(rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
    let populated: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (plain, _))| plain.iter().any(|c| !c.is_whitespace()))
        .map(|(index, _)| index)
        .collect();
    if populated.len() < 2 {
        return None;
    }
    let footer = populated[populated.len() - 1];
    if !text_of(&rows[footer].0).starts_with("  ") {
        return None;
    }
    let mut start = None;
    for index in (0..footer).rev() {
        let plain = text_of(&rows[index].0);
        if plain.starts_with(&COMPOSER_MARKERS[..]) {
            start = Some(index);
            break;
        }
        if !plain.trim().is_empty() && !plain.starts_with("  ") {
            return None;
        }
    }
    let start = start?;
    let (composer, composer_solid) = composer_from(&rows[start..footer], 1);
    Some(ComposerView {
        transcript: join_rows(&rows[..start]),
        composer,
        composer_solid,
        footer: join_rows(&rows[footer..]),
    })
}

/// Whether Codex shows an empty idle composer with a recognized status footer.
///
/// This screen observation establishes readiness only after the caller independently
/// verifies the live recipient. A retained composer during a turn or dialog is not idle.
pub(crate) fn idle_composer(screen: &str) -> bool {
    let Some(view) = composer_view("codex", screen) else {
        return false;
    };
    if !view.composer_solid.trim().is_empty()
        || view.composer.contains("[Pasted text #")
        || view.composer.contains("[Pasted Content ")
    {
        return false;
    }
    let footer = view.footer.to_ascii_lowercase();
    let words: Vec<&str> = footer.split_whitespace().collect();
    let context = words.windows(3).any(|words| {
        words[0].strip_suffix('%').is_some_and(|number| {
            let digits = |value: &str| {
                !value.is_empty() && value.bytes().all(|value| value.is_ascii_digit())
            };
            match number.split_once('.') {
                Some((whole, fraction)) => digits(whole) && digits(fraction),
                None => digits(number),
            }
        }) && words[1] == "context"
            && matches!(words[2], "left" | "remaining")
    });
    let path_footer = footer.contains('·')
        && words
            .iter()
            .any(|word| word.starts_with('/') || word.starts_with("~/"));
    if !path_footer && !footer.contains("? for shortcuts") && !context {
        return false;
    }
    let mut active_timer = false;
    for line in view.transcript.lines() {
        if timer_row(line) {
            active_timer = true;
        } else if line.starts_with(['•', '◦', '■', '✗', '✓'])
            && ![
                "queued follow-up inputs",
                "messages to be submitted after next tool call",
                "messages to be submitted at end of turn",
            ]
            .iter()
            .any(|marker| line.to_ascii_lowercase().contains(marker))
        {
            active_timer = false;
        }
    }
    if active_timer {
        return false;
    }
    let rows = render_screen(screen);
    let Some(last) = rows
        .iter()
        .rposition(|(plain, _)| plain.iter().any(|character| !character.is_whitespace()))
    else {
        return false;
    };
    const REFUSALS: [&str; 22] = [
        "esc to interrupt",
        " to interrupt",
        "tab to queue message",
        "press up to edit queued messages",
        "edit last queued message",
        "queued follow-up inputs",
        "messages to be submitted after next tool call",
        "messages to be submitted at end of turn",
        "paste again to expand",
        "trust this folder?",
        "trust and continue",
        "do you trust the contents of this directory?",
        "approval required",
        "requires approval",
        "approve this",
        "would you like to run",
        "press enter to continue",
        "enter to confirm",
        "esc to cancel",
        "enter to submit answer",
        "enter to submit all",
        "allow command?",
    ];
    let active = &rows[(last + 1).saturating_sub(RUNNING_TURN_ROWS)..=last];
    if ["all results", "filesystem only", "plugins"]
        .iter()
        .all(|marker| {
            active
                .iter()
                .any(|(plain, _)| text_of(plain).to_ascii_lowercase().contains(marker))
        })
    {
        return false;
    }
    !active.iter().any(|(plain, _)| {
        let line = text_of(plain);
        if timer_row(&line) {
            return false;
        }
        let line = line.to_ascii_lowercase();
        REFUSALS.iter().any(|refusal| line.contains(refusal))
    })
}

pub(crate) fn timer_row(line: &str) -> bool {
    let line = line.trim_end();
    let line = if let Some(rest) = line.strip_prefix('•').or_else(|| line.strip_prefix('◦')) {
        if !rest.starts_with([' ', '\t']) {
            return false;
        }
        rest.trim_start_matches([' ', '\t'])
    } else {
        line
    };
    if line.chars().next().is_none_or(|first| {
        first.is_whitespace() || matches!(first, '›' | '•' | '◦' | '■' | '✗' | '✓' | '─')
    }) {
        return false;
    }
    line.match_indices(" (")
        .any(|(index, _)| elapsed_suffix(&line[index + 2..]))
}

fn elapsed_suffix(mut elapsed: &str) -> bool {
    loop {
        let digits = elapsed.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        elapsed = &elapsed[digits..];
        if let Some(rest) = elapsed
            .strip_prefix("h ")
            .or_else(|| elapsed.strip_prefix("m "))
        {
            elapsed = rest;
            continue;
        }
        let Some(rest) = elapsed.strip_prefix('s') else {
            return false;
        };
        let suffix_valid = |suffix: &str| {
            suffix.is_empty()
                || suffix
                    .strip_prefix(" · ")
                    .is_some_and(|text| !text.is_empty())
        };
        if let Some(suffix) = rest.strip_prefix(')') {
            return suffix_valid(suffix);
        }
        let Some(hint) = rest.strip_prefix(" • ") else {
            return false;
        };
        return hint
            .match_indices(" to interrupt)")
            .any(|(end, marker)| end > 0 && suffix_valid(&hint[end + marker.len()..]));
    }
}

/// The messages Codex lists under one of [`PENDING_HEADINGS`] in `transcript`, each
/// without its blanks and without the `↳` that starts it.
///
/// Codex draws each held message as `  ↳ ` and its first row, then up to two more wrapped rows
/// indented by four columns, and a `    …` row when it cut the rest. A preview is therefore the
/// start of the message, not all of it, so a long prompt held this way never shows its end, which
/// is what read-back looks for.
pub(crate) fn pending_previews(transcript: &str) -> Vec<String> {
    let mut previews = Vec::new();
    let mut in_block = false;
    let mut current: Option<String> = None;
    for line in transcript.lines() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_lowercase();
        if trimmed.starts_with('•')
            && PENDING_HEADINGS
                .iter()
                .any(|heading| lower.contains(heading))
        {
            previews.extend(current.take());
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if line.starts_with(' ') {
            if let Some(rest) = trimmed.strip_prefix('↳') {
                previews.extend(current.take());
                current = Some(compact(rest));
                continue;
            }
            if line.starts_with("    ") && !trimmed.is_empty() {
                if let Some(entry) = current.as_mut() {
                    if trimmed == "…" {
                        previews.extend(current.take());
                    } else {
                        entry.push_str(&compact(trimmed));
                    }
                    continue;
                }
            }
        }
        previews.extend(current.take());
        in_block = false;
    }
    previews.extend(current);
    previews.retain(|preview| !preview.is_empty());
    previews
}

/// How many messages Codex lists as held in `transcript` whose preview is the prompt `prompt`
/// (given without blanks) or, when at least [`PREVIEW_MIN_CHARS`] long, its start.
pub(crate) fn pending_copies(transcript: &str, prompt: &str) -> usize {
    if prompt.is_empty() {
        return 0;
    }
    pending_previews(transcript)
        .iter()
        .filter(|preview| {
            preview.as_str() == prompt
                || (preview.chars().count() >= PREVIEW_MIN_CHARS
                    && prompt.starts_with(preview.as_str()))
        })
        .count()
}

/// Codex's rollout for `hints`. A running Codex holds the rollout of each thread it runs open
/// (`<sessions>/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, `<sessions>` being `$CODEX_HOME/sessions`,
/// default `~/.codex/sessions`), sub-agent threads included. The open rollout named by the
/// record's session id wins; else the open rollout of the main thread (whose first entry names no
/// parent thread); else, with no process to ask, the rollout named by the session id.
fn transcript_path(hints: &super::transcript::Hints<'_>) -> Option<PathBuf> {
    let session = hints
        .session
        .filter(|value| !value.is_empty() && !value.contains('/'));
    let open: Vec<PathBuf> = hints
        .open_files()
        .into_iter()
        .filter(|path| rollout_name(path) && path.to_string_lossy().contains("/sessions/"))
        .collect();
    if let Some(session) = session {
        if let Some(path) = open.iter().find(|path| {
            path.to_string_lossy()
                .ends_with(&format!("-{session}.jsonl"))
        }) {
            return Some(path.clone());
        }
    }
    let main: Vec<PathBuf> = open
        .iter()
        .filter(|path| main_thread(path))
        .cloned()
        .collect();
    if let Some(path) = super::transcript::newest(if main.is_empty() { open } else { main }) {
        return Some(path);
    }
    let session = session?;
    let sessions = hints
        .environment("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| hints.home.map(|home| home.join(".codex")))?
        .join("sessions");
    let suffix = format!("-{session}.jsonl");
    super::transcript::find_in_dated_tree(&sessions, 60, &|entry| {
        (rollout_name(entry) && entry.to_string_lossy().ends_with(&suffix))
            .then(|| entry.to_owned())
    })
}

fn rollout_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

/// Whether the rollout's first entry, its `session_meta`, names no parent thread.
fn main_thread(path: &Path) -> bool {
    use std::io::{BufRead, BufReader, Read};
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = Vec::new();
    if BufReader::new(file.take(1 << 20))
        .read_until(b'\n', &mut first)
        .is_err()
    {
        return false;
    }
    let Ok(entry) = serde_json::from_slice::<serde_json::Value>(&first) else {
        return false;
    };
    let payload = &entry["payload"];
    entry["type"] == "session_meta"
        && payload["parent_thread_id"].is_null()
        && payload["source"]["subagent"].is_null()
}

/// The text of the first JavaScript string literal after `key:` in `source`, unescaped.
fn js_string(source: &str, key: &str) -> Option<String> {
    let start = source.find(&format!("{key}:"))? + key.len() + 1;
    let rest = source[start..].trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c if c == quote => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// The number after `key:` in `source`.
fn js_number(source: &str, key: &str) -> Option<f64> {
    let start = source.find(&format!("{key}:"))? + key.len() + 1;
    let rest = source[start..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '_'))
        .unwrap_or(rest.len());
    rest[..end].replace('_', "").parse().ok()
}

/// The command and limit of one call. `exec_command` (directly, or from a code-mode `exec` cell)
/// returns after its yield time even while its process runs on, so it is bounded by that or by
/// unified exec's default yield (10 s); `wait` and `wait_agent` are bounded by their timeouts.
fn call_limit(
    name: &str,
    source: &str,
    arguments: &serde_json::Value,
) -> (Option<String>, super::transcript::ToolLimit) {
    use super::transcript::ToolLimit;
    let command = arguments["cmd"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            arguments["command"].as_array().map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .or_else(|| arguments["command"].as_str().map(str::to_owned))
        .or_else(|| js_string(source, "cmd"))
        .or_else(|| arguments["message"].as_str().map(str::to_owned));
    let explicit = ["timeout_ms", "yield_time_ms"]
        .iter()
        .filter_map(|key| arguments[*key].as_f64().or_else(|| js_number(source, key)))
        .fold(None, |most: Option<f64>, ms| {
            Some(most.map_or(ms, |most| most.max(ms)))
        });
    let limit = match explicit {
        Some(ms) => ToolLimit::Explicit(ms / 1000.0),
        None if name == "exec_command" || source.contains("exec_command") => {
            ToolLimit::HarnessDefault(10.0)
        }
        None => ToolLimit::None,
    };
    (command, limit)
}

/// Read Codex rollout entries. `task_started` opens a turn and `task_complete` closes it;
/// `reasoning`, `message` from the assistant, and `agent_message` are model output; a
/// `function_call` or `custom_tool_call` is output and a call in flight until its `*_output`.
/// Codex writes no entry when it sends a model request, so a prompt, and a tool result that leaves
/// no call in flight, start the wait for the model.
fn read_transcript(lines: &[&[u8]], activity: &mut super::transcript::Activity) {
    use super::transcript::{parse_utc, ToolCall};
    use serde_json::Value;
    let mut open: Vec<ToolCall> = Vec::new();
    for line in lines {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(at) = entry["timestamp"].as_str().and_then(parse_utc) else {
            continue;
        };
        let payload = &entry["payload"];
        let kind = entry["type"].as_str().unwrap_or_default();
        let item = payload["type"].as_str().unwrap_or_default();
        match (kind, item) {
            ("event_msg", "task_started") => {
                open.clear();
                activity.prompt(at);
                activity.request(at);
            }
            ("event_msg", "task_complete" | "turn_aborted") => {
                open.clear();
                activity.event(at, "turn complete");
                activity.request_since = None;
            }
            ("response_item", "reasoning") => activity.output(at, "reasoning"),
            ("response_item", "agent_message") => activity.output(at, "assistant text"),
            ("response_item", "message") => {
                if payload["role"] == "assistant" {
                    activity.output(at, "assistant text");
                } else {
                    activity.bookkeeping(at, "message");
                }
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                let name = payload["name"].as_str().unwrap_or("tool").to_owned();
                let source = payload["input"].as_str().unwrap_or_default();
                let arguments = payload["arguments"]
                    .as_str()
                    .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    .unwrap_or(Value::Null);
                let (command, limit) = call_limit(&name, source, &arguments);
                open.push(ToolCall {
                    id: payload["call_id"].as_str().unwrap_or_default().to_owned(),
                    name,
                    command,
                    started_at: at,
                    limit,
                });
                activity.output(at, "tool call");
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let id = payload["call_id"].as_str().unwrap_or_default();
                open.retain(|call| call.id != id);
                activity.event(at, "tool result");
                if open.is_empty() {
                    activity.request(at);
                }
            }
            // Streamed while the model works, before the item it builds is written.
            (
                "event_msg",
                "agent_reasoning"
                | "agent_reasoning_delta"
                | "agent_reasoning_raw_content"
                | "agent_message"
                | "agent_message_delta"
                | "token_count"
                | "item_completed"
                | "exec_command_begin"
                | "exec_command_output_delta"
                | "exec_command_end",
            ) => activity.event(at, item),
            _ => activity.bookkeeping(at, if item.is_empty() { kind } else { item }),
        }
    }
    activity.set_open_tools(open);
}

#[cfg(test)]
mod tests {
    use super::super::transcript::{Activity, Hints, ToolLimit};
    use super::*;

    const FIXTURE: &str = include_str!("../../testdata/transcripts/codex.jsonl");
    /// 2026-10-27T10:00:00Z, the fixture's first entry.
    const T0: f64 = 1_793_095_200.0;

    fn read(lines: usize) -> Activity {
        let all: Vec<&[u8]> = FIXTURE.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all[..lines.min(all.len())], &mut activity);
        activity
    }

    #[test]
    fn calls_carry_their_yield_or_timeout_and_an_unbounded_one_is_untimed() {
        let tool = read(4).tool.expect("exec in flight");
        assert_eq!(tool.name, "exec");
        assert_eq!(tool.command.as_deref(), Some("cargo test --workspace"));
        assert_eq!(tool.limit, ToolLimit::Explicit(30.0));
        assert_eq!(
            read(7).tool.expect("wait").limit,
            ToolLimit::Explicit(600.0)
        );
        let activity = read(usize::MAX);
        let tool = activity.tool.clone().expect("fetch in flight");
        assert_eq!(tool.name, "fetch_page");
        assert!(tool.untimed());
        assert_eq!(tool.started_at, T0 + 646.0);
        assert_eq!(activity.last_output_at, Some(T0 + 646.0));
        assert_eq!(activity.last_event.as_deref(), Some("token_count"));
        assert_eq!(activity.last_event_at, Some(T0 + 700.0));
    }

    #[test]
    fn a_tool_result_that_leaves_nothing_in_flight_waits_on_the_model() {
        let activity = read(5);
        assert!(activity.tool.is_none());
        assert_eq!(activity.request_since, Some(T0 + 34.0));
        assert_eq!(
            read(3).request_since,
            None,
            "reasoning answered the turn's request"
        );
        let mut done = FIXTURE.to_owned();
        done.push_str(
            "{\"timestamp\":\"2026-10-27T10:12:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n",
        );
        let all: Vec<&[u8]> = done.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all, &mut activity);
        assert!(activity.tool.is_none(), "a finished turn holds no call");
        assert_eq!(activity.last_event.as_deref(), Some("turn complete"));
        let aborted = FIXTURE.to_owned()
            + "{\"timestamp\":\"2026-10-27T10:12:00.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\"}}\n";
        let all: Vec<&[u8]> = aborted.lines().map(str::as_bytes).collect();
        let mut activity = Activity::default();
        read_transcript(&all, &mut activity);
        assert!(activity.tool.is_none(), "an interrupted turn holds no call");
        assert_eq!(activity.request_since, None);
        // Streamed reasoning and token counts are progress before the item is written.
        assert_eq!(read(usize::MAX).last_progress_at, Some(T0 + 700.0));
    }

    #[test]
    fn exec_command_without_a_yield_is_bounded_by_the_default_yield() {
        assert_eq!(
            call_limit("exec_command", "", &serde_json::json!({"cmd": "sleep 999"})),
            (
                Some("sleep 999".to_owned()),
                ToolLimit::HarnessDefault(10.0)
            )
        );
        assert_eq!(
            js_string(r#"x({cmd:"a \"b\"\nc"})"#, "cmd").as_deref(),
            Some("a \"b\"\nc")
        );
        assert_eq!(
            js_number("x({timeout_ms: 1_000})", "timeout_ms"),
            Some(1000.0)
        );
    }

    #[test]
    fn the_main_threads_open_rollout_is_found_through_the_process() {
        let home = crate::turn_state::tests::scratch();
        let day = home.join(".codex/sessions/2026/10/27");
        std::fs::create_dir_all(&day).unwrap();
        let main = day.join("rollout-2026-10-27T10-00-00-main-thread.jsonl");
        let child = day.join("rollout-2026-10-27T10-05-00-child-thread.jsonl");
        std::fs::write(&main, FIXTURE).unwrap();
        std::fs::write(
            &child,
            "{\"type\":\"session_meta\",\"payload\":{\"parent_thread_id\":\"main-thread\"}}\n",
        )
        .unwrap();
        let mut proc = crate::work_state::fake::FakeProc::default();
        proc.spawn(100, 1, 'S', "codex")
            .spawn(101, 100, 'S', "codex-worker")
            .open(101, 21, &main.display().to_string(), None)
            .open(101, 22, &child.display().to_string(), None)
            .open(100, 3, "pipe:[1]", None);
        let hints = Hints {
            session: None,
            cwd: None,
            home: Some(&home),
            pid: Some(100),
            proc: Some(&proc),
        };
        assert_eq!(transcript_path(&hints), Some(main.clone()));
        // The record's session id names the child: that one wins.
        let hints = Hints {
            session: Some("child-thread"),
            ..hints
        };
        assert_eq!(transcript_path(&hints), Some(child));
        // Without a live process the session id is looked up in the dated tree.
        let hints = Hints {
            session: Some("main-thread"),
            pid: None,
            proc: None,
            ..hints
        };
        assert_eq!(transcript_path(&hints), Some(main));
        std::fs::remove_dir_all(home).ok();
    }
}
