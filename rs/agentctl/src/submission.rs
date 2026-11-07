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

/// Whether this module can read `harness`'s composer layout.
pub(crate) fn verified(harness: &str) -> bool {
    crate::harness::adapter(harness).is_some_and(|adapter| adapter.verified_submission())
}
/// Screen rows requested for every composer read.
pub const SCREEN_LINES: usize = 200;
/// Maximum wait for pasted text to appear in the composer before any key is sent.
///
/// The wait ends at the first read that shows the paste, about 0.1 s after it in the
/// usual case. It is long because giving up leaves the paste unsubmitted in the
/// composer, where the draft check then holds every later prompt, and a paste typed
/// into a busy agent's pane has gone unseen for 3 s and then been found in its composer.
/// A paste that shows late or never costs the rest of the wait: the queue drain that
/// typed it keeps its queue and pane locks meanwhile, so other agentctl senders to the
/// pane wait too, and any new paste placeholder drawn in that time, even another
/// sender's, counts as this paste.
pub const STAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// Extra bounded wait, after [`STAGE_TIMEOUT`], for a paste drawn late on a
/// busy pane (poll slows to [`STAGE_POLL_SLOW`]). Total stage bound is
/// STAGE_TIMEOUT + STAGE_GRACE (5 min). Never strands our own paste early.
pub const STAGE_GRACE: Duration = Duration::from_secs(240);
/// Poll interval during the late-paste grace.
pub const STAGE_POLL_SLOW: Duration = Duration::from_secs(2);
/// How long a queue drain waits, holding the pane's locks, for its paste to show (#884
/// pane-lock-held-on-unobserved-paste). Every other sender to the pane waits behind those
/// locks, so a paste not drawn by then is reported stranded and the locks are released; a
/// later pass presses Enter on it once it shows ([`STAGE_TIMEOUT`] plus [`STAGE_GRACE`] after
/// it was typed), or types the prompt again once that window has passed with no sign of it.
pub const STAGE_IN_LOCK: Duration = Duration::from_secs(10);

/// The deadlines of a verified submission made while a queue drain holds the pane's locks:
/// [`STAGE_IN_LOCK`] for the paste, and no late-paste grace under the locks.
#[must_use]
pub fn in_lock_timeouts() -> SubmitTimeouts {
    SubmitTimeouts {
        stage: STAGE_IN_LOCK,
        stage_grace: Duration::ZERO,
        ..SubmitTimeouts::default()
    }
}

/// The start of the refusal a stranded retry gives once the late-paste window passed with no
/// sign of the earlier paste: it was lost. The prompt is never typed again; the drain keeps
/// it stranded and reports it possibly submitted only after a bounded number of such
/// passes (#884 item 4).
pub const LATE_PASTE_LOST: &str = "late_paste_lost";

/// Whether `detail` is the refusal of a stranded retry whose earlier paste was lost.
#[must_use]
pub fn is_late_paste_lost(detail: &str) -> bool {
    detail.starts_with(&format!("{LATE_PASTE_LOST}:"))
}
/// Maximum time spent retrying the submission key and waiting for corroboration.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum time spent waiting, before typing anything, for a recognisable composer.
///
/// Herdr can report a freshly launched harness ready before it has drawn its composer.
/// Nothing is typed while waiting, so the wait cannot duplicate a prompt.
pub const COMPOSER_WAIT: Duration = Duration::from_secs(15);
/// First wait before a still-staged prompt receives another submission key.
pub const FIRST_RETRY: Duration = Duration::from_millis(500);
/// Upper bound on the doubling wait between submission keys.
pub const MAX_RETRY: Duration = Duration::from_secs(4);
/// Screen polling interval while waiting.
pub const POLL: Duration = Duration::from_millis(100);
/// Longest extra wait for printed evidence once weaker evidence has proved a submission.
///
/// A busy agent shows its running-turn marker before the key and after it, so that marker proves
/// only that the prompt left the composer. The screen is then read every [`POLL`] for at most
/// this long, without sending another key, for something the pane printed after the key. The
/// submission deadline does not shorten this wait.
pub const PRINT_GRACE: Duration = Duration::from_secs(2);

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";
// Each harness's queued-input hints, in one list (#574 harness-adapters).
use crate::harness::queue_hints as queue_markers;
use crate::harness::WORKING_MARKER;
/// Rows, ending at the last row with text, that `running_turn` searches for a marker.
pub(crate) const RUNNING_TURN_ROWS: usize = 16;
use crate::harness::claude::PASTE_HINT;
use crate::harness::codex::pending_copies as codex_pending_copies;
#[cfg(test)]
use crate::harness::codex::{
    idle_composer as codex_idle_composer, pending_previews as codex_pending_previews,
    timer_row as codex_timer_row, QUEUE_HINT as CODEX_QUEUE_HINT,
};
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
    /// Whether `evidence` is something the pane printed after the submission key: the prompt
    /// or its paste placeholder above the composer, Codex's preview of it among the messages it
    /// holds for a running turn, or a queued-message or running-turn marker
    /// that no screen read before the key showed. A running-turn marker that was already
    /// showing, as it always is on a busy agent, proves only that the prompt left the composer.
    pub printed: bool,
    /// Whether the evidence held: the wait for printed evidence ran its full course and its last
    /// read showed a recognisable composer without the prompt, or printed evidence was found.
    /// False when the wait ended early, because the caller stopped or a read failed, or ended on
    /// a screen with no recognisable composer: then nothing showed that the prompt did not come
    /// back into the composer after a key the agent dropped.
    pub settled: bool,
    /// Whether the agent took the prompt into its own queue behind a running
    /// turn instead of starting a turn with it: the pane was already busy and
    /// the prompt left the composer, or a queued-message marker is the
    /// evidence. A queued prompt was delivered; it is not a submission that
    /// may or may not have happened (#884 item 4 (B)).
    pub queued: bool,
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

/// Distinct refusal kind for a pane whose terminal grid stays blank while the
/// harness keeps working: the composer exists but cannot be read, so nothing
/// may be typed. Recorded at the start of the `delivery_error` detail and
/// classified by [`is_composer_unreadable`]. The prompt stays pending (exit 75);
/// it is never a `possibly_submitted` outcome (exit 76).
pub const COMPOSER_UNREADABLE_KIND: &str = "composer_unreadable";

/// The named state behind every [`COMPOSER_UNREADABLE_KIND`] refusal: the
/// harness is alive in its pane but its terminal grid is empty. Long-running
/// Claude Code processes were seen in it with their renderer silent (no frame
/// written to the terminal, even after a resize) while the agent loop and the
/// OSC title kept working. Every one had its binary replaced on disk since it
/// started, though most such processes keep drawing, so the replacement is a
/// marker rather than the cause. No resize or redraw restores the state; a
/// child-first restart with `--resume` does (#836 herdr-blank-grid-claude).
pub const BLANK_GRID_STATE: &str = "blank_grid";

/// Whether a `/proc/PID/exe` link target names an executable that was
/// replaced or removed on disk after the process started.
#[must_use]
pub fn exe_link_replaced(target: &str) -> bool {
    target.ends_with(" (deleted)")
}

/// Herdr facts about one pane, probed without typing anything.
///
/// `working` is Herdr's agent status (for example from the OSC title) rather
/// than anything the blank grid shows. `interactive_ready` means the pane
/// still holds an interactive terminal. `foreground_matches` means the pane's
/// foreground process group leader is still the pinned harness process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComposerProbe {
    /// Herdr reports the agent `working`.
    pub working: bool,
    /// The pane still holds an interactive terminal.
    pub interactive_ready: bool,
    /// The foreground pid still matches the pinned harness identity.
    pub foreground_matches: bool,
    /// `scroll.offset_from_bottom` when Herdr reports geometry.
    pub scroll_offset_from_bottom: Option<u64>,
    /// A foreground harness process runs an executable that was replaced on
    /// disk after it started ([`exe_link_replaced`]).
    pub harness_binary_replaced: bool,
}

/// Whether `screen`, once rendered, is blank: every cell whitespace.
///
/// Escape sequences, faint/placeholder styling and the reverse-video cursor
/// do not count as content, matching how [`render_screen`] reads a composer.
#[must_use]
pub fn is_ansi_blank(screen: &str) -> bool {
    render_screen(screen)
        .iter()
        .all(|(plain, _)| plain.iter().all(|c| c.is_whitespace()))
}

/// Whether `detail` reports the [`BLANK_GRID_STATE`].
#[must_use]
pub fn is_blank_grid(detail: &str) -> bool {
    is_composer_unreadable(detail) && detail.contains(&format!(": {BLANK_GRID_STATE}:"))
}

/// Whether `detail` is a [`COMPOSER_UNREADABLE_KIND`] refusal.
#[must_use]
pub fn is_composer_unreadable(detail: &str) -> bool {
    detail.starts_with(COMPOSER_UNREADABLE_KIND)
}

/// The `composer_unreadable` refusal for `pane_id`, or `None`.
///
/// Some only when the rendered screen is ansi-blank and `probe` proves the
/// harness is still there (`interactive_ready` and a matching foreground pid)
/// and either working or running a binary replaced on disk since it started
/// (an idle harness in the [`BLANK_GRID_STATE`] shows no title spinner, but
/// the replaced binary marks it). Every other blank or unreadable screen
/// keeps the generic refusal, so a genuinely absent composer is never
/// misclassified. The detail names the [`BLANK_GRID_STATE`], the blank,
/// scroll and binary facts, and the cure, for `delivery_error`.
pub fn composer_unreadable_reason(
    pane_id: &str,
    harness: &str,
    screen: &str,
    probe: Option<ComposerProbe>,
    waited: Duration,
) -> Option<String> {
    let probe = probe?;
    if !(probe.interactive_ready
        && probe.foreground_matches
        && (probe.working || probe.harness_binary_replaced))
    {
        return None;
    }
    if !is_ansi_blank(screen) {
        return None;
    }
    let scroll = probe
        .scroll_offset_from_bottom
        .map_or_else(|| "unknown".to_owned(), |offset| offset.to_string());
    Some(format!(
        "{COMPOSER_UNREADABLE_KIND}: {BLANK_GRID_STATE}: pane {pane_id} does not show a \
         recognisable {harness} composer after {:.1}s; ansi_blank=true \
         scroll_offset_from_bottom={scroll} working={} interactive_ready=true \
         foreground_match=true harness_binary_replaced={}; a resize or redraw does not \
         restore this state: if it persists, restart the harness child-first with --resume; \
         nothing was typed",
        waited.as_secs_f64(),
        probe.working,
        probe.harness_binary_replaced,
    ))
}

/// Terminal operations used by [`submit_verified`].
pub trait PromptTerminal {
    /// Return visible rows with SGR styling retained.
    fn read_screen(&self, pane_id: &str) -> Result<String>;
    /// Insert literal bytes without a submission key.
    fn send_text(&self, pane_id: &str, text: &str) -> Result<()>;
    /// Send one named key.
    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()>;
    /// Probe Herdr facts for the composer-unreadable classification, or `None`
    /// when this terminal cannot prove them (the generic refusal then stands).
    /// Implementations must not type anything.
    fn composer_probe(&self, _pane_id: &str, _harness: &str) -> Option<ComposerProbe> {
        None
    }
}

/// A prompt terminal that runs each input effect between recipient checks.
///
/// Every write, including each submission-key retry inside [`submit_verified`],
/// goes through one of these methods, so the implementation can prove the pane
/// still holds the intended program immediately before and after it.
pub trait GuardedInput {
    /// Return visible rows with SGR styling retained.
    fn read_screen(&self, pane_id: &str) -> Result<String>;
    /// Insert literal bytes without a submission key.
    fn send_text(&self, pane_id: &str, text: &str) -> Result<()>;
    /// Send one named key.
    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()>;
    /// Submit text plus Enter through Herdr's native prompt primitive.
    fn native_prompt(&self, pane_id: &str, text: &str) -> Result<()>;
    /// Probe Herdr facts for the composer-unreadable classification, or `None`
    /// when this input path cannot prove them. Must not type anything.
    fn composer_probe(&self, _pane_id: &str, _harness: &str) -> Option<ComposerProbe> {
        None
    }
}

/// The [`PromptTerminal`] view of a [`GuardedInput`], for [`submit_verified`].
pub struct GuardedPrompt<'a>(pub &'a dyn GuardedInput);

impl PromptTerminal for GuardedPrompt<'_> {
    fn read_screen(&self, pane_id: &str) -> Result<String> {
        self.0.read_screen(pane_id)
    }
    fn send_text(&self, pane_id: &str, text: &str) -> Result<()> {
        self.0.send_text(pane_id, text)
    }
    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
        self.0.send_keys(pane_id, keys)
    }
    fn composer_probe(&self, pane_id: &str, harness: &str) -> Option<ComposerProbe> {
        self.0.composer_probe(pane_id, harness)
    }
}

/// Deadlines for the two phases of one verified submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitTimeouts {
    /// Wait for the pasted text to become visible.
    pub stage: Duration,
    /// Extra late-paste grace after `stage` (see [`STAGE_GRACE`]).
    pub stage_grace: Duration,
    /// Wait for the submission to be proven.
    pub submit: Duration,
    /// Wait, before typing, for a recognisable composer.
    pub composer: Duration,
}

