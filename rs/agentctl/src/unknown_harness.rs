//! Best-effort driving of a TUI agentctl has no adapter for (`--harness unknown`, #850
//! unknown-harness-mode).
//!
//! Nothing here reads a harness's phrases or layout; it uses the harness-neutral rules in
//! [`crate::screen_heuristics`]:
//!
//! * **Ready** when no menu or dialog is open and the screen has not moved for [`QUIET`]
//!   (clock digits and spinner glyphs masked).
//! * **Delivery** is acknowledged, never assumed: bracketed paste, poll until the paste shows
//!   (our text at the bottom of the screen, or a change that settles), then Enter, then poll until
//!   the text no longer sits at the bottom as it did when staged.
//! * **A receipt needs evidence** (#852 D): the prompt shown as submitted, which is our text on
//!   screen once it no longer sits where it was staged, or on an agentterm screen a new `You:` turn
//!   that shows it
//!   ([`crate::harness::agentterm::submitted_turns`]). Text that only left the bottom, a screen
//!   that only changed, or the text in the composer proves nothing: the submission's outcome is
//!   unknown, so the queue holds the prompt as possibly submitted, never types it again, and the
//!   chat bridge reports the request uncertain.
//! * **Enter under uncertainty**: an identical leftover gets another bare Enter, on a doubling
//!   wait, at most [`MAX_ENTER_PRESSES`] in all, the menu guard checked before every press. Text is
//!   never pasted twice: text of ours already stranded at the bottom gets Enter instead of a
//!   second paste, and a paste is retried only when nothing on screen ever changed.
//! * A prompt still at the bottom after the last press is a [`STRANDED_PROMPT_KIND`] refusal: it
//!   stays pending (never quarantined as possibly submitted), the queue records it as stranded,
//!   and each later pass ([`AgentRuntime::stranded_retry`]) presses Enter only, never typing it
//!   again, until it leaves the bottom or the recovery ladder takes over.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::agent::AgentRuntime;
use crate::error::{AdapterError, Result};
use crate::screen_heuristics::{self as screen, StaticTail};
use crate::submission::{PromptTerminal, Submission, SubmissionReceipt, SubmitTimeouts, POLL};

/// The record's `harness` for a TUI driven by these generic rules.
pub const HARNESS: &str = "unknown";
/// How long the screen must stand still before the pane counts as ready.
pub const QUIET: Duration = Duration::from_secs(2);
/// How long a paste may take to show on screen.
pub const STAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the presses of Enter and their confirmation may take.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Presses of Enter for one prompt, the first included.
pub const MAX_ENTER_PRESSES: u32 = 5;
/// First wait before an identical leftover gets another Enter; doubles up to [`MAX_RETRY`].
pub const FIRST_RETRY: Duration = Duration::from_secs(1);
/// Longest wait between presses of Enter.
pub const MAX_RETRY: Duration = Duration::from_secs(4);
/// How long a changed screen must hold still to count as settled.
const SETTLE: Duration = Duration::from_millis(500);
/// The key agentterm documents as "clear the input line".
const CLEAR_COMPOSER_KEY: &str = "ctrl+c";
/// How long a cleared agentterm composer may take to read back empty.
const CLEAR_WAIT: Duration = Duration::from_secs(3);
/// How long a screen that never showed the paste is watched after its one Enter.
const UNDRAWN_WATCH: Duration = Duration::from_secs(4);
/// The kind that opens the refusal of a prompt that stayed at the bottom of the screen through
/// every press of Enter. It is a [`Submission::NotStaged`], so the prompt stays pending; the
/// drain records it as stranded and later passes only press Enter. The recovery ladder classifies
/// it with [`is_stranded_prompt`].
pub const STRANDED_PROMPT_KIND: &str = "stranded_prompt";

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";

/// Whether `harness` is driven by these generic rules.
#[must_use]
pub fn is_unknown(harness: &str) -> bool {
    harness == HARNESS
}

/// Whether a refusal or delivery error detail reports a [`STRANDED_PROMPT_KIND`] prompt.
#[must_use]
pub fn is_stranded_prompt(detail: &str) -> bool {
    detail.contains(&format!("{STRANDED_PROMPT_KIND}:"))
}

/// The deadlines one generic submission uses.
#[must_use]
pub fn timeouts() -> SubmitTimeouts {
    SubmitTimeouts {
        stage: STAGE_TIMEOUT,
        submit: SUBMIT_TIMEOUT,
        composer: Duration::ZERO,
        ..SubmitTimeouts::default()
    }
}

/// How long one pane's screen has stood still, across repeated reads.
#[derive(Clone, Debug, Default)]
pub struct Quiescence {
    key: Option<String>,
    since: Duration,
}

impl Quiescence {
    /// Note `screen` read at `now`; return how long it has stood still (zero on a change or
    /// the first read).
    pub fn observe(&mut self, screen: &str, now: Duration) -> Duration {
        let key = screen::quiet_key(screen);
        if self.key.as_deref() != Some(key.as_str()) {
            self.key = Some(key);
            self.since = now;
        }
        now.saturating_sub(self.since)
    }
}

/// [`Quiescence::observe`] for `pane_id`, remembered across calls in this process, so every
/// status read of one pane (a readiness wait polls them) adds to the same stillness.
pub fn observe_pane(pane_id: &str, screen: &str, now: Duration) -> Duration {
    static PANES: OnceLock<Mutex<HashMap<String, Quiescence>>> = OnceLock::new();
    let mut panes = PANES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let quiet = panes.entry(pane_id.to_owned()).or_default();
    if now < quiet.since {
        // Another clock read this pane before: start over.
        *quiet = Quiescence::default();
    }
    quiet.observe(screen, now)
}

