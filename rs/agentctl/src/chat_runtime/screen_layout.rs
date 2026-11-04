//! A pane whose program draws its conversation behind a uniform left margin, as a TUI that
//! agentctl drives by generic screen rules can (#852 unknown-tui-bridge), read in the layout the
//! reply scanner expects.
//!
//! The scanner reads a screen the way Claude Code, Codex and Muse draw theirs: each message
//! starts at column 0, and rows that continue an item sit right of it. It opens no block above
//! the first row at column 0, which may continue an item whose start is out of view. A TUI such
//! as agentterm draws every row of its conversation one column in, with its own input frame at
//! the bottom and, while a panel is open, a box down the right edge, so no block ever opened on
//! its screen and its replies were lost without a word. [`normalize`] rewrites such a screen:
//!
//! * the input frame at the bottom (a row at column 0 starting with `╭`, and every row after it)
//!   is dropped;
//! * a box down the right edge (its top-left corner `╭` at column 40 or right of it, on a row that
//!   ends with `╮`, and the rows below it whose box border sits in the same column) is cut off;
//! * the margin every remaining nonblank row shares is removed;
//! * a row labelled `You: ` is the agent's prompt, so it is read as a prompt row (`❯ `), whose
//!   hanging rows the scanner skips; the hanging indent of a row labelled `Agent: `, whose
//!   wrapped rows line up after the label, is removed, so they read as the label's text.
//!
//! A screen with any nonblank row at column 0 above its input frame is left as it is: that is
//! the layout the scanner reads already. So is one with no labelled turn, which may be a capture
//! of another TUI that starts inside an indented item.

use std::borrow::Cow;

use unicode_width::UnicodeWidthChar;

/// The leftmost column at which a box down the right edge is taken for a side panel.
const MIN_PANEL_COLUMN: usize = 40;
/// How far a panel row's border may sit from the top border's column: a wide character left of
/// it shifts a row by a column.
const PANEL_COLUMN_SLACK: usize = 2;
/// The label of a row that starts the agent's prompt.
const PROMPT_LABEL: &str = "You: ";
/// The label of a row that starts the agent's reply text.
const AGENT_LABEL: &str = "Agent: ";

/// `screen` in the scanner's layout, as the module documentation describes; the screen itself
/// when it already is.
pub(super) fn normalize(screen: &str) -> Cow<'_, str> {
    let mut rows: Vec<&str> = screen.split('\n').collect();
    if let Some(frame) = input_frame(&rows) {
        rows.truncate(frame);
    }
    // Only a margined screen whose turns carry the labels: a capture of another TUI can start
    // inside an indented item, whose rows the scanner must not read as starting at column 0.
    let margined = rows
        .iter()
        .all(|row| row.trim().is_empty() || row.starts_with(' '))
        && rows.iter().any(|row| {
            let text = row.trim_start_matches(' ');
            row.len() > text.len()
                && (text.starts_with(PROMPT_LABEL) || text.starts_with(AGENT_LABEL))
        });
    if !margined {
        return Cow::Borrowed(screen);
    }
    let mut rows: Vec<String> = cut_side_panel(&rows);
    let margin = rows
        .iter()
        .filter(|row| !row.trim().is_empty())
        .map(|row| row.len() - row.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    for row in &mut rows {
        let cut = margin.min(row.len() - row.trim_start_matches(' ').len());
        row.replace_range(..cut, "");
    }
    relabel(&mut rows);
    Cow::Owned(rows.join("\n"))
}

/// The index of the first row of the input frame at the bottom of `rows`: the last row at column
/// 0 that starts with `╭`, when every nonblank row after it starts at column 0 with a box border
/// or is indented, as a frame and the hint rows under it are.
fn input_frame(rows: &[&str]) -> Option<usize> {
    let top = rows.iter().rposition(|row| row.starts_with('╭'))?;
    rows.get(top + 1)
        .is_some_and(|row| row.starts_with('│'))
        .then_some(top)
}