impl Default for SubmitTimeouts {
    fn default() -> Self {
        Self {
            stage: STAGE_TIMEOUT,
            stage_grace: STAGE_GRACE,
            submit: SUBMIT_TIMEOUT,
            composer: COMPOSER_WAIT,
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
    verified(harness) && !text.trim_start().starts_with('/')
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

pub(crate) fn text_of(row: &[char]) -> String {
    row.iter().collect()
}

fn is_rule_char(character: char) -> bool {
    matches!(character, '─' | '━' | '═')
}

pub(crate) fn is_rule(line: &[char]) -> bool {
    let text = text_of(line);
    let stripped = text.trim();
    stripped.chars().count() >= 3 && stripped.chars().all(is_rule_char)
}

/// Return whether `line` is a rule, possibly with a label drawn into it.
///
/// Claude can draw session state into the top border of its composer, as in
/// `──────── ultracode ─`. Such a row still starts and ends with rule
/// characters; whatever lies between the two runs is the label.
pub(crate) fn is_labelled_rule(line: &[char]) -> bool {
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

pub(crate) fn join_rows(rows: &[(Vec<char>, Vec<char>)]) -> String {
    rows.iter()
        .map(|(plain, _)| text_of(plain))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn composer_from(
    body: &[(Vec<char>, Vec<char>)],
    marker_end: usize,
) -> (String, String) {
    let mut plain_lines: Vec<String> = body.iter().map(|(plain, _)| text_of(plain)).collect();
    let mut solid_lines: Vec<String> = body.iter().map(|(_, solid)| text_of(solid)).collect();
    plain_lines[0] = text_of(&body[0].0[marker_end.min(body[0].0.len())..]);
    solid_lines[0] = text_of(&body[0].1[marker_end.min(body[0].1.len())..]);
    (plain_lines.join("\n"), solid_lines.join("\n"))
}

/// Locate the composer of a supported harness, or `None` when it is not recognisable.
pub fn composer_view(harness: &str, screen: &str) -> Option<ComposerView> {
    let adapter = crate::harness::adapter(harness)?;
    adapter.composer_view(&render_screen(screen))
}

/// Whether the bottom of `screen` shows that the agent is running a turn: `Some(true)` if it
/// does, `Some(false)` if it shows none, and `None` if it cannot tell.
///
/// Claude Code ends the status row under its input box with `esc to interrupt` while a turn
/// runs, and Codex prints the same words in the progress row above its composer. Both show a
/// queued-message hint only while a turn runs. Only the `RUNNING_TURN_ROWS` rows that end at
/// the last row with text are searched, so the same words higher up in the conversation do not
/// count. A status rule that looks for the input box cannot tell this: Claude Code keeps that
/// box on screen while it works, so such a rule can report a working pane as idle.
///
/// For a while after a paste, Claude Code draws `PASTE_HINT` in place of its status row, whether
/// or not a turn runs. A screen whose last row with text is that hint, and which shows no marker,
/// gives `None`.
pub(crate) fn running_turn(screen: &str) -> Option<bool> {
    let rows = render_screen(screen);
    let Some(last) = rows
        .iter()
        .rposition(|(plain, _)| plain.iter().any(|character| !character.is_whitespace()))
    else {
        return Some(false);
    };
    let marked = rows[(last + 1).saturating_sub(RUNNING_TURN_ROWS)..=last]
        .iter()
        .any(|(plain, _)| {
            let row = text_of(plain);
            row.contains(WORKING_MARKER)
                || queue_markers().iter().any(|marker| row.contains(marker))
        });
    if !marked && text_of(&rows[last].0).contains(PASTE_HINT) {
        return None;
    }
    Some(marked)
}

/// The queued-message marker among the rows `running_turn` searches, if `screen` shows one.
fn queue_marker(screen: &str) -> Option<&'static str> {
    let rows = render_screen(screen);
    let last = rows
        .iter()
        .rposition(|(plain, _)| plain.iter().any(|character| !character.is_whitespace()))?;
    rows[(last + 1).saturating_sub(RUNNING_TURN_ROWS)..=last]
        .iter()
        .find_map(|(plain, _)| {
            let row = text_of(plain);
            queue_markers()
                .iter()
                .copied()
                .find(|marker| row.contains(marker))
        })
}

/// Whether the `rows` rows that end at the last row with text show a queued-message marker, and
/// whether they show any marker of a running turn.
fn markers_within(screen: &str, rows: usize) -> (bool, bool) {
    let rendered = render_screen(screen);
    let Some(last) = rendered
        .iter()
        .rposition(|(plain, _)| plain.iter().any(|character| !character.is_whitespace()))
    else {
        return (false, false);
    };
    let mut queued = false;
    let mut running = false;
    for (plain, _) in &rendered[(last + 1).saturating_sub(rows)..=last] {
        let row = text_of(plain);
        let queue = queue_markers().iter().any(|marker| row.contains(marker));
        queued |= queue;
        running |= queue || row.contains(WORKING_MARKER);
    }
    (queued, running)
}

/// What the screen reads before the submission key took effect showed: their markers, the most
/// copies of the whole prompt above the composer, and, for each paste placeholder the composer
/// showed for this prompt, the most copies of it above the composer.
#[derive(Clone, Debug, Default)]
struct ShownBefore {
    queue_marker: bool,
    running_turn: bool,
    /// The prompt without its blanks.
    prompt: String,
    prompt_copies: usize,
    /// Each numbered placeholder label the composer showed for this paste, without its blanks,
    /// such as `[Pastedtext#3+12lines]`, and its most copies above the composer.
    labels: Vec<(String, usize)>,
    /// The text above the composer before the paste, without its blanks.
    before_paste: String,
    /// Copies of the whole prompt above the composer before the paste (the
    /// fixed baseline [`queued_evidence`] compares against; [`Self::prompt_copies`]
    /// moves as later reads note the prompt wherever it lands).
    prompt_before: usize,
    /// The exact `+k` counts this prompt's paste placeholder can carry.
    line_counts: Vec<usize>,
    /// The most Codex pending-message previews that start the prompt any read showed: see
    /// [`codex_pending_copies`].
    pending_copies: usize,
}

impl ShownBefore {
    fn new(text: &str) -> Self {
        Self {
            prompt: compact(text),
            line_counts: expected_line_counts(text),
            ..Self::default()
        }
    }

    /// Note the screen read before the paste.
    fn note_before_paste(&mut self, view: &ComposerView) {
        self.before_paste = compact(&view.transcript);
        self.prompt_copies = self.before_paste.matches(self.prompt.as_str()).count();
        self.prompt_before = self.prompt_copies;
        self.pending_copies = codex_pending_copies(&view.transcript, &self.prompt);
    }

    /// Note what a screen read after the paste and before the key took effect shows. A tall
    /// composer can push a marker above the rows `running_turn` searches, and the marker comes
    /// back into them once the prompt leaves the composer, so the rows the composer takes are
    /// searched as well. The composer was empty before the paste, so a numbered placeholder it
    /// shows stands for this prompt, unless the prompt's own text holds that label.
    fn note(&mut self, screen: &str, view: Option<&ComposerView>) {
        let composer_rows = view.map_or(0, |view| view.composer.lines().count());
        let (queued, running) = markers_within(screen, RUNNING_TURN_ROWS + composer_rows);
        self.queue_marker |= queued || queue_marker(screen).is_some();
        self.running_turn |= running || running_turn(screen) == Some(true);
        let Some(view) = view else {
            return;
        };
        for label in placeholder_labels(&view.composer) {
            if !self.prompt.contains(label.as_str())
                && !self.labels.iter().any(|(known, _)| *known == label)
            {
                let copies = self.before_paste.matches(label.as_str()).count();
                self.labels.push((label, copies));
            }
        }
        self.pending_copies = self
            .pending_copies
            .max(codex_pending_copies(&view.transcript, &self.prompt));
        let transcript = compact(&view.transcript);
        self.prompt_copies = self
            .prompt_copies
            .max(transcript.matches(self.prompt.as_str()).count());
        for (label, copies) in &mut self.labels {
            *copies = (*copies).max(transcript.matches(label.as_str()).count());
        }
    }
}

/// The numbered paste placeholders in `text`, `[Pasted text #N ...]`, each whole and without its
/// blanks, so that `#12` is not found inside `#123`. Claude Code numbers each paste, so the label
/// names one paste. `[Pasted Content N chars]` names only a length that another paste can share,
/// so it is not returned.
fn placeholder_labels(text: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut rest = text;
    while let Some(position) = rest.find("[Pasted ") {
        let tail = &rest[position..];
        if let Some(close) = tail.char_indices().take(80).find(|(_, c)| *c == ']') {
            let label = &tail[..=close.0];
            if label.starts_with("[Pasted text #") && paste_placeholders(label) == 1 {
                labels.push(compact(label));
            }
        }
        rest = &tail["[Pasted ".len()..];
    }
    labels
}

pub(crate) fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub(crate) fn suffix(text: &str, length: usize) -> String {
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

/// The exact `+k` line counts Claude Code can report for a paste of `text`:
/// the counts the composer flush and the stale-draft matcher share (#884
/// follow-up busy-evidence).
///
/// Claude Code counts logical lines, so screen wrap never changes the count
/// (a real busy pane drew a 5-newline paste whose lines wrap as `+5 lines`).
/// A trailing newline ends the last line instead of starting a new one, and
/// builds differ on whether they count lines or line breaks, so the line
/// count, the break count and lines-minus-one are all accepted; any other
/// size is a different paste.
pub(crate) fn expected_line_counts(text: &str) -> Vec<usize> {
    let lines = text.lines().count();
    let newlines = text.matches('\n').count();
    let mut counts = vec![lines.saturating_sub(1), newlines, lines];
    counts.sort_unstable();
    counts.dedup();
    counts
}

/// Whether the `+k lines` count of a paste placeholder can be the count
/// Claude Code reports for `text`, for a placeholder that appeared in
/// response to our own paste (see [`placeholder_is_ours`]).
///
/// On top of the exact counts ([`expected_line_counts`]), a narrow pane
/// counts wrapped rows: a long logical line drawn across several terminal
/// rows adds to the count, by an amount that depends on the pane's width, so
/// a wrapped count lies between the logical line count and the character
/// count. That range is too weak to attribute a chip already sitting in the
/// composer (any prompt of a similar size qualifies), so the baseline flush
/// and the stale-draft matcher use the exact counts only.
pub(crate) fn paste_line_count_matches(text: &str, count: usize) -> bool {
    if expected_line_counts(text).contains(&count) {
        return true;
    }
    let total = count + 1;
    let logical = text.lines().count().max(1);
    total >= logical && total <= text.chars().count().max(logical)
}

/// Whether a paste placeholder drawn in `view`'s composer is the paste of
/// `text`: it appeared after our paste (more placeholders than
/// `placeholders_before`) and, for a `[Pasted text #N ...]` placeholder, its
/// `+k lines` count matches `text` (see [`paste_line_count_matches`]). A
/// `[Pasted Content N chars]` placeholder names only a length another paste
/// can share, so any new one counts as staged; printed evidence (see
/// [`printed`]) still has to name this prompt before a receipt is believed.
/// A `[Pasted text #N ...]` placeholder of an impossible size is someone
/// else's paste, however recently it appeared (#884 item 4).
fn placeholder_is_ours(view: &ComposerView, text: &str, placeholders_before: usize) -> bool {
    if paste_placeholders(&view.composer) <= placeholders_before {
        return false;
    }
    placeholder_labels(&view.composer).iter().any(|label| {
        crate::stale_draft::pasted_line_count_pub(label)
            .is_some_and(|count| paste_line_count_matches(text, count))
    }) || view.composer.contains("[Pasted Content ")
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
        || placeholder_is_ours(view, text, placeholders_before)
}

/// Bottom `K` non-blank rows of a screen, for harness-agnostic tail checks.
pub(crate) fn bottom_region(screen: &str) -> String {
    let rows: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    rows[rows.len().saturating_sub(30)..].join("\n")
}

fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Harness-agnostic static-tail detector (fallback for every harness):
/// our text fuzzy-matches the bottom region (word shingles / placeholder).
/// True only as a "still sitting" signal; callers also require the tail to
/// be unchanged across polls before pressing Enter on it.
pub(crate) fn static_tail_holds(screen: &str, text: &str) -> bool {
    // Shared harness-neutral heuristics first (screen_heuristics), then
    // the wider bottom-region shingle check below.
    if crate::screen_heuristics::stranded(screen, text) {
        return true;
    }
    let tail = squash(&bottom_region(screen));
    if tail.is_empty() {
        return false;
    }
    if paste_placeholders(&bottom_region(screen)) > 0 && !squash(text).is_empty() {
        // A placeholder in the tail whose line/char size is ours is a clog
        // signal; a placeholder of another size is someone else's paste and
        // must never be submitted as ours (#884 item 4).
        let region = bottom_region(screen);
        let ours = placeholder_labels(&region).iter().any(|label| {
            crate::stale_draft::pasted_line_count_pub(label)
                .is_some_and(|count| paste_line_count_matches(text, count))
        }) || region
            .contains(&format!("[Pasted Content {} chars]", text.chars().count()));
        if ours {
            return true;
        }
    }
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() >= 3)
        .collect();
    if words.len() >= 3 {
        let shingles = words.windows(3).map(|w| w.join(""));
        if shingles.clone().count() > 0 && shingles.into_iter().any(|s| tail.contains(&s)) {
            return true;
        }
    }
    let wanted: Vec<char> = squash(text).chars().collect();
    wanted.len() >= 16 && {
        let suffix: String = wanted[wanted.len().saturating_sub(80)..].iter().collect();
        tail.contains(&suffix)
    }
}

/// Whether `view`'s composer holds this exact prompt (text, fragment, or
/// a placeholder of its size), for the stranded-flush check.
fn composer_is_this_text(view: &ComposerView, text: &str) -> bool {
    let solid = squash(&view.composer_solid);
    let wanted = squash(text);
    if !solid.is_empty() && (solid == wanted || (solid.len() >= 16 && wanted.contains(&solid))) {
        return true;
    }
    // Placeholder forms: line count or char count of this text. The line
    // count uses the shared exact counts ([`expected_line_counts`], lines
    // variant included), so a chip drawn [Pasted text #264 +3 lines] for a
    // 3-line notice is this text here too, not only on a fresh delivery
    // (#884 follow-up busy-evidence). Wrapped-row counts are not accepted
    // here: they would attribute a foreign chip of a similar size.
    let region = &view.composer;
    if let Some(start) = region.find("[Pasted text #") {
        if let Some(end) = region[start..].find(']') {
            let label = &region[start..=start + end];
            if let Some(m) = crate::stale_draft::pasted_line_count_pub(label) {
                return expected_line_counts(text).contains(&m);
            }
        }
    }
    if paste_placeholders(region) > 0 {
        return region.contains(&format!("[Pasted Content {} chars]", text.chars().count()));
    }
    false
}

/// Whether `view`'s composer holds one paste placeholder and nothing else solid: on a stranded
/// retry, that is the paste an earlier pass typed, drawn collapsed (#884).
fn holds_one_placeholder_only(view: &ComposerView) -> bool {
    if paste_placeholders(&view.composer) != 1 {
        return false;
    }
    let mut rest = view.composer_solid.clone();
    while let Some(start) = rest.find("[Pasted ") {
        let Some(end) = rest[start..].find(']') else {
            break;
        };
        rest.replace_range(start..=start + end, "");
    }
    rest.trim().is_empty()
}

/// What a composer showed, for a refusal: its first 160 characters on one line, or that no
/// composer could be read.
fn composer_evidence(view: Option<&ComposerView>) -> String {
    match view {
        None => "no recognisable composer".to_owned(),
        Some(view) => {
            let line = view
                .composer
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if line.is_empty() {
                return "an empty composer".to_owned();
            }
            format!("{:?}", line.chars().take(160).collect::<String>())
        }
    }
}

/// Whether `view`'s composer holds only our paste (no human interleave).
fn composer_holds_only_ours(view: &ComposerView, text: &str, placeholders_before: usize) -> bool {
    let solid = squash(&view.composer_solid);
    if solid.is_empty() {
        return true; // empty or faint placeholder only
    }
    let wanted = squash(text);
    solid == wanted
        || wanted.contains(&solid)
        || paste_placeholders(&view.composer) > placeholders_before
}

fn transcript_count(view: &ComposerView, text: &str) -> usize {
    compact(&view.transcript).matches(&suffix(text, 40)).count()
}

/// Characters of a prompt, blanks aside, that must show unbroken above the composer for it to
/// count as drawn there when its ends do not show.
const TAKEN_RUN: usize = 120;

/// Whether `transcript` shows an unbroken run of [`TAKEN_RUN`] characters of `text`, blanks
/// aside (#953 agentctl-busy-claude-false-possibly-submitted). Claude Code draws a prompt it
/// takes at a tool boundary among that tool's result rows, cut at both ends: without its first
/// rows and without its last, so the tail [`transcript_count`] looks for never shows. A run that
/// long of a prompt typed into this pane alone is that prompt, not a coincidence.
fn shows_run_of(transcript: &str, text: &str) -> bool {
    let prompt: Vec<char> = compact(text).chars().collect();
    let Some(last) = prompt.len().checked_sub(TAKEN_RUN) else {
        return false;
    };
    let shown = compact(transcript);
    (0..=last)
        .step_by(TAKEN_RUN / 4)
        .chain([last])
        .any(|start| shown.contains(&prompt[start..start + TAKEN_RUN].iter().collect::<String>()))
}

fn corroborated(before: &ComposerView, after: &ComposerView, text: &str) -> Option<String> {
    if transcript_count(after, text) > transcript_count(before, text) {
        return Some("prompt text appeared above the composer".to_owned());
    }
    if paste_placeholders(&after.transcript) > paste_placeholders(&before.transcript) {
        return Some("pasted prompt appeared above the composer".to_owned());
    }
    let screen = format!("{}\n{}", after.transcript, after.footer);
    // Claude draws its queued-message hint inside the empty composer. Its paste hint can
    // temporarily replace the working status row, so the footer alone misses that evidence.
    if let Some(marker) = queue_markers()
        .iter()
        .find(|marker| screen.contains(*marker) || after.composer.contains(*marker))
    {
        return Some(format!("agent queue marker is visible ({marker:?})"));
    }
    if screen.contains(WORKING_MARKER) {
        return Some("agent reports an active turn".to_owned());
    }
    None
}

/// Describe what the pane printed after the submission key, if `after` shows any of it: more
/// copies above the composer of the whole prompt, or of a numbered placeholder the composer
/// showed for it, or of Codex's preview of it among held messages, than any earlier read showed,
/// or a marker that no earlier read showed. Each of
/// these names this prompt: another prompt's copy, even one built from the same template, is not
/// the whole of this one, and another paste has another number. `shown` says what the earlier
/// reads showed.
fn printed(after: &ComposerView, after_screen: &str, shown: &ShownBefore) -> Option<String> {
    let transcript = compact(&after.transcript);
    if transcript.matches(shown.prompt.as_str()).count() > shown.prompt_copies {
        return Some("prompt text appeared above the composer".to_owned());
    }
    if shown
        .labels
        .iter()
        .any(|(label, copies)| transcript.matches(label.as_str()).count() > *copies)
    {
        return Some("pasted prompt appeared above the composer".to_owned());
    }
    if codex_pending_copies(&after.transcript, &shown.prompt) > shown.pending_copies {
        return Some("Codex lists the prompt among the messages it holds for its turn".to_owned());
    }
    if let Some(evidence) = queued_evidence(after, after_screen, shown) {
        return Some(evidence);
    }
    if !shown.queue_marker {
        if let Some(marker) = queue_marker(after_screen) {
            return Some(format!("agent queue marker appeared ({marker:?})"));
        }
    }
    if !shown.running_turn && running_turn(after_screen) == Some(true) {
        return Some("agent started reporting an active turn".to_owned());
    }
    None
}

/// Whether the agent's queued-message area holds this prompt, as a busy
/// Claude Code pane draws it (captured on a real pane, #884 follow-up
/// busy-evidence): the prompt in full above the composer's top rule with a
/// `ctrl+enter to send now` row, or collapsed there to its numbered paste
/// placeholder, while the empty composer holds the faint
/// `Press up to edit queued messages` hint and the footer shows the paste
/// hint instead of `esc to interrupt`. The prompt (or a placeholder of its
/// exact size) must be new above the composer since before the paste, so an
/// older queued prompt never vouches for this one.
fn queued_evidence(
    after: &ComposerView,
    after_screen: &str,
    shown: &ShownBefore,
) -> Option<String> {
    let queued_area = after.composer.contains("Press up to edit queued messages")
        || after_screen.contains("ctrl+enter to send now")
        || queue_marker(after_screen).is_some();
    if !queued_area {
        return None;
    }
    let transcript = compact(&after.transcript);
    if transcript.matches(shown.prompt.as_str()).count() > shown.prompt_before {
        return Some("prompt appeared in the queued messages (queue marker visible)".to_owned());
    }
    for label in placeholder_labels(&after.transcript) {
        if shown.before_paste.contains(label.as_str()) {
            continue;
        }
        if crate::stale_draft::pasted_line_count_pub(&label)
            .is_some_and(|count| shown.line_counts.contains(&count))
        {
            return Some(
                "pasted prompt appeared in the queued messages (queue marker visible)".to_owned(),
            );
        }
    }
    None
}

/// Most unsent drafts agentctl typed that one submission submits before its own prompt (#720
/// agentctl-muse-stale-draft).
const MAX_DRAFT_RECOVERIES: u32 = 3;
/// Most presses of the submission key for one unsent draft agentctl typed.
const DRAFT_PRESSES: u32 = 3;
/// Longest wait for such a draft to leave the composer for good after the first key.
const DRAFT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the composer must stay empty after such a draft left it before it counts as
/// submitted: a harness can put a prompt it took back into the composer.
const DRAFT_SETTLE: Duration = Duration::from_secs(3);

/// The draft a composer holds: its solid text, or a paste placeholder, which some Codex builds
/// draw faint. `None` when it is empty.
pub(crate) fn composer_draft(view: &ComposerView) -> Option<crate::stale_draft::Draft> {
    let solid = view.composer_solid.trim();
    if !solid.is_empty() {
        return Some(crate::stale_draft::Draft::from_composer(solid));
    }
    (paste_placeholders(&view.composer) > 0)
        .then(|| crate::stale_draft::Draft::from_composer(&view.composer))
}

/// Whether `screen` shows a question, a menu or a picker in place of `harness`'s composer, where
/// a submission key would choose an option instead of submitting text.
fn blocked(harness: &str, screen: &str) -> bool {
    crate::harness_tui::harness_tui(Some(harness)).is_some_and(|adapter| {
        matches!(
            adapter.classify(screen).state,
            crate::harness_tui::State::BlockingQuestion | crate::harness_tui::State::MenuOrPicker
        )
    })
}

/// Submit a draft that agentctl itself typed and `harness` left unsent in its composer, so that
/// it no longer holds every later prompt, the owner's included (#720 agentctl-muse-stale-draft).
///
/// A draft that [`crate::stale_draft::find`] matches, by [`crate::stale_draft::Vouching::Unproven`]
/// (Up recalls the newest prompt in these harnesses), to prompts the queue at `queue` records as
/// typed (their text as the composer draws it, or the paste placeholder the harness draws for
/// the last of them) is submitted with the key a prompt typed now would get. The key is pressed
/// again (at most [`DRAFT_PRESSES`] times in all) while the same draft is still there, and never
/// while the screen shows a question, a menu or a picker, where it would choose an option; one
/// that shows after a key counts as the draft having left. The
/// draft counts as submitted once the composer has stayed empty for [`DRAFT_SETTLE`]; the queue
/// records note the recovery. Any other draft may be a person typing: nothing is pressed.
///
/// `Ok(None)`: the composer is empty, not recognisable, or holds a draft agentctl did not type.
/// `Ok(Some(evidence))`: a draft agentctl typed was submitted. An error: such a draft is still
/// in the composer after [`DRAFT_SUBMIT_TIMEOUT`], or a read or key failed.
pub(crate) fn recover_typed_draft(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    harness: &str,
    queue: &std::path::Path,
    runtime: &dyn AgentRuntime,
) -> Result<Option<String>> {
    // The composer, or `Err(())` while a question, a menu or a picker shows.
    let read = || -> Result<std::result::Result<Option<ComposerView>, ()>> {
        let screen = terminal.read_screen(pane_id)?;
        Ok(if blocked(harness, &screen) {
            Err(())
        } else {
            Ok(composer_view(harness, &screen))
        })
    };
    let Ok(Some(view)) = read()? else {
        return Ok(None);
    };
    let Some(draft) = composer_draft(&view) else {
        return Ok(None);
    };
    let Some(prompt) = crate::stale_draft::find(
        queue,
        &draft,
        crate::stale_draft::Vouching::Unproven,
        std::time::SystemTime::now(),
    ) else {
        return Ok(None);
    };
    let typed = prompt.describe();
    let mut key = submit_key(harness, &view, runtime);
    terminal.send_keys(pane_id, key)?;
    let pressed = runtime.monotonic();
    let mut presses: u32 = 1;
    let mut wait = FIRST_RETRY;
    let mut next_press = pressed.saturating_add(wait);
    let mut empty_since: Option<Duration> = None;
    loop {
        runtime.sleep(POLL);
        if runtime.cancelled() {
            return Err(AdapterError::cancelled(format!(
                "pane {pane_id}: delivery was cancelled while submitting an unsent draft {typed}"
            )));
        }
        let Ok(view) = read()? else {
            // The agent asked something once the draft left the composer, which it was in when
            // the key was pressed; the caller refuses the blocked pane.
            let evidence = format!(
                "submitted an unsent draft {typed} that {harness} had left in its composer \
                 ({presses} {key} key press(es)), after which it asked a question"
            );
            crate::stale_draft::note_recovered(&prompt, &evidence, unix_seconds());
            return Ok(Some(evidence));
        };
        let now = runtime.monotonic();
        let current = view.as_ref().map(composer_draft);
        if let Some(None) = current {
            let since = *empty_since.get_or_insert(now);
            if now.saturating_sub(since) >= DRAFT_SETTLE {
                let evidence = format!(
                    "submitted an unsent draft {typed} that {harness} had left in its composer \
                     ({presses} {key} key press(es))"
                );
                crate::stale_draft::note_recovered(&prompt, &evidence, unix_seconds());
                return Ok(Some(evidence));
            }
            continue;
        }
        empty_since = None;
        let same = current.flatten().as_ref() == Some(&draft);
        if now.saturating_sub(pressed) >= DRAFT_SUBMIT_TIMEOUT {
            return Err(AdapterError::unavailable(if same {
                format!(
                    "pane {pane_id}: the {harness} composer holds an unsent draft {typed}; it is \
                     still there after {presses} {key} key press(es)"
                )
            } else {
                format!(
                    "pane {pane_id}: the {harness} composer held an unsent draft {typed}; after \
                     {presses} {key} key press(es) the composer is neither empty nor that draft"
                )
            }));
        }
        if let Some(view) = view.as_ref().filter(|_| same) {
            if presses < DRAFT_PRESSES && now >= next_press {
                key = submit_key(harness, view, runtime);
                terminal.send_keys(pane_id, key)?;
                presses += 1;
                wait = (wait * 2).min(MAX_RETRY);
                next_press = now.saturating_add(wait);
            }
        }
    }
}

fn unix_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn submit_key(harness: &str, view: &ComposerView, runtime: &dyn AgentRuntime) -> &'static str {
    if let Some(key) = runtime.followup_key() {
        return key;
    }
    crate::harness::adapter(harness).map_or("Enter", |adapter| adapter.submit_key(view))
}

/// Stage `text` in an empty composer, submit it, and prove that it left the composer.
///
/// The submission key is repeated with a doubling wait while the exact text is
/// still staged, so a key the agent dropped is retried without typing the
/// prompt twice. A key is never repeated once the text has left the composer.
///
/// When only a marker that was already showing proves the submission, the screen is read for
/// up to [`PRINT_GRACE`] more for printed evidence, and the receipt says which kind it got.
pub fn submit_verified(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    harness: &str,
    text: &str,
    timeouts: SubmitTimeouts,
    runtime: &dyn AgentRuntime,
) -> Result<Submission> {
    let refuse = |reason: String| Ok(Submission::NotStaged(reason));
    if !verified(harness) {
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
    let mut composer_deadline = runtime.monotonic() + timeouts.composer;
    let mut recovered: u32 = 0;
    // What the last recovery of a draft agentctl typed did: submitted it, or failed to.
    let mut recovery: Option<std::result::Result<String, String>> = None;
    let (screen, before) = 'baseline: loop {
        let screen = match terminal.read_screen(pane_id) {
            Ok(screen) => screen,
            Err(error) => {
                return refuse(format!(
                    "pane {pane_id}: composer read failed before typing: {error}"
                ))
            }
        };
        if let Some(adapter) = crate::harness_tui::harness_tui(Some(harness)) {
            let view = adapter.classify(&screen);
            if matches!(
                view.state,
                crate::harness_tui::State::BlockingQuestion
                    | crate::harness_tui::State::MenuOrPicker
            ) {
                return refuse(format!(
                    "pane {pane_id} is blocked; {}; nothing was typed",
                    crate::harness_tui::blocked_hint(&view)
                ));
            }
        }
        if let Some(before) = composer_view(harness, &screen) {
            // Stranded own paste for THIS prompt (a pending retry): press
            // Enter only, never re-type. Bare Enter on our text submits it
            // once; on an empty composer it would be harmless.
            if composer_draft(&before).is_some()
                && (composer_is_this_text(&before, text)
                    || (runtime.stranded_retry() && holds_one_placeholder_only(&before)))
                && !blocked(harness, &screen)
            {
                let key = submit_key(harness, &before, runtime);
                terminal.send_keys(pane_id, key)?;
                let pressed_at = runtime.monotonic();
                let mut presses: u32 = 1;
                let mut next_press = pressed_at.saturating_add(FIRST_RETRY);
                let mut wait = FIRST_RETRY;
                loop {
                    runtime.sleep(POLL);
                    if runtime.cancelled() {
                        return Err(AdapterError::unavailable(format!(
                            "pane {pane_id}: delivery cancelled during stranded flush"
                        )));
                    }
                    let now = runtime.monotonic();
                    let current_screen = terminal.read_screen(pane_id)?;
                    let Some(current) = composer_view(harness, &current_screen) else {
                        break;
                    };
                    if composer_draft(&current).is_none() {
                        return Ok(Submission::Verified(SubmissionReceipt {
                            key,
                            key_presses: presses,
                            elapsed: now.saturating_sub(pressed_at),
                            evidence: "stranded prompt flushed with Enter".to_owned(),
                            printed: true,
                            settled: true,
                            queued: false,
                        }));
                    }
                    if now.saturating_sub(pressed_at) >= DRAFT_SUBMIT_TIMEOUT {
                        break;
                    }
                    if presses < DRAFT_PRESSES && now >= next_press {
                        terminal.send_keys(pane_id, key)?;
                        presses += 1;
                        wait = (wait * 2).min(MAX_RETRY);
                        next_press = now.saturating_add(wait);
                    }
                }
                return refuse(format!(
                    "stranded_prompt: pane {pane_id}: stranded prompt is still staged; \
                     no new text was typed"
                ));
            }
            // A draft agentctl typed that the harness left unsent is submitted first, and the
            // composer looked at again (#720 agentctl-muse-stale-draft).
            if composer_draft(&before).is_some() && recovered < MAX_DRAFT_RECOVERIES {
                if let Some(queue) = runtime.typed_prompts_queue() {
                    let outcome = recover_typed_draft(terminal, pane_id, harness, &queue, runtime);
                    if !matches!(outcome, Ok(None)) {
                        recovered = match outcome {
                            Ok(_) => recovered + 1,
                            // Pressed enough: the draft is refused like any other below.
                            Err(_) => MAX_DRAFT_RECOVERIES,
                        };
                        recovery = Some(
                            outcome
                                .map(Option::unwrap_or_default)
                                .map_err(|error| error.to_string()),
                        );
                        composer_deadline = runtime.monotonic() + timeouts.composer;
                        continue 'baseline;
                    }
                }
            }
            // The draft submitted may have started a turn, and a harness that takes a prompt
            // during a turn only when its sender opts in waits for the next delivery.
            if let Some(Ok(evidence)) = &recovery {
                let opt_in = crate::harness::adapter(harness).is_some_and(|adapter| {
                    adapter.followup_policy() == crate::harness::FollowupPolicy::OptIn
                });
                if opt_in
                    && runtime.followup_key().is_none()
                    && running_turn(&screen) != Some(false)
                {
                    return refuse(format!(
                        "pane {pane_id}: {evidence}; it started a turn, so this prompt waits for \
                         the agent; nothing was typed"
                    ));
                }
            }
            break (screen, before);
        }
        if runtime.monotonic() >= composer_deadline || runtime.cancelled() {
            // A grid that stayed ansi-blank while Herdr still proves the harness
            // working in its pinned foreground process is unreadable, not absent:
            // the distinct refusal lets a drain retry it instead of guessing.
            // Nothing has been typed on either path.
            if let Some(reason) = composer_unreadable_reason(
                pane_id,
                harness,
                &screen,
                terminal.composer_probe(pane_id, harness),
                timeouts.composer,
            ) {
                return refuse(reason);
            }
            return refuse(format!(
                "pane {pane_id} does not show a recognisable {harness} composer after {}s \
                 (a dialog or menu may be open); nothing was typed",
                timeouts.composer.as_secs_f64()
            ));
        }
        runtime.sleep(POLL);
    };
    // A prompt an earlier pass typed but never saw drawn (#884): never typed again while its
    // paste may still show; reported lost once the whole late-paste window has passed.
    if runtime.stranded_retry() && composer_draft(&before).is_none() {
        // Before counting the paste lost, look where a busy agent puts a
        // prompt it took: its queued messages above the composer, or the
        // transcript as a user turn. Either is delivery, never a loss
        // (#884 follow-up busy-evidence (a)).
        let queued_area = before.composer.contains("Press up to edit queued messages")
            || screen.contains("ctrl+enter to send now")
            || queue_marker(&screen).is_some();
        let in_transcript = transcript_count(&before, text) > 0
            || placeholder_labels(&before.transcript).iter().any(|label| {
                crate::stale_draft::pasted_line_count_pub(label)
                    .is_some_and(|count| expected_line_counts(text).contains(&count))
            });
        // Taken at a tool boundary: drawn among the tool's result rows, cut at both ends (#953).
        let taken_into_turn = !in_transcript && shows_run_of(&before.transcript, text);
        if in_transcript || taken_into_turn {
            return Ok(Submission::Verified(SubmissionReceipt {
                key: "Enter",
                key_presses: 0,
                elapsed: Duration::ZERO,
                evidence: if queued_area {
                    "stranded prompt is in the agent's queued messages (queue marker visible)"
                        .to_owned()
                } else if taken_into_turn {
                    "stranded prompt was taken into the running turn (part of it shows above the \
                     composer)"
                        .to_owned()
                } else {
                    "stranded prompt appeared above the composer".to_owned()
                },
                printed: true,
                settled: true,
                queued: queued_area || taken_into_turn,
            }));
        }
        let waited = runtime
            .stranded_since()
            .map(|since| (unix_seconds() - since).max(0.0));
        let window = (STAGE_TIMEOUT + STAGE_GRACE).as_secs_f64();
        if waited.is_some_and(|waited| waited >= window) {
            return refuse(format!(
                "{LATE_PASTE_LOST}: pane {pane_id}: the paste an earlier pass typed never showed \
                 in the {harness} composer in {window:.0}s; it was lost, and the prompt is never \
                 typed again"
            ));
        }
        return refuse(format!(
            "{}: late_paste: pane {pane_id}: the paste an earlier pass typed has not shown in \
             the {harness} composer yet ({:.0}s); waiting for it, never typing it again",
            crate::unknown_harness::STRANDED_PROMPT_KIND,
            waited.unwrap_or(0.0)
        ));
    }
    let mut shown = ShownBefore {
        queue_marker: queue_marker(&screen).is_some(),
        // A screen that cannot tell, such as one that shows the paste hint in place of the status
        // row, may be hiding a running turn.
        running_turn: running_turn(&screen) != Some(false),
        ..ShownBefore::new(text)
    };
    // Whether a turn was already running before this paste: on such a pane the
    // agent can only queue a prompt it accepts, never start a turn with it.
    // A running-turn marker that was already showing therefore proves
    // queueing, not submission (#884 item 4 (B)); a marker that appears after
    // the key is a turn this prompt started.
    let turn_before_paste = shown.running_turn;
    shown.note_before_paste(&before);
    // A harness that draws a paste placeholder faint (some Codex builds do) shows no solid text
    // for a paste left unsent; a new paste would join it, and the key would submit both.
    let draft = match before.composer_solid.trim() {
        "" if composer_draft(&before).is_some() => before.composer.trim(),
        solid => solid,
    };
    if !draft.is_empty() {
        let preview: String = draft
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect();
        let failed = match &recovery {
            Some(Err(error)) => format!("{error}; "),
            _ => String::new(),
        };
        return refuse(format!(
            "{failed}pane {pane_id} composer already holds unsubmitted text {preview:?}; \
             refusing to append to it; nothing was typed"
        ));
    }
    let placeholders_before = paste_placeholders(&before.composer);
    // First terminal input of this submission: let the runtime stamp it
    // (a queue drain records `paste_started_at` for inflight recovery).
    runtime.input_started();
    terminal.send_text(
        pane_id,
        &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
    )?;

    // A stop after the paste was typed presses the submission key once on
    // our own staged paste before the unknown outcome is reported, so the
    // paste does not sit in the composer holding every later prompt while
    // the service exits (#884 item 4 (D)). Best effort: the outcome stays
    // unknown to the caller either way.
    let flush_staged_on_cancel = |terminal: &dyn PromptTerminal| {
        let Ok(screen) = terminal.read_screen(pane_id) else {
            return;
        };
        let Some(view) = composer_view(harness, &screen) else {
            return;
        };
        if staged(&view, text, placeholders_before) || composer_is_this_text(&view, text) {
            let key = submit_key(harness, &view, runtime);
            let _ = terminal.send_keys(pane_id, key);
        }
    };
    let cancelled_after_typing = || {
        Err(AdapterError::unavailable(format!(
            "pane {pane_id}: delivery was cancelled after the prompt was pasted; outcome is unknown"
        )))
    };
    let started = runtime.monotonic();
    let stage_deadline = started.saturating_add(timeouts.stage);
    let grace_deadline = stage_deadline.saturating_add(timeouts.stage_grace);
    let mut prev_tail: Option<String> = None;
    let view = loop {
        if runtime.cancelled() {
            flush_staged_on_cancel(terminal);
            return cancelled_after_typing();
        }
        let screen = terminal.read_screen(pane_id)?;
        let view = composer_view(harness, &screen);
        shown.note(&screen, view.as_ref());
        if let Some(view) = &view {
            if staged(view, text, placeholders_before) {
                if let [(label, _)] = shown.labels.as_slice() {
                    runtime.paste_drawn_as(label);
                }
                break view.clone();
            }
            // A busy agent can take the paste straight into its queued
            // messages without ever staging it in the composer: that is
            // delivery, and no submission key is sent on the empty composer
            // (#884 follow-up busy-evidence (a)).
            if let Some(evidence) = queued_evidence(view, &screen, &shown) {
                return Ok(Submission::Verified(SubmissionReceipt {
                    key: "Enter",
                    key_presses: 0,
                    elapsed: runtime.monotonic().saturating_sub(started),
                    evidence,
                    printed: true,
                    settled: true,
                    queued: true,
                }));
            }
            // Fallback for every harness: bottom tail unchanged across
            // polls and fuzzy-matching our paste means it is sitting there.
            let tail = bottom_region(&screen);
            if prev_tail.as_deref() == Some(tail.as_str()) && static_tail_holds(&screen, text) {
                break view.clone();
            }
            prev_tail = Some(tail);
        } else if static_tail_holds(&screen, text) {
            // No composer model on this read (mid-redraw): keep waiting in
            // grace rather than stranding the paste; Enter needs a view.
            prev_tail = Some(bottom_region(&screen));
        }
        // No early give-up on an idle pane with a visibly empty composer:
        // a paste to an idle or done Claude Code pane can render after the
        // in-lock stage window, and giving up here quarantines the prompt as
        // possibly submitted while its paste is still on its way, stranding
        // it in the composer with no later pass able to flush it. Idle panes
        // therefore take the same path as busy ones: the grace deadline
        // below reports the paste stranded (never typed again, flushed by a
        // later pass once it shows) instead of an unknown outcome.
        let now = runtime.monotonic();
        if now >= grace_deadline {
            if timeouts.stage_grace.is_zero() {
                // Reported stranded: the drain keeps the prompt pending and releases every
                // lock, and a later pass presses Enter on it once it shows (#884). What the
                // composer showed is kept, so a paste drawn in a form the match misses can be
                // told from one never drawn.
                return Ok(Submission::NotStaged(format!(
                    "{}: late_paste: pane {pane_id}: pasted prompt was not observed in the \
                     {harness} composer within {:.1}s; no submission key was sent; the composer \
                     showed {}",
                    crate::unknown_harness::STRANDED_PROMPT_KIND,
                    timeouts.stage.as_secs_f64(),
                    composer_evidence(view.as_ref())
                )));
            }
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: pasted prompt was not observed in the {harness} composer within \
                 {:.1}s; no submission key was sent",
                (timeouts.stage + timeouts.stage_grace).as_secs_f64()
            )));
        }
        runtime.sleep(if now >= stage_deadline {
            STAGE_POLL_SLOW
        } else {
            POLL
        });
    };
    // Human-race guard: re-read immediately before Enter. Anything beyond
    // our exact paste means a person typed into the composer; never submit
    // merged text. Leave everything untouched and defer (no Enter).
    {
        let screen = terminal.read_screen(pane_id)?;
        if let Some(current) = composer_view(harness, &screen) {
            if !blocked(harness, &screen)
                && staged(&current, text, placeholders_before)
                && !composer_holds_only_ours(&current, text, placeholders_before)
            {
                return Err(AdapterError::unavailable(format!(
                    "pane {pane_id}: composer changed after our paste (possible human typing); \
                     no submission key was sent; delivery deferred"
                )));
            }
            if !staged(&current, text, placeholders_before)
                && transcript_count(&current, text) > transcript_count(&before, text)
            {
                // Consumed before our Enter (echoed above): no key needed.
                // In the queued-message area it counts as queued (#884
                // follow-up busy-evidence (a)).
                let queued = queued_evidence(&current, &screen, &shown).is_some();
                return Ok(Submission::Verified(SubmissionReceipt {
                    key: "Enter",
                    key_presses: 0,
                    elapsed: runtime.monotonic().saturating_sub(started),
                    evidence: "prompt text appeared above the composer".to_owned(),
                    printed: true,
                    settled: true,
                    queued,
                }));
            }
        }
    }

