//! Verified prompt submission for terminal agent composers.
//!
//! A terminal agent accepts a prompt in two separate steps: the text is staged in
//! its composer, and a submission key moves it into the conversation (or into the
//! agent's own queue while it is busy). Sending the key is not proof that the
//! second step happened. An agent can drop a key while it is redrawing or running
//! its own hooks, and the text then stays in the composer indefinitely.
//!
//! [`submit_verified`] drives both steps against the rendered screen and reports
//! one of three outcomes:
//!
//! * nothing was typed ([`Submission::NotStaged`]), so the prompt is safe to retry;
//! * the text left the composer and the screen corroborates a submission
//!   ([`Submission::Verified`]);
//! * an error: either the text is still staged after the deadline (it is left
//!   visible and must never be typed again automatically) or the outcome is
//!   unknown. Callers treat both as possibly submitted.

use std::time::Duration;

use crate::agent::AgentRuntime;
use crate::error::{AdapterError, Result};

/// Harnesses whose composer layout this module can read.
pub const VERIFIED_HARNESSES: [&str; 2] = ["claude", "codex"];
/// Screen rows requested for every composer read.
pub const SCREEN_LINES: usize = 200;
/// Maximum wait for pasted text to appear in the composer before any key is sent.
pub const STAGE_TIMEOUT: Duration = Duration::from_secs(3);
/// Maximum time spent retrying the submission key and waiting for corroboration.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(60);
/// First wait before a still-staged prompt receives another submission key.
pub const FIRST_RETRY: Duration = Duration::from_millis(500);
/// Upper bound on the doubling wait between submission keys.
pub const MAX_RETRY: Duration = Duration::from_secs(4);
/// Screen polling interval while waiting.
pub const POLL: Duration = Duration::from_millis(100);

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";
const QUEUE_MARKERS: [&str; 3] = [
    "Press up to edit queued messages",
    "edit last queued message",
    "Queued follow-up inputs",
];
const WORKING_MARKER: &str = "esc to interrupt";
const CODEX_QUEUE_HINT: &str = "tab to queue message";

/// Positive evidence that a staged prompt left the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionReceipt {
    /// Submission key used for the final press.
    pub key: &'static str,
    /// Number of submission keys sent, including retries.
    pub key_presses: u32,
    /// Time from the paste to the observed evidence.
    pub elapsed: Duration,
    /// The screen observation that established the submission.
    pub evidence: String,
}

/// How a prompt submission was established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// The screen proved that the prompt left the composer.
    Verified(SubmissionReceipt),
    /// The prompt was handed to a primitive that gives no proof; confirm it another way.
    Unconfirmed,
    /// Nothing was typed; the reason explains why the prompt remains pending.
    NotStaged(String),
}

/// Terminal operations used by [`submit_verified`].
pub trait PromptTerminal {
    /// Return visible rows with SGR styling retained.
    fn read_screen(&self, pane_id: &str) -> Result<String>;
    /// Insert literal bytes without a submission key.
    fn send_text(&self, pane_id: &str, text: &str) -> Result<()>;
    /// Send one named key.
    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()>;
}

/// Deadlines for the two phases of one verified submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitTimeouts {
    /// Wait for the pasted text to become visible.
    pub stage: Duration,
    /// Wait for the submission to be proven.
    pub submit: Duration,
}

impl Default for SubmitTimeouts {
    fn default() -> Self {
        Self {
            stage: STAGE_TIMEOUT,
            submit: SUBMIT_TIMEOUT,
        }
    }
}

/// One screen split around the agent's composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerView {
    /// Rows above the composer.
    pub transcript: String,
    /// Composer rows as displayed, marker removed.
    pub composer: String,
    /// Composer rows with faint and reverse-video cells blanked.
    pub composer_solid: String,
    /// Rows below the composer.
    pub footer: String,
}

/// Return whether `text` for `harness` uses the verified path.
///
/// Slash commands open harness menus and confirmation dialogs rather than
/// submitting through the composer, so they keep the native primitive.
pub fn verifies(harness: &str, text: &str) -> bool {
    VERIFIED_HARNESSES.contains(&harness) && !text.trim_start().starts_with('/')
}

