//! What a running turn looks like from outside: how long it has run, the input waiting behind
//! it, and the agentctl queue backlog (#570 agent-progress-monitoring).
//!
//! A harness that is inside a turn does not take new input the way an idle one does. Codex puts
//! a follow-up typed during a turn in its `Queued follow-up inputs` block and submits that block
//! only when the turn ends; Claude Code holds it as a queued message until its next tool
//! boundary; Muse steers the running turn with Enter but holds a Tab-queued input for the next
//! turn. Each of those counts as "the prompt left the composer", so a send that reports a
//! delivery can still be a message nobody has read. One lane stayed inside a single Codex turn
//! for 46 h 54 m while every message sent to it waited in that block, and `status` said only
//! `working`.
//!
//! This module reads the visible screen for the running-turn row (its elapsed timer and token
//! counter) and for each harness's queued-input block, and reads the agent's agentctl queue for
//! prompts still waiting and for prompts typed since the turn began. Everything here is a pure
//! function of a screen and a queue directory, so tests drive it with captured screens.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json::{json, Value};

/// Bottom screen rows searched for the running-turn row and the queued-input blocks. A Codex
/// queue block of several long inputs sits under the timer row, so this is wider than the
/// prompt window the watcher uses.
const TURN_WINDOW_ROWS: usize = 60;
/// Characters kept of each queued input's first row.
const PREVIEW_CHARS: usize = 100;
/// Queue items read per directory; a backlog past this is reported as at least this many.
const QUEUE_SCAN_LIMIT: usize = 2000;

/// Which kind of queue holds inputs the harness has not submitted yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessQueue {
    /// Codex's `Queued follow-up inputs` (or `Messages to be submitted at end of turn`): submitted
    /// only when the running turn ends.
    CodexEndOfTurn,
    /// Codex's `Messages to be submitted after next tool call`: submitted at the next tool boundary.
    CodexNextToolCall,
    /// Claude Code's queued messages (`Press up to edit queued messages`): taken at its next tool
    /// boundary or when the turn ends.
    ClaudeQueued,
    /// Muse's `Queued input` block: submitted when the turn ends.
    MuseQueued,
}

impl HarnessQueue {
    /// Whether inputs in this queue wait for the whole turn to end, not merely a tool boundary.
    #[must_use]
    pub const fn waits_for_turn_end(self) -> bool {
        matches!(self, Self::CodexEndOfTurn | Self::MuseQueued)
    }

    /// One clause naming the queue and when the harness submits it.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::CodexEndOfTurn => {
                "in Codex's queued follow-up inputs, submitted only when the turn ends"
            }
            Self::CodexNextToolCall => "in Codex's queue for the next tool call",
            Self::ClaudeQueued => {
                "in Claude Code's queued messages, taken at its next tool boundary"
            }
            Self::MuseQueued => "in Muse's queued input, submitted when the turn ends",
        }
    }
}

/// What the visible screen says about the running turn.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ScreenTurn {
    /// A running-turn row is on screen.
    pub running: bool,
    /// Seconds on that row's elapsed timer.
    pub timer_seconds: Option<u64>,
    /// What the timer measures: `turn` (Codex, Claude Code) or `step` (Muse's row times the
    /// current step, not the whole turn).
    pub timer_scope: Option<&'static str>,
    /// The token counter on that row, when it shows one (Claude Code's `↓ 8.4k tokens`).
    pub tokens: Option<u64>,
    /// Inputs the harness holds for later, per the queued-input blocks on screen.
    pub queued: usize,
    /// Whether `queued` is a count of drawn items; Claude Code's hint proves only one or more.
    pub queued_exact: bool,
    /// The first row of each queued input, bounded.
    pub queued_preview: Vec<String>,
    /// Which queue holds them; the one that waits longest when two show.
    pub queue: Option<HarnessQueue>,
}