/// The Herdr-style status of a generic pane: `blocked` with a menu or dialog open, `idle` once
/// the screen stood still for [`QUIET`], else `working`.
#[must_use]
///
/// A screen that ends with agentterm's input frame ([`crate::harness::agentterm`]) is read by
/// that frame instead of by stillness: `idle` or `working` as it says, and `unknown` under any
/// other label.
pub fn status(screen: &str, quiet_for: Duration) -> &'static str {
    use crate::harness::agentterm::{self, FrameState};
    if screen::menu_open(screen, None).is_some() {
        "blocked"
    } else if let Some(frame) = agentterm::frame(screen) {
        match frame.state {
            FrameState::Idle => "idle",
            FrameState::Working => "working",
            FrameState::Other(_) => "unknown",
        }
    } else if quiet_for >= QUIET {
        "idle"
    } else {
        "working"
    }
}

/// What the wait after the paste saw.
enum Staged {
    /// The paste shows (or the screen changed and settled); this is the staged screen.
    Shown(String),
    /// The text already left the bottom and shows above it: taken before any Enter.
    Consumed,
    /// The text is at the bottom but new rows follow it that the screen did not end with
    /// before: most likely the TUI took it and answered, possibly only a hint under the input.
    Answered(String),
    /// Nothing on screen changed within the stage deadline.
    Undrawn(String),
}

/// Paste `text` into a TUI with no known composer and press Enter until the screen shows it
/// was taken, as described in the module documentation.
///
/// # Errors
///
/// A read or key failure after typing, or a menu that opened over a staged prompt; each leaves
/// the outcome unknown. Refusals before typing, and a prompt still stranded at the bottom after
/// every press ([`STRANDED_PROMPT_KIND`]), are `Ok(Submission::NotStaged)`.
pub fn submit(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    text: &str,
    timeouts: SubmitTimeouts,
    runtime: &dyn AgentRuntime,
) -> Result<Submission> {
    let refuse = |reason: String| Ok(Submission::NotStaged(reason));
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
    let before = match terminal.read_screen(pane_id) {
        Ok(screen) => screen,
        Err(error) => {
            return refuse(format!(
                "pane {pane_id}: screen read failed before typing: {error}"
            ))
        }
    };
    if let Some(line) = screen::menu_open(&before, None) {
        return refuse(format!(
            "pane {pane_id} shows a menu or dialog ({line:?}); nothing was typed"
        ));
    }
    let before = match clear_leaked_replies(terminal, pane_id, before, runtime) {
        Ok(screen) => screen,
        Err(reason) => return refuse(reason),
    };
    match flush_own_agentterm_draft(terminal, pane_id, &before, runtime) {
        Ok(Some(evidence)) => {
            return refuse(format!(
                "pane {pane_id}: {evidence}; this prompt waits for the next delivery; nothing was \
                 typed"
            ))
        }
        Ok(None) => {}
        Err(error) => return refuse(format!("pane {pane_id}: {error}; nothing was typed")),
    }
    if let Some(reason) = agentterm_refusal(pane_id, &before, text, runtime) {
        return refuse(reason);
    }
    let started = runtime.monotonic();
    // What counts as the prompt shown submitted (#852 D).
    let agentterm_view = crate::harness::agentterm::frame(&before).is_some();
    let baseline = crate::harness::agentterm::submitted_turns(&before, text);
    // Off agentterm, a line-oriented TUI keeps the submitted line on screen, above its next
    // prompt and often within the bottom rows. Every caller asks this only once the text no
    // longer sits where it was staged.
    let submitted = |screen: &str| {
        if agentterm_view {
            crate::harness::agentterm::submitted_turns(screen, text) > baseline
        } else {
            screen::echoed_above(screen, text) || screen::locate_in_bottom(screen, text).is_some()
        }
    };
    let unproven = |what: &str| -> Result<Submission> {
        Err(AdapterError::unavailable(format!(
            "pane {pane_id}: {what}, but no row shows the prompt submitted{}; its delivery is \
             unproven, outcome is unknown",
            if agentterm_view {
                " (agentterm draws a submitted input as a `You:` turn)"
            } else {
                ""
            }
        )))
    };
    let receipt = |presses: u32, evidence: String, printed: bool, settled: bool| {
        Ok(Submission::Verified(SubmissionReceipt {
            key: "Enter",
            key_presses: presses,
            elapsed: runtime.monotonic().saturating_sub(started),
            evidence,
            printed,
            settled,
            queued: false,
        }))
    };

    // A prompt an earlier pass left stranded is flushed with Enter only, never typed again; one
    // no longer at the bottom left it between passes.
    let staged = if runtime.stranded_retry() {
        if screen::locate_in_bottom(&before, text).is_none() {
            let shown = if agentterm_view {
                baseline > 0
            } else {
                screen::echoed_above(&before, text)
            };
            if !shown {
                return unproven(
                    "the stranded prompt left the bottom of the screen between passes",
                );
            }
            return receipt(
                0,
                "the stranded prompt left the bottom of the screen between passes and shows \
                 submitted"
                    .to_owned(),
                true,
                true,
            );
        }
        before.clone()
    // Our text already at the bottom is an earlier paste the TUI never took: submit it with
    // Enter rather than paste it a second time.
    } else if screen::stranded(&before, text) {
        before.clone()
    } else {
        runtime.input_started();
        terminal.send_text(
            pane_id,
            &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
        )?;
        match wait_staged(terminal, pane_id, text, &before, timeouts.stage, runtime)? {
            Staged::Shown(screen) => screen,
            Staged::Consumed => {
                let deadline = runtime.monotonic().saturating_add(timeouts.submit);
                if !await_submitted(terminal, pane_id, runtime, deadline, &submitted)? {
                    return unproven("the text left the bottom before Enter");
                }
                return receipt(
                    0,
                    "consumed before Enter: the text left the bottom and shows submitted"
                        .to_owned(),
                    true,
                    true,
                );
            }
            Staged::Answered(screen) => {
                // One bare Enter under uncertainty: nothing on an input the TUI already took,
                // the submission if the new rows were only a hint under it.
                press_enter(terminal, pane_id, text, &screen)?;
                let deadline = runtime.monotonic().saturating_add(timeouts.submit);
                if !await_submitted(terminal, pane_id, runtime, deadline, &submitted)? {
                    return unproven(
                        "new rows followed the text before Enter, and one Enter was pressed under \
                         uncertainty",
                    );
                }
                return receipt(
                    1,
                    "taken before Enter: new rows follow the text, which shows submitted after \
                     one Enter pressed under uncertainty"
                        .to_owned(),
                    true,
                    false,
                );
            }
            Staged::Undrawn(screen) => {
                return submit_undrawn(
                    terminal, pane_id, text, &before, &screen, runtime, &submitted, &unproven,
                    receipt,
                )
            }
        }
    };

    let tail = StaticTail::capture(&staged, text);
    let deadline = runtime.monotonic().saturating_add(timeouts.submit);
    press_enter(terminal, pane_id, text, &staged)?;
    let mut presses: u32 = 1;
    let mut wait = FIRST_RETRY;
    let mut next_press = runtime.monotonic().saturating_add(wait);
    let mut last_key = screen::quiet_key(&staged);
    let mut gone_since: Option<Duration> = None;
    loop {
        if runtime.cancelled() {
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: delivery was cancelled after Enter; outcome is unknown"
            )));
        }
        runtime.sleep(POLL);
        let now_screen = terminal.read_screen(pane_id)?;
        let now = runtime.monotonic();
        let key = screen::quiet_key(&now_screen);
        let stable = key == last_key;
        last_key = key;
        if let Some(line) = screen::menu_open(&now_screen, Some(text)) {
            // A menu drawn under the text as it was staged asks about something else; one after
            // the text left, or after new output, is the TUI's answer to it.
            if tail.leftover(&screen::without_trailing_options(&now_screen), text) {
                return Err(AdapterError::unavailable(format!(
                    "pane {pane_id}: a menu or dialog opened while the prompt still sits at the \
                     bottom ({line:?}); no further Enter was sent, outcome is unknown"
                )));
            }
        }
        if !tail.leftover(&now_screen, text) {
            // Gone from the bottom; held for SETTLE so a redraw is not taken for a submission,
            // then confirmed only by a row that shows it submitted (#852 D).
            let since = *gone_since.get_or_insert(now);
            if now.saturating_sub(since) >= SETTLE {
                if submitted(&now_screen) {
                    return receipt(
                        presses,
                        "the text left the bottom of the screen and shows submitted".to_owned(),
                        true,
                        true,
                    );
                }
                if now >= deadline {
                    return unproven("the text left the bottom of the screen");
                }
            }
            continue;
        }
        gone_since = None;
        if now >= deadline || (presses >= MAX_ENTER_PRESSES && now >= next_press) {
            return Ok(Submission::NotStaged(format!(
                "{STRANDED_PROMPT_KIND}: pane {pane_id}: the prompt still sits at the bottom of \
                 the screen after {presses} Enter press(es); it stays pending and later passes \
                 press Enter only"
            )));
        }
        if presses < MAX_ENTER_PRESSES && now >= next_press && stable {
            press_enter(terminal, pane_id, text, &now_screen)?;
            presses += 1;
            wait = (wait * 2).min(MAX_RETRY);
            next_press = now.saturating_add(wait);
        }
    }
}

