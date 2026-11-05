//! The agentterm view of a remote agent session, as the generic rules see it (#852
//! unknown-tui-bridge).
//!
//! agentterm draws a remote session in a local pane. agentctl drives such a pane with the
//! unknown-harness rules ([`crate::unknown_harness`]), which wait for the screen to stand still
//! before they type. agentterm's screen does not stand still while the session is idle, and does
//! while a long turn runs without output, so this module reads what agentterm itself shows in
//! its input frame at the bottom of the screen:
//!
//! ```text
//! ╭ idle ──────────────────────────────────────────────[ lock ]╮
//! │> send a message to the session                              │
//! ╰ ● host ─ model · harness ─ ctx ███░ 37% ─────────────────────╯
//!  ⏎ send · / commands · ^X b sessions · ^D quit · ↓ status
//! ```
//!
//! The frame's label is `idle`, or a spinner glyph and `working M:SS` while a turn runs (its
//! composer then reads `write here to steer during this turn`, and Enter steers the turn). A
//! composer row after `│>` that is neither placeholder is a draft. Any other label is reported
//! as it reads, and nothing is typed under it.

/// What agentterm's input frame says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameState {
    /// `idle`: the session waits for a message.
    Idle,
    /// `working M:SS`: a turn runs, and Enter would steer it.
    Working,
    /// Any other label, as it reads.
    Other(String),
}

/// agentterm's input frame, read from the bottom of a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The frame's label.
    pub state: FrameState,
    /// The composer's text when it holds a draft rather than a placeholder, its rows joined
    /// with newlines.
    pub draft: Option<String>,
}

/// The composer placeholders agentterm draws, which are not a draft: idle, and during a turn.
const PLACEHOLDERS: [&str; 2] = ["send a message to ", "write here to steer during this turn"];

/// Glyphs agentterm draws before `working` while a turn runs.
fn is_spinner(glyph: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&glyph) || "✳✶✻✽·*◐◓◑◒|/-\\".contains(glyph)
}

/// agentterm's input frame on `screen`, or `None` when the screen does not end with one: the
/// last row that opens with `╭ ` and a label, closes with `╮`, and is followed by a `│>` composer
/// row and, after it, a `╰` row.
#[must_use]
pub fn frame(screen: &str) -> Option<Frame> {
    let rows: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let top = rows.iter().rposition(|row| {
        let row = row.trim_start();
        row.starts_with("╭ ") && row.ends_with('╮') && row.contains('─')
    })?;
    let composer = rows.get(top + 1)?.trim_start().strip_prefix("│>")?;
    let mut draft_rows = vec![composer.trim_end_matches('│').trim()];
    let mut closed = false;
    for row in &rows[top + 2..] {
        let row = row.trim_start();
        if row.starts_with('╰') {
            closed = true;
            break;
        }
        draft_rows.push(row.trim_start_matches('│').trim_end_matches('│').trim());
    }
    if !closed {
        return None;
    }
    let label = rows[top]
        .trim_start()
        .trim_start_matches('╭')
        .split('─')
        .next()
        .unwrap_or_default()
        .trim();
    let words = label.trim_start_matches(is_spinner).trim();
    let state = if words == "idle" {
        FrameState::Idle
    } else if words.starts_with("working") {
        FrameState::Working
    } else {
        FrameState::Other(label.to_owned())
    };
    while draft_rows.last().is_some_and(|row| row.is_empty()) {
        draft_rows.pop();
    }
    let text = draft_rows.join("\n");
    let placeholder = draft_rows.len() == 1
        && PLACEHOLDERS
            .iter()
            .any(|placeholder| text.starts_with(placeholder));
    Some(Frame {
        state,
        draft: (!text.is_empty() && !placeholder).then_some(text),
    })
}

/// How many letters and digits from the start of a prompt identify it in a `You:` turn.
const TURN_PROBE: usize = 64;