/// The display column of each character of `row`.
fn columns(row: &str) -> impl Iterator<Item = (usize, usize, char)> + '_ {
    let mut column = 0;
    row.char_indices().map(move |(at, character)| {
        let here = column;
        column += character.width().unwrap_or(0);
        (here, at, character)
    })
}

/// `rows` with a box down the right edge cut off: see the module documentation.
fn cut_side_panel(rows: &[&str]) -> Vec<String> {
    let top = rows.iter().enumerate().find_map(|(index, row)| {
        if !row.trim_end().ends_with('╮') {
            return None;
        }
        columns(row)
            .find(|(column, _, character)| *character == '╭' && *column >= MIN_PANEL_COLUMN)
            .map(|(column, _, _)| (index, column))
    });
    let Some((top, column)) = top else {
        return rows.iter().map(|row| (*row).to_owned()).collect();
    };
    let mut out: Vec<String> = rows[..top].iter().map(|row| (*row).to_owned()).collect();
    let mut open = true;
    for row in &rows[top..] {
        let border = open
            .then(|| {
                columns(row).find(|(at_column, _, character)| {
                    matches!(character, '╭' | '│' | '╰')
                        && at_column.abs_diff(column) <= PANEL_COLUMN_SLACK
                })
            })
            .flatten();
        match border {
            Some((_, at, character)) => {
                out.push(row[..at].trim_end().to_owned());
                if character == '╰' {
                    open = false;
                }
            }
            None => {
                open = false;
                out.push((*row).to_owned());
            }
        }
    }
    out
}