    let mut key = submit_key(harness, &view, runtime);
    terminal.send_keys(pane_id, key)?;
    let mut presses: u32 = 1;
    let mut wait = FIRST_RETRY;
    let mut next_press = runtime.monotonic().saturating_add(wait);
    let submit_deadline = runtime.monotonic().saturating_add(timeouts.submit);
    let mut left_composer = false;
    // A read taken while waiting for printed evidence that showed the prompt staged again. It is
    // handled next as if this loop had just read it, so its bookkeeping, the deadline and the key
    // schedule all apply to it.
    let mut carried: Option<String> = None;
    'submitting: loop {
        let screen = match carried.take() {
            Some(screen) => {
                if runtime.cancelled() {
                    return cancelled_after_typing();
                }
                screen
            }
            None => {
                runtime.sleep(POLL);
                if runtime.cancelled() {
                    flush_staged_on_cancel(terminal);
                    return cancelled_after_typing();
                }
                terminal.read_screen(pane_id)?
            }
        };
        let now = runtime.monotonic();
        let current = composer_view(harness, &screen);
        match current {
            Some(current) if staged(&current, text, placeholders_before) => {
                shown.note(&screen, Some(&current));
                left_composer = false;
                if now >= submit_deadline && runtime.withdraw_unaccepted_prompt() {
                    return withdraw_unaccepted(
                        terminal, pane_id, harness, &shown, presses, key, started, runtime,
                    );
                }
                if now >= submit_deadline {
                    // Stranded, not possibly-submitted: the text is visibly
                    // still in the composer, so the drain keeps the item
                    // pending (never the multi-hour wedge state) and a later
                    // pass/ladder flushes it with bare Enter only.
                    return Ok(Submission::NotStaged(format!(
                        "stranded_prompt: pane {pane_id}: prompt is still staged in the {harness} composer after \
                         {presses} {key} key press(es) over {:.1}s; it was NOT submitted and is \
                         left visible so it is never typed twice",
                        now.saturating_sub(started).as_secs_f64()
                    )));
                }
                if now >= next_press {
                    key = submit_key(harness, &current, runtime);
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
                    // On a pane that was already busy, leaving the composer
                    // means the agent queued the prompt; a queue marker says
                    // the same on any pane (#884 item 4 (B)).
                    let queued = turn_before_paste || evidence.contains("queue marker");
                    let mut receipt = SubmissionReceipt {
                        key,
                        key_presses: presses,
                        elapsed: now.saturating_sub(started),
                        evidence,
                        printed: false,
                        settled: false,
                        queued,
                    };
                    // No key is sent from here on: the prompt has left the composer.
                    let grace_deadline = now.saturating_add(PRINT_GRACE);
                    let mut observed = Some((current, screen, now));
                    // Whether the last read showed a recognisable composer without the prompt.
                    let mut clear = true;
                    loop {
                        if let Some((view, screen, read_at)) = &observed {
                            if let Some(evidence) = printed(view, screen, &shown) {
                                receipt.elapsed = read_at.saturating_sub(started);
                                receipt.queued |= evidence.contains("queue marker");
                                receipt.evidence = evidence;
                                receipt.printed = true;
                                receipt.settled = true;
                                break;
                            }
                        }
                        // A wait that has run its course is complete even if a stop arrives with
                        // its last read; only a stop before that cuts it short.
                        if runtime.monotonic() >= grace_deadline {
                            receipt.settled = clear;
                            break;
                        }
                        if runtime.cancelled() {
                            break;
                        }
                        runtime.sleep(POLL);
                        let Ok(screen) = terminal.read_screen(pane_id) else {
                            break;
                        };
                        let read_at = runtime.monotonic();
                        let view = composer_view(harness, &screen);
                        // The prompt is staged again: the empty composer that seemed to prove the
                        // submission was a transient redraw, and the key may have been dropped.
                        // Without printed evidence nothing is proved, so this read goes back to
                        // the staged path, which notes what it shows, keeps the deadline, and
                        // presses the key again on its schedule.
                        if view
                            .as_ref()
                            .is_some_and(|view| staged(view, text, placeholders_before))
                        {
                            carried = Some(screen);
                            continue 'submitting;
                        }
                        clear = view.is_some();
                        observed = view.map(|view| (view, screen, read_at));
                    }
                    return Ok(Submission::Verified(receipt));
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

/// Keys that move to the end of the composer's current row and delete back to its start. Each
/// pair removes at most one row; a pair sent to a row that is already empty removes the line
/// break before it in Codex, and the screen is read after each pair to see what it did.
pub const CLEAR_ROW_KEYS: [&str; 2] = ["ctrl+e", "ctrl+u"];
/// The key [`clear_composer`] sends when a pair of [`CLEAR_ROW_KEYS`] changed nothing, as on an
/// empty row whose line break the row keys do not remove.
pub const CLEAR_CHARACTER_KEY: &str = "backspace";
/// Most pairs of [`CLEAR_ROW_KEYS`] one [`clear_composer`] sends.
pub const MAX_CLEAR_ROUNDS: usize = 64;

/// What a composer holds, as [`clear_composer`] compares it between reads: its rows as drawn,
/// each without trailing blanks, so an empty last row still counts as a row.
fn composer_content(view: &ComposerView) -> String {
    view.composer
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `view` shows an empty composer: no solid text and no paste placeholder. Faint
/// placeholder and hint text, such as Claude Code's `Press up to edit queued messages`, and the
/// reverse-video cursor cell do not count.
#[must_use]
pub fn composer_is_empty(view: &ComposerView) -> bool {
    view.composer_solid.trim().is_empty() && paste_placeholders(&view.composer) == 0
}

/// Delete everything in the composer of `harness` in `pane_id` without submitting it, and prove
/// from the screen that it is empty.
///
/// Sends the [`CLEAR_ROW_KEYS`] pair, reads the screen, and repeats while the composer still
/// holds text and the last pair changed it, for at most [`MAX_CLEAR_ROUNDS`] pairs. A pair that
/// changed nothing is followed by one [`CLEAR_CHARACTER_KEY`]; if that changes nothing either,
/// the clear fails. It also fails when the composer stops being recognisable, as when a dialog
/// opens, or when the caller cancels. No key that submits, interrupts or opens a menu is sent.
/// Returns the number of keys sent; an `Err` says what the composer was left holding, which may
/// be less than it held before.
pub fn clear_composer(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    harness: &str,
    runtime: &dyn AgentRuntime,
) -> std::result::Result<u32, String> {
    clear_editor(
        terminal,
        pane_id,
        harness,
        &|screen| {
            composer_view(harness, screen)
                .map(|view| (composer_content(&view), composer_is_empty(&view)))
        },
        runtime,
    )
}

/// [`clear_composer`] for any editor `editor` can read: given a screen, it returns what the
/// editor holds, compared between reads, and whether it is empty, or `None` when the screen
/// shows no recognisable editor. `label` names the harness in messages.
pub(crate) fn clear_editor(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    label: &str,
    editor: &dyn Fn(&str) -> Option<(String, bool)>,
    runtime: &dyn AgentRuntime,
) -> std::result::Result<u32, String> {
    let harness = label;
    let read = || -> std::result::Result<(String, bool), String> {
        let screen = terminal
            .read_screen(pane_id)
            .map_err(|error| format!("pane {pane_id}: composer read failed: {error}"))?;
        editor(&screen).ok_or_else(|| {
            format!("pane {pane_id} no longer shows a recognisable {harness} composer")
        })
    };
    let send = |key: &str| -> std::result::Result<(), String> {
        if runtime.cancelled() {
            return Err(format!("pane {pane_id}: composer clear was cancelled"));
        }
        terminal
            .send_keys(pane_id, key)
            .map_err(|error| format!("pane {pane_id}: key {key} failed: {error}"))
    };
    let (mut content, mut empty) = read()?;
    let mut keys = 0_u32;
    for _ in 0..MAX_CLEAR_ROUNDS {
        if empty {
            return Ok(keys);
        }
        let before = content;
        for key in CLEAR_ROW_KEYS {
            send(key)?;
            keys += 1;
        }
        runtime.sleep(POLL);
        (content, empty) = read()?;
        if empty {
            return Ok(keys);
        }
        if content == before {
            send(CLEAR_CHARACTER_KEY)?;
            keys += 1;
            runtime.sleep(POLL);
            (content, empty) = read()?;
            if !empty && content == before {
                return Err(format!(
                    "pane {pane_id}: the {harness} composer did not change after {keys} clearing \
                     key(s); it still holds text"
                ));
            }
        }
    }
    if empty {
        return Ok(keys);
    }
    Err(format!(
        "pane {pane_id}: the {harness} composer still holds text after {keys} clearing key(s)"
    ))
}

/// Take a prompt that the agent never accepted back out of its composer, for
/// [`AgentRuntime::withdraw_unaccepted_prompt`]. Nothing was submitted when the composer is
/// proved empty and the screen shows nothing the pane printed after the submission key: the
/// prompt is then reported as not typed, so it stays pending. Any other outcome is an error,
/// which the caller treats as possibly submitted, as it treats a prompt left staged.
#[allow(clippy::too_many_arguments)]
fn withdraw_unaccepted(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    harness: &str,
    shown: &ShownBefore,
    presses: u32,
    key: &str,
    started: Duration,
    runtime: &dyn AgentRuntime,
) -> Result<Submission> {
    let elapsed = runtime.monotonic().saturating_sub(started).as_secs_f64();
    let left = |detail: String| {
        Err(AdapterError::unavailable(format!(
            "pane {pane_id}: prompt is still staged in the {harness} composer after {presses} \
             {key} key press(es) over {elapsed:.1}s; taking it back out failed, so it is left \
             there and treated as possibly submitted: {detail}"
        )))
    };
    if let Err(detail) = clear_composer(terminal, pane_id, harness, runtime) {
        return left(detail);
    }
    let screen = match terminal.read_screen(pane_id) {
        Ok(screen) => screen,
        Err(error) => return left(format!("the screen read after the clear failed: {error}")),
    };
    let Some(view) = composer_view(harness, &screen) else {
        return left(format!(
            "pane {pane_id} no longer shows a recognisable {harness} composer"
        ));
    };
    if !composer_is_empty(&view) {
        return left("the composer is not empty after the clear".to_owned());
    }
    if let Some(evidence) = printed(&view, &screen, shown) {
        return Err(AdapterError::unavailable(format!(
            "pane {pane_id}: the prompt may have been accepted while it was taken back out of the \
             {harness} composer ({evidence}); outcome is unknown"
        )));
    }
    Ok(Submission::NotStaged(format!(
        "pane {pane_id}: the {harness} agent did not accept the prompt after {presses} {key} key \
         press(es) over {elapsed:.1}s; it was taken back out of the composer, which is empty \
         again, so nothing was submitted"
    )))
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
        /// Reads that still show the dialog before the composer is drawn, as a freshly
        /// launched harness does; the dialog closes on the last of them.
        pub dialog_reads: u32,
        /// Label Claude draws into the composer's top border, such as a mode name.
        pub top_border_label: Option<String>,
        /// Glyph Codex draws before its composer: `›` before v0.159.1, `»` from it.
        pub codex_marker: &'static str,
        /// A busy Claude queues a prompt without showing any of it above the composer, as it does
        /// for a prompt queued behind another that it already holds.
        pub queued_hidden: bool,
        /// The clearing keys `ctrl+u` and `backspace` change nothing, as in a composer that
        /// binds them to something else.
        pub ignore_clear_keys: bool,
        /// Every submission key, Enter or Tab, is dropped; editing keys still work.
        pub drop_submit_keys: bool,
        /// Codex draws a paste longer than this many characters as a `[Pasted Content N chars]`
        /// placeholder, faint as some builds draw it (a current build draws it in colour).
        pub codex_collapse_over: Option<usize>,
        /// An idle Codex answers a submitted prompt at once and is idle again.
        pub codex_instant_turns: bool,
        stale_screen: Option<String>,
        pending_reads: u32,
        paste_counter: u32,
        placeholder: Option<String>,
        /// When set, every screen read returns this verbatim (for example an
        /// ansi-blank grid) instead of the rendered composer.
        pub screen_override: Option<String>,
        /// Herdr facts `composer_probe` reports for this pane.
        pub probe: Option<super::ComposerProbe>,
        /// Human keystrokes landing in the composer right after our paste.
        pub human_after_paste: Option<String>,
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
                codex_marker: "›",
                ..State::default()
            }))
        }

        pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().expect("fake screen state")
        }

        pub(crate) fn render(&self) -> String {
            render(&self.state(), false)
        }

        /// The harness puts `text` back in its empty composer, unsent, drawn as a paste of it
        /// would be: a prompt it took and then returned, or one whose key it missed.
        pub(crate) fn leave_draft(&self, text: &str) {
            let mut state = self.state();
            assert!(state.composer.is_empty(), "the composer must be empty");
            stage(&mut state, text);
        }

        /// The harness puts another returned prompt back behind the draft
        /// already in the composer (#825): the placeholders concatenate.
        pub(crate) fn leave_another_draft(&self, text: &str) {
            let mut state = self.state();
            assert!(!state.composer.is_empty(), "the composer must hold a draft");
            stage(&mut state, text);
        }