/// How many submitted turns on `screen` show `text` (#852 D, unknown-pane receipts need
/// evidence): agentterm draws an input the session took as a `You:` turn in its conversation,
/// above the input frame. A turn shows `text` when its rows, up to the next labelled row,
/// contain the first [`TURN_PROBE`] letters and digits of `text`. The same text sitting in the
/// composer is not a turn: it means the input was not submitted. A side panel down the right edge
/// is cut off first, so its rows never split a wrapped turn.
#[must_use]
pub fn submitted_turns(screen: &str, text: &str) -> usize {
    use crate::screen_heuristics::squash;
    let probe: String = squash(text).chars().take(TURN_PROBE).collect();
    if probe.is_empty() {
        return 0;
    }
    let normalized = crate::chat_runtime::screen_layout::normalize(screen);
    let mut rows: Vec<&str> = normalized.lines().collect();
    // Only the conversation counts: everything from the input frame down is dropped.
    if let Some(top) = rows.iter().rposition(|row| {
        let row = row.trim_start();
        row.starts_with("╭ ") && row.ends_with('╮')
    }) {
        rows.truncate(top);
    }
    let label = |row: &str| -> Option<bool> {
        let row = row.trim_start();
        if row.starts_with("You: ") || row.starts_with("❯ ") {
            Some(true)
        } else if row.starts_with("Agent: ") || row.starts_with("··· ") || row.starts_with("↳ ")
        {
            Some(false)
        } else {
            None
        }
    };
    let mut count = 0;
    let mut turn: Option<String> = None;
    for row in rows.iter().chain(std::iter::once(&"Agent: ")) {
        match label(row) {
            Some(prompt) => {
                if turn.take().is_some_and(|text| text.contains(&probe)) {
                    count += 1;
                }
                if prompt {
                    turn = Some(squash(row));
                }
            }
            None => {
                if let Some(text) = turn.as_mut() {
                    text.push_str(&squash(row));
                }
            }
        }
    }
    count
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The bottom of an idle agentterm screen, laid out as a recorded one (names replaced).
    pub(crate) const IDLE: &str = "\
 Agent: Nothing is stuck. No lane is waiting on an answer.                                 │                      │
 8:49 AM ET · 19s                                                                          ╰──────────────────────╯

╭ idle ────────────────────────────────────────────────────────────────────────────────────────────────[ lock ]╮
│> send a message to the session                                                                               │
╰ ● host-a ─ model-x · harness-y ─ ctx ███████░ 87% ───────────────────────────────────────────────────────────╯
 ↻ tasks   ◎ 1 sub   ▣ 51 artifacts
 ⏎ send · / commands · ^X b sessions · ^D quit · ↓ status                                   \"session-a\" 2bed1f68
";

    /// The bottom of an agentterm screen while a turn runs, laid out as a recorded one.
    pub(crate) const WORKING: &str = "\
 ✳ Weighing… (230s · ↓ 1.3k tokens · esc to interrupt)                                     ╰──────────────────────╯
 ↳ queued: You: [from lane-a via agentctl · reply: agentctl send lane-a] report 1 of 5

╭ ⠹ working 3:50 ──────────────────────────────────────────────────────────────────────────────────────[ lock ]╮
│> write here to steer during this turn                                                                        │
╰ ● host-a ─ model-x · harness-y ─ ctx ██░░░░░░ ~24% ──────────────────────────────────────────────────────────╯
 ↻ tasks   ▣ 7 artifacts
 esc interrupt · ⏎ steer · /eot after-turn · ↓ status                                       \"session-a\" 2bed1f68
";

    #[test]
    fn an_idle_frame_with_its_placeholder_holds_no_draft() {
        assert_eq!(
            frame(IDLE),
            Some(Frame {
                state: FrameState::Idle,
                draft: None
            })
        );
    }

    #[test]
    fn a_working_frame_is_working_whatever_its_spinner_glyph() {
        assert_eq!(
            frame(WORKING),
            Some(Frame {
                state: FrameState::Working,
                draft: None
            })
        );
        let other_glyph = WORKING.replace("⠹ working", "⠼ working");
        assert_eq!(frame(&other_glyph).unwrap().state, FrameState::Working);
    }

    #[test]
    fn composer_text_that_is_no_placeholder_is_a_draft_on_every_row() {
        let draft = IDLE.replace(
            "│> send a message to the session                                                                               │",
            "│> half a sentence the owner typed                                                                             │\n│  and its second row                                                                                          │",
        );
        assert_eq!(
            frame(&draft).unwrap().draft.as_deref(),
            Some("half a sentence the owner typed\nand its second row")
        );
    }

    #[test]
    fn another_label_reads_as_it_is_and_a_screen_without_the_frame_has_none() {
        let stale = IDLE.replace("╭ idle ─", "╭ reconnecting ─");
        assert_eq!(
            frame(&stale).unwrap().state,
            FrameState::Other("reconnecting".to_owned())
        );
        assert_eq!(frame("plain shell prompt $ \n"), None);
        // A side panel's top border is no input frame: it has no composer row under it.
        assert_eq!(frame(" text ╭ nodes ────╮\n more │attached │\n"), None);
    }
    #[test]
    fn a_submitted_turn_shows_the_prompt_and_the_composer_never_does() {
        // #852 D: evidence is a `You:` turn in the conversation, never the composer.
        let prompt = "[from U0OWNER via slack C0CHANNEL · sent: 2026.11.04:14:46 PST · reply: \
                      <CHAT_REPLY_047> or agentctl chat reply 047]\n\n<chat_message>\nDid you \
                      edit the weekly post to link it?\n</chat_message>";
        let screen = crate::chat_runtime::screen_layout::tests::agentterm_screen();
        assert_eq!(
            submitted_turns(&screen, prompt),
            1,
            "wrapped beside a side panel"
        );
        assert_eq!(
            submitted_turns(&screen, "[from someone else via slack] other"),
            0
        );
        // The same prompt only in the composer: not submitted.
        let composed = IDLE.replace(
            "│> send a message to the session                                                                               │",
            "│> [from U0OWNER via slack C0CHANNEL · sent: 2026.11.04:14:46 PST · reply: <CHAT_REPLY_047> or agentctl  │",
        );
        assert_eq!(submitted_turns(&composed, prompt), 0);
        // Nor a queued one, which waits for the turn to end.
        assert_eq!(
            submitted_turns(
                WORKING,
                "[from lane-a via agentctl · reply: agentctl send lane-a] report 1 of 5"
            ),
            0
        );
    }
}