/// Return `(plain, solid)` per row; `solid` blanks faint and reverse-video cells.
///
/// Agents draw composer placeholders and the cursor cell in faint or reverse
/// video. Blanking those cells lets an empty composer be told apart from one
/// holding a typed draft without a list of placeholder sentences.
pub fn render_screen(screen: &str) -> Vec<(Vec<char>, Vec<char>)> {
    let mut rows = Vec::new();
    let mut faint = false;
    let mut reverse = false;
    for raw in screen.replace('\r', "").split('\n') {
        let chars: Vec<char> = raw.chars().collect();
        let mut plain = Vec::new();
        let mut solid = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let character = chars[index];
            if character == '\u{1b}' {
                index = skip_escape(&chars, index, &mut faint, &mut reverse);
                continue;
            }
            index += 1;
            if character < ' ' && character != '\t' {
                continue;
            }
            plain.push(character);
            solid.push(if faint || reverse { ' ' } else { character });
        }
        rows.push((plain, solid));
    }
    rows
}

/// Consume one escape sequence starting at `start`, applying SGR attributes.
fn skip_escape(chars: &[char], start: usize, faint: &mut bool, reverse: &mut bool) -> usize {
    let Some(&kind) = chars.get(start + 1) else {
        return start + 1;
    };
    match kind {
        '[' => {
            let mut index = start + 2;
            let parameters_start = index;
            while index < chars.len()
                && matches!(chars[index], '0'..='9' | ';' | ':' | '?' | '<' | '=' | '>')
            {
                index += 1;
            }
            let parameters_end = index;
            while index < chars.len() && (' '..='/').contains(&chars[index]) {
                index += 1;
            }
            match chars.get(index) {
                Some(&last) if ('@'..='~').contains(&last) => {
                    let parameters: String =
                        chars[parameters_start..parameters_end].iter().collect();
                    if last == 'm'
                        && parameters_end == index
                        && parameters
                            .chars()
                            .all(|c| c.is_ascii_digit() || c == ';' || c == ':')
                    {
                        apply_sgr(&parameters, faint, reverse);
                    }
                    index + 1
                }
                _ => start + 1,
            }
        }
        ']' => {
            let mut index = start + 2;
            while index < chars.len() {
                if chars[index] == '\u{7}' {
                    return index + 1;
                }
                if chars[index] == '\u{1b}' {
                    return if chars.get(index + 1) == Some(&'\\') {
                        index + 2
                    } else {
                        start + 1
                    };
                }
                index += 1;
            }
            start + 1
        }
        'P' | 'X' | '^' | '_' => {
            let mut index = start + 2;
            while index < chars.len() {
                if chars[index] == '\u{1b}' {
                    return if chars.get(index + 1) == Some(&'\\') {
                        index + 2
                    } else {
                        start + 1
                    };
                }
                index += 1;
            }
            start + 1
        }
        '@'..='Z' | '\\' => start + 2,
        _ => start + 1,
    }
}

fn apply_sgr(parameters: &str, faint: &mut bool, reverse: &mut bool) {
    let codes: Vec<&str> = if parameters.is_empty() {
        vec!["0"]
    } else {
        parameters
            .split(';')
            .map(|part| part.split(':').next().unwrap_or(""))
            .collect()
    };
    let mut index = 0;
    while index < codes.len() {
        match codes[index] {
            "" | "0" => {
                *faint = false;
                *reverse = false;
            }
            "2" => *faint = true,
            "22" => *faint = false,
            "7" => *reverse = true,
            "27" => *reverse = false,
            "38" | "48" | "58" if index + 1 < codes.len() => {
                // Extended colours carry operands (5;N or 2;R;G;B) that are not attributes.
                index += match codes[index + 1] {
                    "5" => 2,
                    "2" => 4,
                    _ => 0,
                };
            }
            _ => {}
        }
        index += 1;
    }
}

fn text_of(row: &[char]) -> String {
    row.iter().collect()
}

fn is_rule_char(character: char) -> bool {
    matches!(character, '─' | '━' | '═')
}

fn is_rule(line: &[char]) -> bool {
    let text = text_of(line);
    let stripped = text.trim();
    stripped.chars().count() >= 3 && stripped.chars().all(is_rule_char)
}

/// Return whether `line` is a rule, possibly with a label drawn into it.
///
/// Claude can draw session state into the top border of its composer, as in
/// `──────── ultracode ─`. Such a row still starts and ends with rule
/// characters; whatever lies between the two runs is the label.
fn is_labelled_rule(line: &[char]) -> bool {
    let text = text_of(line);
    let stripped: Vec<char> = text.trim().chars().collect();
    let leading = stripped.iter().take_while(|c| is_rule_char(**c)).count();
    let trailing = stripped
        .iter()
        .rev()
        .take_while(|c| is_rule_char(**c))
        .count();
    stripped.len() >= 3 && leading >= 1 && trailing >= 1 && leading + trailing >= 3
}

fn join_rows(rows: &[(Vec<char>, Vec<char>)]) -> String {
    rows.iter()
        .map(|(plain, _)| text_of(plain))
        .collect::<Vec<_>>()
        .join("\n")
}