/// Why nothing may be typed into an agentterm pane showing `screen` ([`crate::harness::agentterm`]):
/// a running turn, which Enter would steer; a frame label that is neither idle nor working; or a
/// composer draft that is not this prompt left by an earlier pass. `None` for any other screen.
fn agentterm_refusal(
    pane_id: &str,
    screen: &str,
    text: &str,
    runtime: &dyn AgentRuntime,
) -> Option<String> {
    use crate::harness::agentterm::{self, FrameState};
    let frame = agentterm::frame(screen)?;
    match frame.state {
        FrameState::Working => {
            return Some(format!(
                "pane {pane_id}: agentterm shows a running turn, and Enter would steer it; \
                 nothing was typed"
            ))
        }
        FrameState::Other(label) => {
            return Some(format!(
                "pane {pane_id}: agentterm's input frame reads {label:?}, neither idle nor \
                 working; nothing was typed"
            ))
        }
        FrameState::Idle => {}
    }
    let draft = frame.draft?;
    if runtime.stranded_retry() || screen::stranded(screen, text) {
        return None;
    }
    let preview: String = draft
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect();
    Some(format!(
        "pane {pane_id} composer already holds unsubmitted text {preview:?}; refusing to append \
         to it; nothing was typed"
    ))
}

/// Submit, with one Enter, a draft in an idle agentterm composer that agentctl itself typed (#989
/// agentctl-own-draft-submit): a prompt the queue records as typed, or a failed-queue digest
/// found by the messages it names. It is never retyped, the recovery is noted on its record, and
/// the prompt being delivered waits for the next pass, behind the turn the draft starts. `None`
/// for any other screen or draft, which [`agentterm_refusal`] refuses as before.
fn flush_own_agentterm_draft(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    screen: &str,
    runtime: &dyn AgentRuntime,
) -> Result<Option<String>> {
    use crate::harness::agentterm::{self, FrameState};
    let Some(frame) = agentterm::frame(screen) else {
        return Ok(None);
    };
    let (FrameState::Idle, Some(draft)) = (&frame.state, frame.draft) else {
        return Ok(None);
    };
    let Some(queue) = runtime.typed_prompts_queue() else {
        return Ok(None);
    };
    let Some(prompt) = crate::stale_draft::find(
        &queue,
        &crate::stale_draft::Draft::Text(draft),
        crate::stale_draft::Vouching::Unproven,
        std::time::SystemTime::now(),
    ) else {
        return Ok(None);
    };
    terminal.send_keys(pane_id, "Enter")?;
    let evidence = format!(
        "submitted an unsent draft {} that agentterm had left in its composer (1 Enter key press)",
        prompt.describe()
    );
    crate::stale_draft::note_recovered(&prompt, &evidence, agent_queue::queue::unix_seconds());
    Ok(Some(evidence))
}