        /// Codex's Escape during a turn: the turn ends and every input it held for the turn
        /// moves back into the composer, one after another, where Enter submits them.
        pub(crate) fn interrupt(&self) {
            let mut state = self.state();
            assert_eq!(state.harness, "codex");
            state.busy = false;
            state.transcript.retain(|row| {
                !row.contains("Working")
                    && !row.contains("Queued follow-up")
                    && !row.starts_with("  ↳")
            });
            state.transcript.push(
                "■ Conversation interrupted - use /feedback if something went wrong".to_owned(),
            );
            let held = std::mem::take(&mut state.queued);
            state.composer = held.join("\n");
        }
    }

    /// Add a paste of `body` to the composer, drawn as the harness draws one.
    fn stage(state: &mut State, body: &str) {
        state.composer.push_str(body);
        let newlines = body.matches('\n').count();
        if state.harness == "claude" && newlines >= 3 {
            state.paste_counter += 1;
            let label = format!("[Pasted text #{} +{newlines} lines]", state.paste_counter);
            // A second paste behind one already held concatenates its
            // placeholder, as Claude Code draws it (#825).
            state.placeholder = Some(match state.placeholder.take() {
                Some(held) => format!("{held}{label}"),
                None => label,
            });
        }
        if state.harness == "codex"
            && state
                .codex_collapse_over
                .is_some_and(|limit| body.chars().count() > limit)
        {
            state.placeholder = Some(format!("[Pasted Content {} chars]", body.chars().count()));
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
                lines.push(format!(
                    "{} {}",
                    state.codex_marker,
                    dim("Ask Codex to do anything")
                ));
            } else if let Some(placeholder) = &state.placeholder {
                lines.push(format!("{} {}", state.codex_marker, dim(placeholder)));
            } else {
                composer_lines(state.codex_marker, &mut lines);
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
        fn composer_probe(&self, _pane_id: &str, _harness: &str) -> Option<super::ComposerProbe> {
            self.state().probe
        }

        fn read_screen(&self, _pane_id: &str) -> Result<String> {
            let mut state = self.state();
            if let Some(override_screen) = state.screen_override.clone() {
                return Ok(override_screen);
            }
            if state.dialog_reads > 0 {
                state.dialog_reads -= 1;
                state.dialog = state.dialog_reads > 0;
            }
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
            stage(&mut state, body);
            if let Some(human) = state.human_after_paste.take() {
                state.composer.push_str(&human);
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
            if state.drop_submit_keys && matches!(keys, "Enter" | "Tab") {
                return Ok(());
            }
            let before = render(&state, false);
            let text = state.composer.clone();
            if text.is_empty() {
                return Ok(());
            }
            // Editing keys, modelled with the cursor at the end of the composer: `ctrl+e` moves
            // nothing, `ctrl+u` deletes the last row, or on an empty last row the line break
            // before it in Codex only, and `backspace` deletes one character. A paste
            // placeholder goes as a whole.
            match keys {
                "ctrl+e" => return Ok(()),
                "ctrl+u" | "backspace" if state.ignore_clear_keys => return Ok(()),
                "ctrl+u" | "backspace" if state.placeholder.is_some() => {
                    state.composer.clear();
                    state.placeholder = None;
                    return Ok(());
                }
                "ctrl+u" => {
                    let start = text.rfind('\n').map_or(0, |index| index + 1);
                    if start < text.len() {
                        state.composer.truncate(start);
                    } else if state.harness == "codex" {
                        state.composer.truncate(start - 1);
                    }
                    return Ok(());
                }
                "backspace" => {
                    state.composer.pop();
                    return Ok(());
                }
                _ => {}
            }
            match (state.harness, keys) {
                ("claude", "Enter") => {
                    if !(state.busy && state.queued_hidden) {
                        state.transcript.push(format!("❯ {text}"));
                    }
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
                    } else if state.codex_instant_turns {
                        state.submitted.push(text);
                        state.transcript.push("• Done.".to_owned());
                    } else {
                        state.submitted.push(text);
                        state
                            .transcript
                            .push("• Working (0s • esc to interrupt)".to_owned());
                    }
                    state.busy |= !state.codex_instant_turns;
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
        withdraw: AtomicBool,
        /// The queue a drain would name, recording the prompts it typed.
        queue: Option<std::path::PathBuf>,
        /// The follow-up key a drain passes when it types into a working pane.
        followup: Option<&'static str>,
        /// When an earlier pass left the prompt stranded, for a stranded retry (#884).
        stranded_since: Option<f64>,
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

        fn withdraw_unaccepted_prompt(&self) -> bool {
            self.withdraw.load(Ordering::SeqCst)
        }

        fn typed_prompts_queue(&self) -> Option<std::path::PathBuf> {
            self.queue.clone()
        }

        fn followup_key(&self) -> Option<&'static str> {
            self.followup
        }

        fn stranded_retry(&self) -> bool {
            self.stranded_since.is_some()
        }

        fn stranded_since(&self) -> Option<f64> {
            self.stranded_since
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
    fn ansi_blank_means_whitespace_only_once_rendered() {
        assert!(is_ansi_blank(""));
        assert!(is_ansi_blank("   \n\n  \t \n"));
        assert!(is_ansi_blank(
            "\u{1b}[2m   \u{1b}[0m\n\u{1b}[7m \u{1b}[0m\n"
        ));
        assert!(!is_ansi_blank("  x  \n"));
        assert!(!is_ansi_blank(
            FakeScreen::new("codex", false).render().as_str()
        ));
    }

    #[test]
    fn blank_screen_while_working_is_composer_unreadable_and_types_nothing() {
        // The proven defect: Herdr reads 0 bytes of grid while the agent works
        // (OSC title) with its pinned harness in the foreground.
        let screen = FakeScreen::new("codex", true);
        {
            let mut state = screen.state();
            state.screen_override = Some("\u{1b}[0m   \n\n  \t \n".to_owned());
            state.probe = Some(ComposerProbe {
                working: true,
                interactive_ready: true,
                foreground_matches: true,
                scroll_offset_from_bottom: Some(2),
                harness_binary_replaced: false,
            });
        }
        let timeouts = SubmitTimeouts {
            composer: Duration::from_millis(300),
            ..SubmitTimeouts::default()
        };
        let reason = refusal(submit_with(&screen, "run the tests", timeouts).0);
        assert!(is_composer_unreadable(&reason), "{reason}");
        assert!(reason.contains("ansi_blank=true"), "{reason}");
        assert!(reason.contains("scroll_offset_from_bottom=2"), "{reason}");
        assert!(reason.contains("nothing was typed"), "{reason}");
        let state = screen.state();
        assert!(state.pastes.is_empty(), "nothing may be typed: {state:?}");
        assert!(state.keys.is_empty(), "no key may be sent: {state:?}");
    }

    #[test]
    fn the_captured_blank_coordinator_screen_is_classified_as_the_incident_recorded_it() {
        // #834: the start of `herdr pane read --source visible --ansi` of the coordinator pane
        // that refused eleven messages for an hour: rows of CRLF with stray spaces, no text.
        let captured = "\r\n\r\n\r\n\r\n  \r\n  \r\n    \r\n    \r\n\r\n\r\n    \r\n\r\n\r\n\r\n\
                        \r\n\r\n\r\n\r\n\r\n\r\n    \r\n\r\n    \r\n\r\n\r\n\r\n\r\n\r\n  \r\n\
                        \r\n  \r\n    \r\n\r\n\r\n\r\n\r\n   \r\n\r\n\r\n   \r\n\r\n  \r\n      \r\n   ";
        assert!(is_ansi_blank(captured));
        let probe = ComposerProbe {
            working: true,
            interactive_ready: true,
            foreground_matches: true,
            scroll_offset_from_bottom: Some(0),
            harness_binary_replaced: false,
        };
        assert_eq!(
            composer_unreadable_reason(
                "w16:pD",
                "claude",
                captured,
                Some(probe),
                Duration::from_secs(15)
            )
            .as_deref(),
            Some(
                "composer_unreadable: blank_grid: pane w16:pD does not show a recognisable \
                 claude composer after 15.0s; ansi_blank=true scroll_offset_from_bottom=0 \
                 working=true interactive_ready=true foreground_match=true \
                 harness_binary_replaced=false; a resize or redraw does not restore this state: \
                 if it persists, restart the harness child-first with --resume; nothing was typed"
            )
        );
    }

    #[test]
    fn a_replaced_harness_binary_names_the_blank_grid_state_and_its_cure() {
        // Captured 2026-11-03 from a coordinator pane whose Claude Code process
        // had run ~31 h, its binary replaced on disk by an upgrade: the visible
        // grid is whitespace, the harness idle (no title spinner), its
        // foreground process alive. A resize made it write nothing (#836).
        let captured = include_str!("../testdata/blank-grid/claude-renderer-silent.txt");
        assert!(is_ansi_blank(captured));
        let screen = FakeScreen::new("claude", false);
        {
            let mut state = screen.state();
            state.screen_override = Some(captured.to_owned());
            state.probe = Some(ComposerProbe {
                working: false,
                interactive_ready: true,
                foreground_matches: true,
                scroll_offset_from_bottom: Some(0),
                harness_binary_replaced: true,
            });
        }
        let timeouts = SubmitTimeouts {
            composer: Duration::from_millis(300),
            ..SubmitTimeouts::default()
        };
        let reason = refusal(submit_with(&screen, "status?", timeouts).0);
        assert!(is_composer_unreadable(&reason), "{reason}");
        assert!(is_blank_grid(&reason), "{reason}");
        for fact in [
            "working=false",
            "harness_binary_replaced=true",
            "child-first with --resume",
            "nothing was typed",
        ] {
            assert!(reason.contains(fact), "{fact}: {reason}");
        }
        let state = screen.state();
        assert!(
            state.pastes.is_empty() && state.keys.is_empty(),
            "{state:?}"
        );
    }

    #[test]
    fn a_working_blank_grid_is_named_with_its_binary_fact() {
        let reason = composer_unreadable_reason(
            "w1:p1",
            "claude",
            "  \n\n",
            Some(ComposerProbe {
                working: true,
                interactive_ready: true,
                foreground_matches: true,
                scroll_offset_from_bottom: None,
                harness_binary_replaced: false,
            }),
            Duration::from_secs(15),
        )
        .unwrap();
        assert!(is_blank_grid(&reason), "{reason}");
        assert!(reason.contains("harness_binary_replaced=false"), "{reason}");
        assert!(reason.contains("working=true"), "{reason}");
        // A screen with content is never the blank-grid state.
        assert!(composer_unreadable_reason(
            "w1:p1",
            "claude",
            "❯ \n",
            Some(ComposerProbe {
                working: true,
                interactive_ready: true,
                foreground_matches: true,
                scroll_offset_from_bottom: None,
                harness_binary_replaced: true,
            }),
            Duration::from_secs(15),
        )
        .is_none());
        assert!(!is_blank_grid(
            "pane w1:p1 does not show a recognisable claude composer"
        ));
    }

    #[test]
    fn replaced_executables_are_read_from_the_proc_exe_link() {
        assert!(exe_link_replaced(
            "/usr/local/bin/claude_code/claude (deleted)"
        ));
        assert!(!exe_link_replaced("/usr/local/bin/claude_code/claude"));
        assert!(!exe_link_replaced("/opt/deleted/bin/codex"));
    }

    #[test]
    fn blank_screen_without_working_proof_keeps_the_generic_refusal() {
        for probe in [
            None,
            Some(ComposerProbe {
                working: false,
                interactive_ready: true,
                foreground_matches: true,
                scroll_offset_from_bottom: None,
                harness_binary_replaced: false,
            }),
            Some(ComposerProbe {
                working: true,
                interactive_ready: true,
                foreground_matches: false,
                scroll_offset_from_bottom: None,
                harness_binary_replaced: false,
            }),
            Some(ComposerProbe {
                working: false,
                interactive_ready: true,
                foreground_matches: false,
                scroll_offset_from_bottom: None,
                harness_binary_replaced: true,
            }),
        ] {
            let screen = FakeScreen::new("codex", false);
            {
                let mut state = screen.state();
                state.screen_override = Some("   \n\n".to_owned());
                state.probe = probe;
            }
            let timeouts = SubmitTimeouts {
                composer: Duration::from_millis(300),
                ..SubmitTimeouts::default()
            };
            let reason = refusal(submit_with(&screen, "run the tests", timeouts).0);
            assert!(!is_composer_unreadable(&reason), "{reason}");
            assert!(reason.contains("does not show a recognisable"), "{reason}");
            assert!(screen.state().pastes.is_empty());
        }
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
                ..SubmitTimeouts::default()
            },
        );
        let message = refusal(outcome);
        assert!(message.contains("stranded_prompt:"), "{message}");
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
    fn late_composer_is_awaited_before_typing() {
        let screen = FakeScreen::new("claude", false);
        screen.state().dialog = true;
        screen.state().dialog_reads = 3;
        let receipt = receipt(submit(&screen, "the brief"));
        assert_eq!(receipt.key, "Enter");
        let state = screen.state();
        assert_eq!(state.pastes, ["the brief"]);
        assert_eq!(state.submitted, ["the brief"]);
    }

    #[test]
    fn composer_wait_is_bounded_and_types_nothing() {
        let screen = FakeScreen::new("claude", false);
        screen.state().dialog = true;
        let (outcome, elapsed) = submit_with(
            &screen,
            "the brief",
            SubmitTimeouts {
                composer: Duration::from_secs(2),
                ..SubmitTimeouts::default()
            },
        );
        let reason = refusal(outcome);
        assert!(reason.contains("after 2s"), "{reason}");
        assert!(reason.contains("nothing was typed"), "{reason}");
        assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
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

    #[test]
    fn a_fresh_collapsed_paste_is_submitted_on_the_first_attempt() {
        // #884 item 4: a fresh 19-line prompt Claude Code draws only as
        // "[Pasted text #N +18 lines]" is recognised as our paste on the
        // first pass: one paste, one Enter, no stranded retry.
        let screen = FakeScreen::new("claude", false);
        let text = (1..=19)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (outcome, elapsed) = submit_with(&screen, &text, in_lock_timeouts());
        let receipt = receipt(outcome);
        assert_eq!(receipt.key_presses, 1);
        assert!(
            elapsed <= STAGE_IN_LOCK + Duration::from_secs(1),
            "{elapsed:?}"
        );
        let state = screen.state();
        assert_eq!(state.pastes.len(), 1);
        assert_eq!(state.pastes[0], text);
        assert_eq!(state.keys, ["Enter"]);
        assert_eq!(state.submitted, [text]);
    }

    #[test]
    fn a_collapsed_paste_with_a_trailing_newline_is_submitted_on_the_first_attempt() {
        // The typed text ends with a newline; Claude Code's `+k` count for it
        // still names this paste, under either trailing-newline convention.
        let screen = FakeScreen::new("claude", false);
        let text = "line one\nline two\nline three\nline four\nline five\n";
        let receipt = receipt(submit(&screen, text));
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(screen.state().submitted, [text]);
    }

    #[test]
    fn a_lone_chip_counted_as_its_line_count_is_flushed_with_a_bare_enter_on_any_attempt() {
        // #884 follow-up busy-evidence (b): a previous pass left our paste in
        // the composer as one collapsed chip, drawn [Pasted text #264 +3
        // lines] for a 3-line notice (the build counts lines, not line
        // breaks). Even a fresh, non-stranded attempt recognises it through
        // the shared line-count matcher and presses one bare Enter; it never
        // types the text again. Screen shape from a real busy Claude Code
        // pane (chip in the composer, paste-hint footer).
        let text = "watcher notice line one is long enough to wrap across the pane\n\
                    - second line of the notice also long enough to wrap around\n\
                    read the reply and give the agent its next task or stop it";
        let replay = Replay::new(
            claude_screen(&["• earlier"], "[Pasted text #264 +3 lines]", PASTE_STATUS),
            claude_screen(&["• earlier"], "[Pasted text #264 +3 lines]", PASTE_STATUS),
            claude_screen(&["• earlier"], "", IDLE_STATUS),
        );
        let clock = Clock::default();
        let outcome = submit_verified(&replay, "w1:p1", "claude", text, in_lock_timeouts(), &clock);
        let receipt = receipt(outcome);
        assert_eq!(receipt.key_presses, 1, "{receipt:?}");
        assert!(replay.pastes.lock().unwrap().is_empty(), "never retyped");
        assert_eq!(replay.keys.lock().unwrap().as_slice(), ["Enter"]);
    }

    #[test]
    fn a_fresh_delivery_never_submits_a_placeholder_of_another_size() {
        // A placeholder that appears while we wait but is not our paste's
        // size is someone else's: no Enter is pressed on it, and the delivery
        // is reported stranded with what the composer showed (#884 item 4).
        let replay = Replay::new(
            claude_screen(&["• earlier"], "", IDLE_STATUS),
            claude_screen(&["• earlier"], "[Pasted text #999 +99 lines]", PASTE_STATUS),
            claude_screen(&["• earlier"], "", IDLE_STATUS),
        );
        let clock = Clock::default();
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            "line one\nline two\nline three\nline four",
            in_lock_timeouts(),
            &clock,
        );
        let reason = refusal(outcome);
        assert!(
            crate::unknown_harness::is_stranded_prompt(&reason),
            "{reason}"
        );
        assert!(reason.contains("[Pasted text #999 +99 lines]"), "{reason}");
        assert_eq!(replay.pastes.lock().unwrap().len(), 1);
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    /// Rows Codex v0.159.1 drew at the bottom of its pane, read with
    /// `herdr pane read --source visible --format ansi`. Escape sequences, attributes and row
    /// layout are as captured; runs of padding are shortened, and the model name, working
    /// directory, links and prompt text were replaced with neutral words.
    mod codex_v0_159_1 {
        pub(super) const PROMPT: &str = "Reply with exactly READY and nothing else.";
        const SHADED_BLANK: &str = "\u{1b}[0m\u{1b}[48;2;30;30;30m                    \u{1b}[0m";
        const EMPTY_COMPOSER: &str = concat!(
            "\u{1b}[0m\u{1b}[1m\u{1b}[38;2;190;142;250m\u{1b}[48;2;30;30;30m»\u{1b}[0m",
            "\u{1b}[48;2;30;30;30m \u{1b}[0m\u{1b}[2m\u{1b}[48;2;30;30;30mAsk Codex to do anything",
            "\u{1b}[0m\u{1b}[48;2;30;30;30m          \u{1b}[0m",
        );
        const STAGED_COMPOSER: &str = concat!(
            "\u{1b}[0m\u{1b}[1m\u{1b}[38;2;190;142;250m\u{1b}[48;2;30;30;30m»\u{1b}[0m",
            "\u{1b}[48;2;30;30;30m Reply with exactly READY and nothing else.          \u{1b}[0m",
        );
        const FOOTER: &str = concat!(
            "  \u{1b}[0m\u{1b}[38;2;246;226;183mmodel high\u{1b}[0m",
            "\u{1b}[38;2;132;132;132m · \u{1b}[0m\u{1b}[38;2;171;223;167m~/project\u{1b}[0m",
            "          \u{1b}[0m\u{1b}[38;2;132;132;132m⚠ \u{1b}[0m",
            "\u{1b}[38;2;196;167;103m1 warning\u{1b}[0m\u{1b}[38;2;132;132;132m · \u{1b}[0m",
            "\u{1b}[1m\u{1b}[38;2;220;220;220mf2\u{1b}[0m\u{1b}[38;2;132;132;132m to view\u{1b}[0m",
        );
        const SESSION_FOOTER: &str = concat!(
            "  \u{1b}[0m\u{1b}[38;5;6msession\u{1b}[0m\u{1b}[2m · \u{1b}[0m",
            "\u{1b}[38;2;246;226;183mmodel high\u{1b}[0m\u{1b}[38;2;132;132;132m · \u{1b}[0m",
            "\u{1b}[38;2;171;223;167m~/project\u{1b}[0m\u{1b}[38;2;132;132;132m · \u{1b}[0m",
            "\u{1b}[38;2;242;205;205mReply READY\u{1b}[0m          \u{1b}[0m",
        );
        const NOTICE: &str = "\u{1b}[0m\u{1b}[2m• \u{1b}[0mTip: /status shows the session.";
        /// The submitted prompt as the transcript shows it: Codex still draws `›` there.
        const SUBMITTED_PROMPT: &str = concat!(
            "\u{1b}[0m\u{1b}[1m\u{1b}[2m\u{1b}[48;2;40;40;40m› \u{1b}[0m",
            "\u{1b}[48;2;40;40;40mReply with exactly READY and nothing else.          \u{1b}[0m",
        );
        const HOOK: &str = "\u{1b}[0m\u{1b}[2m↳ Hook · \u{1b}[0mDiscovered 3 skills";
        const WORKING: &str = concat!(
            "\u{1b}[0m\u{1b}[1m\u{1b}[38;2;151;151;151m•\u{1b}[0m \u{1b}[0m",
            "\u{1b}[38;2;167;167;167mW\u{1b}[0m\u{1b}[38;2;118;118;118mo\u{1b}[0m",
            "\u{1b}[38;2;110;110;110mrking\u{1b}[0m \u{1b}[0m\u{1b}[2m(0s • \u{1b}[0m",
            "\u{1b}[1m\u{1b}[38;2;220;220;220mesc\u{1b}[0m\u{1b}[2m to interrupt)\u{1b}[0m",
        );
        const REPLY: &str = "\u{1b}[0m\u{1b}[2m• \u{1b}[0mREADY";
        const WORKED: &str = "\u{1b}[0m\u{1b}[2m  Worked for 52s • 12:56 PM\u{1b}[0m";

        fn screen(rows: &[&str]) -> String {
            rows.join("\n") + "\n"
        }

        /// A freshly started session, before any prompt.
        pub(super) fn fresh_idle() -> String {
            screen(&[
                NOTICE,
                "",
                " ",
                SHADED_BLANK,
                EMPTY_COMPOSER,
                SHADED_BLANK,
                FOOTER,
            ])
        }

        /// The same session with [`PROMPT`] pasted into the composer, before the submission key.
        pub(super) fn staged() -> String {
            screen(&[
                NOTICE,
                "",
                " ",
                SHADED_BLANK,
                STAGED_COMPOSER,
                SHADED_BLANK,
                FOOTER,
            ])
        }

        /// The first redraw after Enter: the prompt moved into the transcript.
        pub(super) fn submitted() -> String {
            screen(&[
                NOTICE,
                "",
                SUBMITTED_PROMPT,
                "",
                " ",
                SHADED_BLANK,
                EMPTY_COMPOSER,
                SHADED_BLANK,
                FOOTER,
            ])
        }

        /// The turn is running; the composer stays drawn below the progress row.
        pub(super) fn working() -> String {
            screen(&[
                SUBMITTED_PROMPT,
                "",
                "",
                HOOK,
                " ",
                WORKING,
                " ",
                SHADED_BLANK,
                EMPTY_COMPOSER,
                SHADED_BLANK,
                FOOTER,
            ])
        }

        /// The working screen with `rows` drawn between the progress row and the composer, where
        /// Codex lists the messages it holds for the running turn, and `draft` in the composer.
        pub(super) fn working_with(rows: &[&str], draft: &str) -> String {
            let composer = if draft.is_empty() {
                EMPTY_COMPOSER.to_owned()
            } else {
                format!(
                    "\u{1b}[0m\u{1b}[1m\u{1b}[38;2;190;142;250m\u{1b}[48;2;30;30;30m»\u{1b}[0m\
                     \u{1b}[48;2;30;30;30m {draft}          \u{1b}[0m"
                )
            };
            let mut all = vec![SUBMITTED_PROMPT, "", "", HOOK, " ", WORKING];
            all.extend_from_slice(rows);
            all.extend([" ", SHADED_BLANK, composer.as_str(), SHADED_BLANK, FOOTER]);
            screen(&all)
        }

        /// The working screen with its composer rows removed.
        pub(super) fn working_without_composer() -> String {
            screen(&[SUBMITTED_PROMPT, "", "", HOOK, " ", WORKING, " ", FOOTER])
        }

        /// Idle again after a finished turn, with the session footer.
        pub(super) fn session_idle() -> String {
            screen(&[
                SUBMITTED_PROMPT,
                "",
                REPLY,
                "",
                WORKED,
                "",
                " ",
                SHADED_BLANK,
                EMPTY_COMPOSER,
                SHADED_BLANK,
                SESSION_FOOTER,
            ])
        }

        /// The folder-trust question Codex asks before its first composer in a new directory.
        pub(super) fn trust_prompt() -> String {
            screen(&[
                SHADED_BLANK,
                "\u{1b}[0m\u{1b}[1m\u{1b}[48;2;30;30;30m  Folder access\u{1b}[0m\u{1b}[48;2;30;30;30m          \u{1b}[0m",
                "\u{1b}[0m\u{1b}[48;2;30;30;30m  \u{1b}[0m\u{1b}[2m\u{1b}[48;2;30;30;30m/tmp/project          \u{1b}[0m\u{1b}[48;2;30;30;30m  \u{1b}[0m",
                SHADED_BLANK,
                "\u{1b}[0m\u{1b}[48;2;30;30;30m  Trust this folder? Codex can read, edit, and run files here.   \u{1b}[0m",
                "\u{1b}[0m\u{1b}[48;2;30;30;30m  Continue only if you trust these files.          \u{1b}[0m",
                SHADED_BLANK,
                "\u{1b}[0m\u{1b}[1m\u{1b}[38;2;0;0;46m\u{1b}[48;2;99;168;248m› 1. Trust and continue          \u{1b}[0m",
                "\u{1b}[0m\u{1b}[48;2;30;30;30m  2. Quit          \u{1b}[0m",
                SHADED_BLANK,
                concat!(
                    "\u{1b}[0m\u{1b}[48;2;30;30;30m  \u{1b}[0m\u{1b}[1m\u{1b}[38;2;220;220;220m",
                    "\u{1b}[48;2;30;30;30menter\u{1b}[0m\u{1b}[2m\u{1b}[48;2;30;30;30m continue · \u{1b}[0m",
                    "\u{1b}[1m\u{1b}[38;2;220;220;220m\u{1b}[48;2;30;30;30mesc\u{1b}[0m\u{1b}[2m",
                    "\u{1b}[48;2;30;30;30m quit\u{1b}[0m\u{1b}[48;2;30;30;30m          \u{1b}[0m",
                ),
            ])
        }
    }

    /// Rows Claude Code drew at the bottom of its pane while a turn was running, read with
    /// `herdr pane read --source visible --format ansi`: before a paste, with an 11-line paste
    /// collapsed into a placeholder, and after one Enter queued that paste. The composer, rule
    /// and footer rows are as captured, escape sequences included, except that runs of padding
    /// and rule characters are shortened and the placeholder carries the paste number of the
    /// session the bridge failed on. The rows above them are simplified. Among the differences:
    /// their words are neutral, a 10-row file preview and a hint row are left out, the later
    /// frames reuse the first frame's transcript rows and the staged frame its spinner row, and
    /// the queued prompt rows lack their trailing reset.
    mod claude_busy {
        /// Eleven lines, which Claude Code collapses into `[Pasted text #N +10 lines]`.
        pub(super) const PROMPT: &str = concat!(
            "Reply with one line for each item below.\n",
            "item 2\nitem 3\nitem 4\nitem 5\nitem 6\nitem 7\nitem 8\nitem 9\nitem 10\nitem 11",
        );
        const TOOL_CALL: &str =
            "\u{1b}[0m\u{1b}[38;5;114m● \u{1b}[0m\u{1b}[1mRead\u{1b}[0m(notes.md)";
        const TOOL_RESULT: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;246m  ⎿ \u{a0}\u{1b}[0mRead \u{1b}[0m\u{1b}[1m45\u{1b}[0m",
            " lines\u{1b}[0m",
        );
        const RUNNING: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;246m  \u{1b}[0mRunning the test suite\u{1b}[0m",
            "\u{1b}[38;5;246m · 3s\u{1b}[0m",
        );
        const RUNNING_COMMAND: &str = "\u{1b}[0m\u{1b}[38;5;246m  ⎿  $ cargo test\u{1b}[0m";
        const SPINNER: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;174m·\u{1b}[0m \u{1b}[0m\u{1b}[38;5;180mFermenting…\u{1b}[0m",
            "\u{1b}[38;5;174m \u{1b}[0m\u{1b}[38;5;246m(16m 8s · ↓\u{1b}[0m \u{1b}[0m",
            "\u{1b}[38;5;246m12.3k tokens)\u{1b}[0m",
        );
        /// The spinner just after Enter, while Claude Code runs its prompt hooks.
        const SPINNER_HOOKS: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;174m✢\u{1b}[0m \u{1b}[0m\u{1b}[38;5;216mFermenting…\u{1b}[0m",
            "\u{1b}[38;5;174m \u{1b}[0m\u{1b}[38;5;246m(running UserPromptSubmit hooks… 8/10 · ",
            "16m 9s · ↓ 12.3k tokens)\u{1b}[0m",
        );
        const PADDING: &str = "                    ";
        const RULE: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;244m",
            "────────────────────────────────────────",
            "\u{1b}[0m",
        );
        const EMPTY_COMPOSER: &str = "\u{1b}[0m\u{1b}[38;5;246m❯\u{a0}\u{1b}[0m\u{1b}[7m \u{1b}[0m";
        const PASTED_COMPOSER: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;246m❯\u{a0}\u{1b}[0m[Pasted text #923 +10 lines]\u{1b}[0m",
            "\u{1b}[7m \u{1b}[0m",
        );
        const QUEUED_COMPOSER: &str = concat!(
            "\u{1b}[0m\u{1b}[38;5;246m❯\u{a0}\u{1b}[0m\u{1b}[7mP\u{1b}[0m",
            "\u{1b}[2mress up to edit queued messages\u{1b}[0m",
        );
        const WORKING_FOOTER: &str = concat!(
            "  \u{1b}[0m\u{1b}[38;5;220m⏵⏵ auto mode on\u{1b}[0m",
            "\u{1b}[38;5;246m (shift+tab to cycle) · esc to interrupt · ← 2 agents\u{1b}[0m",
        );
        /// While a placeholder is in the composer, this replaces the running-turn footer.
        const PASTE_FOOTER: &str = "  \u{1b}[0m\u{1b}[38;5;246mpaste again to expand\u{1b}[0m";

        fn screen(rows: &[&str]) -> String {
            rows.join("\n") + "\n"
        }

        /// The running turn with an empty composer, before the paste.
        pub(super) fn before() -> String {
            screen(&[
                TOOL_CALL,
                TOOL_RESULT,
                "",
                RUNNING,
                RUNNING_COMMAND,
                "",
                SPINNER,
                PADDING,
                RULE,
                EMPTY_COMPOSER,
                RULE,
                WORKING_FOOTER,
            ])
        }

        /// The same turn with [`PROMPT`] pasted: only its placeholder is drawn.
        pub(super) fn staged() -> String {
            screen(&[
                TOOL_CALL,
                TOOL_RESULT,
                "",
                RUNNING,
                RUNNING_COMMAND,
                "",
                SPINNER,
                PADDING,
                RULE,
                PASTED_COMPOSER,
                RULE,
                PASTE_FOOTER,
            ])
        }

        /// After one Enter: the prompt is queued above the spinner, in full.
        pub(super) fn queued() -> String {
            let mut rows = vec![
                TOOL_CALL.to_owned(),
                TOOL_RESULT.to_owned(),
                String::new(),
                RUNNING.to_owned(),
                RUNNING_COMMAND.to_owned(),
                "     ".to_owned(),
            ];
            for (index, line) in PROMPT.lines().enumerate() {
                let lead = if index == 0 {
                    "\u{1b}[0m\u{1b}[38;5;239m\u{1b}[48;5;237m❯ "
                } else {
                    "\u{1b}[0m\u{1b}[48;5;237m  "
                };
                rows.push(format!(
                    "{lead}\u{1b}[0m\u{1b}[38;5;246m\u{1b}[48;5;237m{line}\u{1b}[0m\u{1b}[48;5;237m{PADDING}"
                ));
            }
            rows.extend(
                [
                    "",
                    SPINNER_HOOKS,
                    PADDING,
                    RULE,
                    QUEUED_COMPOSER,
                    RULE,
                    PASTE_FOOTER,
                ]
                .map(str::to_owned),
            );
            rows.join("\n") + "\n"
        }
    }

    /// Plays captured screens back: one before the paste, one while the paste is staged,
    /// and one after the submission key. The staged screen can be held back for a number of
    /// reads, as when a busy agent draws a paste late. Every key press is recorded.
    struct Replay {
        frames: [String; 3],
        phase: std::sync::Mutex<usize>,
        /// Reads after the paste that still return the screen from before it.
        undrawn_reads: std::sync::Mutex<u32>,
        pastes: std::sync::Mutex<Vec<String>>,
        keys: std::sync::Mutex<Vec<String>>,
    }

    impl Replay {
        fn new(before: String, staged: String, after: String) -> Self {
            Self {
                frames: [before, staged, after],
                phase: std::sync::Mutex::new(0),
                undrawn_reads: std::sync::Mutex::new(0),
                pastes: std::sync::Mutex::default(),
                keys: std::sync::Mutex::default(),
            }
        }

        /// Draw the paste only on the read after `reads` reads that miss it.
        fn drawn_after(self, reads: u32) -> Self {
            *self.undrawn_reads.lock().unwrap() = reads;
            self
        }
    }

    impl PromptTerminal for Replay {
        fn read_screen(&self, _pane_id: &str) -> Result<String> {
            let phase = *self.phase.lock().unwrap();
            let mut undrawn = self.undrawn_reads.lock().unwrap();
            if phase == 1 && *undrawn > 0 {
                *undrawn -= 1;
                return Ok(self.frames[0].clone());
            }
            Ok(self.frames[phase].clone())
        }

        fn send_text(&self, _pane_id: &str, text: &str) -> Result<()> {
            self.pastes.lock().unwrap().push(text.to_owned());
            *self.phase.lock().unwrap() = 1;
            Ok(())
        }

        fn send_keys(&self, _pane_id: &str, keys: &str) -> Result<()> {
            self.keys.lock().unwrap().push(keys.to_owned());
            if keys == "Enter" {
                *self.phase.lock().unwrap() = 2;
            }
            Ok(())
        }
    }

    #[test]
    fn codex_0_159_1_idle_composer_is_recognised() {
        for screen in [codex_v0_159_1::fresh_idle(), codex_v0_159_1::session_idle()] {
            let view = composer_view("codex", &screen)
                .unwrap_or_else(|| panic!("no composer in {screen:?}"));
            assert_eq!(view.composer_solid.trim(), "", "{view:?}");
            assert!(
                view.composer.contains("Ask Codex to do anything"),
                "{view:?}"
            );
            assert!(view.footer.contains("~/project"), "{view:?}");
            assert!(!view.transcript.contains("Ask Codex"), "{view:?}");
        }
        let view = composer_view("codex", &codex_v0_159_1::session_idle()).unwrap();
        assert!(view.transcript.contains("Worked for 52s"), "{view:?}");
    }

    #[test]
    fn herdr_093_codex_idle_composer_accepts_only_recognized_empty_idle_frames() {
        for screen in [codex_v0_159_1::fresh_idle(), codex_v0_159_1::session_idle()] {
            assert!(codex_idle_composer(&screen), "{screen:?}");
        }
        for marker in ['›', '»'] {
            for footer in [
                "? for shortcuts",
                "100% context left",
                "82% context remaining",
                "82.5% context remaining",
                "model high · /work/project",
            ] {
                let screen = format!(
                    "Codex\n{marker} \u{1b}[2mAsk Codex to do anything\u{1b}[0m\n  {footer}\n"
                );
                assert!(codex_idle_composer(&screen), "{screen:?}");
            }
        }
        for screen in [
            codex_v0_159_1::working(),
            codex_v0_159_1::staged(),
            codex_v0_159_1::trust_prompt(),
            "Codex\n» draft text\n  ? for shortcuts\n".to_owned(),
            "Codex\n» \u{1b}[2m[Pasted text #4 +12 lines]\u{1b}[0m\n  ? for shortcuts\n".to_owned(),
            "Codex\n» \u{1b}[2m[Pasted Content 50 chars]\u{1b}[0m\n  ? for shortcuts\n".to_owned(),
            "shell$\n".to_owned(),
            "Codex\n»\n  arbitrary footer\n".to_owned(),
        ] {
            assert!(!codex_idle_composer(&screen), "{screen:?}");
        }
    }

    #[test]
    fn herdr_093_codex_idle_composer_refuses_current_attention_cues_without_scanning_history() {
        for cue in [
            "esc to interrupt",
            "tab to queue message",
            "Press up to edit queued messages",
            "edit last queued message",
            "Queued follow-up inputs",
            "paste again to expand",
            "Trust this folder?",
            "Trust and continue",
            "Approval required",
            "requires approval",
            "Approve this",
            "Would you like to run",
            "Press enter to continue",
            "Enter to confirm",
            "Esc to cancel",
        ] {
            let screen = format!("{cue}\n»\n  ? for shortcuts\n");
            assert!(!codex_idle_composer(&screen), "{cue}");
        }
        let screen = format!(
            "Approval required in an old quoted message\n{}»\n  ? for shortcuts\n",
            "prior answer\n".repeat(20)
        );
        assert!(codex_idle_composer(&screen));
    }

    #[test]
    fn herdr_093_unknown_codex_live_timers_follow_the_source_grammar_and_response_tail() {
        let timer = regex::Regex::new(
            r"^(?:[•◦][ \t]+)?[^\s›•◦■✗✓─][^\r\n]* \((?:[0-9]+[hm] )*[0-9]+s(?: • [^\r\n]+? to interrupt)?\)(?: · [^\r\n]*)?$"
        ).unwrap();
        for row in [
            "Working (7s)",
            "Working (7s)   ",
            "◦ Exploring (1m 2s • Ctrl+c to interrupt)",
            "• Working (7s • esc to interrupt)",
            "Processing inputs (7s) · command (background)",
            "Working (1h 2m 3s • remapped (key) to interrupt) · model",
            "• Thinking about changes (12s)",
            "› old prompt (7s)",
            "■ prior response (7s)",
            "Working (soon)",
            "Working (7s) extra",
            "Working (1m 2s • to interrupt)",
        ] {
            assert_eq!(
                codex_timer_row(row),
                timer.is_match(row.trim_end()),
                "{row:?}"
            );
        }
        for row in [
            "Working (7s)",
            "Working (7s)   ",
            "◦ Exploring (1m 2s • Ctrl+c to interrupt)",
            "• Working (7s • esc to interrupt)",
            "Processing inputs (7s) · command (background)",
            "Working (1h 2m 3s • remapped (key) to interrupt) · model",
            "Working (7s)\u{1b}[48;5;235m   \u{1b}[0m",
        ] {
            let screen = format!("{row}\n»\n  ? for shortcuts\n");
            assert!(!codex_idle_composer(&screen), "active {row:?}");
            let screen = format!("{row}\n■ completed response\n»\n  ? for shortcuts\n");
            assert!(codex_idle_composer(&screen), "completed {row:?}");
        }
        let queued = format!(
            "Working (7s)\n• Messages to be submitted after next tool call\n{}»\n  ? for shortcuts\n",
            "queued body\n".repeat(30)
        );
        assert!(!codex_idle_composer(&queued));
        let complete = format!(
            "Working (7s)\n• Messages to be submitted after next tool call\n{}✓ completed response\n»\n  ? for shortcuts\n",
            "queued body\n".repeat(30)
        );
        assert!(codex_idle_composer(&complete));
        let queued_uppercase =
            queued.replace("Messages to be submitted", "MESSAGES TO BE SUBMITTED");
        assert!(!codex_idle_composer(&queued_uppercase));
    }

    #[test]
    fn herdr_093_unknown_codex_refuses_current_source_dialogs_and_grouped_search_menu() {
        for cue in [
            "enter to submit answer",
            "enter to submit all",
            "allow command?",
            "Do you trust the contents of this directory?",
            "All Results   Filesystem Only   Plugins",
            "All Results\nFilesystem Only\nPlugins",
        ] {
            let screen = format!("{cue}\n»\n  ? for shortcuts\n");
            assert!(!codex_idle_composer(&screen), "{cue}");
        }
        for cue in ["All Results", "Plugins", "All Results   Filesystem Only"] {
            let screen = format!("{cue}\n»\n  ? for shortcuts\n");
            assert!(codex_idle_composer(&screen), "incomplete menu {cue}");
        }
        let history = format!(
            "All Results   Filesystem Only   Plugins\n{}»\n  ? for shortcuts\n",
            "prior answer\n".repeat(20)
        );
        assert!(codex_idle_composer(&history));
    }

    #[test]
    fn codex_0_159_1_prompt_is_staged_submitted_and_verified() {
        let replay = Replay::new(
            codex_v0_159_1::fresh_idle(),
            codex_v0_159_1::staged(),
            codex_v0_159_1::submitted(),
        );
        let clock = Clock::default();
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "codex",
            codex_v0_159_1::PROMPT,
            SubmitTimeouts::default(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert_eq!(receipt.key, "Enter");
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(receipt.evidence, "prompt text appeared above the composer");
        assert_eq!(
            *replay.pastes.lock().unwrap(),
            [format!(
                "{BRACKETED_PASTE_START}{}{BRACKETED_PASTE_END}",
                codex_v0_159_1::PROMPT
            )]
        );
        assert_eq!(*replay.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn codex_0_159_1_working_screen_keeps_its_progress_row_out_of_the_draft() {
        let view = composer_view("codex", &codex_v0_159_1::working()).unwrap();
        assert_eq!(view.composer_solid.trim(), "", "{view:?}");
        assert!(view.transcript.contains(WORKING_MARKER), "{view:?}");
        assert!(!view.footer.contains(CODEX_QUEUE_HINT), "{view:?}");
    }

    #[test]
    fn codex_0_159_1_screens_without_a_composer_are_refused_before_typing() {
        assert!(composer_view("codex", &codex_v0_159_1::working_without_composer()).is_none());
        for screen in [
            codex_v0_159_1::working_without_composer(),
            codex_v0_159_1::trust_prompt(),
        ] {
            let replay = Replay::new(screen.clone(), screen.clone(), screen);
            let clock = Clock::default();
            let outcome = submit_verified(
                &replay,
                "w1:p1",
                "codex",
                "hello",
                SubmitTimeouts::default(),
                &clock,
            );
            let reason = refusal(outcome);
            assert!(reason.contains("nothing was typed"), "{reason}");
            assert!(replay.pastes.lock().unwrap().is_empty());
            assert!(replay.keys.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn codex_flows_hold_under_both_composer_markers() {
        for marker in ["›", "»"] {
            let idle = FakeScreen::new("codex", false);
            idle.state().codex_marker = marker;
            let view = composer_view("codex", &idle.render()).expect(marker);
            assert_eq!(view.composer_solid.trim(), "", "{marker}");
            assert_eq!(receipt(submit(&idle, "start now")).key, "Enter", "{marker}");
            assert_eq!(idle.state().submitted, ["start now"], "{marker}");

            let busy = FakeScreen::new("codex", true);
            busy.state().codex_marker = marker;
            busy.state().drop_keys = 1;
            assert_eq!(
                receipt(submit(&busy, "follow up later")).key,
                "Tab",
                "{marker}"
            );
            assert_eq!(busy.state().queued, ["follow up later"], "{marker}");
            assert!(busy.state().steered.is_empty(), "{marker}");

            let drafted = FakeScreen::new("codex", false);
            drafted.state().codex_marker = marker;
            drafted.state().composer = "someone else's words".to_owned();
            let reason = refusal(submit(&drafted, "mine"));
            assert!(reason.contains("refusing to append"), "{marker}: {reason}");
            assert!(drafted.state().pastes.is_empty(), "{marker}");

            let dialog = FakeScreen::new("codex", false);
            dialog.state().codex_marker = marker;
            dialog.state().dialog = true;
            let reason = refusal(submit(&dialog, "hello"));
            assert!(
                reason.contains("recognisable codex composer"),
                "{marker}: {reason}"
            );
            assert!(dialog.state().pastes.is_empty(), "{marker}");
        }
    }

    #[test]
    fn busy_claude_screens_show_the_paste_only_as_a_placeholder() {
        let before = composer_view("claude", &claude_busy::before()).expect("busy composer");
        assert_eq!(before.composer_solid.trim(), "", "{before:?}");
        assert_eq!(paste_placeholders(&before.composer), 0, "{before:?}");
        assert!(before.footer.contains(WORKING_MARKER), "{before:?}");

        let pasted = composer_view("claude", &claude_busy::staged()).expect("pasted composer");
        assert_eq!(
            pasted.composer_solid.trim(),
            "[Pasted text #923 +10 lines]",
            "{pasted:?}"
        );
        assert!(staged(&pasted, claude_busy::PROMPT, 0), "{pasted:?}");
        // The footer stops saying that a turn is running while the placeholder is drawn.
        assert!(!pasted.footer.contains(WORKING_MARKER), "{pasted:?}");
        assert!(
            pasted.footer.contains("paste again to expand"),
            "{pasted:?}"
        );

        let queued = composer_view("claude", &claude_busy::queued()).expect("queued composer");
        assert_eq!(queued.composer_solid.trim(), "", "{queued:?}");
        assert!(!staged(&queued, claude_busy::PROMPT, 0), "{queued:?}");
        assert_eq!(
            corroborated(&before, &queued, claude_busy::PROMPT).as_deref(),
            Some("prompt text appeared above the composer")
        );
    }

    #[test]
    fn placeholder_left_in_a_busy_claude_composer_holds_the_next_prompt() {
        let screen = claude_busy::staged();
        let replay = Replay::new(screen.clone(), screen.clone(), screen);
        let reason = refusal(submit_verified(
            &replay,
            "w1:p1",
            "claude",
            "the next prompt",
            SubmitTimeouts::default(),
            &Clock::default(),
        ));
        assert!(reason.contains("refusing to append"), "{reason}");
        assert!(replay.pastes.lock().unwrap().is_empty());
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    // Verified submission once saw no paste in a busy Claude Code composer within 3 s of
    // typing it; the paste was there later, unsubmitted, and held later prompts:
    // https://github.com/rrnewton/agent-utils/issues/215
    #[test]
    fn busy_claude_paste_drawn_late_is_submitted_with_one_enter() {
        let replay = Replay::new(
            claude_busy::before(),
            claude_busy::staged(),
            claude_busy::queued(),
        )
        .drawn_after(45);
        let clock = Clock::default();
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            SubmitTimeouts::default(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert_eq!(receipt.key, "Enter");
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(receipt.evidence, "prompt text appeared above the composer");
        // Reads 100 ms apart missed the placeholder for 4.5 s; the evidence came one poll after
        // the Enter.
        assert_eq!(receipt.elapsed, Duration::from_millis(4_600));
        assert_eq!(
            *replay.pastes.lock().unwrap(),
            [format!(
                "{BRACKETED_PASTE_START}{}{BRACKETED_PASTE_END}",
                claude_busy::PROMPT
            )]
        );
        assert_eq!(*replay.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_paste_taken_straight_into_the_queued_messages_is_delivered_without_a_key() {
        // #884 follow-up busy-evidence (a), from a real busy Claude Code
        // pane: the paste never stages in the composer; the prompt appears
        // in full in the queued messages above the composer, the composer
        // holds the faint queued-messages hint, and the footer shows the
        // paste hint. No submission key is sent; the receipt is queued.
        let replay = Replay::new(
            claude_busy::before(),
            claude_busy::queued(),
            claude_busy::queued(),
        );
        let clock = Clock::default();
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            in_lock_timeouts(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert!(receipt.queued, "{receipt:?}");
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(receipt.key_presses, 0, "{receipt:?}");
        assert!(replay.keys.lock().unwrap().is_empty());
        assert_eq!(replay.pastes.lock().unwrap().len(), 1, "typed once");
    }

    #[test]
    fn a_prompt_entered_into_the_queued_messages_as_a_chip_is_delivered_queued() {
        // #884 follow-up busy-evidence (a): after Enter, the queued area
        // holds only our collapsed placeholder, not the full prompt.
        let replay = Replay::new(
            claude_screen(&["• earlier"], "", BUSY_STATUS),
            claude_screen(&["• earlier"], "[Pasted text #924 +10 lines]", PASTE_STATUS),
            claude_screen(
                &["• earlier", "❯ [Pasted text #924 +10 lines]"],
                QUEUED_PLACEHOLDER,
                PASTE_STATUS,
            ),
        );
        let clock = Clock::default();
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            SubmitTimeouts::default(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert!(receipt.queued, "{receipt:?}");
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(replay.keys.lock().unwrap().as_slice(), ["Enter"]);
        assert_eq!(replay.pastes.lock().unwrap().len(), 1, "typed once");
    }

    #[test]
    fn a_stranded_prompt_found_in_the_queued_messages_is_delivered_never_lost() {
        // #884 follow-up busy-evidence (a): an earlier pass typed the prompt
        // and never saw it in the composer, because Claude Code had taken
        // it into its queued messages. The stranded pass finds it there and
        // records it delivered (queued), not late_paste_lost, and never
        // types it again.
        let replay = Replay::new(
            claude_busy::queued(),
            claude_busy::queued(),
            claude_busy::queued(),
        );
        let clock = Clock {
            stranded_since: Some(unix_seconds()),
            ..Clock::default()
        };
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            in_lock_timeouts(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert!(receipt.queued, "{receipt:?}");
        assert!(receipt.printed, "{receipt:?}");
        assert!(replay.pastes.lock().unwrap().is_empty(), "never retyped");
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn a_stranded_prompt_found_as_a_user_turn_is_delivered() {
        // #884 follow-up busy-evidence (a): the stranded prompt later
        // appears in the transcript as a user turn (its queued turn came
        // up). That is delivery, not a lost paste.
        let mut rows: Vec<String> = vec!["• earlier".to_owned()];
        for (index, line) in claude_busy::PROMPT.lines().enumerate() {
            rows.push(if index == 0 {
                format!("❯ {line}")
            } else {
                format!("  {line}")
            });
        }
        let screen = claude_screen(
            &rows.iter().map(String::as_str).collect::<Vec<_>>(),
            "",
            BUSY_STATUS,
        );
        let replay = Replay::new(screen.clone(), screen.clone(), screen);
        let clock = Clock {
            stranded_since: Some(unix_seconds()),
            ..Clock::default()
        };
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            in_lock_timeouts(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert!(!receipt.queued, "{receipt:?}");
        assert!(receipt.printed, "{receipt:?}");
        assert!(replay.pastes.lock().unwrap().is_empty(), "never retyped");
    }

    #[test]
    fn busy_claude_paste_never_drawn_gives_up_at_the_stage_timeout_without_a_key() {
        let replay = Replay::new(
            claude_busy::before(),
            claude_busy::staged(),
            claude_busy::queued(),
        )
        .drawn_after(u32::MAX);
        let clock = Clock::default();
        let message = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            SubmitTimeouts::default(),
            &clock,
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("within 300.0s; no submission key was sent"),
            "{message}"
        );
        assert_eq!(
            clock.monotonic() - Duration::from_millis(1_000_000),
            STAGE_TIMEOUT + STAGE_GRACE
        );
        assert_eq!(replay.pastes.lock().unwrap().len(), 1);
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn paste_drawn_70s_late_is_delivered_with_one_enter() {
        let screen = FakeScreen::new("claude", true);
        // 700 reads at 100 ms = 70 s before the paste is drawn.
        screen.state().paste_visible_after_reads = 700;
        let receipt = receipt(submit(&screen, "flush the stranded paste please"));
        assert_eq!(receipt.key_presses, 1);
        let state = screen.state();
        assert_eq!(state.keys, ["Enter"]);
        assert_eq!(state.submitted.len() + state.queued.len(), 1);
    }

    #[test]
    fn paste_drawn_late_as_placeholder_is_delivered_with_one_enter() {
        let screen = FakeScreen::new("claude", true);
        screen.state().paste_visible_after_reads = 700;
        let text = "line one of the stranded paste\nline two\nline three\nline four";
        let receipt = receipt(submit(&screen, text));
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(screen.state().keys, ["Enter"]);
    }

    #[test]
    fn an_unobserved_paste_under_the_locks_is_reported_stranded_within_seconds() {
        // #884 pane-lock-held-on-unobserved-paste: a paste that does not render must not hold
        // the pane's locks for the late-paste grace; the caller gets a stranded refusal at the
        // short in-lock stage window, no Enter sent, and what the composer showed.
        let screen = FakeScreen::new("claude", true);
        screen.state().paste_visible_after_reads = u32::MAX;
        let (outcome, elapsed) = submit_with(&screen, "lost paste never drawn", in_lock_timeouts());
        let reason = refusal(outcome);
        assert!(
            crate::unknown_harness::is_stranded_prompt(&reason),
            "{reason}"
        );
        assert!(reason.contains("late_paste:"), "{reason}");
        assert!(reason.contains("the composer showed"), "{reason}");
        assert!(
            elapsed <= STAGE_IN_LOCK + Duration::from_secs(1),
            "{elapsed:?}"
        );
        assert!(screen.state().keys.is_empty());
        assert_eq!(screen.state().pastes.len(), 1);
    }

    #[test]
    fn an_unobserved_paste_on_an_idle_pane_under_the_locks_is_reported_stranded() {
        // A paste to an idle or done Claude Code pane can render after the
        // in-lock stage window (its composer is verifiably empty at the
        // deadline). It is reported stranded like a busy pane's, so the
        // prompt stays pending and a later pass flushes the paste once it
        // shows; it is never quarantined as possibly submitted.
        let screen = FakeScreen::new("claude", false);
        screen.state().paste_visible_after_reads = u32::MAX;
        let (outcome, elapsed) =
            submit_with(&screen, "late paste on an idle pane", in_lock_timeouts());
        let reason = refusal(outcome);
        assert!(
            crate::unknown_harness::is_stranded_prompt(&reason),
            "{reason}"
        );
        assert!(reason.contains("late_paste:"), "{reason}");
        assert!(
            elapsed <= STAGE_IN_LOCK + Duration::from_secs(1),
            "{elapsed:?}"
        );
        assert!(screen.state().keys.is_empty());
        assert_eq!(screen.state().pastes.len(), 1);
    }

    fn stranded_clock(seconds_ago: f64) -> Clock {
        Clock {
            stranded_since: Some(unix_seconds() - seconds_ago),
            ..Clock::default()
        }
    }

    #[test]
    fn a_stranded_retry_never_types_again_while_the_paste_may_still_show() {
        let screen = FakeScreen::new("claude", false);
        let clock = stranded_clock(30.0);
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "the stranded prompt",
            in_lock_timeouts(),
            &clock,
        );
        let reason = refusal(outcome);
        assert!(
            crate::unknown_harness::is_stranded_prompt(&reason),
            "{reason}"
        );
        assert!(screen.state().pastes.is_empty(), "nothing typed again");
        assert!(screen.state().keys.is_empty());
    }

    /// A prompt as long as the messages agents send each other, one line of it per item.
    const LONG_STRANDED: &str = concat!(
        "[from coordinator via agentctl · reply: agentctl send coordinator]\n",
        "Coordinator: the evidence for the busy-pane issue, without screens but with rows:\n",
        "- every false failure today is a receipt-backed not-seen row on the coordinator pane,\n",
        "  each from the liaison or the watcher while the coordinator ran a long tool call;\n",
        "- six acknowledged entries in the failed archive, every one of which did arrive;\n",
        "Pattern: all arrived mid-turn, so the five-second check ran while input was queued.\n",
        "Repro: keep a Claude pane busy with a long tool call and send two or three prompts.",
    );

    /// A busy Claude Code screen that drew `prompt`, taken at a tool boundary, among the result
    /// rows of the tool it ran: without its first two rows and its last, as captured live.
    fn taken_at_tool_boundary(prompt: &str) -> String {
        let lines: Vec<&str> = prompt.lines().collect();
        let mut rows = vec![
            "● Bash(grep -n receipt src/read_back.rs)".to_owned(),
            "  ⎿  113:pub struct SubmissionReceipt {".to_owned(),
            "     … +17 lines (ctrl+o to expand)".to_owned(),
        ];
        rows.extend(
            lines[2..lines.len() - 1]
                .iter()
                .map(|line| format!("  {line}")),
        );
        rows.push(String::new());
        rows.push("● Reading the rest of the receipt.".to_owned());
        claude_screen(
            &rows.iter().map(String::as_str).collect::<Vec<_>>(),
            "",
            BUSY_STATUS,
        )
    }

    #[test]
    fn a_stranded_prompt_taken_at_a_tool_boundary_is_delivered_never_lost() {
        // #953 agentctl-busy-claude-false-possibly-submitted: Claude Code took the stranded
        // prompt into its running turn at a tool boundary and drew it among that tool's result
        // rows, cut at both ends, so neither its tail nor the whole prompt ever showed. Once
        // the late-paste window passed, it was reported lost though it had arrived.
        let screen = taken_at_tool_boundary(LONG_STRANDED);
        let replay = Replay::new(screen.clone(), screen.clone(), screen);
        let clock = stranded_clock((STAGE_TIMEOUT + STAGE_GRACE).as_secs_f64() + 5.0);
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            LONG_STRANDED,
            in_lock_timeouts(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert!(receipt.queued, "{receipt:?}");
        assert!(
            receipt.evidence.contains("taken into the running turn"),
            "{receipt:?}"
        );
        assert!(replay.pastes.lock().unwrap().is_empty(), "never retyped");
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn a_short_run_of_a_stranded_prompt_above_the_composer_is_not_delivery() {
        // Words a prompt shares with other text, such as its sender header, prove nothing.
        let shared = "[from coordinator via agentctl · reply: agentctl send coordinator]";
        assert!(!shows_run_of(shared, LONG_STRANDED));
        let screen = claude_screen(&["● Done.", shared], "", BUSY_STATUS);
        let replay = Replay::new(screen.clone(), screen.clone(), screen);
        let clock = stranded_clock((STAGE_TIMEOUT + STAGE_GRACE).as_secs_f64() + 5.0);
        let outcome = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            LONG_STRANDED,
            in_lock_timeouts(),
            &clock,
        );
        assert!(is_late_paste_lost(&refusal(outcome)));
    }

    #[test]
    fn a_stranded_retry_reports_the_paste_lost_after_the_late_paste_window() {
        let screen = FakeScreen::new("claude", false);
        let clock = stranded_clock((STAGE_TIMEOUT + STAGE_GRACE).as_secs_f64() + 5.0);
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "the lost prompt",
            in_lock_timeouts(),
            &clock,
        );
        let reason = refusal(outcome);
        assert!(is_late_paste_lost(&reason), "{reason}");
        assert!(screen.state().pastes.is_empty());
    }

    #[test]
    fn a_stranded_retry_submits_its_paste_drawn_as_a_placeholder_of_another_size() {
        // The paste shows collapsed, with a line count the size check does not expect (the
        // composer wraps it): on a stranded retry a lone placeholder is that paste, pressed
        // with Enter once, never typed again.
        let screen = FakeScreen::new("claude", false);
        screen.state().composer = "[Pasted text #2 +9 lines]".to_owned();
        let clock = stranded_clock(40.0);
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "line one\nline two\nline three",
            in_lock_timeouts(),
            &clock,
        );
        let receipt = receipt(outcome);
        assert_eq!(receipt.evidence, "stranded prompt flushed with Enter");
        assert!(screen.state().pastes.is_empty());
        assert_eq!(screen.state().keys, ["Enter"]);
        // Not on a fresh delivery: a placeholder of another size is someone else's draft.
        let fresh = FakeScreen::new("claude", false);
        fresh.state().composer = "[Pasted text #2 +9 lines]".to_owned();
        let reason = refusal(submit(&fresh, "line one\nline two\nline three"));
        assert!(
            reason.contains("already holds unsubmitted text"),
            "{reason}"
        );
    }

    #[test]
    fn human_keystrokes_after_our_paste_block_the_enter() {
        let screen = FakeScreen::new("claude", false);
        screen.state().human_after_paste = Some(" human words".to_owned());
        let outcome = submit(&screen, "our delivery text here");
        assert!(outcome.is_err(), "{outcome:?}");
        let state = screen.state();
        assert!(
            state.keys.is_empty(),
            "no Enter may be sent: {:?}",
            state.keys
        );
        assert!(
            state.composer.contains("human words"),
            "human text untouched"
        );
        assert!(state.submitted.is_empty());
    }

    #[test]
    fn static_tail_detector_matches_our_words_in_the_bottom_rows() {
        let screen = "old output\nour stranded prompt words here\nmore tail\n";
        assert!(static_tail_holds(screen, "our stranded prompt words here"));
        assert!(!static_tail_holds(
            screen,
            "completely different request text"
        ));
    }

    /// A clock whose stop request arrives once `stop_after` has passed.
    struct StoppingClock {
        clock: Clock,
        stop_after: Duration,
    }

    impl AgentRuntime for StoppingClock {
        fn monotonic(&self) -> Duration {
            self.clock.monotonic()
        }

        fn sleep(&self, duration: Duration) {
            self.clock.sleep(duration);
            if self.monotonic() - Duration::from_millis(1_000_000) >= self.stop_after {
                self.clock.cancel.store(true, Ordering::SeqCst);
            }
        }

        fn cancelled(&self) -> bool {
            self.clock.cancelled()
        }
    }

    #[test]
    fn stop_ends_the_wait_for_a_late_paste_at_the_next_poll() {
        let replay = Replay::new(
            claude_busy::before(),
            claude_busy::staged(),
            claude_busy::queued(),
        )
        .drawn_after(u32::MAX);
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: Duration::from_secs(1),
        };
        let message = submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            SubmitTimeouts::default(),
            &runtime,
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("cancelled after the prompt was pasted"),
            "{message}"
        );
        assert_eq!(
            runtime.monotonic() - Duration::from_millis(1_000_000),
            Duration::from_secs(1)
        );
        assert!(replay.keys.lock().unwrap().is_empty());
    }

    #[test]
    fn a_running_turn_is_read_from_the_bottom_rows_of_the_screen() {
        // Claude Code ends its status row with the marker while it works, and shows the queued
        // hint as the placeholder of its empty input box.
        let claude = FakeScreen::new("claude", true);
        assert_eq!(
            running_turn(&claude.render()),
            Some(true),
            "{}",
            claude.render()
        );
        claude.state().busy = false;
        assert_eq!(
            running_turn(&claude.render()),
            Some(false),
            "{}",
            claude.render()
        );
        claude.state().queued.push("next".to_owned());
        assert_eq!(
            running_turn(&claude.render()),
            Some(true),
            "{}",
            claude.render()
        );

        // Codex prints the marker in its progress row, above the composer.
        assert_eq!(running_turn(&codex_v0_159_1::working()), Some(true));
        for idle in [
            codex_v0_159_1::fresh_idle(),
            codex_v0_159_1::submitted(),
            codex_v0_159_1::session_idle(),
        ] {
            assert_eq!(running_turn(&idle), Some(false), "{idle:?}");
        }
        let queued = concat!(
            "• Queued follow-up inputs\n",
            "  ↳ check the logs\n",
            "\n",
            "› Ask Codex to do anything\n",
            "\n",
            "  GPT default · /tmp/project\n",
        );
        assert_eq!(running_turn(queued), Some(true));

        assert_eq!(running_turn(""), Some(false));
        assert_eq!(running_turn(" \n\n \n"), Some(false));
    }

    #[test]
    fn running_turn_words_above_the_bottom_rows_do_not_count() {
        // The marker sits `above` rows over the last row with text; blank rows below that row
        // are not counted.
        let screen = |above: usize| {
            let mut rows = vec!["• Claude prints esc to interrupt while it works".to_owned()];
            rows.extend((1..above).map(|row| format!("• line {row}")));
            rows.push("  ⏵⏵ auto mode on".to_owned());
            rows.extend([String::new(), String::new()]);
            rows.join("\n")
        };
        assert_eq!(running_turn(&screen(RUNNING_TURN_ROWS - 1)), Some(true));
        assert_eq!(running_turn(&screen(RUNNING_TURN_ROWS)), Some(false));
    }

    #[test]
    fn a_screen_whose_last_row_is_the_paste_hint_cannot_tell_whether_a_turn_runs() {
        // A busy Claude Code as captured: the marker ends the status row before the paste, the
        // hint takes that row's place while the paste is staged, and the queued-message hint
        // shows once Enter has queued the paste.
        assert_eq!(running_turn(&claude_busy::before()), Some(true));
        assert_eq!(running_turn(&claude_busy::staged()), None);
        assert_eq!(running_turn(&claude_busy::queued()), Some(true));

        // The hint follows a paste into an idle pane too, so it says nothing either way.
        let rows = |above: &str, status: &str| {
            [
                "● Done.",
                above,
                "────────────────────────────────────────",
                "❯ [Pasted text #923 +10 lines]",
                "────────────────────────────────────────",
                status,
                "",
            ]
            .join("\n")
        };
        assert_eq!(running_turn(&rows("", "  paste again to expand")), None);
        // A marker still counts beside the hint, as in a spinner row that ends with it.
        let spinner = "✻ Fermenting… (16m 8s · ↓ 12.3k tokens · esc to interrupt)";
        assert_eq!(
            running_turn(&rows(spinner, "  paste again to expand")),
            Some(true)
        );
        // Only the last row with text is the status row: the hint's words above it do not count.
        let quoted = "  Claude Code prints paste again to expand after a paste";
        assert_eq!(
            running_turn(&rows(quoted, "  ⏵⏵ auto mode on")),
            Some(false)
        );
    }

    /// Plays a screen from before the paste, one while the paste is staged, and then each
    /// screen of `after` in turn, one per read after the submission key, repeating the last.
    struct Frames {
        before: String,
        staged: String,
        after: Vec<String>,
        phase: std::sync::Mutex<usize>,
        reads_after_key: std::sync::Mutex<usize>,
        keys: std::sync::Mutex<Vec<String>>,
    }

    impl Frames {
        fn new(before: String, staged: String, after: Vec<String>) -> Self {
            Self {
                before,
                staged,
                after,
                phase: std::sync::Mutex::new(0),
                reads_after_key: std::sync::Mutex::new(0),
                keys: std::sync::Mutex::default(),
            }
        }
    }

    impl PromptTerminal for Frames {
        fn read_screen(&self, _pane_id: &str) -> Result<String> {
            match *self.phase.lock().unwrap() {
                0 => Ok(self.before.clone()),
                1 => Ok(self.staged.clone()),
                _ => {
                    let mut reads = self.reads_after_key.lock().unwrap();
                    let frame = self.after[(*reads).min(self.after.len() - 1)].clone();
                    *reads += 1;
                    Ok(frame)
                }
            }
        }

        fn send_text(&self, _pane_id: &str, _text: &str) -> Result<()> {
            *self.phase.lock().unwrap() = 1;
            Ok(())
        }

        fn send_keys(&self, _pane_id: &str, keys: &str) -> Result<()> {
            self.keys.lock().unwrap().push(keys.to_owned());
            if keys == "Enter" {
                *self.phase.lock().unwrap() = 2;
            }
            Ok(())
        }
    }

    const IDLE_STATUS: &str = "  ⏵⏵ auto mode on";
    const BUSY_STATUS: &str = "  ⏵⏵ auto mode on · esc to interrupt";
    const PASTE_STATUS: &str = "  paste again to expand";
    /// The placeholder Claude Code draws in its empty input box while messages are queued.
    const QUEUED_PLACEHOLDER: &str =
        "\u{1b}[7mP\u{1b}[0m\u{1b}[2mress up to edit queued messages\u{1b}[22m";

    /// A plain Claude Code screen: `transcript` rows, the input box holding `composer`, and the
    /// status row.
    fn claude_screen(transcript: &[&str], composer: &str, status: &str) -> String {
        let rule = "─".repeat(40);
        let mut rows: Vec<String> = transcript.iter().map(|row| (*row).to_owned()).collect();
        rows.extend([
            rule.clone(),
            format!("❯\u{a0}{composer}"),
            rule,
            status.to_owned(),
        ]);
        rows.join("\n") + "\n"
    }

    /// Submit `prompt` to `frames` as Claude Code, returning the receipt and the time it took.
    fn submit_frames(frames: &Frames, prompt: &str) -> (SubmissionReceipt, Duration) {
        let clock = Clock::default();
        let outcome = submit_verified(
            frames,
            "w1:p1",
            "claude",
            prompt,
            SubmitTimeouts::default(),
            &clock,
        );
        (
            receipt(outcome),
            clock.monotonic() - Duration::from_millis(1_000_000),
        )
    }

    #[test]
    fn a_prompt_drawn_above_the_composer_is_printed_evidence() {
        for (harness, busy) in [
            ("claude", false),
            ("claude", true),
            ("codex", false),
            ("codex", true),
        ] {
            let screen = FakeScreen::new(harness, busy);
            let receipt = receipt(submit(&screen, "run the tests"));
            assert!(receipt.printed, "{harness} busy={busy}: {receipt:?}");
            assert_eq!(
                receipt.evidence, "prompt text appeared above the composer",
                "{harness} busy={busy}"
            );
        }

        // The captured busy Claude Code screens: Enter queued the paste and drew it in full.
        let replay = Replay::new(
            claude_busy::before(),
            claude_busy::staged(),
            claude_busy::queued(),
        );
        let receipt = receipt(submit_verified(
            &replay,
            "w1:p1",
            "claude",
            claude_busy::PROMPT,
            SubmitTimeouts::default(),
            &Clock::default(),
        ));
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(receipt.evidence, "prompt text appeared above the composer");
    }

    #[test]
    fn a_prompt_staged_again_after_a_transient_empty_composer_is_not_verified() {
        // The key was dropped: one redraw after it looks empty, so the running-turn marker that
        // was already showing seems to corroborate a submission, and then the prompt is staged
        // in the composer again on every read. Nothing proved the submission, so the key is
        // pressed again until the deadline, and the prompt is reported as not submitted.
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        let frames = Frames::new(busy.clone(), staged.clone(), vec![busy, staged]);
        let clock = Clock::default();
        let outcome = submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &clock,
        );
        match outcome {
            Ok(Submission::NotStaged(reason)) => {
                assert!(reason.contains("it was NOT submitted"), "{reason}")
            }
            other => panic!("expected the prompt to be reported as not submitted, got {other:?}"),
        }
        assert!(frames.keys.lock().unwrap().len() > 1);
    }

    #[test]
    fn a_weak_verification_settles_only_when_the_whole_wait_shows_the_composer_clear() {
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        // The whole wait shows the composer clear: settled.
        let frames = Frames::new(busy.clone(), staged.clone(), vec![busy.clone()]);
        let (held, _) = submit_frames(&frames, "run the tests");
        assert!(!held.printed && held.settled, "{held:?}");
        // The wait ends on a screen with no recognisable composer: not settled.
        let frames = Frames::new(
            busy.clone(),
            staged.clone(),
            vec![
                busy.clone(),
                "Replace goal?\n  › 1. Replace\n  2. Cancel\n".to_owned(),
            ],
        );
        let (held, _) = submit_frames(&frames, "run the tests");
        assert!(!held.printed && !held.settled, "{held:?}");
        // Stopped during the wait, before the prompt could be seen back in the composer: not
        // settled.
        let frames = Frames::new(busy.clone(), staged, vec![busy.clone(), busy]);
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: Duration::from_millis(150),
        };
        let held = receipt(submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &runtime,
        ));
        assert!(!held.printed && !held.settled, "{held:?}");
    }

    #[test]
    fn a_stop_arriving_with_the_last_read_of_a_complete_wait_still_settles() {
        // Weak evidence at 100 ms; the wait's last read at 2,100 ms comes with a stop.
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        let frames = Frames::new(busy.clone(), staged, vec![busy]);
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: POLL + PRINT_GRACE,
        };
        let held = receipt(submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &runtime,
        ));
        assert!(!held.printed && held.settled, "{held:?}");
    }

    #[test]
    fn a_stop_before_a_carried_staged_read_sends_no_further_key() {
        // Empty-looking redraws from 100 to 400 ms, a stop at 500 ms, and the prompt staged
        // again on the read at 500 ms: that read is not handled, so the scheduled second key is
        // never sent.
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        let frames = Frames::new(
            busy.clone(),
            staged.clone(),
            vec![busy.clone(), busy.clone(), busy.clone(), busy, staged],
        );
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: Duration::from_millis(450),
        };
        let outcome = submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &runtime,
        );
        assert!(outcome.is_err(), "{outcome:?}");
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn redraws_alternating_with_the_staged_prompt_keep_the_key_schedule_and_the_deadline() {
        // The key was dropped, and the pane alternates between an empty-looking redraw and the
        // staged prompt. Each staged read goes through the staged path, so the key is pressed
        // again on its schedule and the prompt is reported as not submitted at the deadline.
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        let after = (0..4_000)
            .map(|read| {
                if read % 2 == 0 {
                    busy.clone()
                } else {
                    staged.clone()
                }
            })
            .collect();
        let frames = Frames::new(busy.clone(), staged.clone(), after);
        let clock = Clock::default();
        let started = clock.monotonic();
        let outcome = submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &clock,
        );
        match outcome {
            Ok(Submission::NotStaged(reason)) => {
                assert!(reason.contains("it was NOT submitted"), "{reason}")
            }
            other => panic!("expected the prompt to be reported as not submitted, got {other:?}"),
        }
        assert!(frames.keys.lock().unwrap().len() > 2);
        assert!(clock.monotonic() - started <= SubmitTimeouts::default().submit + PRINT_GRACE);
    }

    #[test]
    fn a_row_first_seen_while_the_prompt_was_staged_again_is_not_printed_evidence() {
        // After a dropped key: an empty-looking redraw, then the prompt staged again with an
        // older identical row newly visible above it, then an empty-looking redraw with that same
        // row. The row was seen while the prompt was staged, so it proves nothing.
        let row = "❯ run the tests";
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", BUSY_STATUS);
        let staged_with_row = claude_screen(&["• earlier", row], "run the tests", BUSY_STATUS);
        let busy_with_row = claude_screen(&["• earlier", row], "", BUSY_STATUS);
        let frames = Frames::new(
            busy.clone(),
            staged,
            vec![
                busy,
                staged_with_row.clone(),
                busy_with_row,
                staged_with_row,
            ],
        );
        let clock = Clock::default();
        let outcome = submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &clock,
        );
        match outcome {
            Ok(Submission::NotStaged(reason)) => {
                assert!(reason.contains("it was NOT submitted"), "{reason}")
            }
            other => panic!("expected the prompt to be reported as not submitted, got {other:?}"),
        }
    }

    #[test]
    fn a_running_turn_marker_that_was_already_showing_is_not_printed_evidence() {
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let frames = Frames::new(
            busy.clone(),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![busy],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert_eq!(receipt.evidence, "agent reports an active turn");
        assert!(!receipt.printed, "{receipt:?}");
        // The first read after the key proved the submission. The reads that then looked for
        // printed evidence, for `PRINT_GRACE`, sent no key.
        assert_eq!(receipt.elapsed, POLL);
        assert_eq!(took, POLL + PRINT_GRACE);
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    /// A prompt longer than Codex's three-row preview, and the rows Codex 0.159.1 draws for it
    /// while it holds it for the next tool call of a running turn.
    const HELD_PROMPT: &str = "Coordinator: queueing an owner request for after your hand-back. \
        Every backend must refuse an unacknowledged seccomp filter and record a probe hash before \
        it runs the guest; read the issue first and reply with a short plan.";

    fn held_rows(first: &str) -> Vec<String> {
        vec![
            "\u{1b}[0m\u{1b}[2m• \u{1b}[0mMessages to be submitted after next tool call\u{1b}[2m \
             (press \u{1b}[0m\u{1b}[1mesc\u{1b}[0m\u{1b}[2m to interrupt and send immediately)\u{1b}[0m"
                .to_owned(),
            format!("\u{1b}[0m\u{1b}[2m  ↳ {first}\u{1b}[0m"),
            "\u{1b}[0m\u{1b}[2m    Every backend must refuse an unacknowledged seccomp filter and\u{1b}[0m"
                .to_owned(),
            "\u{1b}[0m\u{1b}[2m    record a probe hash before it runs the guest; read the\u{1b}[0m".to_owned(),
            "\u{1b}[0m\u{1b}[2m    …\u{1b}[0m".to_owned(),
            "\u{1b}[0m  \u{1b}[1m⌥ + ↑\u{1b}[0m edit last queued message".to_owned(),
        ]
    }

    #[test]
    fn codex_pending_previews_read_each_held_message_up_to_its_cut() {
        let first = "Coordinator: queueing an owner request for after your hand-back.";
        let rows = held_rows(first);
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let screen = codex_v0_159_1::working_with(&rows, "");
        let view = composer_view("codex", &screen).unwrap();
        let previews = codex_pending_previews(&view.transcript);
        assert_eq!(previews.len(), 1, "{previews:?}");
        assert!(
            compact(HELD_PROMPT).starts_with(&previews[0]),
            "{previews:?}"
        );
        assert!(previews[0].ends_with("readthe"), "{previews:?}");
        assert_eq!(
            codex_pending_copies(&view.transcript, &compact(HELD_PROMPT)),
            1
        );
        // Another prompt that shares only a short start is not this one.
        assert_eq!(
            codex_pending_copies(&view.transcript, &compact("Coordinator: something else")),
            0
        );
        // Rows outside a held-message list are not previews.
        assert!(codex_pending_previews("  ↳ Coordinator: queueing an owner request").is_empty());
    }

    #[test]
    fn a_long_prompt_codex_holds_for_its_next_tool_call_is_printed_evidence() {
        // Enter in a busy Codex steers the turn: Codex holds the prompt until its next tool call
        // and shows only its first three rows, so the prompt's end never shows in the pane
        // before then. The preview is the evidence that the prompt reached this pane.
        let first = "Coordinator: queueing an owner request for after your hand-back.";
        let rows = held_rows(first);
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let frames = Frames::new(
            codex_v0_159_1::working_with(&[], ""),
            codex_v0_159_1::working_with(&[], HELD_PROMPT),
            vec![codex_v0_159_1::working_with(&rows, "")],
        );
        let clock = Clock::default();
        let receipt = receipt(submit_verified(
            &frames,
            "w1:p1",
            "codex",
            HELD_PROMPT,
            SubmitTimeouts::default(),
            &clock,
        ));
        assert_eq!(
            receipt.evidence,
            "Codex lists the prompt among the messages it holds for its turn"
        );
        assert!(receipt.printed && receipt.settled, "{receipt:?}");
        assert_eq!(frames.keys.lock().unwrap().as_slice(), ["Enter"]);
    }

    #[test]
    fn a_held_preview_codex_already_showed_is_not_printed_evidence() {
        // The same prompt held from an earlier send is not evidence about this one.
        let first = "Coordinator: queueing an owner request for after your hand-back.";
        let rows = held_rows(first);
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let frames = Frames::new(
            codex_v0_159_1::working_with(&rows, ""),
            codex_v0_159_1::working_with(&rows, HELD_PROMPT),
            vec![codex_v0_159_1::working_with(&rows, "")],
        );
        let clock = Clock::default();
        let receipt = receipt(submit_verified(
            &frames,
            "w1:p1",
            "codex",
            HELD_PROMPT,
            SubmitTimeouts::default(),
            &clock,
        ));
        assert!(!receipt.printed, "{receipt:?}");
    }

    #[test]
    fn printed_evidence_that_follows_a_marker_already_showing_is_waited_for() {
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let queued = claude_screen(&["• earlier", "❯ run the tests"], "", BUSY_STATUS);
        let frames = Frames::new(
            busy.clone(),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![busy.clone(), busy.clone(), busy, queued],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert_eq!(receipt.evidence, "prompt text appeared above the composer");
        assert!(receipt.printed, "{receipt:?}");
        // Reads 100 ms apart: the fourth after the key showed the prompt.
        assert_eq!(receipt.elapsed, 4 * POLL);
        assert_eq!(took, 4 * POLL);
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_queued_message_marker_that_appears_after_the_key_is_printed_evidence() {
        let frames = Frames::new(
            claude_screen(&["• earlier"], "", BUSY_STATUS),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![claude_screen(
                &["• earlier"],
                QUEUED_PLACEHOLDER,
                BUSY_STATUS,
            )],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert_eq!(
            receipt.evidence,
            "agent queue marker appeared (\"Press up to edit queued messages\")"
        );
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!((receipt.elapsed, took), (POLL, POLL));
    }

    #[test]
    fn a_new_queue_placeholder_proves_submission_while_the_paste_hint_hides_the_status() {
        let frames = Frames::new(
            claude_screen(&["• earlier"], "", BUSY_STATUS),
            claude_screen(&["• earlier"], "run the tests", PASTE_STATUS),
            vec![claude_screen(
                &["• earlier"],
                QUEUED_PLACEHOLDER,
                PASTE_STATUS,
            )],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert!(receipt.printed && receipt.settled, "{receipt:?}");
        assert_eq!(
            receipt.evidence,
            "agent queue marker appeared (\"Press up to edit queued messages\")"
        );
        assert_eq!((receipt.elapsed, took), (POLL, POLL));
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_preexisting_queue_placeholder_with_a_paste_hint_is_not_printed_evidence() {
        let queued = claude_screen(&["• earlier"], QUEUED_PLACEHOLDER, PASTE_STATUS);
        let frames = Frames::new(
            queued.clone(),
            claude_screen(&["• earlier"], "run the tests", PASTE_STATUS),
            vec![queued],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert!(!receipt.printed && receipt.settled, "{receipt:?}");
        assert_eq!(took, POLL + PRINT_GRACE);
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_queue_placeholder_redraw_does_not_prove_a_dropped_key_was_accepted() {
        let queued = claude_screen(&["• earlier"], QUEUED_PLACEHOLDER, PASTE_STATUS);
        let staged = claude_screen(&["• earlier"], "run the tests", PASTE_STATUS);
        let frames = Frames::new(queued.clone(), staged.clone(), vec![queued, staged]);
        let result = submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts {
                submit: Duration::from_secs(1),
                ..SubmitTimeouts::default()
            },
            &Clock::default(),
        );
        match result {
            Ok(Submission::NotStaged(reason)) => {
                assert!(reason.contains("it was NOT submitted"), "{reason}")
            }
            other => panic!("a transient redraw must not confirm a dropped key: {other:?}"),
        }
        assert!(frames.keys.lock().unwrap().len() > 1);
    }

    #[test]
    fn a_queued_message_marker_that_was_already_showing_is_not_printed_evidence() {
        let queued = claude_screen(&["• earlier"], QUEUED_PLACEHOLDER, BUSY_STATUS);
        let frames = Frames::new(
            queued.clone(),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![queued],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(took, POLL + PRINT_GRACE);
    }

    #[test]
    fn a_running_turn_marker_that_appears_after_the_key_is_printed_evidence() {
        let frames = Frames::new(
            claude_screen(&["• earlier"], "", IDLE_STATUS),
            claude_screen(&["• earlier"], "run the tests", IDLE_STATUS),
            vec![claude_screen(&["• earlier"], "", BUSY_STATUS)],
        );
        let (receipt, took) = submit_frames(&frames, "run the tests");
        assert_eq!(receipt.evidence, "agent started reporting an active turn");
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!((receipt.elapsed, took), (POLL, POLL));
    }

    #[test]
    fn a_prompt_row_shown_while_the_prompt_was_staged_is_not_printed_evidence() {
        let before = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(
            &["• earlier", "❯ run the tests"],
            "run the tests",
            BUSY_STATUS,
        );
        let after = claude_screen(&["• earlier", "❯ run the tests"], "", BUSY_STATUS);
        let frames = Frames::new(before, staged, vec![after]);
        let (receipt, _) = submit_frames(&frames, "run the tests");
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_placeholder_row_shown_while_the_prompt_was_staged_is_not_printed_evidence() {
        let before = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let staged = claude_screen(
            &["• earlier", "❯ [Pasted text #923 +10 lines]"],
            "[Pasted text #924 +10 lines]",
            BUSY_STATUS,
        );
        let after = claude_screen(
            &["• earlier", "❯ [Pasted text #923 +10 lines]"],
            "",
            BUSY_STATUS,
        );
        let frames = Frames::new(before, staged, vec![after]);
        let (receipt, _) = submit_frames(&frames, claude_busy::PROMPT);
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_new_prompt_ending_like_an_older_row_is_printed_evidence_by_its_start() {
        // The older row scrolls away as the new one appears, so the count of rows ending like the
        // prompt does not grow, but the count of rows starting like it does.
        let tail = "x".repeat(40);
        let old_row = format!("❯ OLD unrelated request {tail}");
        let prompt = format!("NEW current request {tail}");
        let new_row = format!("❯ {prompt}");
        let frames = Frames::new(
            claude_screen(&["• earlier", old_row.as_str()], "", BUSY_STATUS),
            claude_screen(&["• earlier", old_row.as_str()], &prompt, BUSY_STATUS),
            vec![claude_screen(
                &["• earlier", new_row.as_str()],
                "",
                BUSY_STATUS,
            )],
        );
        let (receipt, _) = submit_frames(&frames, &prompt);
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(receipt.evidence, "prompt text appeared above the composer");
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_new_chat_prompt_sharing_its_edges_with_an_older_row_is_printed_evidence() {
        let header = "The user's request arrived through the configured chat bridge.";
        let trailer = "Complete this request using your normal instructions and tools. Use the \
                       same two lines for every reply, with no tool call between them.";
        let old_prompt =
            format!("{header}\nSource: spaces/example/messages/old\nOld request body\n\n{trailer}");
        let prompt =
            format!("{header}\nSource: spaces/example/messages/new\nNew request body\n\n{trailer}");
        let old_row = format!("❯ {old_prompt}");
        let new_row = format!("❯ {prompt}");
        let frames = Frames::new(
            claude_screen(&["• earlier", old_row.as_str()], "", BUSY_STATUS),
            claude_screen(&["• earlier", old_row.as_str()], &prompt, BUSY_STATUS),
            vec![claude_screen(
                &["• earlier", new_row.as_str()],
                "",
                BUSY_STATUS,
            )],
        );
        let (receipt, _) = submit_frames(&frames, &prompt);
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(*frames.keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn a_running_turn_marker_above_a_tall_staged_composer_is_not_printed_evidence() {
        /// Frames whose Tab key, which a busy Codex queues with, also takes effect.
        struct TabFrames(Frames);
        impl PromptTerminal for TabFrames {
            fn read_screen(&self, pane_id: &str) -> Result<String> {
                self.0.read_screen(pane_id)
            }
            fn send_text(&self, pane_id: &str, text: &str) -> Result<()> {
                self.0.send_text(pane_id, text)
            }
            fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
                self.0.send_keys(pane_id, keys)?;
                if keys == "Tab" {
                    *self.0.phase.lock().unwrap() = 2;
                }
                Ok(())
            }
        }
        let prompt = (0..20)
            .map(|n| format!("request line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let before = "• earlier\n› \u{1b}[2mAsk Codex to do anything\u{1b}[22m\n\n  99% context\n"
            .to_owned();
        // The turn started while the prompt was staged; its progress row sits above a composer
        // of 20 rows, out of the rows `running_turn` searches.
        let mut staged_rows = vec![
            "• earlier".to_owned(),
            "• Working (esc to interrupt)".to_owned(),
        ];
        staged_rows.extend(
            prompt
                .lines()
                .enumerate()
                .map(|(index, line)| format!("{} {line}", if index == 0 { "›" } else { " " })),
        );
        staged_rows.extend([
            String::new(),
            "  tab to queue message                 99% context".to_owned(),
        ]);
        let staged = staged_rows.join("\n") + "\n";
        assert_eq!(running_turn(&staged), Some(false));
        let after = "• earlier\n• Working (esc to interrupt)\n› \u{1b}[2mAsk Codex to do \
                     anything\u{1b}[22m\n\n  99% context\n"
            .to_owned();
        assert_eq!(
            composer_view("codex", &staged).unwrap().transcript,
            composer_view("codex", &after).unwrap().transcript
        );
        let frames = TabFrames(Frames::new(before, staged, vec![after]));
        let receipt = receipt(submit_verified(
            &frames,
            "w1:p1",
            "codex",
            &prompt,
            SubmitTimeouts::default(),
            &Clock::default(),
        ));
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(*frames.0.keys.lock().unwrap(), ["Tab"]);
    }

    #[test]
    fn a_running_turn_marker_shown_while_the_prompt_was_staged_is_not_printed_evidence() {
        // The turn started between the paste and the key, so the marker after the key is not
        // something the key caused.
        let frames = Frames::new(
            claude_screen(&["• earlier"], "", IDLE_STATUS),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![claude_screen(&["• earlier"], "", BUSY_STATUS)],
        );
        let (receipt, _) = submit_frames(&frames, "run the tests");
        assert_eq!(receipt.evidence, "agent reports an active turn");
        assert!(!receipt.printed, "{receipt:?}");
    }

    #[test]
    fn a_paste_hint_before_the_paste_may_hide_a_running_turn() {
        // Claude Code draws the hint in place of its status row for a while after any paste, so
        // the screen before this paste cannot tell whether a turn was already running.
        let frames = Frames::new(
            claude_screen(&["• earlier"], "", PASTE_STATUS),
            claude_screen(&["• earlier"], "[Pasted text #924 +10 lines]", PASTE_STATUS),
            vec![claude_screen(&["• earlier"], "", BUSY_STATUS)],
        );
        let (receipt, took) = submit_frames(&frames, claude_busy::PROMPT);
        assert_eq!(receipt.evidence, "agent reports an active turn");
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(took, POLL + PRINT_GRACE);
    }

    #[test]
    fn stop_ends_the_wait_for_printed_evidence_at_the_next_poll() {
        let busy = claude_screen(&["• earlier"], "", BUSY_STATUS);
        let frames = Frames::new(
            busy.clone(),
            claude_screen(&["• earlier"], "run the tests", BUSY_STATUS),
            vec![busy],
        );
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: Duration::from_millis(500),
        };
        let receipt = receipt(submit_verified(
            &frames,
            "w1:p1",
            "claude",
            "run the tests",
            SubmitTimeouts::default(),
            &runtime,
        ));
        assert!(!receipt.printed, "{receipt:?}");
        assert_eq!(
            runtime.monotonic() - Duration::from_millis(1_000_000),
            Duration::from_millis(500)
        );
    }

    /// A busy Claude Code screen captured from a live pane (task names removed): Claude Code
    /// holds a queued prompt, so its empty composer shows the reverse-video cursor on `P` and the
    /// faint hint `ress up to edit queued messages`. Rows end in CR LF, as Herdr returns them.
    fn captured_busy_claude_with_queue_hint() -> String {
        let rule = format!(
            "\u{1b}[0m\u{1b}[38;2;136;136;136m{}\u{1b}[0m",
            "─".repeat(152)
        );
        [
            "  \u{1b}[0m\u{1b}[38;2;153;153;153mearlier output\u{1b}[0m",
            "\u{1b}[0m\u{1b}[38;2;215;119;87m· \u{1b}[0m\u{1b}[38;2;225;139;107mQuantumizing…\u{1b}[0m\u{1b}[38;2;215;119;87m \u{1b}[0m\u{1b}[38;2;153;153;153m(running UserPromptSubmit hooks… 9/10 · 10m 8s · ↓\u{1b}[0m \u{1b}[0m\u{1b}[38;2;153;153;153m21.2k tokens)\u{1b}[0m",
            &rule,
            "\u{1b}[0m\u{1b}[38;2;153;153;153m❯\u{a0}\u{1b}[0m\u{1b}[7mP\u{1b}[0m\u{1b}[2mress up to edit queued messages\u{1b}[0m",
            &rule,
            "  \u{1b}[0m\u{1b}[38;2;255;107;128m⏵⏵ bypass permissions on\u{1b}[0m\u{1b}[38;2;153;153;153m (shift+tab to cycle) · esc to interrupt · ← 2 agents · ↓ to manage\u{1b}[0m",
            "",
        ]
        .join("\r\n")
    }

    /// An idle Claude Code screen captured from a live pane (transcript removed): the empty
    /// composer shows only the reverse-video cursor cell.
    fn captured_idle_claude() -> String {
        let rule = format!(
            "\u{1b}[0m\u{1b}[38;2;136;136;136m{}\u{1b}[0m",
            "─".repeat(152)
        );
        [
            "\u{1b}[0m\u{1b}[38;2;153;153;153m✻\u{1b}[0m \u{1b}[0m\u{1b}[38;2;153;153;153mCooked for 2m 8s · done 7:09 PM\u{1b}[0m",
            &rule,
            "❯\u{a0}\u{1b}[0m\u{1b}[7m \u{1b}[0m",
            &rule,
            "  \u{1b}[0m\u{1b}[38;2;255;193;7m⏵⏵ auto mode on\u{1b}[0m",
            "",
        ]
        .join("\r\n")
    }

    #[test]
    fn claude_hint_text_and_cursor_in_an_empty_composer_are_not_a_draft() {
        for (label, screen) in [
            ("queue hint", captured_busy_claude_with_queue_hint()),
            ("idle", captured_idle_claude()),
        ] {
            let view = composer_view("claude", &screen).expect(label);
            assert!(composer_is_empty(&view), "{label}: {view:?}");
        }
        let busy = composer_view("claude", &captured_busy_claude_with_queue_hint()).unwrap();
        assert!(busy.composer.contains("ress up to edit queued messages"));
        assert_eq!(
            running_turn(&captured_busy_claude_with_queue_hint()),
            Some(true)
        );
        assert_eq!(running_turn(&captured_idle_claude()), Some(false));
        // A typed draft in the same frame does count.
        let drafted = captured_idle_claude().replace(
            "❯\u{a0}\u{1b}[0m\u{1b}[7m \u{1b}[0m",
            "❯\u{a0}muse-wonder-full-1.0\u{1b}[0m\u{1b}[7m \u{1b}[0m",
        );
        let view = composer_view("claude", &drafted).unwrap();
        assert!(!composer_is_empty(&view));
        assert_eq!(view.composer_solid.trim(), "muse-wonder-full-1.0");
    }

    #[test]
    fn a_busy_claude_with_only_the_queue_hint_is_typed_into_and_the_prompt_is_queued() {
        // The hint the captured frame shows: the composer is empty, so the prompt is typed.
        let screen = FakeScreen::new("claude", true);
        screen.state().queued.push("an earlier prompt".to_owned());
        screen.state().queued_hidden = true;
        let receipt = receipt(submit(&screen, "check the build"));
        assert_eq!(receipt.key, "Enter");
        // #884 item 4 (B): taken into Claude Code's queue is a delivered
        // receipt, marked queued — never an uncertain one.
        assert!(receipt.queued, "{receipt:?}");
        let state = screen.state();
        assert_eq!(state.queued, ["an earlier prompt", "check the build"]);
        assert!(state.composer.is_empty());
    }

    #[test]
    fn a_prompt_an_idle_claude_takes_is_not_a_queued_receipt() {
        let screen = FakeScreen::new("claude", false);
        let receipt = receipt(submit(&screen, "check the build"));
        assert!(!receipt.queued, "{receipt:?}");
        assert_eq!(screen.state().submitted, ["check the build"]);
    }

    #[test]
    fn a_stop_after_the_paste_presses_enter_before_reporting_the_outcome_unknown() {
        // #884 item 4 (D): a service stopping right after pasting must not
        // leave its paste sitting in the composer. The first Enter is
        // dropped; the stop arrives; one more bare Enter goes out before
        // the unknown outcome is reported. Nothing is ever retyped.
        let screen = FakeScreen::new("claude", false);
        screen.state().drop_keys = 1;
        let runtime = StoppingClock {
            clock: Clock::default(),
            stop_after: Duration::from_millis(300),
        };
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "check the build",
            SubmitTimeouts::default(),
            &runtime,
        );
        assert!(outcome.is_err(), "{outcome:?}");
        let state = screen.state();
        assert_eq!(state.keys, ["Enter", "Enter"]);
        assert_eq!(state.submitted, ["check the build"]);
        assert_eq!(state.pastes.len(), 1, "typed once, never retyped");
    }

    #[test]
    fn clear_composer_deletes_every_row_and_proves_the_composer_empty() {
        for harness in ["claude", "codex"] {
            let screen = FakeScreen::new(harness, false);
            screen.state().composer = "first row\nsecond row\nthird".to_owned();
            let clock = Clock::default();
            let keys = clear_composer(&screen, "w1:p1", harness, &clock).expect(harness);
            assert!(keys >= 6, "{harness}: {keys}");
            let state = screen.state();
            assert!(state.composer.is_empty(), "{harness}: {:?}", state.composer);
            assert!(state.submitted.is_empty() && state.queued.is_empty());
            assert!(
                state
                    .keys
                    .iter()
                    .all(|key| ["ctrl+e", "ctrl+u", "backspace"].contains(&key.as_str())),
                "{harness}: {:?}",
                state.keys
            );
        }
    }

    #[test]
    fn clear_composer_fails_without_progress_and_reports_the_text_left() {
        let screen = FakeScreen::new("claude", false);
        screen.state().composer = "keep me".to_owned();
        screen.state().ignore_clear_keys = true;
        let error = clear_composer(&screen, "w1:p1", "claude", &Clock::default()).unwrap_err();
        assert!(error.contains("did not change"), "{error}");
        assert_eq!(screen.state().composer, "keep me");
        assert_eq!(screen.state().keys, ["ctrl+e", "ctrl+u", "backspace"]);
    }

    #[test]
    fn a_prompt_a_working_claude_never_accepts_is_withdrawn_and_reported_not_typed() {
        let screen = FakeScreen::new("claude", true);
        screen.state().drop_submit_keys = true;
        let clock = Clock::default();
        clock.withdraw.store(true, Ordering::SeqCst);
        let outcome = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "check the build",
            SubmitTimeouts::default(),
            &clock,
        );
        let reason = refusal(outcome);
        assert!(
            reason.contains("taken back out of the composer"),
            "{reason}"
        );
        let state = screen.state();
        assert!(state.composer.is_empty(), "{:?}", state.composer);
        assert!(state.queued.is_empty() && state.submitted.is_empty());
        assert!(state.keys.iter().filter(|key| *key == "Enter").count() > 1);
        assert!(state.keys.iter().any(|key| key == "ctrl+u"));
    }

    #[test]
    fn a_prompt_never_accepted_is_left_staged_without_the_withdrawal_request() {
        let screen = FakeScreen::new("claude", true);
        screen.state().drop_keys = u32::MAX;
        let reason = refusal(submit(&screen, "check the build"));
        assert!(reason.contains("stranded_prompt:"), "{reason}");
        assert!(reason.contains("still staged"), "{reason}");
        assert_eq!(screen.state().composer, "check the build");
        assert!(!screen.state().keys.iter().any(|key| key == "ctrl+u"));
    }

    #[test]
    fn a_prompt_that_cannot_be_withdrawn_stays_possibly_submitted() {
        let screen = FakeScreen::new("claude", true);
        screen.state().drop_keys = u32::MAX;
        screen.state().ignore_clear_keys = true;
        let clock = Clock::default();
        clock.withdraw.store(true, Ordering::SeqCst);
        let error = submit_verified(
            &screen,
            "w1:p1",
            "claude",
            "check the build",
            SubmitTimeouts::default(),
            &clock,
        )
        .unwrap_err();
        assert!(error.to_string().contains("possibly submitted"), "{error}");
        assert_eq!(screen.state().composer, "check the build");
    }

    // #720 agentctl-muse-stale-draft: a prompt agentctl typed that Claude Code or Codex left in
    // the composer is submitted by the next delivery; any other draft is never touched.

    const LANE: &str = "[from lane-a via agentctl · reply: agentctl send lane-a]";
    const OWNER: &str = "[from Ada via slack #team · sent: 2026.11.01:06:21 EST · reply: <r3>]";

    /// A queue holding the records of the prompts a drain typed, removed when dropped.
    struct TypedQueue(std::path::PathBuf);

    impl TypedQueue {
        fn new(tag: &str) -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let path = std::env::temp_dir().join(format!(
                "agentctl-typed-queue-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            for phase in ["processed", "failed"] {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(path.join(phase))
                    .unwrap();
            }
            Self(path)
        }

        /// A prompt the drain typed and proved delivered.
        fn typed(&self, id: &str, text: &str, at: f64) {
            self.record_in("processed", id, text, at, None);
        }

        /// A prompt the drain typed and quarantined, not proved delivered (one still staged when
        /// its key was given up on, say), drawn as `label` if the harness showed a placeholder.
        fn quarantined(&self, id: &str, text: &str, at: f64, label: Option<&str>) {
            self.record_in("failed", id, text, at, label);
        }

        /// A prompt the drain typed and proved delivered, drawn as `label`.
        fn typed_drawn(&self, id: &str, text: &str, at: f64, label: &str) {
            self.record_in("processed", id, text, at, Some(label));
        }

        fn record_in(&self, phase: &str, id: &str, text: &str, at: f64, label: Option<&str>) {
            let mut record = serde_json::json!({"id": id, "text": text,
                                                "delivery_state": phase, "inflight_at": at});
            if let Some(label) = label {
                record["paste_label"] = serde_json::json!(label);
            }
            crate::agent::atomic_json(&self.0.join(format!("{phase}/{id}.json")), &record).unwrap();
        }

        fn record(&self, id: &str) -> serde_json::Value {
            ["processed", "failed"]
                .iter()
                .find_map(|phase| {
                    crate::agent::read_private_json(&self.0.join(format!("{phase}/{id}.json"))).ok()
                })
                .unwrap()
        }
    }

    impl Drop for TypedQueue {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn submit_from(screen: &FakeScreen, queue: &TypedQueue, text: &str) -> Result<Submission> {
        submit_following(screen, queue, text, None)
    }

    /// As [`submit_from`], from a drain that passes `followup` as it types into a working pane.
    fn submit_following(
        screen: &FakeScreen,
        queue: &TypedQueue,
        text: &str,
        followup: Option<&'static str>,
    ) -> Result<Submission> {
        let harness = screen.state().harness;
        let clock = Clock {
            queue: Some(queue.0.clone()),
            followup,
            ..Clock::default()
        };
        submit_verified(
            screen,
            "w1:p1",
            harness,
            text,
            SubmitTimeouts::default(),
            &clock,
        )
    }

    /// Everything the fake pane took, in order: submitted, queued and steered prompts.
    fn taken(screen: &FakeScreen) -> Vec<String> {
        let state = screen.state();
        [&state.submitted, &state.queued, &state.steered]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    #[test]
    fn a_prompt_claude_or_codex_left_unsent_in_the_composer_is_submitted_by_the_next_one() {
        let short = format!("{LANE}\nthe gate is green at 87daeb34");
        // Long enough for Claude Code's `[Pasted text #N +4 lines]` and Codex's `[Pasted Content
        // N chars]`, so only the placeholder shows.
        let long = format!("{LANE}\nfirst finding\nsecond finding\nthird finding\nfourth finding");
        for harness in ["claude", "codex"] {
            for busy in [false, true] {
                for lane in [&short, &long] {
                    let label = format!("{harness} busy={busy} lines={}", lane.lines().count());
                    let queue = TypedQueue::new(harness);
                    // The placeholder Claude Code drew when the drain typed it, which it shows
                    // again for the paste it left unsent.
                    let drawn_as =
                        (harness == "claude" && lane == &long).then_some("[Pastedtext#1+4lines]");
                    queue.quarantined("lane", lane, 1.0, drawn_as);
                    let screen = FakeScreen::new(harness, busy);
                    screen.state().codex_collapse_over = Some(60);
                    screen.leave_draft(lane);
                    if lane == &long {
                        let drawn = screen.render();
                        assert!(drawn.contains("[Pasted "), "{label}: {drawn}");
                        assert!(!drawn.contains("fourth finding"), "{label}: {drawn}");
                    }
                    let owner = format!("{OWNER}\nstatus?");
                    // Claude Code takes a prompt during a turn only when the drain opted in, as
                    // a drain typing into a working pane does.
                    let followup = (harness == "claude").then_some("Enter");
                    let receipt = receipt(submit_following(&screen, &queue, &owner, followup));
                    assert_eq!(taken(&screen), [lane.clone(), owner], "{label}");
                    // One key for the draft and one for the prompt, each the key a prompt typed
                    // then gets: Codex holds a prompt for a running turn (Tab), and the draft
                    // started one.
                    let keys = if harness == "codex" {
                        [if busy { "Tab" } else { "Enter" }, "Tab"]
                    } else {
                        ["Enter", "Enter"]
                    };
                    assert_eq!(screen.state().keys, keys, "{label}");
                    assert_eq!(receipt.key_presses, 1, "{label}");
                    let record = queue.record("lane");
                    assert!(
                        record["draft_recovered_evidence"]
                            .as_str()
                            .unwrap()
                            .contains("typed by agentctl for lane-a"),
                        "{label}: {record}"
                    );
                }
            }
        }
    }

    /// #808 agentctl-stranded-draft-blocks-delivery: a long prompt a drain
    /// typed into a working Claude Code pane was proved delivered, yet the
    /// paste itself sat unsubmitted in the composer and every later drain
    /// refused to append. The placeholder's paste number is the one
    /// agentctl recorded, so the next drain submits the draft and then
    /// delivers its own prompt behind the turn that starts.
    #[test]
    fn a_stranded_proved_claude_paste_is_submitted_by_the_next_drain() {
        let lane = format!("{LANE}\nfirst finding\nsecond finding\nthird finding\nfourth finding");
        let queue = TypedQueue::new("claude-stranded");
        queue.typed_drawn("lane", &lane, 1.0, "[Pastedtext#1+4lines]");
        let screen = FakeScreen::new("claude", true);
        screen.leave_draft(&lane);
        let drawn = screen.render();
        assert!(drawn.contains("[Pasted text #1 +4 lines]"), "{drawn}");
        let owner = format!("{OWNER}\nstatus?");
        receipt(submit_following(&screen, &queue, &owner, Some("Enter")));
        assert_eq!(taken(&screen), [lane.clone(), owner]);
        assert_eq!(screen.state().keys, ["Enter", "Enter"]);
        let record = queue.record("lane");
        assert!(
            record["draft_recovered_evidence"]
                .as_str()
                .unwrap()
                .contains("typed by agentctl for lane-a"),
            "{record}"
        );
    }

    /// #989 agentctl-own-draft-submit: a failed-queue digest a pass typed into a busy Claude
    /// Code pane stayed in its composer after the pass withdrew the digest's record, and every
    /// later drain refused to append to it. The digest names a failed message whose record
    /// remains, so the next drain submits it with one Enter, never retyped, and then delivers
    /// its own prompt; for both labels.
    #[test]
    fn a_digest_left_in_the_composer_without_its_record_is_submitted_by_the_next_drain() {
        for (label, error) in [
            (
                "may have arrived",
                "prompt left the composer without visible submission evidence",
            ),
            (
                "may not have reached you",
                "prompt was submitted but never showed in pane w1:p1",
            ),
        ] {
            let queue = TypedQueue::new("digest-without-record");
            let message = format!("{LANE}\nmemory at 64 GiB, not 48");
            let failed = queue.0.join("failed");
            crate::agent::atomic_json(
                &failed.join("msg-1.json"),
                &serde_json::json!({"id": "msg-1", "text": message}),
            )
            .unwrap();
            std::fs::write(
                failed.join("msg-1.json.error"),
                serde_json::json!({"error": error}).to_string(),
            )
            .unwrap();
            std::fs::write(
                queue.0.join(crate::failed_digest::SHOWN_FILE),
                serde_json::json!({"shown": {"msg-1": 1.0}}).to_string(),
            )
            .unwrap();
            let digest = format!("{label}: msg-1 lane-a memory at 64 GiB, not 48");
            let screen = FakeScreen::new("claude", false);
            screen.leave_draft(&digest);
            let owner = format!("{OWNER}\nstatus?");
            receipt(submit_following(&screen, &queue, &owner, Some("Enter")));
            assert_eq!(taken(&screen), [digest.clone(), owner], "{label}");
            assert_eq!(screen.state().keys, ["Enter", "Enter"], "{label}");
            assert_eq!(
                screen.state().pastes.len(),
                1,
                "{label}: the digest is never retyped"
            );
            let record: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(failed.join("msg-1.json")).unwrap())
                    .unwrap();
            assert!(
                record["draft_recovered_at"].is_number(),
                "{label}: {record}"
            );
        }
    }

    /// #825 agentctl-stranded-multi-paste: two prompts a working Claude
    /// Code pane had queued came back as two concatenated placeholders.
    /// Both labels are recorded deliveries, so the next drain submits the
    /// pair and delivers its own prompt behind the turn that starts.
    #[test]
    fn two_stranded_claude_pastes_are_submitted_by_the_next_drain() {
        let first = format!("{LANE}\nalpha one\nalpha two\nalpha three\nalpha four");
        let second = format!("{LANE}\nbravo one\nbravo two\nbravo three\nbravo four");
        let queue = TypedQueue::new("claude-multi-stranded");
        queue.typed_drawn("first", &first, 1.0, "[Pastedtext#1+4lines]");
        queue.typed_drawn("second", &second, 2.0, "[Pastedtext#2+4lines]");
        let screen = FakeScreen::new("claude", true);
        screen.leave_draft(&first);
        screen.leave_another_draft(&second);
        let drawn = screen.render();
        assert!(
            drawn.contains("[Pasted text #1 +4 lines][Pasted text #2 +4 lines]"),
            "{drawn}"
        );
        let owner = format!("{OWNER}\nstatus?");
        receipt(submit_following(&screen, &queue, &owner, Some("Enter")));
        assert_eq!(taken(&screen), [format!("{first}{second}"), owner]);
        for id in ["first", "second"] {
            let record = queue.record(id);
            assert!(record["draft_recovered_at"].is_number(), "{id}: {record}");
        }
    }

    /// #731 agentctl-paste-tail-flush: a split paste leaves only its last
    /// line in the composer, header gone. It is still ours (a fragment of
    /// the prompt the queue records as typed), so the next delivery
    /// submits it first, in both Claude Code and Codex.
    #[test]
    fn a_paste_tail_left_by_claude_or_codex_is_submitted_by_the_next_prompt() {
        let tail = "after. Needs your disposition (ledger-tree file). Details in the queue.";
        let lane = format!("{LANE}\nfirst line of the message\nsecond line follows here\n{tail}");
        for harness in ["claude", "codex"] {
            let queue = TypedQueue::new(harness);
            queue.typed("lane", &lane, 1.0);
            let screen = FakeScreen::new(harness, false);
            screen.leave_draft(tail);
            let owner = format!("{OWNER}\nstatus?");
            let followup = (harness == "claude").then_some("Enter");
            let receipt = receipt(submit_following(&screen, &queue, &owner, followup));
            assert_eq!(taken(&screen), [tail.to_owned(), owner], "{harness}");
            assert_eq!(receipt.key_presses, 1, "{harness}");
            assert!(
                queue.record("lane")["draft_recovered_at"].is_number(),
                "{harness}"
            );
        }
    }

    #[test]
    fn a_draft_agentctl_did_not_type_is_left_untouched_in_claude_and_codex() {
        let lane = format!("{LANE}\none\ntwo\nthree\nfour");
        for harness in ["claude", "codex"] {
            for draft in [
                // A person typing, a person's paste of another size, and the text of a prompt
                // agentctl typed with a person's word added.
                "half-typed note".to_owned(),
                "pasted\nlog\nlines\nhere\nand\nmore".to_owned(),
                format!("{lane} please"),
            ] {
                let label = format!("{harness}: {draft:?}");
                let queue = TypedQueue::new(harness);
                // Drawn as paste #7 when it was typed; a person's paste now is a later one.
                queue.quarantined("lane", &lane, 1.0, Some("[Pastedtext#7+4lines]"));
                let screen = FakeScreen::new(harness, false);
                screen.state().codex_collapse_over = Some(25);
                screen.leave_draft(&draft);
                let reason = refusal(submit_from(&screen, &queue, &format!("{OWNER}\nhi")));
                assert!(reason.contains("already holds"), "{label}: {reason}");
                assert!(reason.ends_with("nothing was typed"), "{label}: {reason}");
                let state = screen.state();
                assert!(state.keys.is_empty(), "{label}: {:?}", state.keys);
                assert!(state.pastes.is_empty(), "{label}");
                assert_eq!(state.composer, draft, "{label}");
                assert!(queue.record("lane").get("draft_recovered_at").is_none());
            }
        }
    }

    #[test]
    fn a_paste_placeholder_left_in_codex_is_never_pasted_over() {
        // Drawn faint, as some Codex builds draw it, the placeholder shows no solid text; a paste
        // joined to it would be submitted with it. Without a queue to vouch for it, it is refused.
        let screen = FakeScreen::new("codex", false);
        screen.state().codex_collapse_over = Some(10);
        screen.leave_draft("someone else's long paste");
        let reason = refusal(submit(&screen, "hello"));
        assert!(
            reason.contains(
                "composer already holds unsubmitted text \"[Pasted Content 25 chars]\"; refusing"
            ),
            "{reason}"
        );
        assert!(screen.state().keys.is_empty());
        assert!(screen.state().pastes.is_empty());
    }

    #[test]
    fn the_paste_placeholders_live_harnesses_draw_are_read_as_drafts() {
        use crate::stale_draft::Draft;
        // Captured 2026-11-01 from an idle Codex and an idle Claude Code after a bracketed paste
        // of 1500 characters and of 12 lines; the footers' model and path are rewritten.
        let codex = "\u{1b}[1m›\u{1b}[0m \u{1b}[38;5;6m[Pasted Content 1500 chars]\u{1b}[39m\n\n  \
                     model high · ~/project\n";
        let claude = format!(
            "{RULE}\n❯\u{a0}[Pasted text #1 +11 lines]\u{1b}[7m \u{1b}[0m\n{RULE}\n  paste again to \
             expand\n",
            RULE = "─".repeat(40)
        );
        let draft = |harness: &str, screen: &str| {
            composer_draft(&composer_view(harness, screen).expect("a composer"))
        };
        assert_eq!(draft("codex", codex), Some(Draft::Placeholder(1500)));
        assert_eq!(
            draft("claude", &claude),
            Some(Draft::PastedLines("[Pastedtext#1+11lines]".to_owned()))
        );
    }

    #[test]
    fn inputs_codex_moved_back_into_its_composer_on_an_interrupt_are_submitted_together() {
        let queue = TypedQueue::new("interrupt");
        let screen = FakeScreen::new("codex", true);
        let first = format!("{LANE}\nfirst");
        let second = format!("{OWNER}\nsecond");
        for (index, (id, text)) in [("first", &first), ("second", &second)]
            .into_iter()
            .enumerate()
        {
            assert_eq!(receipt(submit_from(&screen, &queue, text)).key, "Tab");
            queue.typed(id, text, 1.0 + index as f64);
        }
        screen.interrupt();
        assert_eq!(screen.state().composer, format!("{first}\n{second}"));
        let third = format!("{LANE}\nthird");
        receipt(submit_from(&screen, &queue, &third));
        // Submitted together as Codex joined them, and the new prompt held for the turn they
        // started.
        assert_eq!(screen.state().submitted, [format!("{first}\n{second}")]);
        assert_eq!(screen.state().queued, [third]);
        for id in ["first", "second"] {
            assert!(queue.record(id)["draft_recovered_at"].is_number(), "{id}");
        }
    }

    #[test]
    fn a_delivered_prompt_a_person_recalled_into_claude_or_codex_is_left_alone() {
        // Up on an empty composer recalls the newest prompt, which for a managed agent is the
        // last one agentctl delivered: it is a person's to edit, not a prompt left unsent.
        let lane = format!("{LANE}\nthe gate is green");
        for harness in ["claude", "codex"] {
            let queue = TypedQueue::new("recalled");
            queue.typed("lane", &lane, 1.0);
            let screen = FakeScreen::new(harness, false);
            screen.leave_draft(&lane);
            let reason = refusal(submit_from(&screen, &queue, &format!("{OWNER}\nhi")));
            assert!(
                reason.contains("already holds unsubmitted text"),
                "{harness}: {reason}"
            );
            assert!(screen.state().keys.is_empty(), "{harness}");
            assert_eq!(screen.state().composer, lane, "{harness}");
        }
    }

    #[test]
    fn a_turn_the_recovered_draft_started_makes_claudes_next_prompt_wait_unless_opted_in() {
        let lane = format!("{LANE}\nthe gate is green");
        let queue = TypedQueue::new("opt-in");
        queue.quarantined("lane", &lane, 1.0, None);
        let screen = FakeScreen::new("claude", false);
        screen.leave_draft(&lane);
        let reason = refusal(submit_from(&screen, &queue, &format!("{OWNER}\nhi")));
        assert!(
            reason.contains("it started a turn, so this prompt waits for the agent"),
            "{reason}"
        );
        assert_eq!(screen.state().submitted, [lane]);
        assert!(
            screen.state().pastes.is_empty(),
            "nothing of the new prompt was typed"
        );
    }

    #[test]
    fn a_draft_agentctl_typed_gets_no_key_while_a_dialog_shows_and_at_most_three_otherwise() {
        let lane = format!("{LANE}\nready");
        let queue = TypedQueue::new("dialog");
        queue.quarantined("lane", &lane, 1.0, None);
        let clock = Clock {
            queue: Some(queue.0.clone()),
            ..Clock::default()
        };
        // A question in the composer's place: Enter would choose an option.
        let screen = FakeScreen::new("claude", false);
        screen.leave_draft(&lane);
        screen.state().dialog = true;
        let outcome = recover_typed_draft(&screen, "w1:p1", "claude", &queue.0, &clock);
        assert!(matches!(outcome, Ok(None)), "{outcome:?}");
        assert!(screen.state().keys.is_empty());
        // A harness that never takes the draft: three presses, then a report.
        screen.state().dialog = false;
        screen.state().drop_submit_keys = true;
        let error = recover_typed_draft(&screen, "w1:p1", "claude", &queue.0, &clock).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("still there after 3 Enter key press(es)"),
            "{error}"
        );
        assert_eq!(screen.state().keys, ["Enter", "Enter", "Enter"]);
        assert_eq!(screen.state().composer, lane);
        // Through a submission, nothing of the new prompt is typed.
        screen.state().keys.clear();
        // Through a submission, nothing of the new prompt is typed, and the refusal still reads
        // as a draft in the composer, which the chat bridge's stale-composer guard looks for.
        let reason = refusal(submit_from(&screen, &queue, "next"));
        assert!(reason.contains("still there after 3 Enter"), "{reason}");
        assert!(
            reason.contains("composer already holds unsubmitted text \"[from lane-a"),
            "{reason}"
        );
        assert!(screen.state().pastes.is_empty());
    }

    /// An extra Enter on an empty composer, idle or mid-turn, submits nothing and draws nothing
    /// in Claude Code and Codex: the steady state a draft recovery's last press can meet. Both
    /// harnesses, idle, left the screen byte for byte unchanged after two such presses
    /// (2026-11-01); the busy case is modelled.
    #[test]
    fn an_extra_enter_on_an_empty_claude_or_codex_composer_changes_nothing() {
        for harness in ["claude", "codex"] {
            for busy in [false, true] {
                let screen = FakeScreen::new(harness, busy);
                receipt(submit(&screen, "status?"));
                let before = (screen.render(), taken(&screen));
                screen.send_keys("w1:p1", "Enter").unwrap();
                assert_eq!(
                    (screen.render(), taken(&screen)),
                    before,
                    "{harness} busy={busy}"
                );
            }
        }
    }
}