fn composer_from(body: &[(Vec<char>, Vec<char>)], marker_end: usize) -> (String, String) {
    let mut plain_lines: Vec<String> = body.iter().map(|(plain, _)| text_of(plain)).collect();
    let mut solid_lines: Vec<String> = body.iter().map(|(_, solid)| text_of(solid)).collect();
    plain_lines[0] = text_of(&body[0].0[marker_end.min(body[0].0.len())..]);
    solid_lines[0] = text_of(&body[0].1[marker_end.min(body[0].1.len())..]);
    (plain_lines.join("\n"), solid_lines.join("\n"))
}

fn claude_view(rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
    // A labelled top border is tried only when two plain rules do not already
    // frame a composer, so a draft row that happens to look like a labelled
    // rule cannot change how an unlabelled screen is split.
    claude_view_framed(rows, is_rule).or_else(|| claude_view_framed(rows, is_labelled_rule))
}

/// Split `rows` at the last plain rule and the nearest row above it accepted by `is_top`.
fn claude_view_framed(
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

fn codex_view(rows: &[(Vec<char>, Vec<char>)]) -> Option<ComposerView> {
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
        if plain.starts_with('›') {
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

/// Locate the composer of a supported harness, or `None` when it is not recognisable.
pub fn composer_view(harness: &str, screen: &str) -> Option<ComposerView> {
    let rows = render_screen(screen);
    match harness {
        "claude" => claude_view(&rows),
        "codex" => codex_view(&rows),
        _ => None,
    }
}

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn suffix(text: &str, length: usize) -> String {
    let chars: Vec<char> = compact(text).chars().collect();
    chars[chars.len().saturating_sub(length)..].iter().collect()
}

/// Count `[Pasted text #N` and `[Pasted Content N chars` placeholders.
fn paste_placeholders(text: &str) -> usize {
    let mut count = 0;
    let mut rest = text;
    while let Some(position) = rest.find("[Pasted ") {
        let tail = &rest[position + "[Pasted ".len()..];
        let digits = |value: &str| value.chars().take_while(char::is_ascii_digit).count();
        if let Some(after) = tail.strip_prefix("text #") {
            if digits(after) > 0 {
                count += 1;
            }
        } else if let Some(after) = tail.strip_prefix("Content ") {
            let length = digits(after);
            if length > 0 && after[length..].starts_with(" chars") {
                count += 1;
            }
        }
        rest = tail;
    }
    count
}

fn staged(view: &ComposerView, text: &str, placeholders_before: usize) -> bool {
    // Faint placeholder text is excluded so a short prompt is not "found" inside it.
    let composer = compact(&view.composer_solid);
    let wanted = compact(text);
    if wanted.is_empty() {
        return false;
    }
    composer.contains(&wanted)
        || composer.contains(&suffix(text, 80))
        || paste_placeholders(&view.composer) > placeholders_before
}

fn transcript_count(view: &ComposerView, text: &str) -> usize {
    compact(&view.transcript).matches(&suffix(text, 40)).count()
}

fn corroborated(before: &ComposerView, after: &ComposerView, text: &str) -> Option<String> {
    if transcript_count(after, text) > transcript_count(before, text) {
        return Some("prompt text appeared above the composer".to_owned());
    }
    if paste_placeholders(&after.transcript) > paste_placeholders(&before.transcript) {
        return Some("pasted prompt appeared above the composer".to_owned());
    }
    let screen = format!("{}\n{}", after.transcript, after.footer);
    if let Some(marker) = QUEUE_MARKERS.iter().find(|marker| screen.contains(*marker)) {
        return Some(format!("agent queue marker is visible ({marker:?})"));
    }
    if screen.contains(WORKING_MARKER) {
        return Some("agent reports an active turn".to_owned());
    }
    None
}

fn submit_key(harness: &str, view: &ComposerView) -> &'static str {
    // A busy Codex steers the active turn on Enter and queues on Tab. It shows
    // the Tab hint only while it is busy and holds staged text.
    if harness == "codex" && view.footer.contains(CODEX_QUEUE_HINT) {
        "Tab"
    } else {
        "Enter"
    }
}

/// Stage `text` in an empty composer, submit it, and prove that it left the composer.
///
/// The submission key is repeated with a doubling wait while the exact text is
/// still staged, so a key the agent dropped is retried without typing the
/// prompt twice. A key is never repeated once the text has left the composer.
pub fn submit_verified(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    harness: &str,
    text: &str,
    timeouts: SubmitTimeouts,
    runtime: &dyn AgentRuntime,
) -> Result<Submission> {
    let refuse = |reason: String| Ok(Submission::NotStaged(reason));
    if !VERIFIED_HARNESSES.contains(&harness) {
        return refuse(format!(
            "no composer model for harness {harness:?}; nothing was typed"
        ));
    }
    if text.trim().is_empty() {
        return refuse("prompt text is empty; nothing was typed".to_owned());
    }
    if text.contains(['\u{1b}', '\0']) {
        return refuse(
            "prompt text contains NUL or terminal escape characters; nothing was typed".to_owned(),
        );
    }
    if runtime.cancelled() {
        return refuse(format!(
            "pane {pane_id}: delivery was cancelled before typing"
        ));
    }
    let screen = match terminal.read_screen(pane_id) {
        Ok(screen) => screen,
        Err(error) => {
            return refuse(format!(
                "pane {pane_id}: composer read failed before typing: {error}"
            ))
        }
    };
    let Some(before) = composer_view(harness, &screen) else {
        return refuse(format!(
            "pane {pane_id} does not show a recognisable {harness} composer \
             (a dialog or menu may be open); nothing was typed"
        ));
    };
    let draft = before.composer_solid.trim();
    if !draft.is_empty() {
        let preview: String = draft
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect();
        return refuse(format!(
            "pane {pane_id} composer already holds unsubmitted text {preview:?}; \
             refusing to append to it; nothing was typed"
        ));
    }
    let placeholders_before = paste_placeholders(&before.composer);
    terminal.send_text(
        pane_id,
        &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
    )?;

    let cancelled_after_typing = || {
        Err(AdapterError::unavailable(format!(
            "pane {pane_id}: delivery was cancelled after the prompt was pasted; outcome is unknown"
        )))
    };
    let started = runtime.monotonic();
    let stage_deadline = started.saturating_add(timeouts.stage);
    let view = loop {
        if runtime.cancelled() {
            return cancelled_after_typing();
        }
        if let Some(view) = composer_view(harness, &terminal.read_screen(pane_id)?) {
            if staged(&view, text, placeholders_before) {
                break view;
            }
        }
        if runtime.monotonic() >= stage_deadline {
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: pasted prompt was not observed in the {harness} composer within \
                 {:.1}s; no submission key was sent",
                timeouts.stage.as_secs_f64()
            )));
        }
        runtime.sleep(POLL);
    };

    let mut key = submit_key(harness, &view);
    terminal.send_keys(pane_id, key)?;
    let mut presses: u32 = 1;
    let mut wait = FIRST_RETRY;
    let mut next_press = runtime.monotonic().saturating_add(wait);
    let submit_deadline = runtime.monotonic().saturating_add(timeouts.submit);
    let mut left_composer = false;
    loop {
        runtime.sleep(POLL);
        if runtime.cancelled() {
            return cancelled_after_typing();
        }
        let now = runtime.monotonic();
        let current = composer_view(harness, &terminal.read_screen(pane_id)?);
        match current {
            Some(current) if staged(&current, text, placeholders_before) => {
                left_composer = false;
                if now >= submit_deadline {
                    return Err(AdapterError::unavailable(format!(
                        "pane {pane_id}: prompt is still staged in the {harness} composer after \
                         {presses} {key} key press(es) over {:.1}s; it was NOT submitted and is \
                         left visible so it is never typed twice",
                        now.saturating_sub(started).as_secs_f64()
                    )));
                }
                if now >= next_press {
                    key = submit_key(harness, &current);
                    terminal.send_keys(pane_id, key)?;
                    presses += 1;
                    wait = (wait * 2).min(MAX_RETRY);
                    next_press = now.saturating_add(wait);
                }
                continue;
            }
            Some(current) => {
                left_composer = true;
                if let Some(evidence) = corroborated(&before, &current, text) {
                    return Ok(Submission::Verified(SubmissionReceipt {
                        key,
                        key_presses: presses,
                        elapsed: now.saturating_sub(started),
                        evidence,
                    }));
                }
            }
            None => {}
        }
        if now >= submit_deadline {
            let state = if left_composer {
                "left the composer without visible submission evidence"
            } else {
                "is no longer in a recognisable composer"
            };
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: prompt {state} after {presses} {key} key press(es); outcome is unknown"
            )));
        }
    }
}