/// Clear an idle agentterm composer that holds nothing but terminal query replies (#900
/// agentterm-paste-escape-replies), so the paste is not submitted behind them: Ctrl+C, which
/// agentterm documents as "clear the input line", then the composer must read back empty. A
/// composer holding anything else is returned untouched, and [`agentterm_refusal`] reports it.
/// `Err` is the refusal when the composer does not read back empty; nothing was typed.
fn clear_leaked_replies(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    screen: String,
    runtime: &dyn AgentRuntime,
) -> std::result::Result<String, String> {
    use crate::harness::agentterm::{self, FrameState};
    let leaked = agentterm::frame(&screen).is_some_and(|frame| {
        frame.state == FrameState::Idle
            && frame
                .draft
                .as_deref()
                .is_some_and(agentterm::only_query_replies)
    });
    if !leaked || runtime.stranded_retry() {
        return Ok(screen);
    }
    terminal
        .send_keys(pane_id, CLEAR_COMPOSER_KEY)
        .map_err(|error| {
            format!(
                "pane {pane_id}: agentterm's composer holds only leaked terminal replies, and \
                 the key that clears it failed: {error}; nothing was typed"
            )
        })?;
    let deadline = runtime.monotonic().saturating_add(CLEAR_WAIT);
    loop {
        let now = terminal.read_screen(pane_id).map_err(|error| {
            format!(
                "pane {pane_id}: screen read failed after clearing leaked terminal replies: \
                 {error}; nothing was typed"
            )
        })?;
        if agentterm::frame(&now)
            .is_some_and(|frame| frame.state == FrameState::Idle && frame.draft.is_none())
        {
            return Ok(now);
        }
        if runtime.monotonic() >= deadline || runtime.cancelled() {
            return Err(format!(
                "pane {pane_id}: agentterm's composer held only leaked terminal replies, and it \
                 did not read back empty after {CLEAR_COMPOSER_KEY}; nothing was typed"
            ));
        }
        runtime.sleep(POLL);
    }
}

/// Poll after the paste until it shows, was consumed, or the stage deadline passes.
fn wait_staged(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    text: &str,
    before: &str,
    limit: Duration,
    runtime: &dyn AgentRuntime,
) -> Result<Staged> {
    let before_key = screen::quiet_key(before);
    let deadline = runtime.monotonic().saturating_add(limit);
    let mut last_key = before_key.clone();
    let mut changed_since: Option<Duration> = None;
    loop {
        if runtime.cancelled() {
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: delivery was cancelled after the prompt was pasted; outcome is \
                 unknown"
            )));
        }
        runtime.sleep(POLL);
        let now_screen = terminal.read_screen(pane_id)?;
        let now = runtime.monotonic();
        let key = screen::quiet_key(&now_screen);
        if key != before_key {
            match screen::above_static_tail(before, &now_screen, text) {
                Some(true) => return Ok(Staged::Shown(now_screen)),
                Some(false) => return Ok(Staged::Answered(now_screen)),
                None => {}
            }
            if screen::echoed_above(&now_screen, text) {
                return Ok(Staged::Consumed);
            }
            // A change without our text (a paste placeholder, a scrolled box): wait for it to
            // settle, then take it as the staged screen.
            if key != last_key {
                changed_since = Some(now);
            } else if changed_since.is_some_and(|since| now.saturating_sub(since) >= SETTLE) {
                return Ok(Staged::Shown(now_screen));
            }
        }
        last_key = key;
        if now >= deadline {
            return Ok(Staged::Undrawn(now_screen));
        }
    }
}

/// Poll `pane_id` until `submitted` holds for its screen or `deadline` passes; whether it held.
fn await_submitted(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    runtime: &dyn AgentRuntime,
    deadline: Duration,
    submitted: &dyn Fn(&str) -> bool,
) -> Result<bool> {
    loop {
        if runtime.cancelled() {
            return Err(AdapterError::unavailable(format!(
                "pane {pane_id}: delivery was cancelled while its submission was confirmed; \
                 outcome is unknown"
            )));
        }
        if submitted(&terminal.read_screen(pane_id)?) {
            return Ok(true);
        }
        if runtime.monotonic() >= deadline {
            return Ok(false);
        }
        runtime.sleep(POLL);
    }
}