/// Read the labels of a margined screen: see the module documentation.
fn relabel(rows: &mut [String]) {
    let hanging = " ".repeat(AGENT_LABEL.len());
    let mut in_agent_text = false;
    for row in rows.iter_mut() {
        if let Some(rest) = row.strip_prefix(PROMPT_LABEL) {
            *row = format!("❯ {rest}");
            in_agent_text = false;
        } else if row.starts_with(AGENT_LABEL) {
            in_agent_text = true;
        } else if in_agent_text
            && row.starts_with(&hanging)
            && !row[hanging.len()..].starts_with(' ')
        {
            row.replace_range(..hanging.len(), "");
        } else {
            in_agent_text = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width of the conversation left of the panel in [`agentterm_screen`].
    const LEFT: usize = 104;

    /// A screen drawn the way agentterm draws one (recorded from a live pane, with its text
    /// replaced): a one-column margin, a panel down the right edge, labelled turns, and the input
    /// frame and its hints at the bottom.
    pub(crate) fn agentterm_screen() -> String {
        let panel = [
            "╭ nodes ──────────────╮",
            "│attached              │",
            "│  ● host-a · read…    │",
            "│available             │",
            "│  · host-b · read…    │",
        ];
        let conversation = [
            "You: [from a teammate via agentctl · reply: agentctl send teammate]       5:46 PM ET",
            "teammate -> coord: the chat bridge now targets you.",
            "··· worked 4s · 1 step",
            "Agent: The chat bridge has moved to me.",
            "",
            "You: [from U0OWNER via slack C0CHANNEL · sent: 2026.11.04:14:46 PST · reply: <CHAT_REPLY_047> or",
            "     agentctl chat reply 047]",
            "",
            "<chat_message>",
            "Did you edit the weekly post to link it?",
            "</chat_message>",
            "",
            "Reply between a line <CHAT_REPLY_047> and a line </CHAT_REPLY_047>, with no tool call between them;",
            "send several replies when that helps (progress updates, one reply per topic), each in its own pair.",
            "",
            "Agent: <CHAT_REPLY_047>",
            "Not yet. I sent the link to the lead for the weekly post, but I have not edited the post myself. I'll",
            "do it now, and reply here once it is done.",
            "</CHAT_REPLY_047>",
            "··· worked 26s · 4 steps",
            "Agent: The lead says the bridge cannot read my replies from the pane yet, so I am checking the two",
            "       replies I wrote.",
            "✳ Weighing… (230s · ↓ 1.3k tokens · esc to interrupt)",
        ];
        let mut rows = Vec::new();
        let last = conversation.len() - 1;
        for (index, text) in conversation.iter().enumerate() {
            let side = if index == last {
                "╰──────────────────────╯"
            } else {
                panel
                    .get(index)
                    .copied()
                    .unwrap_or("│                      │")
            };
            let pad = LEFT - text.chars().count();
            rows.push(format!(" {text}{}{side}", " ".repeat(pad)));
        }
        rows.extend([
            " ↳ queued: You: [from teammate via agentctl · reply: agentctl send teammate] Handover notes".to_owned(),
            String::new(),
            format!("╭ ⠹ working 3:50 {}[ lock ]╮", "─".repeat(100)),
            format!("│> write here to steer during this turn{}│", " ".repeat(86)),
            format!("╰ ● host-a ─ opus 5.5 · claude_code ─ ctx ~24% {}╯", "─".repeat(78)),
            " ↻ tasks   ▣ 7 artifacts".to_owned(),
            " esc interrupt · ⏎ steer · /eot after-turn · ↓ status                \"coord\" 2bed1f68".to_owned(),
        ]);
        rows.join("\n")
    }

    #[test]
    fn an_agentterm_screen_loses_its_margin_panel_and_frame_and_reads_its_labels() {
        let screen = agentterm_screen();
        let normalized = normalize(&screen);
        let rows: Vec<&str> = normalized.split('\n').collect();
        assert_eq!(
            rows[0],
            "❯ [from a teammate via agentctl · reply: agentctl send teammate]       5:46 PM ET"
        );
        assert!(rows.contains(&"Agent: <CHAT_REPLY_047>"), "{normalized}");
        assert!(rows.contains(&"</CHAT_REPLY_047>"), "{normalized}");
        assert!(
            rows.contains(&"do it now, and reply here once it is done."),
            "{normalized}"
        );
        // The hanging rows of a labelled reply read as its text.
        assert!(rows.contains(&"replies I wrote."), "{normalized}");
        // The hanging rows of the prompt stay right of its prompt row.
        assert!(
            rows.contains(&"     agentctl chat reply 047]"),
            "{normalized}"
        );
        assert!(!normalized.contains('│'), "{normalized}");
        assert!(!normalized.contains("write here to steer"), "{normalized}");
        assert!(rows
            .iter()
            .all(|row| row.trim().is_empty() || !row.ends_with(' ')));
    }

    #[test]
    fn a_screen_with_rows_at_column_zero_is_left_as_it_is() {
        let claude = "⏺ <CHAT_REPLY_001>\n  Done.\n  </CHAT_REPLY_001>\n\n╭────╮\n│ >  │\n╰────╯";
        assert!(matches!(normalize(claude), Cow::Borrowed(_)));
        let codex = "• Ran tests\n  └ ok\n\n› next prompt";
        assert!(matches!(normalize(codex), Cow::Borrowed(_)));
        // A capture that starts inside an indented item has no labelled turn.
        let cut = "  <CHAT_REPLY_001>\n  an answer\n  </CHAT_REPLY_001>";
        assert!(matches!(normalize(cut), Cow::Borrowed(_)));
        assert!(matches!(normalize(""), Cow::Borrowed(_)));
    }

    #[test]
    fn a_table_is_not_taken_for_a_side_panel() {
        let screen = concat!(
            " Agent: the numbers:\n",
            " ╭──────────────────────────────────────────────╮\n",
            " │ step                                  │ secs │\n",
            " ╰──────────────────────────────────────────────╯\n",
        );
        let normalized = normalize(screen);
        assert!(normalized.contains("│ step"), "{normalized}");
        assert!(normalized.contains("│ secs │"), "{normalized}");
    }
}