/// Seconds in an elapsed timer such as `46h 54m 14s`, `10m 30s`, `8s`, or `2d 3h`. Every word
/// must be a number with a `d`, `h`, `m`, or `s` suffix.
#[must_use]
pub fn parse_elapsed(text: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut words = 0;
    for word in text.split_whitespace() {
        let unit = word.chars().last()?;
        let digits = &word[..word.len() - unit.len_utf8()];
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let value: u64 = digits.parse().ok()?;
        let scale = match unit {
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        total = total.checked_add(value.checked_mul(scale)?)?;
        words += 1;
    }
    (words > 0).then_some(total)
}

/// A token count such as `8.4k tokens`, `↓ 812 tokens`, or `↑ 1.2m tokens`.
fn parse_tokens(segment: &str) -> Option<u64> {
    let segment = segment
        .trim()
        .trim_start_matches(['↓', '↑'])
        .trim()
        .strip_suffix("tokens")?
        .trim();
    let (number, scale) = match segment.chars().last()? {
        'k' => (&segment[..segment.len() - 1], 1_000.0),
        'm' => (&segment[..segment.len() - 1], 1_000_000.0),
        _ => (segment, 1.0),
    };
    let value: f64 = number.parse().ok()?;
    (value >= 0.0).then(|| (value * scale).round() as u64)
}

/// Glyphs that open a harness's running-turn row at column 0: Codex's `•`/`◦`, Muse's `◆`/`◇`,
/// and Claude Code's spinner frames.
const RUNNING_GLYPHS: [char; 10] = ['•', '◦', '◆', '◇', '✻', '✽', '✶', '✳', '✢', '*'];

/// The running-turn row's timer and tokens, when `line` is one: a row at column 0 that opens
/// with a running glyph and ends in a parenthesized group whose first segment is an elapsed time
/// and which says `esc to interrupt` (Codex, Muse, older Claude Code) or carries a token counter
/// (Claude Code, whose newer builds put `esc to interrupt` in the footer). Tool output, which is
/// indented, and prose, which has no such group, are not rows.
fn running_row(line: &str) -> Option<(Option<u64>, Option<u64>, &'static str)> {
    let line = line.trim_end();
    let glyph = line.chars().next()?;
    if !RUNNING_GLYPHS.contains(&glyph) {
        return None;
    }
    let rest = line[glyph.len_utf8()..].strip_prefix([' ', '\u{a0}'])?;
    let group = rest.strip_suffix(')')?;
    let open = group.rfind('(')?;
    let segments: Vec<&str> = group[open + 1..].split(['·', '•']).map(str::trim).collect();
    let interrupt = segments.contains(&"esc to interrupt");
    let tokens = segments.iter().find_map(|segment| parse_tokens(segment));
    if !interrupt && tokens.is_none() {
        return None;
    }
    let timer = segments.first().and_then(|segment| parse_elapsed(segment));
    if timer.is_none() && !interrupt {
        return None;
    }
    let scope = if matches!(glyph, '◆' | '◇') {
        "step"
    } else {
        "turn"
    };
    Some((timer, tokens, scope))
}

/// Strip the first row of a queued item to its text and bound it.
fn preview(text: &str) -> String {
    text.trim().chars().take(PREVIEW_CHARS).collect()
}

/// Read the running turn and the queued inputs from the bottom of `screen`.
#[must_use]
pub fn read_screen(screen: &str) -> ScreenTurn {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(TURN_WINDOW_ROWS);
    let window = &lines[start..];
    let mut turn = ScreenTurn::default();
    if let Some((timer, tokens, scope)) = window.iter().rev().find_map(|line| running_row(line)) {
        turn.running = true;
        turn.timer_seconds = timer;
        turn.timer_scope = timer.map(|_| scope);
        turn.tokens = tokens;
    }
    let mut queues: Vec<HarnessQueue> = Vec::new();
    let mut index = 0;
    while index < window.len() {
        let line = window[index].trim_end();
        let heading = line
            .strip_prefix("• ")
            .unwrap_or(line)
            .trim()
            .to_ascii_lowercase();
        let codex = match heading.as_str() {
            "queued follow-up inputs" | "messages to be submitted at end of turn" => {
                Some(HarnessQueue::CodexEndOfTurn)
            }
            "messages to be submitted after next tool call" => {
                Some(HarnessQueue::CodexNextToolCall)
            }
            _ => None,
        };
        if let Some(kind) = codex {
            // Items open with `↳`; their wrapped rows and the `…` that elides the rest are
            // indented under them. The block ends at its edit hint, a blank row, or the composer.
            let mut items = 0;
            index += 1;
            while index < window.len() {
                let row = window[index];
                let trimmed = row.trim();
                if trimmed.is_empty()
                    || trimmed.contains("edit last queued message")
                    || !row.starts_with(' ')
                {
                    break;
                }
                if let Some(text) = trimmed.strip_prefix('↳') {
                    items += 1;
                    turn.queued_preview.push(preview(text));
                }
                index += 1;
            }
            if items > 0 {
                turn.queued += items;
                queues.push(kind);
            }
            continue;
        }
        if line.starts_with("◆ Queued input") {
            // Muse draws each queued input under the heading as a `└` row.
            let mut items = 0;
            index += 1;
            while index < window.len() {
                let trimmed = window[index].trim();
                let Some(text) = trimmed.strip_prefix('└') else {
                    break;
                };
                items += 1;
                turn.queued_preview.push(preview(text));
                index += 1;
            }
            turn.queued += items.max(1);
            queues.push(HarnessQueue::MuseQueued);
            continue;
        }
        index += 1;
    }
    turn.queued_exact = !queues.is_empty();
    // Claude Code shows only a hint in its empty input box while messages wait: one or more.
    if window
        .iter()
        .any(|line| line.contains("Press up to edit queued messages"))
    {
        if turn.queued == 0 {
            turn.queued = 1;
            turn.queued_exact = false;
        }
        queues.push(HarnessQueue::ClaudeQueued);
    }
    turn.queue = queues
        .iter()
        .copied()
        .find(|queue| queue.waits_for_turn_end())
        .or_else(|| queues.first().copied());
    turn
}

/// Whether the screen shows a turn running: a running-turn row, or `esc to interrupt` as a
/// `·`-separated or parenthesized segment of a row near the bottom (Claude Code's footer). Used
/// where Herdr's own status cannot be trusted.
#[must_use]
pub fn screen_running(screen: &str) -> bool {
    if read_screen(screen).running {
        return true;
    }
    let lines: Vec<&str> = screen.lines().collect();
    lines[lines.len().saturating_sub(12)..].iter().any(|line| {
        line.split(['·', '(', ')'])
            .any(|segment| segment.trim() == "esc to interrupt")
    })
}

/// A screen with the running-turn row's timer masked, for telling a screen that changed from
/// one whose only change is the clock. The token counter is kept: tokens moving is the model
/// streaming, which is progress.
#[must_use]
pub fn without_timer(screen: &str) -> String {
    let mut out = String::with_capacity(screen.len());
    for line in screen.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if running_row(body).is_some() {
            if let Some(open) = body.rfind('(') {
                let group = &body[open + 1..body.len() - 1];
                let mut segments = group.splitn(2, ['·', '•']);
                let first = segments.next().unwrap_or_default();
                if parse_elapsed(first.trim()).is_some() {
                    out.push_str(&body[..=open]);
                    out.push_str(&first[..first.len() - first.trim_start().len()]);
                    out.push_str("<elapsed>");
                    out.push_str(&first[first.trim_end().len()..]);
                    out.push_str(&group[first.len()..]);
                    out.push(')');
                    out.push_str(&line[body.len()..]);
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

/// Composer placeholders and hints that are not a draft.
const COMPOSER_PLACEHOLDERS: [&str; 3] = [
    "Ask Codex to do anything",
    "Press up to edit queued messages",
    "Type your message",
];

/// The text waiting in the harness's input box, when it holds a draft: the last row that opens
/// with a composer marker (`»` or `›` for Codex, `❯` for Claude Code and Muse) and its indented
/// continuation rows. After Escape interrupts a Codex turn, Codex moves its queued follow-up
/// inputs into this box, where Enter submits them all.
#[must_use]
pub fn composer_draft(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(TURN_WINDOW_ROWS);
    let index = start
        + lines[start..]
            .iter()
            .rposition(|line| line.starts_with(['»', '›', '❯']))?;
    let first = lines[index]
        .trim_start_matches(['»', '›', '❯'])
        .trim_matches([' ', '\u{a0}']);
    let mut text = vec![first.to_owned()];
    for line in &lines[index + 1..] {
        if line.trim().is_empty() || !line.starts_with(' ') || line.trim().starts_with('─') {
            break;
        }
        text.push(line.trim().to_owned());
    }
    let text = text.join("\n").trim().to_owned();
    (!text.is_empty()
        && !COMPOSER_PLACEHOLDERS
            .iter()
            .any(|hint| text.starts_with(hint)))
    .then_some(text)
}

/// The agentctl queue's view of the same agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Backlog {
    /// Prompts not yet typed (`inbox/`, `inbox.v2/`) or crossing the injection barrier (`inflight/`).
    pub waiting: usize,
    /// Seconds since the oldest of them was queued.
    pub oldest_waiting_seconds: Option<f64>,
    /// Prompts agentctl typed into the pane that were queued after the running turn began: input
    /// the harness took while busy.
    pub sent_during_turn: usize,
    /// Seconds since the oldest of those was queued.
    pub oldest_sent_during_turn_seconds: Option<f64>,
}

fn queued_at(path: &Path) -> Option<f64> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["queued_at"].as_f64()
}

fn json_files(directory: &Path) -> Vec<(std::path::PathBuf, Option<f64>)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .take(QUEUE_SCAN_LIMIT)
        .map(|entry| {
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|age| age.as_secs_f64());
            (entry.path(), modified)
        })
        .collect()
}

/// Read the agentctl queue at `queue` as of `now`; `turn_start` (Unix seconds) selects the
/// processed prompts queued during the running turn. A queue that does not exist is empty.
#[must_use]
pub fn backlog(queue: &Path, now: f64, turn_start: Option<f64>) -> Backlog {
    let mut result = Backlog::default();
    for lane in ["inbox", "inbox.v2", "inflight"] {
        for (path, modified) in json_files(&queue.join(lane)) {
            result.waiting += 1;
            if let Some(at) = queued_at(&path).or(modified) {
                let age = (now - at).max(0.0);
                result.oldest_waiting_seconds = Some(
                    result
                        .oldest_waiting_seconds
                        .map_or(age, |old| old.max(age)),
                );
            }
        }
    }
    if let Some(start) = turn_start {
        for (path, modified) in json_files(&queue.join("processed")) {
            // A prompt file is written once, when it is queued, and only renamed after that,
            // so an older file cannot hold a prompt queued during this turn.
            if modified.is_some_and(|modified| modified + 1.0 < start) {
                continue;
            }
            let Some(at) = queued_at(&path) else {
                continue;
            };
            if at >= start {
                result.sent_during_turn += 1;
                let age = (now - at).max(0.0);
                result.oldest_sent_during_turn_seconds = Some(
                    result
                        .oldest_sent_during_turn_seconds
                        .map_or(age, |old| old.max(age)),
                );
            }
        }
    }
    result
}

/// An age for a one-line notice: `46 h 54 m`, `2 h 5 m`, `40 min`, `45 s`.
#[must_use]
pub fn format_age(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds >= 3_600 {
        format!("{} h {} m", seconds / 3_600, seconds % 3_600 / 60)
    } else if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// The turn's age in seconds: the screen's turn timer when it shows one, else `fallback` (such
/// as how long a watcher has seen the agent working).
#[must_use]
pub fn turn_age(screen: &ScreenTurn, fallback: Option<f64>) -> Option<f64> {
    match (screen.timer_scope, screen.timer_seconds) {
        (Some("turn"), Some(seconds)) => Some(seconds as f64),
        _ => fallback,
    }
}

/// How many messages sent to the agent are known not to have reached the model: the queued
/// items on screen, or, for a queue that waits for the turn to end, every prompt agentctl typed
/// during the turn when that is more (Codex merges consecutive inputs into one item).
#[must_use]
pub fn undelivered(screen: &ScreenTurn, backlog: &Backlog) -> usize {
    let typed = if screen.queue.is_some_and(HarnessQueue::waits_for_turn_end) {
        backlog.sent_during_turn
    } else {
        0
    };
    screen.queued.max(typed) + backlog.waiting
}

/// One line saying what holds the agent's input, or `None` when nothing waits.
#[must_use]
pub fn summary(screen: &ScreenTurn, backlog: &Backlog, age: Option<f64>) -> Option<String> {
    if screen.queued == 0 && backlog.waiting == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(age) = age.filter(|_| screen.running) {
        parts.push(format!("inside one turn for {}", format_age(age)));
    }
    if screen.queued > 0 {
        let count = if screen.queued_exact {
            screen.queued.to_string()
        } else {
            format!("{}+", screen.queued)
        };
        let place = screen
            .queue
            .map_or("in the harness", HarnessQueue::describe);
        parts.push(format!(
            "{count} input{} queued {place}",
            if screen.queued == 1 && screen.queued_exact {
                ""
            } else {
                "s"
            }
        ));
        if screen.queue.is_some_and(HarnessQueue::waits_for_turn_end)
            && backlog.sent_during_turn > 0
        {
            parts.push(format!(
                "{} message{} typed during this turn, the oldest {} ago",
                backlog.sent_during_turn,
                if backlog.sent_during_turn == 1 {
                    ""
                } else {
                    "s"
                },
                format_age(backlog.oldest_sent_during_turn_seconds.unwrap_or(0.0))
            ));
        }
    }
    if backlog.waiting > 0 {
        parts.push(format!(
            "{} prompt{} waiting in agentctl's queue, the oldest {} old",
            backlog.waiting,
            if backlog.waiting == 1 { "" } else { "s" },
            format_age(backlog.oldest_waiting_seconds.unwrap_or(0.0))
        ));
    }
    Some(parts.join("; "))
}

/// The `turn` object `agentctl status` and `list` add for an interactive agent, or `None` when
/// the screen shows no running turn and nothing waits.
#[must_use]
pub fn turn_document(screen: &ScreenTurn, backlog: &Backlog) -> Option<Value> {
    if !screen.running && screen.queued == 0 && backlog.waiting == 0 {
        return None;
    }
    let age = turn_age(screen, None);
    let oldest_queued =
        if screen.queued > 0 && screen.queue.is_some_and(HarnessQueue::waits_for_turn_end) {
            backlog.oldest_sent_during_turn_seconds
        } else {
            None
        };
    Some(json!({
        "running": screen.running,
        "age_seconds": age,
        "timer_seconds": screen.timer_seconds,
        "timer_scope": screen.timer_scope,
        "tokens": screen.tokens,
        "harness_queued": screen.queued,
        "harness_queued_exact": screen.queued_exact,
        "harness_queue": screen.queue,
        "harness_queued_oldest_seconds": oldest_queued,
        "harness_queued_preview": screen.queued_preview,
        "sent_during_turn": backlog.sent_during_turn,
        "agentctl_waiting": backlog.waiting,
        "agentctl_oldest_waiting_seconds": backlog.oldest_waiting_seconds,
        "undelivered": undelivered(screen, backlog),
        "summary": summary(screen, backlog, age),
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Captured Codex screens, redacted; see testdata/harness-turns/README.md.
    const CODEX_LONG: &str = include_str!("../testdata/harness-turns/codex-long-turn-queued.txt");
    const CODEX_QUEUED: &str = include_str!("../testdata/harness-turns/codex-working-queued.txt");
    const CODEX_INTERRUPTED: &str =
        include_str!("../testdata/harness-turns/codex-interrupted-composer.txt");
    const CODEX_AFTER: &str = include_str!("../testdata/harness-turns/codex-after-interrupt.txt");
    const CLAUDE_QUEUED: &str = include_str!("../testdata/harness-turns/claude-working-queued.txt");
    const MUSE_QUEUED: &str = include_str!("../testdata/muse-busy/muse-working-queued.txt");
    const MUSE_THINKING: &str = include_str!("../testdata/muse-busy/muse-working-thinking.txt");

    #[test]
    fn elapsed_timers_parse_and_anything_else_does_not() {
        assert_eq!(parse_elapsed("46h 54m 14s"), Some(168_854));
        assert_eq!(parse_elapsed("10m 30s"), Some(630));
        assert_eq!(parse_elapsed("8s"), Some(8));
        assert_eq!(parse_elapsed("2d 3h"), Some(183_600));
        for text in ["", "esc to interrupt", "8", "s", "1.5m", "10m thirty"] {
            assert_eq!(parse_elapsed(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_codex_turn_of_46_hours_with_queued_inputs_is_read_from_its_screen() {
        let turn = read_screen(CODEX_LONG);
        assert!(turn.running);
        assert_eq!(turn.timer_seconds, Some(46 * 3600 + 54 * 60 + 14));
        assert_eq!(turn.timer_scope, Some("turn"));
        assert_eq!(turn.queued, 4);
        assert!(turn.queued_exact);
        assert_eq!(turn.queue, Some(HarnessQueue::CodexEndOfTurn));
        assert!(turn.queued_preview[1].starts_with("coordinator: PHASE 2"));
        let backlog = Backlog::default();
        assert_eq!(
            summary(&turn, &backlog, turn_age(&turn, None)).unwrap(),
            "inside one turn for 46 h 54 m; 4 inputs queued in Codex's queued follow-up inputs, submitted only when the turn ends"
        );
    }

    #[test]
    fn wrapped_and_elided_codex_items_count_once_each() {
        let turn = read_screen(CODEX_QUEUED);
        assert_eq!(turn.timer_seconds, Some(2 * 3600 + 40 * 60 + 18));
        assert_eq!(turn.queued, 2);
        assert!(turn.queued_preview[0].starts_with("coordinator DISK, urgent"));
    }

    #[test]
    fn after_the_interrupt_the_queue_is_gone_and_the_new_turn_is_young() {
        let turn = read_screen(CODEX_INTERRUPTED);
        assert!(!turn.running, "Escape ended the turn");
        assert_eq!(turn.queued, 0, "the queued inputs moved into the composer");
        let turn = read_screen(CODEX_AFTER);
        assert_eq!(turn.timer_seconds, Some(630));
        assert_eq!(turn.queued, 0);
        assert_eq!(
            turn_document(&turn, &Backlog::default()).unwrap()["summary"],
            Value::Null
        );
    }

    #[test]
    fn the_restored_queue_is_the_composer_draft_after_an_interrupt() {
        let draft = composer_draft(CODEX_INTERRUPTED).unwrap();
        assert!(draft.starts_with("coordinator: answered"), "{draft}");
        assert_eq!(draft.lines().count(), 3);
        for screen in [
            CODEX_AFTER,
            CODEX_LONG,
            CODEX_QUEUED,
            CLAUDE_QUEUED,
            MUSE_QUEUED,
        ] {
            assert_eq!(composer_draft(screen), None, "{screen}");
        }
        assert_eq!(
            composer_draft("❯ fix the flake\n────\n"),
            Some("fix the flake".to_owned())
        );
    }

    #[test]
    fn claude_codes_spinner_and_queued_hint_are_read() {
        let turn = read_screen(CLAUDE_QUEUED);
        assert!(turn.running);
        assert_eq!(turn.timer_seconds, Some(12 * 60 + 4));
        assert_eq!(turn.tokens, Some(8_400));
        assert_eq!(turn.queued, 1);
        assert!(
            !turn.queued_exact,
            "the hint proves one or more, not how many"
        );
        assert_eq!(turn.queue, Some(HarnessQueue::ClaudeQueued));
        let line = summary(&turn, &Backlog::default(), turn_age(&turn, None)).unwrap();
        assert!(
            line.contains("1+ inputs queued in Claude Code's queued messages"),
            "{line}"
        );
    }

    #[test]
    fn muse_times_a_step_not_the_turn_and_its_queued_input_counts() {
        let turn = read_screen(MUSE_QUEUED);
        assert!(turn.running);
        assert_eq!(turn.timer_seconds, Some(55));
        assert_eq!(turn.timer_scope, Some("step"));
        assert_eq!(
            turn_age(&turn, None),
            None,
            "a step timer is not a turn age"
        );
        assert_eq!(turn_age(&turn, Some(900.0)), Some(900.0));
        assert_eq!(turn.queued, 1);
        assert_eq!(turn.queue, Some(HarnessQueue::MuseQueued));
        let thinking = read_screen(MUSE_THINKING);
        assert!(thinking.running);
        assert_eq!(thinking.queued, 0);
    }

    #[test]
    fn quoted_rows_and_prose_are_not_a_running_turn() {
        let screen = "  • Working (3h 1m 2s • esc to interrupt)\nwe saw Working (5s • esc to interrupt) earlier\n• Ran cargo test (exit 1)\n";
        assert_eq!(read_screen(screen), ScreenTurn::default());
    }

    #[test]
    fn masking_the_timer_leaves_every_other_change_visible() {
        let before = "• Ran build\n\n• Working (2h 40m 18s • esc to interrupt)\n";
        let later = "• Ran build\n\n• Working (2h 41m 18s • esc to interrupt)\n";
        assert_eq!(without_timer(before), without_timer(later));
        assert!(without_timer(before).contains("• Working (<elapsed> • esc to interrupt)"));
        let claude = "✻ Sautéing… (12m 4s · ↓ 8.4k tokens · esc to interrupt)";
        let streamed = "✻ Sautéing… (12m 9s · ↓ 8.9k tokens · esc to interrupt)";
        assert_ne!(
            without_timer(claude),
            without_timer(streamed),
            "tokens moved"
        );
        assert_eq!(
            without_timer(claude),
            without_timer("✻ Sautéing… (13m 0s · ↓ 8.4k tokens · esc to interrupt)")
        );
        assert_eq!(without_timer("plain\nscreen"), "plain\nscreen");
    }

    fn write_item(directory: &Path, id: &str, queued_at: f64) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join(format!("{id}.json")),
            json!({"id": id, "text": "x", "queued_at": queued_at}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn the_backlog_counts_waiting_prompts_and_those_typed_during_the_turn() {
        let root = scratch();
        let queue = root.as_path();
        let now = UNIX_EPOCH.elapsed().unwrap().as_secs_f64();
        let start = now - 46.0 * 3600.0;
        write_item(&queue.join("processed"), "before", start - 600.0);
        write_item(&queue.join("processed"), "during-1", start + 3600.0);
        write_item(&queue.join("processed"), "during-2", now - 60.0);
        write_item(&queue.join("inbox"), "waiting", now - 300.0);
        let backlog = backlog(queue, now, Some(start));
        assert_eq!(backlog.waiting, 1);
        assert!((backlog.oldest_waiting_seconds.unwrap() - 300.0).abs() < 1.0);
        assert_eq!(backlog.sent_during_turn, 2);
        assert!((backlog.oldest_sent_during_turn_seconds.unwrap() - 45.0 * 3600.0).abs() < 1.0);
        let turn = read_screen(CODEX_QUEUED);
        assert_eq!(
            undelivered(&turn, &backlog),
            3,
            "two typed this turn plus one waiting"
        );
        let line = summary(&turn, &backlog, Some(46.0 * 3600.0)).unwrap();
        assert!(
            line.contains("2 messages typed during this turn, the oldest 45 h 0 m ago"),
            "{line}"
        );
        assert!(line.ends_with("1 prompt waiting in agentctl's queue, the oldest 5 min old"));
        let document = turn_document(&turn, &backlog).unwrap();
        assert_eq!(document["harness_queued"], 2);
        assert_eq!(document["undelivered"], 3);
        assert!(backlog_missing_is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    /// A scratch directory unique across threads and runs.
    pub(crate) fn scratch() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = UNIX_EPOCH.elapsed().map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "agentctl-turn-state-{}-{nanos}-{index}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn backlog_missing_is_empty() -> bool {
        backlog(Path::new("/nonexistent/agentctl-queue"), 0.0, Some(0.0)) == Backlog::default()
    }

    #[test]
    fn ages_read_at_a_glance() {
        assert_eq!(format_age(168_854.0), "46 h 54 m");
        assert_eq!(format_age(2_400.0), "40 min");
        assert_eq!(format_age(45.4), "45 s");
        assert_eq!(format_age(-3.0), "0 s");
    }
}