/// A paste that never changed the screen: press Enter once (it may sit invisibly in the TUI's
/// input) and watch. A change that shows the prompt submitted means the TUI took it; a change
/// that does not proves nothing (#852 D); no change at all means it took no input, and the
/// prompt is safe to retry.
#[allow(clippy::too_many_arguments)]
fn submit_undrawn(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    text: &str,
    before: &str,
    screen_now: &str,
    runtime: &dyn AgentRuntime,
    submitted: &dyn Fn(&str) -> bool,
    unproven: &dyn Fn(&str) -> Result<Submission>,
    receipt: impl Fn(u32, String, bool, bool) -> Result<Submission>,
) -> Result<Submission> {
    press_enter(terminal, pane_id, text, screen_now)?;
    let quiet = screen::quiet_key(screen_now);
    let mut deadline = runtime.monotonic().saturating_add(UNDRAWN_WATCH);
    let mut changed = false;
    loop {
        runtime.sleep(POLL);
        let after = terminal.read_screen(pane_id)?;
        if screen::quiet_key(&after) != quiet {
            if screen::above_static_tail(before, &after, text) == Some(true) {
                // The late render arrived after the Enter: still at the bottom. Leave it for the
                // next attempt, which submits it with Enter rather than pasting again.
                return Ok(Submission::NotStaged(format!(
                    "{STRANDED_PROMPT_KIND}: pane {pane_id}: the paste showed only after Enter \
                     and still sits at the bottom; it stays pending and later passes press Enter \
                     only"
                )));
            }
            if submitted(&after) {
                return receipt(
                    1,
                    "the screen showed the prompt submitted only after Enter on a paste it never \
                     showed"
                        .to_owned(),
                    true,
                    false,
                );
            }
            if !changed {
                // A change: give it the submit deadline to show the prompt submitted.
                changed = true;
                deadline = runtime.monotonic().saturating_add(SUBMIT_TIMEOUT);
            }
        }
        if runtime.monotonic() >= deadline {
            if changed {
                return unproven("the screen changed only after Enter on a paste it never showed");
            }
            return Ok(Submission::NotStaged(format!(
                "pane {pane_id}: the paste never showed on screen and one Enter changed nothing: \
                 the TUI took no input; safe to retry"
            )));
        }
    }
}

/// Press Enter unless a menu or dialog (other than our own text) is open on `screen_now`.
fn press_enter(
    terminal: &dyn PromptTerminal,
    pane_id: &str,
    text: &str,
    screen_now: &str,
) -> Result<()> {
    if let Some(line) = screen::menu_open(screen_now, Some(text)) {
        return Err(AdapterError::unavailable(format!(
            "pane {pane_id}: a menu or dialog opened over the staged prompt ({line:?}); no Enter \
             was sent, outcome is unknown"
        )));
    }
    terminal.send_keys(pane_id, "Enter")
}

#[cfg(test)]
pub(crate) mod fake {
    //! A minimal line-editor TUI: a transcript, one `> ` input row, and knobs for the ways real
    //! TUIs misbehave.
    use std::sync::Mutex;

    use crate::error::Result;
    use crate::submission::PromptTerminal;

    #[derive(Default)]
    pub struct State {
        pub transcript: Vec<String>,
        /// Text in the input buffer, drawn or not.
        pub input: String,
        /// Reads left before a fresh paste is drawn (a late render).
        pub draw_after_reads: usize,
        /// Whether the buffer is drawn yet.
        pub drawn: bool,
        /// Presses of Enter to drop before one is taken.
        pub drop_enters: usize,
        /// Bracketed pastes are ignored entirely.
        pub ignore_paste: bool,
        /// Take a paste as soon as it arrives, without Enter.
        pub auto_submit: bool,
        /// Draw the input inside a box with a footer, instead of a bare line.
        pub boxed: bool,
        /// A menu shown under the input.
        pub menu: Option<String>,
        /// Status rows drawn under everything else.
        pub footer: Vec<String>,
        /// Draw the input in agentterm's input frame under this label (`idle`, `⠹ working
        /// 0:12`, ...), instead of a bare line.
        pub agentterm: Option<String>,
        /// Enter takes the input but nothing shows it: no transcript row.
        pub swallow: bool,
        /// agentterm's Ctrl+C leaves the composer as it was.
        pub ignore_clear: bool,
        /// Every prompt the TUI took, in order.
        pub submitted: Vec<String>,
        /// Every key and paste received.
        pub inputs: Vec<String>,
        pub reads: usize,
    }

    pub struct FakeTui(pub Mutex<State>);