/// Scriptable agent composer shared by the submission and queue tests.
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::Mutex;

    use super::{PromptTerminal, BRACKETED_PASTE_END, BRACKETED_PASTE_START};
    use crate::error::Result;

    const RULE: &str = "────────────────────────────────────────";

    fn dim(text: &str) -> String {
        format!("\u{1b}[2m{text}\u{1b}[22m")
    }

    /// Observable and controllable state of one fake agent pane.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct State {
        pub harness: &'static str,
        pub busy: bool,
        pub composer: String,
        pub transcript: Vec<String>,
        pub queued: Vec<String>,
        pub submitted: Vec<String>,
        pub steered: Vec<String>,
        pub pastes: Vec<String>,
        pub keys: Vec<String>,
        pub drop_keys: u32,
        /// Reads that still render the composer empty after a paste; `u32::MAX` never shows it.
        pub paste_visible_after_reads: u32,
        pub redraw_lag_reads: u32,
        pub dialog: bool,
        /// Label Claude draws into the composer's top border, such as a mode name.
        pub top_border_label: Option<String>,
        stale_screen: Option<String>,
        pending_reads: u32,
        paste_counter: u32,
        placeholder: Option<String>,
    }

    /// A terminal agent composer with a controllable key drop and redraw lag.
    #[derive(Debug)]
    pub(crate) struct FakeScreen(pub Mutex<State>);

    impl FakeScreen {
        pub(crate) fn new(harness: &'static str, busy: bool) -> Self {
            Self(Mutex::new(State {
                harness,
                busy,
                transcript: vec!["• earlier output".to_owned()],
                ..State::default()
            }))
        }

        pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().expect("fake screen state")
        }

        pub(crate) fn render(&self) -> String {
            render(&self.state(), false)
        }
    }

    fn render(state: &State, hide_composer: bool) -> String {
        let composer = if hide_composer {
            ""
        } else {
            state.composer.as_str()
        };
        let mut lines = state.transcript.clone();
        if state.dialog {
            lines.extend(["Replace goal?", "  › 1. Replace", "  2. Cancel"].map(str::to_owned));
            return lines.join("\n");
        }
        let composer_lines = |marker: &str, lines: &mut Vec<String>| {
            let mut rows = composer.split('\n');
            lines.push(format!("{marker} {}", rows.next().unwrap_or("")));
            lines.extend(rows.map(|row| format!("  {row}")));
        };
        if state.harness == "claude" {
            if state.busy && !lines.iter().any(|line| line.contains("Working")) {
                lines.push("✻ Working… (running PreToolUse hooks…)".to_owned());
            }
            lines.push(match &state.top_border_label {
                // Coloured as Claude draws it: grey rule, violet label, grey tail.
                Some(label) => format!(
                    "\u{1b}[38;5;244m{RULE}\u{1b}[0m \u{1b}[38;5;147m{label} \u{1b}[38;5;244m─\u{1b}[0m"
                ),
                None => RULE.to_owned(),
            });
            match (&state.placeholder, composer.is_empty()) {
                (Some(placeholder), false) => lines.push(format!("❯ {placeholder}")),
                (None, false) => composer_lines("❯", &mut lines),
                (_, true) if !state.queued.is_empty() => lines.push(format!(
                    "❯ \u{1b}[7mP\u{1b}[0m{}",
                    dim("ress up to edit queued messages")
                )),
                (_, true) => lines.push("❯\u{a0}".to_owned()),
            }
            lines.push(RULE.to_owned());
            let mut footer = "  ⏵⏵ auto mode on".to_owned();
            if state.busy {
                footer.push_str(" · esc to interrupt");
            }
            lines.push(footer);
        } else {
            if composer.is_empty() {
                lines.push(format!("› {}", dim("Ask Codex to do anything")));
            } else {
                composer_lines("›", &mut lines);
            }
            lines.push(String::new());
            lines.push(if state.busy && !composer.is_empty() {
                "  tab to queue message                 99% context".to_owned()
            } else {
                "  GPT default · /tmp/project".to_owned()
            });
        }
        lines.join("\n") + "\n"
    }

    impl PromptTerminal for FakeScreen {
        fn read_screen(&self, _pane_id: &str) -> Result<String> {
            let mut state = self.state();
            if state.pending_reads > 0 {
                if let Some(stale) = state.stale_screen.clone() {
                    state.pending_reads -= 1;
                    return Ok(stale);
                }
            }
            state.stale_screen = None;
            if state.paste_visible_after_reads > 0 {
                if state.paste_visible_after_reads != u32::MAX {
                    state.paste_visible_after_reads -= 1;
                }
                return Ok(render(&state, true));
            }
            Ok(render(&state, false))
        }

        fn send_text(&self, _pane_id: &str, text: &str) -> Result<()> {
            let body = text
                .strip_prefix(BRACKETED_PASTE_START)
                .and_then(|rest| rest.strip_suffix(BRACKETED_PASTE_END))
                .expect("prompt text is sent as one bracketed paste");
            let mut state = self.state();
            state.pastes.push(body.to_owned());
            state.composer.push_str(body);
            let newlines = body.matches('\n').count();
            if state.harness == "claude" && newlines >= 3 {
                state.paste_counter += 1;
                state.placeholder = Some(format!(
                    "[Pasted text #{} +{newlines} lines]",
                    state.paste_counter
                ));
            }
            Ok(())
        }

        fn send_keys(&self, _pane_id: &str, keys: &str) -> Result<()> {
            let mut state = self.state();
            state.keys.push(keys.to_owned());
            if state.drop_keys > 0 {
                state.drop_keys -= 1;
                return Ok(());
            }
            let before = render(&state, false);
            let text = state.composer.clone();
            if text.is_empty() {
                return Ok(());
            }
            match (state.harness, keys) {
                ("claude", "Enter") => {
                    state.transcript.push(format!("❯ {text}"));
                    if state.busy {
                        state.queued.push(text);
                    } else {
                        state.submitted.push(text);
                        state
                            .transcript
                            .push("✻ Working… (running UserPromptSubmit hooks…)".to_owned());
                    }
                    state.busy = true;
                }
                ("codex", "Enter") => {
                    state.transcript.push(format!("› {text}"));
                    if state.busy {
                        state.steered.push(text);
                    } else {
                        state.submitted.push(text);
                        state
                            .transcript
                            .push("• Working (0s • esc to interrupt)".to_owned());
                    }
                    state.busy = true;
                }
                ("codex", "Tab") if state.busy => {
                    state.queued.push(text.clone());
                    state
                        .transcript
                        .push("• Queued follow-up inputs".to_owned());
                    state.transcript.push(format!("  ↳ {text}"));
                }
                _ => return Ok(()),
            }
            state.composer.clear();
            state.placeholder = None;
            if state.redraw_lag_reads > 0 {
                state.stale_screen = Some(before);
                state.pending_reads = state.redraw_lag_reads;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::fake::FakeScreen;
    use super::*;

    #[derive(Default)]
    struct Clock {
        millis: AtomicU64,
        cancel: AtomicBool,
    }

    impl AgentRuntime for Clock {
        fn monotonic(&self) -> Duration {
            Duration::from_millis(1_000_000 + self.millis.load(Ordering::SeqCst))
        }

        fn sleep(&self, duration: Duration) {
            self.millis.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::SeqCst,
            );
        }

        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
    }

    fn submit_with(
        screen: &FakeScreen,
        text: &str,
        timeouts: SubmitTimeouts,
    ) -> (Result<Submission>, Duration) {
        let clock = Clock::default();
        let harness = screen.state().harness;
        let outcome = submit_verified(screen, "w1:p1", harness, text, timeouts, &clock);
        (
            outcome,
            clock.monotonic() - Duration::from_millis(1_000_000),
        )
    }

    fn submit(screen: &FakeScreen, text: &str) -> Result<Submission> {
        submit_with(screen, text, SubmitTimeouts::default()).0
    }

    fn receipt(outcome: Result<Submission>) -> SubmissionReceipt {
        match outcome {
            Ok(Submission::Verified(receipt)) => receipt,
            other => panic!("expected a verified submission, got {other:?}"),
        }
    }

    fn refusal(outcome: Result<Submission>) -> String {
        match outcome {
            Ok(Submission::NotStaged(reason)) => reason,
            other => panic!("expected a refusal before typing, got {other:?}"),
        }
    }

    #[test]
    fn screen_parser_separates_placeholder_from_draft() {
        let claude = FakeScreen::new("claude", false);
        let view = composer_view("claude", &claude.render()).unwrap();
        assert_eq!(view.composer_solid.trim(), "");
        claude.state().queued.push("x".to_owned());
        let view = composer_view("claude", &claude.render()).unwrap();
        assert_eq!(view.composer_solid.trim(), "");
        assert!(view.composer.contains("ress up") && !view.composer_solid.contains("ress up"));
        claude.state().composer = "draft".to_owned();
        let view = composer_view("claude", &claude.render()).unwrap();
        assert_eq!(view.composer_solid.trim(), "draft");

        let codex = FakeScreen::new("codex", false);
        let view = composer_view("codex", &codex.render()).unwrap();
        assert_eq!(view.composer_solid.trim(), "");
        assert!(view.composer.contains("Ask Codex"));
        codex.state().composer = "draft".to_owned();
        let view = composer_view("codex", &codex.render()).unwrap();
        assert_eq!(view.composer_solid.trim(), "draft");
    }

    #[test]
    fn extended_colour_operands_are_not_attributes() {
        // 38;2;2;7;2 is an RGB colour whose operands spell the faint and reverse codes.
        let rows = render_screen("\u{1b}[38;2;2;7;2mA\u{1b}[48;5;2mB\u{1b}[2mC\u{1b}[0mD");
        assert_eq!(text_of(&rows[0].0), "ABCD");
        assert_eq!(text_of(&rows[0].1), "AB D");
    }

    #[test]
    fn slash_commands_and_unknown_harnesses_keep_the_native_path() {
        assert!(verifies("claude", "hello"));
        assert!(verifies("codex", "hello"));
        assert!(!verifies("codex", "  /goal ship it"));
        assert!(!verifies("custom", "hello"));
    }

    #[test]
    fn dropped_submit_key_is_retried_without_retyping() {
        for harness in ["claude", "codex"] {
            let screen = FakeScreen::new(harness, false);
            screen.state().drop_keys = 2;
            let receipt = receipt(submit(&screen, "run the tests"));
            assert_eq!(receipt.key, "Enter");
            assert_eq!(receipt.key_presses, 3, "{harness}");
            let state = screen.state();
            assert_eq!(state.pastes, ["run the tests"], "{harness}");
            assert_eq!(state.keys, ["Enter", "Enter", "Enter"], "{harness}");
            assert_eq!(state.submitted, ["run the tests"], "{harness}");
            assert!(state.composer.is_empty());
        }
    }

    #[test]
    fn already_accepted_prompt_gets_exactly_one_key_despite_redraw_lag() {
        for harness in ["claude", "codex"] {
            let screen = FakeScreen::new(harness, false);
            screen.state().redraw_lag_reads = 3;
            let receipt = receipt(submit(&screen, "only once please"));
            assert_eq!(receipt.key_presses, 1, "{harness}");
            let state = screen.state();
            assert_eq!(state.keys, ["Enter"], "{harness}");
            assert_eq!(state.submitted, ["only once please"], "{harness}");
        }
    }

    #[test]
    fn late_redraw_retry_lands_on_an_empty_composer_without_a_duplicate() {
        // The stale screen outlives the first retry interval, so a second key is
        // sent; it reaches an empty composer and cannot submit anything twice.
        let screen = FakeScreen::new("claude", false);
        screen.state().redraw_lag_reads = 8;
        let receipt = receipt(submit(&screen, "late redraw"));
        assert_eq!(receipt.key_presses, 2);
        let state = screen.state();
        assert_eq!(state.submitted, ["late redraw"]);
        assert_eq!(state.pastes.len(), 1);
        assert_eq!(
            state
                .transcript
                .iter()
                .filter(|line| line.contains("late redraw"))
                .count(),
            1
        );
    }

    #[test]
    fn busy_claude_queues_with_enter() {
        let screen = FakeScreen::new("claude", true);
        let receipt = receipt(submit(&screen, "after this turn"));
        assert_eq!(receipt.key, "Enter");
        assert_eq!(screen.state().queued, ["after this turn"]);
    }

    #[test]
    fn busy_codex_queues_with_tab_and_never_steers() {
        let screen = FakeScreen::new("codex", true);
        screen.state().drop_keys = 1;
        let receipt = receipt(submit(&screen, "follow up later"));
        assert_eq!(receipt.key, "Tab");
        let state = screen.state();
        assert_eq!(state.keys, ["Tab", "Tab"]);
        assert_eq!(state.queued, ["follow up later"]);
        assert!(state.steered.is_empty());
    }

    #[test]
    fn idle_codex_submits_with_enter() {
        let screen = FakeScreen::new("codex", false);
        let receipt = receipt(submit(&screen, "start now"));
        assert_eq!(receipt.key, "Enter");
        assert_eq!(screen.state().submitted, ["start now"]);
    }

    #[test]
    fn prompt_left_staged_times_out_as_not_submitted() {
        let screen = FakeScreen::new("claude", false);
        screen.state().drop_keys = u32::MAX;
        let (outcome, elapsed) = submit_with(
            &screen,
            "never accepted",
            SubmitTimeouts {
                stage: STAGE_TIMEOUT,
                submit: Duration::from_secs(20),
            },
        );
        let message = outcome.unwrap_err().to_string();
        assert!(message.contains("still staged"), "{message}");
        assert!(message.contains("NOT submitted"), "{message}");
        let state = screen.state();
        assert_eq!(state.pastes.len(), 1);
        // Presses at 0, 0.5, 1.5, 3.5, 7.5, 11.5, 15.5 and 19.5 s.
        assert!((5..=8).contains(&state.keys.len()), "{:?}", state.keys);
        assert_eq!(state.composer, "never accepted");
        assert!(elapsed >= Duration::from_secs(20) && elapsed < Duration::from_secs(21));
    }

    #[test]
    fn existing_draft_is_refused_before_typing() {
        let screen = FakeScreen::new("claude", false);
        screen.state().composer = "someone else's words".to_owned();
        let reason = refusal(submit(&screen, "mine"));
        assert!(reason.contains("refusing to append"), "{reason}");
        let state = screen.state();
        assert!(state.pastes.is_empty() && state.keys.is_empty());
    }

    #[test]
    fn open_dialog_is_refused_before_typing() {
        let screen = FakeScreen::new("codex", false);
        screen.state().dialog = true;
        let reason = refusal(submit(&screen, "hello"));
        assert!(reason.contains("recognisable codex composer"), "{reason}");
        assert!(screen.state().pastes.is_empty());
    }

    #[test]
    fn labelled_claude_top_border_still_frames_the_composer() {
        let screen = FakeScreen::new("claude", false);
        screen.state().top_border_label = Some("ultracode".to_owned());
        let view = composer_view("claude", &screen.render())
            .expect("a label in the top border must not hide the composer");
        assert_eq!(view.composer_solid.trim(), "");
        assert!(!view.transcript.contains("ultracode"), "{view:?}");
        assert!(view.footer.contains("auto mode on"), "{view:?}");

        let submitted = receipt(submit(&screen, "reply to the owner"));
        assert_eq!(submitted.key_presses, 1);
        assert_eq!(screen.state().submitted, ["reply to the owner"]);

        let busy = FakeScreen::new("claude", true);
        busy.state().top_border_label = Some("ultracode".to_owned());
        receipt(submit(&busy, "after this turn"));
        assert_eq!(busy.state().queued, ["after this turn"]);

        let drafted = FakeScreen::new("claude", false);
        drafted.state().top_border_label = Some("ultracode".to_owned());
        drafted.state().composer = "someone else's words".to_owned();
        let reason = refusal(submit(&drafted, "mine"));
        assert!(reason.contains("refusing to append"), "{reason}");
        assert!(drafted.state().pastes.is_empty());
    }

    #[test]
    fn labelled_rules_only_frame_a_composer_that_plain_rules_do_not() {
        let rule = "─".repeat(40);
        // Plain rules win: a draft row shaped like a labelled rule stays part of the draft.
        let screen = format!("• earlier\n{rule}\n❯ first line\n  ── notes ──\n{rule}\n  footer\n");
        let view = composer_view("claude", &screen).unwrap();
        assert!(view.composer_solid.contains("── notes ──"), "{view:?}");
        assert_eq!(view.transcript, "• earlier");

        // A labelled rule with no composer marker below it frames nothing.
        let no_composer = format!("{rule} label ─\n  Replace goal?\n{rule}\n");
        assert!(composer_view("claude", &no_composer).is_none());
        // Edge runs of rule characters are required on both sides of a label.
        for row in ["label ───", "─── label", "─ x", "──"] {
            let chars: Vec<char> = row.chars().collect();
            assert!(!is_labelled_rule(&chars), "{row:?}");
        }
        for row in ["─── label ─", "── label ─────", "───"] {
            let chars: Vec<char> = row.chars().collect();
            assert!(is_labelled_rule(&chars), "{row:?}");
        }
    }

    #[test]
    fn escape_characters_empty_text_and_cancellation_are_refused_before_typing() {
        let screen = FakeScreen::new("claude", false);
        assert!(refusal(submit(&screen, "a\u{1b}[31mb")).contains("escape"));
        assert!(refusal(submit(&screen, "  \n")).contains("empty"));
        let clock = Clock::default();
        clock.cancel.store(true, Ordering::SeqCst);
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "x",
            SubmitTimeouts::default(),
            &clock,
        );
        assert!(refusal(outcome).contains("cancelled before typing"));
        assert!(screen.state().pastes.is_empty());
    }

    #[test]
    fn invisible_paste_sends_no_submission_key() {
        let screen = FakeScreen::new("claude", false);
        screen.state().paste_visible_after_reads = u32::MAX;
        let message = submit(&screen, "lost paste").unwrap_err().to_string();
        assert!(message.contains("no submission key was sent"), "{message}");
        assert!(screen.state().keys.is_empty());
    }

    #[test]
    fn slow_paste_is_waited_for_before_the_key() {
        let screen = FakeScreen::new("codex", false);
        screen.state().paste_visible_after_reads = 5;
        let receipt = receipt(submit(&screen, "slow paste"));
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(screen.state().submitted, ["slow paste"]);
    }

    #[test]
    fn long_paste_placeholder_counts_as_staged_and_submitted() {
        let screen = FakeScreen::new("claude", false);
        let text = "line one\nline two\nline three\nline four\nline five";
        let receipt = receipt(submit(&screen, text));
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(screen.state().submitted, [text]);
    }
}