    impl FakeTui {
        pub fn new() -> Self {
            Self(Mutex::new(State {
                transcript: vec!["orc minimal 1.0".to_owned()],
                drawn: true,
                ..State::default()
            }))
        }
        pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().unwrap()
        }
    }

    fn take(state: &mut State) {
        let line = std::mem::take(&mut state.input);
        if !state.swallow {
            // agentterm draws a submitted input as a `You:` turn, one column in.
            let (first, rest, answer) = if state.agentterm.is_some() {
                (" You: ", "      ", " Agent: ")
            } else {
                ("> ", "  ", "")
            };
            for (index, row) in line.split('\n').enumerate() {
                let glyph = if index == 0 { first } else { rest };
                state.transcript.push(format!("{glyph}{row}"));
            }
            state
                .transcript
                .push(format!("{answer}ok: got {} chars", line.len()));
        }
        state.submitted.push(line);
        state.drawn = true;
    }

    impl PromptTerminal for FakeTui {
        fn read_screen(&self, _pane_id: &str) -> Result<String> {
            let mut state = self.state();
            state.reads += 1;
            if !state.drawn {
                if state.draw_after_reads == 0 {
                    state.drawn = true;
                } else {
                    state.draw_after_reads -= 1;
                }
            }
            let mut rows = state.transcript.clone();
            let input = if state.drawn {
                state.input.clone()
            } else {
                String::new()
            };
            let lines: Vec<String> = input
                .split('\n')
                .enumerate()
                .map(|(index, row)| format!("{}{row}", if index == 0 { "> " } else { "  " }))
                .collect();
            if let Some(label) = &state.agentterm {
                rows.push(format!("╭ {label} ──────────────────────────────[ lock ]╮"));
                if input.is_empty() {
                    rows.push("│> send a message to the session".to_owned());
                } else {
                    rows.extend(lines.iter().map(|row| format!("│{row}")));
                }
                rows.push("╰ ● host-a ─ model-x ─ ctx ███░ 37% ─────────────╯".to_owned());
                rows.push(" ⏎ send · / commands · ^X b sessions · ^D quit".to_owned());
            } else if state.boxed {
                rows.push("┌ Input ─────────────────────────┐".to_owned());
                rows.extend(lines.iter().map(|row| format!("│ {row}")));
                rows.push("└────────────────────────────────┘".to_owned());
                rows.push(format!("[idle] {} reads", state.reads));
            } else {
                rows.extend(lines);
            }
            if let Some(menu) = &state.menu {
                rows.push(menu.clone());
            }
            rows.extend(state.footer.iter().cloned());
            Ok(rows.join("\n"))
        }

        fn send_text(&self, _pane_id: &str, text: &str) -> Result<()> {
            let mut state = self.state();
            state.inputs.push(format!("paste:{text}"));
            if state.ignore_paste {
                return Ok(());
            }
            let body = text
                .trim_start_matches("\u{1b}[200~")
                .trim_end_matches("\u{1b}[201~");
            state.input.push_str(body);
            if state.draw_after_reads > 0 {
                state.drawn = false;
            }
            if state.auto_submit {
                take(&mut state);
            }
            Ok(())
        }

        fn send_keys(&self, _pane_id: &str, keys: &str) -> Result<()> {
            let mut state = self.state();
            state.inputs.push(format!("key:{keys}"));
            if keys == "ctrl+c" && state.agentterm.is_some() && !state.ignore_clear {
                state.input.clear();
            }
            if keys == "Enter" {
                if state.drop_enters > 0 {
                    state.drop_enters -= 1;
                } else if !state.input.is_empty() {
                    take(&mut state);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::fake::FakeTui;
    use super::*;

    #[derive(Default)]
    struct Clock(AtomicU64);

    impl AgentRuntime for Clock {
        fn monotonic(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
        fn sleep(&self, duration: Duration) {
            self.0.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::SeqCst,
            );
        }
    }

    const PROMPT: &str =
        "[from coordb via agentctl · reply: agentctl send coordb]\nplease run the gate and report the commit";

    fn run(tui: &FakeTui) -> Result<Submission> {
        submit(tui, "w1:p1", PROMPT, timeouts(), &Clock::default())
    }

    fn enters(tui: &FakeTui) -> usize {
        tui.state()
            .inputs
            .iter()
            .filter(|input| *input == "key:Enter")
            .count()
    }

    fn pastes(tui: &FakeTui) -> usize {
        tui.state()
            .inputs
            .iter()
            .filter(|input| input.starts_with("paste:"))
            .count()
    }

    #[test]
    fn delivers_into_a_line_editor_with_one_enter() {
        let tui = FakeTui::new();
        let Submission::Verified(receipt) = run(&tui).unwrap() else {
            panic!("expected verified");
        };
        assert_eq!(receipt.key_presses, 1);
        assert!(receipt.printed, "{receipt:?}");
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(enters(&tui), 1);
    }

    #[test]
    fn delivers_into_an_idle_agentterm_frame() {
        // #852 unknown-tui-bridge.
        let tui = FakeTui::new();
        tui.state().agentterm = Some("idle".to_owned());
        let outcome = run(&tui).unwrap();
        assert!(matches!(outcome, Submission::Verified(_)), "{outcome:?}");
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 1);
    }

    #[test]
    fn agentterm_is_never_typed_into_during_a_turn_or_under_another_label() {
        let tui = FakeTui::new();
        tui.state().agentterm = Some("⠹ working 3:50".to_owned());
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a refusal");
        };
        assert!(reason.contains("running turn"), "{reason}");
        tui.state().agentterm = Some("reconnecting".to_owned());
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a refusal");
        };
        assert!(reason.contains("\"reconnecting\""), "{reason}");
        assert!(tui.state().inputs.is_empty(), "nothing was typed");
    }

    /// What late colour answers leave in agentterm's composer, wrapped over two rows (#900).
    fn leaked_replies() -> String {
        let pair = r"]10;rgb:dcdc/dcdc/dcdc\]11;rgb:0000/0000/0000\";
        format!("{}\n{}", pair.repeat(20), pair.repeat(20))
    }

    #[test]
    fn leaked_terminal_replies_are_cleared_and_the_message_goes_alone() {
        // #900 agentterm-paste-escape-replies.
        let tui = FakeTui::new();
        tui.state().agentterm = Some("idle".to_owned());
        tui.state().input = leaked_replies();
        let outcome = run(&tui).unwrap();
        assert!(matches!(outcome, Submission::Verified(_)), "{outcome:?}");
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        let inputs = tui.state().inputs.clone();
        assert_eq!(inputs.first().map(String::as_str), Some("key:ctrl+c"));
        assert_eq!(pastes(&tui), 1);
    }

    #[test]
    fn typed_text_beside_leaked_replies_is_kept_and_the_paste_refused() {
        let tui = FakeTui::new();
        tui.state().agentterm = Some("idle".to_owned());
        let draft = format!("the owner typed this {}", leaked_replies());
        tui.state().input = draft.clone();
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a refusal");
        };
        assert!(reason.contains("composer already holds"), "{reason}");
        assert!(
            tui.state().inputs.is_empty(),
            "nothing was typed or cleared"
        );
        assert_eq!(tui.state().input, draft);
    }

    #[test]
    fn leaked_replies_that_do_not_clear_refuse_the_paste() {
        let tui = FakeTui::new();
        {
            let mut state = tui.state();
            state.agentterm = Some("idle".to_owned());
            state.input = leaked_replies();
            state.ignore_clear = true;
        }
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a refusal");
        };
        assert!(reason.contains("did not read back empty"), "{reason}");
        assert_eq!(tui.state().inputs, vec!["key:ctrl+c".to_owned()]);
        assert!(tui.state().submitted.is_empty());
    }

    #[test]
    fn an_agentterm_draft_is_refused_but_our_own_stranded_paste_is_sent() {
        let tui = FakeTui::new();
        tui.state().agentterm = Some("idle".to_owned());
        tui.state().input = "half a sentence the owner typed".to_owned();
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a refusal");
        };
        assert!(
            reason.contains("composer already holds unsubmitted text \"half a sentence"),
            "{reason}"
        );
        assert!(tui.state().inputs.is_empty(), "nothing was typed");
        // A paste of this very prompt left by an earlier pass gets Enter, never a second paste.
        tui.state().input = PROMPT.to_owned();
        assert!(matches!(run(&tui).unwrap(), Submission::Verified(_)));
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 0);
    }

    #[test]
    fn an_agentterm_pane_reads_idle_or_working_from_its_frame_not_its_stillness() {
        use crate::harness::agentterm::tests::{IDLE, WORKING};
        assert_eq!(status(IDLE, Duration::ZERO), "idle");
        assert_eq!(status(WORKING, Duration::from_secs(600)), "working");
        let other = IDLE.replace("╭ idle ─", "╭ reconnecting ─");
        assert_eq!(status(&other, Duration::from_secs(600)), "unknown");
    }

    #[test]
    fn delivers_into_a_boxed_composer_with_a_ticking_footer() {
        let tui = FakeTui::new();
        tui.state().boxed = true;
        assert!(matches!(run(&tui).unwrap(), Submission::Verified(_)));
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(enters(&tui), 1);
    }

    #[test]
    fn a_late_render_is_submitted_once() {
        let tui = FakeTui::new();
        tui.state().draw_after_reads = 30;
        assert!(matches!(run(&tui).unwrap(), Submission::Verified(_)));
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 1);
    }

    #[test]
    fn a_paste_drawn_only_after_the_stage_deadline_is_submitted_by_the_enter_under_uncertainty() {
        let tui = FakeTui::new();
        // Never drawn within 20 s of 100 ms reads.
        tui.state().draw_after_reads = 1_000;
        let outcome = run(&tui).unwrap();
        assert!(matches!(outcome, Submission::Verified(_)), "{outcome:?}");
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 1);
        assert_eq!(enters(&tui), 1);
    }

    #[test]
    fn a_stuck_prompt_is_flushed_with_extra_enter_presses() {
        let tui = FakeTui::new();
        tui.state().drop_enters = 2;
        let Submission::Verified(receipt) = run(&tui).unwrap() else {
            panic!("expected verified");
        };
        assert_eq!(receipt.key_presses, 3);
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 1, "retries are bare Enter presses only");
    }

    #[test]
    fn a_prompt_that_never_leaves_is_stranded_after_the_press_cap() {
        let tui = FakeTui::new();
        tui.state().drop_enters = 100;
        // Pending, never possibly submitted.
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a pending refusal");
        };
        assert!(is_stranded_prompt(&reason), "{reason}");
        assert_eq!(enters(&tui), MAX_ENTER_PRESSES as usize);
        assert_eq!(pastes(&tui), 1);
        assert!(tui.state().submitted.is_empty());
        // The next attempt finds its own text stranded and presses Enter instead of pasting.
        tui.state().drop_enters = 0;
        assert!(matches!(run(&tui).unwrap(), Submission::Verified(_)));
        assert_eq!(pastes(&tui), 1, "never pasted twice");
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
    }

    /// A drain's runtime for a prompt an earlier pass left stranded.
    #[derive(Default)]
    struct StrandedRetry(Clock);

    impl AgentRuntime for StrandedRetry {
        fn monotonic(&self) -> Duration {
            self.0.monotonic()
        }
        fn sleep(&self, duration: Duration) {
            self.0.sleep(duration);
        }
        fn stranded_retry(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_stranded_retry_presses_enter_only_and_never_pastes() {
        let tui = FakeTui::new();
        {
            let mut state = tui.state();
            // A tall footer puts the text beyond the rows a fresh delivery would call stranded.
            state.boxed = true;
            state.footer = vec!["Session: orc".to_owned(), "Agents: total=1".to_owned()];
            state.input = PROMPT.to_owned();
            state.drop_enters = 1;
        }
        assert!(!screen::stranded(
            &tui.read_screen("w1:p1").unwrap(),
            PROMPT
        ));
        let runtime = StrandedRetry::default();
        let outcome = submit(&tui, "w1:p1", PROMPT, timeouts(), &runtime).unwrap();
        assert!(matches!(outcome, Submission::Verified(_)), "{outcome:?}");
        assert_eq!(pastes(&tui), 0, "a stranded prompt is never typed again");
        assert_eq!(
            enters(&tui),
            2,
            "{outcome:?} {:?}",
            tui.read_screen("w1:p1")
        );
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        // Gone from the bottom since the last pass, but nothing shows it submitted: unproven,
        // never confirmed (#852 D).
        let error = submit(
            &tui,
            "w1:p1",
            "some other prompt text entirely",
            timeouts(),
            &runtime,
        )
        .unwrap_err();
        assert!(error.message().contains("unproven"), "{error:?}");
        assert_eq!(enters(&tui), 2);
        assert_eq!(pastes(&tui), 0);
    }

    #[test]
    fn a_receipt_needs_the_prompt_shown_submitted_never_only_gone() {
        // #852 D chat-receipt evidence: a TUI that takes the input but shows nothing gives no
        // receipt; the outcome is unknown, so the queue never types the prompt again.
        let tui = FakeTui::new();
        tui.state().swallow = true;
        let error = run(&tui).unwrap_err();
        assert!(
            error
                .message()
                .contains("no row shows the prompt submitted"),
            "{error:?}"
        );
        assert_eq!(pastes(&tui), 1);
    }

    #[test]
    fn an_agentterm_receipt_needs_a_new_you_turn_and_text_in_the_composer_is_none() {
        // #852 D: on agentterm the evidence is a `You:` turn in the conversation that shows the
        // prompt. Text in the composer means it was not submitted.
        let tui = FakeTui::new();
        tui.state().agentterm = Some("idle".to_owned());
        let Submission::Verified(receipt) = run(&tui).unwrap() else {
            panic!("expected verified");
        };
        assert!(receipt.evidence.contains("shows submitted"), "{receipt:?}");
        let screen = tui.read_screen("w1:p1").unwrap();
        assert_eq!(
            crate::harness::agentterm::submitted_turns(&screen, PROMPT),
            1
        );
        // Taken, but no turn drawn: unproven.
        let tui = FakeTui::new();
        {
            let mut state = tui.state();
            state.agentterm = Some("idle".to_owned());
            state.swallow = true;
        }
        let error = run(&tui).unwrap_err();
        assert!(error.message().contains("`You:` turn"), "{error:?}");
        // Still in the composer after every Enter: stranded and pending, never a receipt.
        let tui = FakeTui::new();
        {
            let mut state = tui.state();
            state.agentterm = Some("idle".to_owned());
            state.drop_enters = 100;
        }
        let Submission::NotStaged(reason) = run(&tui).unwrap() else {
            panic!("expected a pending refusal");
        };
        assert!(is_stranded_prompt(&reason), "{reason}");
        let screen = tui.read_screen("w1:p1").unwrap();
        assert_eq!(
            crate::harness::agentterm::submitted_turns(&screen, PROMPT),
            0
        );
    }

    #[test]
    fn a_paste_taken_before_enter_is_not_submitted_twice() {
        let tui = FakeTui::new();
        tui.state().auto_submit = true;
        let Submission::Verified(receipt) = run(&tui).unwrap() else {
            panic!("expected verified");
        };
        // New rows follow the text: taken already. One bare Enter under uncertainty lands on an
        // empty input and takes nothing more.
        assert_eq!(receipt.key_presses, 1);
        assert_eq!(tui.state().submitted, vec![PROMPT.to_owned()]);
        assert_eq!(pastes(&tui), 1);
    }

    #[test]
    fn a_paste_the_tui_ignores_is_safe_to_retry() {
        let tui = FakeTui::new();
        tui.state().ignore_paste = true;
        let outcome = run(&tui).unwrap();
        assert!(matches!(outcome, Submission::NotStaged(_)), "{outcome:?}");
        assert!(tui.state().submitted.is_empty());
    }

    #[test]
    fn an_open_menu_is_never_typed_into() {
        let tui = FakeTui::new();
        tui.state().menu = Some("❯ 1. Approve\n  2. Deny".to_owned());
        assert!(matches!(run(&tui).unwrap(), Submission::NotStaged(_)));
        assert!(tui.state().inputs.is_empty());
    }

    #[test]
    fn a_menu_that_opens_over_the_staged_prompt_stops_the_enter_presses() {
        let tui = FakeTui::new();
        tui.state().drop_enters = 1;
        // The menu appears after the paste and the first Enter.
        struct MenuAfterFirstEnter<'a>(&'a FakeTui);
        impl PromptTerminal for MenuAfterFirstEnter<'_> {
            fn read_screen(&self, pane: &str) -> Result<String> {
                self.0.read_screen(pane)
            }
            fn send_text(&self, pane: &str, text: &str) -> Result<()> {
                self.0.send_text(pane, text)
            }
            fn send_keys(&self, pane: &str, keys: &str) -> Result<()> {
                self.0.send_keys(pane, keys)?;
                self.0.state().menu = Some("❯ 1. Yes\n  2. No".to_owned());
                Ok(())
            }
        }
        let error = submit(
            &MenuAfterFirstEnter(&tui),
            "w1:p1",
            PROMPT,
            timeouts(),
            &Clock::default(),
        )
        .unwrap_err();
        assert!(error.message().contains("menu"), "{error:?}");
        assert_eq!(enters(&tui), 1);
    }

    #[test]
    fn readiness_waits_for_a_quiet_screen() {
        let mut quiet = Quiescence::default();
        let at = Duration::from_secs;
        assert_eq!(status("a\n> ", quiet.observe("a\n> ", at(0))), "working");
        assert_eq!(status("a\n> ", quiet.observe("a\n> ", at(1))), "working");
        assert_eq!(status("a\n> ", quiet.observe("a\n> ", at(2))), "idle");
        assert_eq!(status("b\n> ", quiet.observe("b\n> ", at(3))), "working");
        let menu = "❯ 1. Yes\n  2. No";
        assert_eq!(status(menu, quiet.observe(menu, at(9))), "blocked");
    }
}
