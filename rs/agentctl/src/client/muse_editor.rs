//! Muse's ruled editor: its regions, its status and the proofs that input reached it.
//!
//! Muse draws its input editor between the last two horizontal rules on screen, with one status
//! footer below them. Everything here is a pure function of the visible screen text.
//!
//! Lines break at `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, `\u{85}`, `\u{2028}` and
//! `\u{2029}`, and whitespace is Unicode `White_Space` plus `\x1c`-`\x1f`, so a frame reads the
//! same whichever agentctl edition reads it.
//!
//! The "verified process" variants drop the `Muse Code X.Y.Z` header requirement. Muse 1.4 scrolls
//! the header off screen, so they are only meaningful after the caller has pinned the live Muse
//! process generation, which is the independent proof that the pane is still Muse.

// A line-for-line port of py/agentctl/client.py, from `_muse_composer_regions` through
// `muse_verified_process_prompt_transcript_count`, with the status order of the Muse branch of
// `pane_info` in py/agentctl/subagents.py. The helpers below reproduce Python's
// `str.splitlines`, `str.split()` and `str.strip()` exactly rather than approximating them with
// `str::lines` and `char::is_whitespace`: a proof one edition accepts and the other refuses would
// make delivery depend on which edition is installed.

use std::collections::BTreeSet;

/// Placeholder hints Muse draws dim inside an empty composer a few seconds after a turn
/// settles (`crates/tui/src/state/prompt_hint.rs` in Muse 1.4.23, read from the installed
/// `muse.real` strings). One table, versioned with Muse: a new Muse release that changes
/// its hints updates this list in the same change. A capture returns a hint as plain text,
/// so a text read cannot tell it from typed text by styling; an ANSI read can (see
/// [`muse_ansi_composer_is_empty`]). Only an exact whole-editor match counts: anything
/// longer, or any other text, is a draft and fails closed (#1773
/// agentctl-muse-placeholder-hint-as-draft).
pub(crate) const MUSE_PROMPT_HINTS: [&str; 23] = [
    "Type @ to search and insert workspace file paths",
    "Start a message with ! to run a shell command yourself",
    "Paste an image with Ctrl+V — file paths and URLs work too",
    "Press Alt+V to dictate instead of typing",
    "Tab queues or steers while a run is active",
    "Press ? on an empty composer to see keyboard shortcuts",
    "Press Ctrl+O to expand collapsed tool output",
    "/side asks a quick question without touching this thread",
    "/fork branches the conversation from the latest point",
    "/goal pins a session objective with a progress bar",
    "/resume reopens a past session (--last for the latest)",
    "Ask to 'use a workflow' to fan out parallel agents on big tasks",
    "Say 'use a workflow' to fan out agents on big tasks",
    "Ask to monitor a running build or log and get pinged on changes",
    "/agent opens the subagent command center",
    "/tasks shows workflows, subagents and terminals in one place",
    "/loop 10m <prompt> schedules a recurring prompt",
    "Ctrl+B sends the selected task to the background queue",
    "/skills browses ready-made skills for repeatable procedures",
    "/plugins marketplace installs new commands and skills",
    "/effort adjusts reasoning depth vs speed",
    "/usage shows this session's token usage",
    "/export saves the conversation to a text file",
];

/// Whether `text` (draft text without its prompt marker, whitespace already collapsed by
/// the caller or not) is exactly one placeholder hint. Used by the generic draft readers
/// that see only draft text, not a whole Muse frame.
pub(crate) fn is_muse_placeholder_hint(text: &str) -> bool {
    let joined = normalize(text);
    !joined.is_empty()
        && MUSE_PROMPT_HINTS
            .iter()
            .any(|hint| normalize(hint) == joined)
}

/// Shortest truncated hint prefix that still proves a ghost: shorter prefixes
/// could be the start of a typed draft, so they fail closed.
const HINT_PREFIX_MIN: usize = 20;

/// Whether draft `text` is a truncated hint: an ellipsis-ended prefix, or (when
/// `at_edge` says the text fills its row to the pane edge) a plain prefix, of at
/// least [`HINT_PREFIX_MIN`] chars of exactly the hint it prefixes. A prefix of
/// no hint, or a short one, is a draft.
fn is_hint_prefix(text: &str, at_edge: bool) -> bool {
    let joined = normalize(text);
    let (core, ellipsis) = match joined
        .strip_suffix('…')
        .or_else(|| joined.strip_suffix("..."))
    {
        Some(core) => (normalize(core), true),
        None => (joined.clone(), false),
    };
    if core.chars().count() < HINT_PREFIX_MIN || !(ellipsis || at_edge) {
        return false;
    }
    MUSE_PROMPT_HINTS.iter().any(|hint| {
        let hint = normalize(hint);
        hint.len() > core.len() && hint.starts_with(&core)
    })
}

/// Whether generic draft `text` is exactly one hint or an ellipsis-truncated
/// one (no pane width is known on that path, so a bare prefix never qualifies).
pub(crate) fn is_muse_hint_text(text: &str) -> bool {
    is_muse_placeholder_hint(text) || is_hint_prefix(text, false)
}

/// The pane width a Muse frame shows: the char count of its last rule, or
/// `None` when the screen holds no ruled frame.
pub(crate) fn muse_divider_width(screen: &str) -> Option<usize> {
    splitlines(screen)
        .into_iter()
        .rfind(|line| is_divider(line))
        .map(|line| strip(line).chars().count())
}

/// Whether the editor region `editor` holds exactly one placeholder hint and
/// nothing else: the whole hint (wrapped rows join with a space or with
/// nothing, as [`muse_editor_matches`] allows), or the hint truncated at the
/// pane edge (`width` is the frame's rule width; a hint row filling it proves
/// the cut), or cut with an ellipsis. Anything else is a draft.
fn muse_editor_is_hint_w(editor: &str, width: Option<usize>) -> bool {
    let Some(segments) = muse_editor_segments(editor) else {
        return false;
    };
    if segments.is_empty() {
        return false;
    }
    let spaced = normalize(&segments.join(" "));
    let tight = normalize(&segments.join(""));
    if MUSE_PROMPT_HINTS
        .iter()
        .any(|hint| normalize(hint) == spaced || normalize(hint) == tight)
    {
        return true;
    }
    // Truncation shows one partial text (joined either way can be the prefix
    // when the cut falls mid-word across no row break, so try both joins).
    let at_edge = width.is_some_and(|width| {
        splitlines(editor)
            .into_iter()
            .any(|line| line.trim_end().chars().count() + 1 >= width)
    });
    is_hint_prefix(&spaced, at_edge) || is_hint_prefix(&tight, at_edge)
}

/// Whether the composer region `composer` holds nothing a person typed: no text at
/// all, or only a placeholder hint. Styling is handled by
/// [`muse_ansi_composer_is_empty`] where an ANSI screen is available.
fn muse_composer_is_empty_w(composer: &str, width: Option<usize>) -> bool {
    if PROMPT_MARKERS.contains(&strip(composer)) {
        return true;
    }
    muse_editor_segments(composer).is_some_and(|segments| segments.is_empty())
        || muse_editor_is_hint_w(composer, width)
}

/// Foreground colour of one rendered cell, for the ANSI styling proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellFg {
    Default,
    Rgb(u32, u32, u32),
    Indexed(u32),
}

/// Muse's hint grey on a live 1.4.23 seat: truecolor `38;2;103;108;116`, the same
/// grey as the status line (the prompt glyph is `38;2;90;160;255`). Not SGR faint.
const HINT_GREY: CellFg = CellFg::Rgb(103, 108, 116);

/// Parse `screen` into rows of `(char, foreground, faint)`, tracking SGR state
/// across rows as a terminal does. Mirrors [`crate::submission::render_screen`]'s
/// row splitting (CR removed, rows split on LF, other controls dropped).
fn ansi_cells(screen: &str) -> Vec<Vec<(char, CellFg, bool)>> {
    let mut rows: Vec<Vec<(char, CellFg, bool)>> = Vec::new();
    let mut row: Vec<(char, CellFg, bool)> = Vec::new();
    let mut fg = CellFg::Default;
    let mut faint = false;
    let chars: Vec<char> = screen.replace('\r', "").chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '\n' {
            rows.push(std::mem::take(&mut row));
            index += 1;
            continue;
        }
        if character == '\u{1b}' {
            // CSI SGR is the only sequence that changes fg/faint here; other
            // escapes are skipped with the same bounds as `render_screen`.
            if chars.get(index + 1) == Some(&'[') {
                let mut end = index + 2;
                while end < chars.len()
                    && matches!(chars[end], '0'..='9' | ';' | ':' | '?' | '<' | '=' | '>')
                {
                    end += 1;
                }
                let params: String = chars[index + 2..end].iter().collect();
                let mut cursor = end;
                while cursor < chars.len() && (' '..='/').contains(&chars[cursor]) {
                    cursor += 1;
                }
                if chars.get(cursor) == Some(&'m')
                    && params
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == ';' || c == ':')
                {
                    let codes: Vec<&str> = if params.is_empty() {
                        vec!["0"]
                    } else {
                        params
                            .split(';')
                            .map(|p| p.split(':').next().unwrap_or(""))
                            .collect()
                    };
                    let mut at = 0;
                    while at < codes.len() {
                        match codes[at] {
                            "" | "0" => {
                                fg = CellFg::Default;
                                faint = false;
                            }
                            "2" => faint = true,
                            "22" => faint = false,
                            "39" => fg = CellFg::Default,
                            "38" if at + 1 < codes.len() => match codes[at + 1] {
                                "2" if at + 4 < codes.len() => {
                                    let num = |s: &str| s.parse::<u32>().unwrap_or(0);
                                    fg = CellFg::Rgb(
                                        num(codes[at + 2]),
                                        num(codes[at + 3]),
                                        num(codes[at + 4]),
                                    );
                                    at += 4;
                                }
                                "5" if at + 2 < codes.len() => {
                                    fg = CellFg::Indexed(codes[at + 2].parse().unwrap_or(0));
                                    at += 2;
                                }
                                _ => {}
                            },
                            _ => {}
                        }
                        at += 1;
                    }
                    index = cursor + 1;
                    continue;
                }
            }
            // Non-SGR escape: skip it (two chars for a simple escape, else one).
            index += if matches!(chars.get(index + 1), Some('@'..='Z' | '\\')) {
                2
            } else {
                1
            };
            continue;
        }
        index += 1;
        if character < ' ' && character != '\t' {
            continue;
        }
        row.push((character, fg, faint));
    }
    rows.push(row);
    rows
}

/// Whether an ANSI `screen`'s Muse composer is empty by styling. Live Muse draws
/// its ghost hint in truecolor grey ([`HINT_GREY`], the status line's grey) after a
/// blue prompt glyph, not in SGR faint; faint or reverse-video ghosts count too.
/// So: no composer text at all, faint-only text, or text whose every cell carries
/// the hint grey (or the screen's own status-line foreground) is empty. Typed text
/// is default-intensity and never qualifies, so a real draft still counts.
/// `None` when the screen carries no styling or shows no Muse frame, so the caller
/// falls back to the hint table (#1773, preference 1: herdr reads the pane with
/// `--format ansi`, as `read_screen_with_cancellation` already does).
pub(crate) fn muse_ansi_composer_is_empty(screen: &str) -> Option<bool> {
    if !screen.contains('\u{1b}') {
        return None;
    }
    let rendered = crate::submission::render_screen(screen);
    let plain: String = rendered
        .iter()
        .map(|(row, _)| crate::submission::text_of(row))
        .collect::<Vec<_>>()
        .join("\n");
    muse_composer_regions(&plain, false)?;
    let plain_lines = splitlines(&plain);
    let dividers: Vec<usize> = plain_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_divider(line).then_some(index))
        .collect();
    if dividers.len() < 2 {
        return None;
    }
    let (top, bottom) = (dividers[dividers.len() - 2], dividers[dividers.len() - 1]);
    // Faint/reverse-only text is blank in the solid view.
    let solid: String = rendered[top + 1..bottom]
        .iter()
        .map(|(_, row)| crate::submission::text_of(row))
        .collect::<Vec<_>>()
        .join("\n");
    let solid = solid.trim_start_matches(['❯', '›', ' ', '\t']);
    if strip(solid).is_empty() {
        return Some(true);
    }
    let cells = ansi_cells(screen);
    if cells.len() != rendered.len() {
        return None;
    }
    // The status line's foreground: the most common non-default colour below the
    // bottom rule. The hint shares it, so a future grey still matches its screen.
    let mut footer_counts: Vec<(CellFg, usize)> = Vec::new();
    for row in &cells[bottom + 1..] {
        for (character, fg, _) in row {
            if character.is_whitespace() || *fg == CellFg::Default {
                continue;
            }
            if let Some(entry) = footer_counts.iter_mut().find(|(known, _)| known == fg) {
                entry.1 += 1;
            } else {
                footer_counts.push((*fg, 1));
            }
        }
    }
    let footer_fg = footer_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(fg, _)| *fg);
    // Composer text cells: everything except whitespace and the leading glyph.
    let mut text_cells: Vec<(CellFg, bool)> = Vec::new();
    for (offset, row) in cells[top + 1..bottom].iter().enumerate() {
        for (position, (character, fg, faint)) in row.iter().enumerate() {
            if character.is_whitespace() {
                continue;
            }
            if offset == 0 && position <= 1 && matches!(character, '❯' | '›') {
                continue;
            }
            text_cells.push((*fg, *faint));
        }
    }
    if text_cells.is_empty() {
        return Some(true);
    }
    Some(
        text_cells.iter().all(|(_, faint)| *faint)
            || text_cells.iter().all(|(fg, _)| *fg == HINT_GREY)
            || footer_fg.is_some_and(|grey| text_cells.iter().all(|(fg, _)| *fg == grey)),
    )
}

/// Markers Muse draws in front of the editor's first line and in front of each user turn.
const PROMPT_MARKERS: [&str; 2] = ["❯", "›"];
/// Markers that end one rendered user turn: the answer marker, the next user turn, or the progress
/// markers Muse 1.4 draws below a submitted turn while it works (`◇ Thinking (0s · esc to
/// interrupt)`, `◈ Double checking`).
const TURN_END_MARKERS: [&str; 5] = ["◆", "❯", "›", "◇", "◈"];
/// The only footer line that reports a paused goal.
const GOAL_PAUSED: &str = "Goal (paused)";

// Python's `str.isspace`: Unicode `White_Space`, plus the four ASCII information separators
// `\x1c`-`\x1f` that Python also counts.
fn is_space(value: char) -> bool {
    value.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&value)
}

// Python's `text.strip()`.
fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

// Python's `" ".join(text.split())`: words separated by single spaces.
fn normalize(text: &str) -> String {
    text.split(is_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// Python's `text.splitlines()`: no trailing empty line, and `\r\n` is one boundary.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, value)) = characters.next() {
        if !matches!(
            value,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + value.len_utf8();
        if value == '\r' && characters.next_if(|(_, next)| *next == '\n').is_some() {
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// One of Muse's horizontal rules: at least three rule characters and nothing else.
fn is_divider(line: &str) -> bool {
    let stripped = strip(line);
    stripped.chars().count() >= 3
        && stripped
            .chars()
            .all(|value| matches!(value, '─' | '━' | '═'))
}

/// Whether `line` is exactly `Muse Code X.Y.Z`, with ASCII digits in each version part.
fn is_versioned_header(line: &str) -> bool {
    line.strip_prefix("Muse Code ").is_some_and(|version| {
        let parts = version.split('.').collect::<Vec<_>>();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|value| value.is_ascii_digit()))
    })
}

/// Whether `line` is exactly one of `markers`, or one of them followed by a space.
fn starts_with_marker(line: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| {
        line == *marker
            || line
                .strip_prefix(marker)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// Split one footer-region line into the footer itself and any notice Muse right-aligns on it.
///
/// Muse right-aligns transient notices on the footer line, after a gap of three or more
/// whitespace characters: `Paste again to expand for editing` after a paste, `Hook ·
/// UserPromptSubmit · ...` after a submission, and `Goal (4h 28m) 86% ████████▋░` or `Goal
/// (blocked · 3m) 95% █████████▌` while a goal is active. Whatever the notice says, the footer is
/// the text before the gap; a line with no such gap is all footer.
fn split_notice(line: &str) -> (&str, Option<&str>) {
    let (mut start, mut run) = (0, 0);
    for (index, value) in line.char_indices() {
        if is_space(value) {
            if run == 0 {
                start = index;
            }
            run += 1;
            continue;
        }
        if run >= 3 {
            return (&line[..start], Some(&line[index..]));
        }
        run = 0;
    }
    (line, None)
}

/// Glyphs that open a row of Muse's terminals panel.
const PANEL_ROW_GLYPHS: [char; 3] = ['├', '└', '│'];
/// The hint on the header row of Muse's terminals panel.
const PANEL_HEADER_HINT: &str = "↓ to select";

/// Return the mode of one structurally valid Muse footer line, or `None` for any other line.
///
/// The footer is recognized by its structure alone: at least three non-empty `·`-separated
/// fields (`model · effort · dir`, then the approval mode when Muse shows one), optionally
/// followed by a right-aligned notice (see [`split_notice`]). The model name, the effort and the
/// approval mode are never checked against a list: the owner launches in YOLO as a matter of
/// course, and an unknown model, effort or mode must not make an idle editor read as busy. The
/// mode returned is the fourth field, or `standard` when Muse shows only three.
///
/// Rows of Muse's terminals panel, which sits between the bottom rule and the footer while a
/// background shell or a sub-agent exists, are never footers: its `main · ↓ to select` header and
/// its tree rows (`└ ◇ counter ·3  shared · Run the shell command ...`), whose `·` fields would
/// otherwise make a second footer and hide the editor for as long as a sub-agent runs (#678
/// muse-bridge-latency).
/// Startup lines Muse prints on a resumed session that can land in the
/// footer region while the frame draws. They are not status footers, so they
/// must not count as a second footer authority (#946).
fn is_benign_startup_line(line: &str) -> bool {
    (line.starts_with("Skills:") && line.contains(" loaded ·"))
        || line.starts_with("warning: the previous run of this session ended abnormally")
        || (line.starts_with("reasoning effort ")
            && line.contains(" is not available")
            && line.contains("; using "))
}

fn muse_status_mode(line: &str) -> Option<String> {
    let line = strip(line);
    if line.starts_with(PANEL_ROW_GLYPHS) || line.contains(PANEL_HEADER_HINT) {
        return None;
    }
    if is_benign_startup_line(line) {
        return None;
    }
    let (footer, _) = split_notice(line);
    let fields = footer.split('·').map(strip).collect::<Vec<_>>();
    if fields.len() < 3 || fields.iter().any(|field| field.is_empty()) {
        return None;
    }
    Some(fields.get(3).copied().unwrap_or("standard").to_owned())
}

/// Return all structurally valid footer modes; callers require one authority.
fn muse_footer_modes(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| !strip(line).is_empty())
        .filter_map(|line| muse_status_mode(line))
        .collect()
}

/// Split transcript/composer using Muse's ruled editor and status footer.
fn muse_composer_regions(screen: &str, require_header: bool) -> Option<(String, String)> {
    let lines = splitlines(screen);
    let dividers = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_divider(line).then_some(index))
        .collect::<Vec<_>>();
    if dividers.len() < 2 {
        return None;
    }
    let (top, bottom) = (dividers[dividers.len() - 2], dividers[dividers.len() - 1]);
    let footer = &lines[bottom + 1..];
    let versioned_header = lines[..top]
        .iter()
        .any(|line| is_versioned_header(strip(line)));
    if !((versioned_header || !require_header) && muse_footer_modes(footer).len() == 1) {
        return None;
    }
    Some((lines[..top].join("\n"), lines[top + 1..bottom].join("\n")))
}

/// Markers of the progress row Muse 1.4 draws above its editor while a turn runs. The row
/// animates through all three (`◇ Thinking`, `◈ Double checking`, `◆ Calling tools`), so a
/// reader that knows only some of them misreads a running turn as idle on the other frames.
const PROGRESS_MARKERS: [&str; 3] = ["◆", "◇", "◈"];

/// Transcript rows above the editor searched for Muse's running-turn row.
const PROGRESS_ROWS: usize = 16;

/// Whether the rows above Muse's editor show its running-turn row near the bottom, so a turn is
/// still running.
///
/// Muse 1.4 keeps its editor empty and drawn while it works, with a progress row such as
/// `◇ Thinking (3s · esc to interrupt)` above it, so that row, not the editor, says whether a
/// turn is running. It is not always the last row: Muse draws a foreground command's output and
/// hint below it (`└ (ctrl+b to send to background)`), and a `◆ Queued input` block. The
/// post-turn `◈ Double checking (1m 35s · esc to interrupt)` phase is a running turn too, during
/// which Herdr already reports the pane done (#678 muse-bridge-latency).
///
/// The row starts at the left edge and ends with `esc to interrupt)`, so tool output, a quoted
/// capture (indented under `└`) or an answer that mentions the phrase does not count.
fn muse_turn_running(transcript: &str) -> bool {
    splitlines(transcript)
        .into_iter()
        .map(|line| line.trim_end_matches(is_space))
        .filter(|line| !line.is_empty())
        .rev()
        .take(PROGRESS_ROWS)
        .any(|line| {
            starts_with_marker(line, &PROGRESS_MARKERS) && line.ends_with("esc to interrupt)")
        })
}

/// An empty editor below a transcript that does not end in Muse's progress line.
/// A composer holding only a placeholder hint is empty (#1773).
fn muse_editor_idle_w(transcript: &str, composer: &str, width: Option<usize>) -> bool {
    muse_composer_is_empty_w(composer, width) && !muse_turn_running(transcript)
}

/// Recognize Muse's progress line above the editor after live-Muse process verification.
pub(crate) fn muse_verified_process_turn_running(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(transcript, _)| muse_turn_running(&transcript))
}

/// Recognize Muse's idle composer without mistaking a choice prompt for it.
pub(crate) fn muse_idle_composer(screen: &str) -> bool {
    let width = muse_divider_width(screen);
    muse_composer_regions(screen, true)
        .is_some_and(|(transcript, composer)| muse_editor_idle_w(&transcript, &composer, width))
}

/// Require the Auto-review footer on the active empty composer.
// Delivery no longer requires this footer on an adopted pane (owner-directed, so an adopted
// coordinator in YOLO still takes messages); the tests keep pinning what the footer proof means,
// as py/agentctl/client.py does.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only tests call it since delivery accepts any footer mode"
    )
)]
pub(crate) fn muse_auto_review_idle_composer(screen: &str) -> bool {
    let Some((transcript, composer)) = muse_composer_regions(screen, true) else {
        return false;
    };
    if !muse_editor_idle_w(&transcript, &composer, muse_divider_width(screen)) {
        return false;
    }
    let lines = splitlines(screen);
    let Some(bottom) = lines.iter().rposition(|line| is_divider(line)) else {
        return false;
    };
    muse_footer_modes(&lines[bottom + 1..]) == ["Auto-review"]
}

/// Recognize an idle editor after exact live-Muse process verification.
pub(crate) fn muse_verified_process_idle_composer(screen: &str) -> bool {
    let width = muse_divider_width(screen);
    muse_composer_regions(screen, false)
        .is_some_and(|(transcript, composer)| muse_editor_idle_w(&transcript, &composer, width))
}

/// Recognize a possibly nonempty editor after live-Muse process verification.
pub(crate) fn muse_verified_process_composer(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(_, composer)| muse_editor_segments(&composer).is_some())
}

/// Whether one footer-region line or notice says the goal is paused: exactly `Goal (paused)`,
/// or the same words with an elapsed duration, as Muse draws a long pause (`Goal (paused ·
/// 1d 17h 40m) 97% █…`). An active goal's notice (`Goal (45m) 47% …`) never matches.
fn goal_paused_text(text: &str) -> bool {
    text == GOAL_PAUSED
        || text
            .strip_prefix("Goal (paused")
            .is_some_and(|rest| rest.starts_with([')', ' ', '·']))
}

/// Recognize Muse's explicit paused-goal state in a verified UI frame.
///
/// Only the footer region below the last rule counts, so a transcript line that merely quotes
/// `Goal (paused)` never counts. Muse draws it on a line of its own; the same notice
/// right-aligned on the footer line counts too. Detection only: since #734
/// chat-status-disagreement a paused goal does not hold delivery (see
/// [`muse_verified_pane_status`]), so nothing outside the tests calls this.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only tests call it since a paused goal reads idle"
    )
)]
pub(crate) fn muse_verified_process_goal_paused(screen: &str) -> bool {
    if muse_composer_regions(screen, false).is_none() {
        return false;
    }
    let lines = splitlines(screen);
    let Some(bottom) = lines.iter().rposition(|line| is_divider(line)) else {
        return false;
    };
    lines[bottom + 1..].iter().any(|line| {
        let line = strip(line);
        goal_paused_text(line)
            || (muse_status_mode(line).is_some()
                && split_notice(line).1.is_some_and(goal_paused_text))
    })
}

/// Whether `screen` holds a Muse composer frame at all (its ruled editor and one status
/// footer), as [`muse_verified_pane_status`] reads one. A viewport scrolled away from the
/// pane's bottom holds none, however idle the pane itself is (#734 chat-status-disagreement).
pub(crate) fn muse_screen_has_composer(screen: &str) -> bool {
    muse_composer_regions(screen, false).is_some()
}

/// The status a pinned Muse pane reports: idle, staged or working, in that order.
///
/// A frame whose transcript ends in Muse's progress line is working, never idle or staged.
/// `reported_status` is Herdr's own report for the pane. A paused Muse **goal**
/// (`Goal (paused …)` on the footer) is a Muse planning state, not a hold: Muse accepts
/// and acts on typed input while its goal is paused, and the bridge delivered to such a
/// pane all day in the incident, so a paused goal with an empty editor is `idle` and takes
/// input like any idle pane (#734 chat-status-disagreement). Only an explicit
/// `agentctl pause` (a record state, not this screen status), a staged draft, or a running
/// turn holds input. A headerless (verified-process) frame counts only while Herdr itself
/// reports the pane idle or done; a frame with the versioned header is idle on its own
/// evidence.
pub(crate) fn muse_verified_pane_status(reported_status: &str, screen: &str) -> &'static str {
    // The plugin-hooks trust dialog is a pane waiting on a person, named as
    // itself whatever the terminal host reports (#1893
    // agentctl-launch-failed-but-live). Nothing ever answers it.
    if crate::client::muse_hooks_trust_prompt(screen) {
        return "waiting: hooks trust";
    }
    let reported_idle = matches!(reported_status, "idle" | "done");
    if muse_idle_composer(screen) || (reported_idle && muse_verified_process_idle_composer(screen))
    {
        "idle"
    } else if reported_idle
        && muse_verified_process_composer(screen)
        && !muse_verified_process_turn_running(screen)
    {
        "staged"
    } else {
        "working"
    }
}

/// Return normalized editor lines after one leading Muse prompt marker.
fn muse_editor_segments(editor: &str) -> Option<Vec<String>> {
    let lines = splitlines(editor)
        .into_iter()
        .map(normalize)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let first = lines.first()?;
    let marker = PROMPT_MARKERS.iter().find(|candidate| {
        first == *candidate
            || first
                .strip_prefix(*candidate)
                .is_some_and(|rest| rest.starts_with(' '))
    })?;
    let initial = strip(&first[marker.len()..]);
    let mut segments = Vec::with_capacity(lines.len());
    if !initial.is_empty() {
        segments.push(initial.to_owned());
    }
    segments.extend(lines[1..].iter().cloned());
    Some(segments)
}

/// Match a logical prompt against one conservatively rendered Muse editor.
///
/// Each rendered line joins the previous one with a space or with nothing: Muse 1.4.14 wraps a
/// long line at any character, inside a word or path (`abcde` / `fghij`, `odd/` / `places`), as
/// well as after a hyphen, and the screen does not say which it did. `exact` requires the whole
/// editor to be the prompt; otherwise the editor need only start with it as whole words.
fn muse_editor_matches(editor: &str, text: &str, exact: bool) -> bool {
    let wanted = normalize(text);
    let segments = muse_editor_segments(editor);
    let Some(segments) = segments.filter(|_| !wanted.is_empty()) else {
        return false;
    };
    let wanted_word = format!("{wanted} ");
    let mut prefixes = BTreeSet::from([String::new()]);
    for (index, segment) in segments.iter().enumerate() {
        let separators: &[&str] = if index == 0 { &[""] } else { &[" ", ""] };
        let mut next_prefixes = BTreeSet::new();
        for prefix in &prefixes {
            for separator in separators {
                let rendered = format!("{prefix}{separator}{segment}");
                if !exact && (rendered == wanted || rendered.starts_with(&wanted_word)) {
                    return true;
                }
                if wanted.starts_with(&rendered) {
                    next_prefixes.insert(rendered);
                }
            }
        }
        if next_prefixes.is_empty() {
            return false;
        }
        prefixes = next_prefixes;
    }
    prefixes.contains(&wanted)
}

fn muse_prompt_in_composer_region(
    screen: &str,
    text: &str,
    require_header: bool,
    exact: bool,
) -> bool {
    muse_composer_regions(screen, require_header)
        .is_some_and(|(_, composer)| muse_editor_matches(&composer, text, exact))
}

/// Conservatively detect a prompt retained in Muse's bottom editor.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_in_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, true, false)
}

/// Detect retained input after exact live-Muse process verification.
// Like the other verified-process prompt proofs and `muse_prompt_in_transcript`, nothing outside
// the tests calls it, in this edition or in py/agentctl/client.py; it completes the ported Muse
// API whose meaning those tests pin.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_verified_process_prompt_in_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, false, false)
}

/// The character count Muse shows for a paste of `text`: code points, CRLF as one, each tab as
/// the four spaces Muse expands it to, and no other C0 control or DEL, which Muse drops.
///
/// Measured on Muse 1.4.15 (#1085 muse-send-staged-unsubmitted): a tab adds 4 wherever it sits
/// in its line, and form feed, vertical tab, bell and DEL add nothing. A count that ignored the
/// tabs never matched a brief that held one, so every such paste timed out unsubmitted.
pub(crate) fn muse_paste_length(text: &str) -> usize {
    let counted = text
        .chars()
        .map(|value| match value {
            '\t' => 4,
            '\n' | '\r' => 1,
            '\u{0}'..='\u{1f}' | '\u{7f}' => 0,
            _ => 1,
        })
        .sum::<usize>();
    counted - text.matches("\r\n").count()
}

/// Whether the editor is exactly Muse's collapsed placeholder for a paste of `text`.
///
/// Muse 1.4 collapses a long paste into one editor line, `[Pasted Content N chars]`.
fn muse_editor_holds_paste(editor: &str, text: &str) -> bool {
    let Some(segments) = muse_editor_segments(editor) else {
        return false;
    };
    let [segment] = segments.as_slice() else {
        return false;
    };
    segment
        .strip_prefix("[Pasted Content ")
        .and_then(|rest| rest.strip_suffix(" chars]"))
        .filter(|count| !count.is_empty() && count.bytes().all(|value| value.is_ascii_digit()))
        .and_then(|count| count.parse::<usize>().ok())
        == Some(muse_paste_length(text))
}

/// Whether `command` (already normalized) is one bare slash command: `/`, an ASCII letter,
/// then ASCII letters, digits, `_`, `:` or `-`, with no arguments. The same name shape as
/// `sender_header::is_slash_command`, narrowed to the argument-free form.
fn is_bare_slash_command(command: &str) -> bool {
    let Some(name) = command.strip_prefix('/') else {
        return false;
    };
    !command.contains(' ')
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-'))
}

/// Whether the editor holds a bare slash command under its open command menu.
///
/// Measured live on Muse 1.4.16 (#1136 raw-message-bridge): a pasted bare slash command
/// opens the command menu under the editor line (`❯ /help` over `/help  Show help`), so
/// the editor is the command plus its slash-led menu rows, never the command alone, and
/// Enter runs the menu's first row. Staged only when the first menu row is the typed
/// command itself: a mere prefix, or a menu led by another command, would run something
/// else, so it refuses. A command with arguments stages as literal text or as its
/// placeholder, as before.
fn muse_editor_holds_slash_command(editor: &str, text: &str) -> bool {
    let command = normalize(text);
    if !is_bare_slash_command(&command) {
        return false;
    }
    let Some(segments) = muse_editor_segments(editor) else {
        return false;
    };
    if segments.len() < 2 || segments[0] != command {
        return false;
    }
    let rows = &segments[1..];
    rows.iter().all(|row| row.starts_with('/'))
        && rows[0].split(' ').next() == Some(command.as_str())
}

/// Require the whole editor to be this paste after pinning the live Muse process.
///
/// The editor shows the literal text, or, for a long paste, only Muse's placeholder with the
/// paste's exact character count, or a bare slash command under its open menu. Delivery
/// proves the editor empty before its one paste.
pub(crate) fn muse_verified_process_paste_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_composer_regions(screen, false).is_some_and(|(_, editor)| {
        muse_editor_matches(&editor, text, true)
            || muse_editor_holds_paste(&editor, text)
            || muse_editor_holds_slash_command(&editor, text)
    })
}

/// How much of a paste of `text` a pinned Muse pane's editor shows (#1085
/// muse-send-staged-unsubmitted), for a delivery deciding what to do once its paste has not
/// shown as the whole editor in time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MusePasteDrawn {
    /// The editor is empty: Muse has drawn none of the paste.
    Nothing,
    /// The editor holds the start of the paste, but not all of it: Muse is still taking it in.
    Part,
    /// The editor holds the whole paste, as text or as its placeholder.
    Whole,
    /// The editor holds something that is not this paste or a start of it.
    Other,
    /// The screen shows no recognisable Muse editor.
    Unreadable,
}

/// Classify what a pinned Muse pane's editor shows of a paste of `text`.
///
/// A Muse that is slow to read its terminal draws a paste late, and can draw it a piece at a
/// time; the pieces are literal rows, each joining the last as [`muse_editor_matches`] allows, so
/// a start of the prompt that ends at a row boundary or inside a row counts as [`Part`]. A
/// placeholder only ever stands for a whole paste, so one with another count is [`Other`].
///
/// [`Part`]: MusePasteDrawn::Part
/// [`Other`]: MusePasteDrawn::Other
pub(crate) fn muse_verified_process_paste_drawn(screen: &str, text: &str) -> MusePasteDrawn {
    let Some((_, editor)) = muse_composer_regions(screen, false) else {
        return MusePasteDrawn::Unreadable;
    };
    let Some(segments) = muse_editor_segments(&editor) else {
        return MusePasteDrawn::Unreadable;
    };
    if segments.is_empty() {
        return MusePasteDrawn::Nothing;
    }
    if muse_editor_matches(&editor, text, true)
        || muse_editor_holds_paste(&editor, text)
        || muse_editor_holds_slash_command(&editor, text)
    {
        return MusePasteDrawn::Whole;
    }
    // A ghost hint is no draw of this paste (#1773): checked after `Whole` so a
    // staged paste whose text is itself a hint still counts as drawn.
    if muse_editor_is_hint_w(&editor, muse_divider_width(screen)) {
        return MusePasteDrawn::Nothing;
    }
    let wanted = normalize(text);
    let mut prefixes = BTreeSet::from([String::new()]);
    for (index, segment) in segments.iter().enumerate() {
        let separators: &[&str] = if index == 0 { &[""] } else { &[" ", ""] };
        prefixes = prefixes
            .iter()
            .flat_map(|prefix| {
                separators
                    .iter()
                    .map(move |separator| format!("{prefix}{separator}{segment}"))
            })
            .filter(|rendered| wanted.starts_with(rendered.as_str()))
            .collect();
        if prefixes.is_empty() {
            return MusePasteDrawn::Other;
        }
    }
    MusePasteDrawn::Part
}

/// Detect this paste retained in the editor, as text or as its placeholder.
pub(crate) fn muse_verified_process_paste_in_composer(screen: &str, text: &str) -> bool {
    muse_composer_regions(screen, false).is_some_and(|(_, editor)| {
        muse_editor_matches(&editor, text, false) || muse_editor_holds_paste(&editor, text)
    })
}

/// Require the entire active Muse editor to be this literal prompt.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, true, true)
}

/// Require exact editor text after pinning the live Muse process.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_verified_process_prompt_is_exact_composer(screen: &str, text: &str) -> bool {
    muse_prompt_in_composer_region(screen, text, false, true)
}

/// Count complete marker-delimited user turns, never prompt prefixes.
///
/// A turn starts at a prompt marker and runs to the next answer or prompt marker, or to Muse's
/// `Working...`/`Thinking...` line. An assistant answer that echoes the prompt is never a turn.
fn muse_transcript_prompt_count(transcript: &str, text: &str) -> usize {
    if normalize(text).is_empty() {
        return 0;
    }
    let lines = splitlines(transcript);
    let mut count = 0;
    let mut index = 0;
    while index < lines.len() {
        if !starts_with_marker(strip(lines[index]), &PROMPT_MARKERS) {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < lines.len() {
            let next_line = strip(lines[end]);
            if starts_with_marker(next_line, &TURN_END_MARKERS)
                || matches!(
                    next_line,
                    "Working..." | "Working\u{2026}" | "Thinking..." | "Thinking\u{2026}"
                )
            {
                break;
            }
            end += 1;
        }
        if muse_editor_matches(&lines[index..end].join("\n"), text, true) {
            count += 1;
        }
        index = end;
    }
    count
}

/// Require the exact prompt as a user turn above the active Muse editor.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests call it, as in Python")
)]
pub(crate) fn muse_prompt_in_transcript(screen: &str, text: &str) -> bool {
    muse_prompt_transcript_count(screen, text) > 0
}

/// Count bounded prompt renderings above the composer for transition proofs.
pub(crate) fn muse_prompt_transcript_count(screen: &str, text: &str) -> usize {
    muse_composer_regions(screen, true).map_or(0, |(transcript, _)| {
        muse_transcript_prompt_count(&transcript, text)
    })
}

/// Count exact user turns after pinning the live Muse process.
pub(crate) fn muse_verified_process_prompt_transcript_count(screen: &str, text: &str) -> usize {
    muse_composer_regions(screen, false).map_or(0, |(transcript, _)| {
        muse_transcript_prompt_count(&transcript, text)
    })
}

// Rust-only helpers for follow-ups typed into a running Muse turn (#505 agentctl-muse-followup).
// The Python edition still waits for an idle editor, so nothing in py/agentctl mirrors them.

/// The rows of Muse's editor as drawn, each without trailing blanks, after live-Muse process
/// verification; `None` when the screen shows no ruled region whose first row starts with a
/// prompt marker, above exactly one status footer.
///
/// An approval form or a panel can take the ruled region or the footer's place, and then this
/// is `None`.
pub(crate) fn muse_verified_process_editor_rows(screen: &str) -> Option<String> {
    let (_, editor) = muse_composer_regions(screen, false)?;
    muse_editor_segments(&editor)?;
    Some(
        splitlines(&editor)
            .into_iter()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The draft Muse's editor holds, after live-Muse process verification: its rows as drawn, or
/// the length a long paste's placeholder shows. `None` when the screen shows no recognisable
/// editor, an empty one, or one holding only a placeholder hint (#720
/// agentctl-muse-stale-draft, #1773 agentctl-muse-placeholder-hint-as-draft).
pub(crate) fn muse_verified_process_editor_draft(
    screen: &str,
) -> Option<crate::stale_draft::Draft> {
    let (_, editor) = muse_composer_regions(screen, false)?;
    if muse_editor_is_hint_w(&editor, muse_divider_width(screen)) {
        return None;
    }
    let segments = muse_editor_segments(&editor)?;
    if let [segment] = segments.as_slice() {
        let placeholder = segment
            .strip_prefix("[Pasted Content ")
            .and_then(|rest| rest.strip_suffix(" chars]"))
            .filter(|count| !count.is_empty() && count.bytes().all(|value| value.is_ascii_digit()))
            .and_then(|count| count.parse::<usize>().ok());
        if let Some(length) = placeholder {
            return Some(crate::stale_draft::Draft::Placeholder(length));
        }
    }
    (!segments.is_empty()).then(|| crate::stale_draft::Draft::Text(segments.join("\n")))
}

/// Whether `rows` from [`muse_verified_process_editor_rows`] hold nothing but the prompt
/// marker, or only a placeholder hint (#1773).
pub(crate) fn muse_editor_rows_empty(rows: &str) -> bool {
    muse_editor_rows_empty_w(rows, None)
}

/// [`muse_editor_rows_empty`] with the frame's rule `width` for edge-truncated hints.
pub(crate) fn muse_editor_rows_empty_w(rows: &str, width: Option<usize>) -> bool {
    muse_editor_segments(rows).is_some_and(|segments| segments.is_empty())
        || muse_editor_is_hint_w(rows, width)
}

/// How many items of Muse's `◆ Queued input` block above its editor show `text`: its text,
/// words collapsed, or the placeholder Muse draws for a long paste of it.
///
/// Muse can hold an Enter-submitted prompt in this queue rather than steer the running turn at
/// once, drawing `◆ Queued input · ↑ to edit` and `└ [Pasted Content 1637 chars]`, and gives it
/// to the model at its next step; the user turn and the journal's accepted intent come only then
/// (#678 muse-bridge-latency). A caller compares the count before and after its Enter, since an
/// older queued paste of the same length draws the same placeholder.
pub(crate) fn muse_verified_process_prompt_queued_count(screen: &str, text: &str) -> usize {
    let Some((transcript, _)) = muse_composer_regions(screen, false) else {
        return 0;
    };
    let lines = splitlines(&transcript);
    let Some(start) = lines
        .iter()
        .rposition(|line| line.starts_with("◆ Queued input"))
    else {
        return 0;
    };
    let mut items: Vec<String> = Vec::new();
    for line in &lines[start + 1..] {
        let row = strip(line);
        if row.is_empty() {
            continue;
        }
        if let Some(rest) = row.strip_prefix('└').or_else(|| row.strip_prefix('├')) {
            items.push(strip(rest).to_owned());
        } else if line.starts_with(is_space) && !items.is_empty() {
            let item = items.last_mut().expect("an item was pushed");
            item.push(' ');
            item.push_str(row);
        } else {
            break;
        }
    }
    let wanted = normalize(text);
    let placeholder = format!("[Pasted Content {} chars]", muse_paste_length(text));
    items
        .iter()
        .filter(|item| normalize(item) == wanted || **item == placeholder)
        .count()
}

/// Whether the rows above Muse's editor show its running-turn row anywhere near the bottom.
///
/// Unlike [`muse_verified_process_turn_running`], which reads only the last transcript row, this
/// also finds the row when Muse 1.4.14 draws more below it while it works: command output
/// (`└ tick 2 (ctrl+b to send to background)`), a `◆ Queued input` block, or the row in its
/// `◆` animation phase (`◆ Calling tools (26s · esc to interrupt)`).
pub(crate) fn muse_verified_process_turn_in_progress(screen: &str) -> bool {
    muse_composer_regions(screen, false)
        .is_some_and(|(transcript, _)| muse_turn_running(&transcript))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from py/tests/test_agentctl_client_launch.py.

    #[test]
    fn current_muse_yolo_footer_is_an_idle_composer() {
        let screen = "Muse Code at Meta (https://fb.workplace.com/groups/27315719428107177)\n\
                      Using AI Gateway (Meta Model API upstream)\n\n  Muse Code 1.4.0\n\n\
                      ────────────────\n❯\n────────────────\n  \
                      kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n";
        assert!(muse_idle_composer(screen));
        assert!(!muse_auto_review_idle_composer(&screen.replace(
            "Using AI Gateway",
            "Auto-review was previously enabled"
        )));
        let ambiguous = screen.replace(
            "  kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
            "  kiki · xhigh · /work/project · Auto-review\n  \
             kiki_gb300_mxfp8_6p2_840_nwr · xhigh · /work/project · YOLO\n",
        );
        assert!(!muse_idle_composer(&ambiguous));
        assert!(!muse_auto_review_idle_composer(&ambiguous));
        let choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n");
        assert!(!muse_idle_composer(&choice));
    }

    #[test]
    fn resumed_headerless_frame_with_startup_warnings_is_idle() {
        // #946: a resumed Muse scrolls its header off screen and prints the
        // abnormal-end and effort-fallback warnings; the pinned-process proof
        // must still read the idle composer, including when a benign startup
        // line lands in the footer region.
        let screen = "◆ prior answer\n\
                      warning: the previous run of this session ended abnormally around \
                      2026-11-06 10:09 UTC-08:00 (no clean shutdown was recorded). It was resumed\n\
                      cleanly.\n\
                      reasoning effort ultra is not available (gate ultra_reasoning_effort is \
                      closed); using xhigh\n\
                      ────────────────\n❯\n────────────────\n  \
                      watermelon-full-preview · xhigh · /work/project · YOLO\n\
                      Skills: 82 loaded · 1 warning · 3 details hidden (ctrl+o to expand)\n";
        assert!(!muse_idle_composer(screen));
        assert!(muse_verified_process_idle_composer(screen));
        let busy = screen.replace(
            "◆ prior answer\n",
            "◆ prior answer\n◇ Thinking (3s · esc to interrupt)\n",
        );
        assert!(!muse_verified_process_idle_composer(&busy));
        let draft = screen.replace("❯\n", "❯ draft\n");
        assert!(!muse_verified_process_idle_composer(&draft));
        let doubled = screen.replace(
            "Skills: 82 loaded",
            "other · xhigh · /work/project · YOLO\nSkills: 82 loaded",
        );
        assert!(!muse_verified_process_idle_composer(&doubled));
    }

    #[test]
    fn a_muse_resumed_three_times_is_ready_without_its_scrolled_off_header() {
        // The third consecutive revive of one session: the conversation is reprinted and each
        // unclean end added a warning, so the header is no longer among the visible rows.
        let mut rows = Vec::new();
        for turn in 0..12 {
            rows.push(format!("❯ question {turn}"));
            rows.push(format!("◆ answer {turn}"));
        }
        for _ in 0..3 {
            rows.push(
                "  warning: the previous run of this session ended abnormally (no clean shutdown was recorded)"
                    .to_owned(),
            );
        }
        let screen = format!(
            "{}\n────────────────\n❯\n────────────────\n  model · xhigh · /work/project · YOLO\n",
            rows.join("\n")
        );
        assert!(
            !muse_idle_composer(&screen),
            "a screen alone never proves Muse without its header"
        );
        assert!(
            muse_verified_process_idle_composer(&screen),
            "the pinned process stands in for the header"
        );
        // A turn still running is never ready, verified or not.
        let working = screen.replace(
            "\n────────────────\n❯",
            "\n◇ Thinking (3s · esc to interrupt)\n────────────────\n❯",
        );
        assert!(!muse_verified_process_idle_composer(&working));
        // With the header on screen it is ready as before.
        let fresh = format!("  Muse Code 1.4.15\n{screen}");
        assert!(muse_idle_composer(&fresh));
    }

    #[test]
    fn muse_launch_overrides_footer_is_an_idle_composer() {
        // Captured 2026-10-19 from a live Muse launched with extra harness
        // arguments (`--approval-mode never` among them): the status footer
        // reports `Launch overrides` where a settings-default launch reports
        // its approval mode. This frame timed out `start` as launch_failed
        // (#382 muse-approval-never-ready).
        let screen = "Muse Code 1.4.0\n\
                      ◆ prior answer\n\n\
                      ────────────────\n❯\n────────────────\n  \
                      watermelon-full-preview · xhigh · /work/project · Launch overrides\n";
        assert!(muse_idle_composer(screen));
        assert!(muse_verified_process_idle_composer(screen));
        assert!(!muse_auto_review_idle_composer(screen));
        assert_eq!(muse_verified_pane_status("working", screen), "idle");
        let ambiguous = screen.replace(
            "Launch overrides\n",
            "Launch overrides\n  kiki · xhigh · /work/project · YOLO\n",
        );
        assert!(!muse_idle_composer(&ambiguous));
        let choice = screen.replace("❯\n", "❯ Yes, continue\n  No, exit\n");
        assert!(!muse_idle_composer(&choice));
    }

    /// The captured frames of `testdata/muse-frames.json` and the exact texts pasted into them.
    fn captured() -> (serde_json::Value, serde_json::Value) {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../../testdata/muse-frames.json")).unwrap();
        (corpus["frames"].clone(), corpus["briefs"].clone())
    }

    #[test]
    fn captured_launch_overrides_frames_carry_the_delivery_proofs() {
        // Muse 1.4.12 launched with --trust-workspace --approval-mode never (#395
        // muse-launch-overrides-input): an empty editor, then a 3214-character brief that Muse
        // collapses to a placeholder, then the submitted turn.
        let (frames, briefs) = captured();
        let frame = |name: &str| frames[name].as_str().unwrap().to_owned();
        let brief = briefs["long"].as_str().unwrap();
        let idle = frame("launch-overrides-idle");
        assert!(muse_idle_composer(&idle));
        assert!(muse_verified_process_idle_composer(&idle));
        assert!(!muse_auto_review_idle_composer(&idle));
        assert_eq!(muse_verified_pane_status("working", &idle), "idle");
        // The same editor under the Auto-review footer.
        let auto_review = idle.replace("· Launch overrides", "· Auto-review");
        assert!(muse_auto_review_idle_composer(&auto_review));
        assert!(muse_verified_process_idle_composer(&auto_review));

        let staged = frame("launch-overrides-placeholder");
        assert!(staged.contains("[Pasted Content 3214 chars]"));
        assert_eq!(muse_paste_length(brief), 3214);
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, brief
        ));
        assert!(muse_verified_process_paste_in_composer(&staged, brief));
        // The literal-text proofs cannot see a collapsed paste.
        assert!(!muse_prompt_is_exact_composer(&staged, brief));
        // The count must be exact.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &brief[1..]
        ));
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &format!("{brief}x")
        ));
        assert!(!muse_verified_process_idle_composer(&staged));
        assert_eq!(muse_verified_pane_status("idle", &staged), "staged");
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&staged, brief),
            0
        );

        // Right after Enter a hook notice shares the footer line, and the turn ends at Muse's
        // progress marker instead of an answer.
        let submitted = frame("launch-overrides-submitted");
        assert!(submitted.contains("Launch overrides   Hook · UserPromptSubmit"));
        assert!(submitted.contains("◈ Thinking (3s · esc to interrupt)"));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&submitted, brief),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&submitted, brief));
        // The empty editor below Muse's progress line is a running turn, never idle input.
        assert!(!muse_idle_composer(&submitted));
        assert!(!muse_verified_process_idle_composer(&submitted));
        assert!(muse_verified_process_turn_running(&submitted));
        assert_eq!(muse_verified_pane_status("idle", &submitted), "working");
        assert!(!muse_auto_review_idle_composer(
            &submitted.replace("· Launch overrides", "· Auto-review")
        ));
        let double_checking = submitted.replace(
            "◈ Thinking (3s · esc to interrupt)",
            "◈ Double checking (1s · esc to interrupt)",
        );
        assert!(!muse_verified_process_idle_composer(&double_checking));
        // A finished answer below the turn leaves the editor idle again.
        assert!(muse_idle_composer(
            &submitted.replace("◈ Thinking (3s · esc to interrupt)", "◆ RECEIVED 40")
        ));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(
                &submitted.replace("◈ Thinking", "Still thinking"),
                brief
            ),
            0
        );

        let answered = frame("launch-overrides-answered");
        assert_eq!(muse_prompt_transcript_count(&answered, brief), 1);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&answered, brief),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&answered, brief));
        assert!(muse_idle_composer(&answered));
    }

    #[test]
    fn captured_headerless_frames_count_pastes_in_code_points() {
        // After a few turns Muse 1.4 has scrolled its versioned header out of the frame, so only
        // the verified-process proofs, which rely on the pinned process instead, can apply.
        let (frames, briefs) = captured();
        let frame = |name: &str| frames[name].as_str().unwrap().to_owned();
        let staged = frame("headerless-placeholder");
        let brief = briefs["headerless"].as_str().unwrap();
        assert!(!staged.contains("Muse Code 1."));
        assert!(!muse_idle_composer(
            &staged.replace("❯ [Pasted Content 2939 chars]", "❯")
        ));
        assert!(muse_verified_process_idle_composer(
            &staged.replace("❯ [Pasted Content 2939 chars]", "❯")
        ));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, brief
        ));

        // Read while Muse's paste hint shared the footer line.
        let hinted = frame("headerless-placeholder-paste-hint");
        let hint_brief = briefs["paste-hint"].as_str().unwrap();
        assert!(hinted.contains("Launch overrides             Paste again to expand for editing"));
        assert!(muse_verified_process_paste_is_exact_composer(
            &hinted, hint_brief
        ));
        assert_eq!(muse_verified_pane_status("idle", &hinted), "staged");
        // The footer is structural: whatever follows a gap of three spaces is a notice, and
        // the text before it needs three non-empty fields, whatever they say.
        let footer = "watermelon-full-preview · xhigh · ~/work/project · Launch overrides";
        for line in [
            format!("{footer}   Paste again to expand for editing"),
            format!("{footer}   Goal (paused)"),
            format!("{footer}   Paste again to expand"),
            format!("{footer}   Hook"),
            format!("{footer}X   anything at all"),
        ] {
            assert!(muse_status_mode(&line).is_some(), "{line}");
        }
        assert_eq!(
            muse_status_mode("model · high · /work · YOLO\u{3000}\t Hook · UserPromptSubmit")
                .as_deref(),
            Some("YOLO")
        );
        for line in [
            "   Paste again to expand for editing",
            "Goal (blocked · 3m) 95% █████████▌",
            "◇ Thinking (16s · esc to interrupt)",
            "model · · /work · YOLO",
            "model · xhigh",
            "Goal (paused)",
            "",
        ] {
            assert_eq!(muse_status_mode(line), None, "{line}");
        }

        // 3175 code points, 3824 bytes and 3234 UTF-16 units with one CRLF: Muse shows 3174.
        let unicode = briefs["unicode-crlf"].as_str().unwrap();
        assert_eq!(unicode.chars().count(), 3175);
        assert_eq!(unicode.matches("\r\n").count(), 1);
        let staged = frame("headerless-unicode-placeholder");
        assert!(staged.contains("[Pasted Content 3174 chars]"));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, unicode
        ));
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged,
            &unicode.replace("\r\n", "\n")
        ));
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &unicode.replace("\r\n", "\n\n")
        ));
        let answered = frame("headerless-unicode-answered");
        assert_eq!(muse_prompt_transcript_count(&answered, unicode), 0);
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&answered, unicode),
            1
        );
        assert!(!muse_verified_process_paste_in_composer(&answered, unicode));
    }

    #[test]
    fn verified_headerless_muse_composer_distinguishes_buffered_input() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO\n";
        let idle = format!("old transcript\n{divider}\n❯\n{divider}\n{footer}");
        assert!(!muse_idle_composer(&idle));
        assert!(muse_verified_process_idle_composer(&idle));
        assert!(muse_verified_process_composer(&idle));

        let prompt = "require the full deterministic-scheduling-review skill";
        let staged = idle.replace("❯\n", &format!("❯ {prompt}\n"));
        assert!(!muse_prompt_is_exact_composer(&staged, prompt));
        assert!(muse_verified_process_prompt_is_exact_composer(
            &staged, prompt
        ));
        assert!(muse_verified_process_prompt_in_composer(&staged, prompt));
        let accepted = idle.replace("old transcript\n", &format!("❯ {prompt}\n◆ Working\n"));
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&accepted, prompt),
            1
        );
    }

    // Footers read from live Muse panes with an active goal (2026-10-28): Muse right-aligns the
    // goal's elapsed time, state and progress bar on the footer line itself.
    const GOAL_FOOTERS: [&str; 4] = [
        "watermelon-full-preview · xhigh · ~/work/project · YOLO          Goal (4h 28m) 86% ████████▋░",
        "watermelon-full-preview · xhigh · ~/work/project · Auto-review                                                                             Goal (blocked · 3m) 95% █████████▌",
        "watermelon-full-preview · xhigh · ~/work/project · YOLO   Goal (45m) 47% ████▊░░░░░",
        "watermelon-full-preview · xhigh · ~/work/project · YOLO   Goal (13m) 34% ███▍░░░░░░",
    ];

    #[test]
    fn an_active_goal_notice_on_the_footer_leaves_the_editor_readable() {
        let divider = "─".repeat(40);
        for footer in GOAL_FOOTERS {
            let idle = format!("◆ prior answer\n{divider}\n❯\n{divider}\n  {footer}\n");
            assert!(muse_verified_process_idle_composer(&idle), "{footer}");
            assert!(
                muse_idle_composer(&format!("Muse Code 1.4.0\n{idle}")),
                "{footer}"
            );
            assert_eq!(muse_verified_pane_status("idle", &idle), "idle", "{footer}");
            assert!(!muse_verified_process_goal_paused(&idle), "{footer}");
            assert_eq!(
                muse_verified_process_editor_rows(&idle).as_deref(),
                Some("❯"),
                "{footer}"
            );
            // Muse's progress line above the editor is a running turn, goal or no goal.
            let running = idle.replace(
                "◆ prior answer\n",
                "◆ prior answer\n◇ Thinking (16s · esc to interrupt)\n",
            );
            assert!(!muse_verified_process_idle_composer(&running), "{footer}");
            assert!(muse_verified_process_turn_running(&running), "{footer}");
            assert_eq!(muse_verified_pane_status("idle", &running), "working");
            // A draft is still a draft.
            let staged = idle.replace("❯\n", "❯ owner draft\n");
            assert_eq!(muse_verified_pane_status("idle", &staged), "staged");
            // Two footers are still no single authority.
            let doubled = idle.replace(
                &format!("  {footer}\n"),
                &format!("  {footer}\n  kiki · high · /work · YOLO\n"),
            );
            assert!(!muse_verified_process_idle_composer(&doubled), "{footer}");
        }
        // A goal quoted in the transcript, or drawn alone below the rule, is no footer.
        let quoted = format!("{}\n{divider}\n❯\n{divider}\n", GOAL_FOOTERS[0]);
        assert!(!muse_verified_process_idle_composer(&quoted));
        let alone = format!("old\n{divider}\n❯\n{divider}\nGoal (45m) 47% ████▊░░░░░\n");
        assert!(!muse_verified_process_idle_composer(&alone));
        assert_eq!(muse_verified_pane_status("idle", &alone), "working");
    }

    #[test]
    fn a_footer_with_an_unknown_model_effort_or_mode_is_still_a_footer() {
        // Detection must not depend on the model, the effort or the approval mode: none of these
        // strings is known to agentctl, and each footer still frames an idle editor.
        let divider = "─".repeat(40);
        for footer in [
            "some-future-model-v9 · turbo-max · /work/project · Dangerously skip everything",
            "GPT-6.1 Sol ultra · ultra · ~/work/project · YOLO",
            "model with spaces · EFFORT · /work · ",
            "m · e · /d",
            "m · e · /d · Mode · and another field",
        ] {
            let idle = format!("old transcript\n{divider}\n❯\n{divider}\n{footer}\n");
            let footer = strip(footer);
            if footer.ends_with('·') {
                // An empty field is no footer.
                assert!(!muse_verified_process_idle_composer(&idle), "{footer}");
                continue;
            }
            assert!(muse_verified_process_idle_composer(&idle), "{footer}");
            assert_eq!(muse_verified_pane_status("done", &idle), "idle", "{footer}");
        }
    }

    #[test]
    fn a_paused_goal_on_the_footer_line_reads_idle() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO";
        let paused = format!("old\n{divider}\n❯\n{divider}\n{footer}   Goal (paused)\n");
        assert!(muse_verified_process_goal_paused(&paused));
        assert_eq!(muse_verified_pane_status("idle", &paused), "idle");
        // An active goal's notice is not a pause.
        let active = paused.replace("Goal (paused)", "Goal (45m) 47% ████▊░░░░░");
        assert!(!muse_verified_process_goal_paused(&active));
        assert_eq!(muse_verified_pane_status("idle", &active), "idle");
    }

    // #734 chat-status-disagreement: a pause long enough for Muse to draw its elapsed
    // duration is detected, and still reads idle; the incident footer was `Goal (paused · 1d 17h 40m)`.
    #[test]
    fn a_paused_goal_with_an_elapsed_duration_reads_idle() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO";
        for notice in [
            "Goal (paused · 1d 17h 40m) 97% █████████▊",
            "Goal (paused · 3m) 95% █████████▌",
            "Goal (paused) 95% █████████▌",
        ] {
            let paused = format!("old\n{divider}\n❯\n{divider}\n{footer}   {notice}\n");
            assert!(muse_verified_process_goal_paused(&paused), "{notice}");
            assert_eq!(
                muse_verified_pane_status("done", &paused),
                "idle",
                "{notice}"
            );
        }
        let alone = format!("old\n{divider}\n❯\n{divider}\nGoal (paused · 1d 17h 40m)\n{footer}\n");
        assert!(muse_verified_process_goal_paused(&alone));
        assert!(!muse_screen_has_composer(
            "scrolled transcript, no editor\n❯ an old prompt\n"
        ));
        assert!(muse_screen_has_composer(&alone));
    }

    #[test]
    fn verified_muse_paused_goal_requires_footer_region() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work/project · YOLO";
        let paused = format!("Muse Code 1.4.0\n{divider}\n❯\n{divider}\nGoal (paused)\n{footer}\n");
        let transcript_echo =
            format!("Muse Code 1.4.0\nGoal (paused)\n{divider}\n❯\n{divider}\n{footer}\n");
        assert!(muse_verified_process_goal_paused(&paused));
        assert!(!muse_verified_process_goal_paused(&transcript_echo));
    }

    #[test]
    fn muse_prompt_must_move_from_composer_to_transcript() {
        let prompt = "literal $(unexpanded) delivery\nsecond line";
        let header = "Muse Code 1.3.0\n";
        let divider = "────────────────\n";
        let footer = "watermelon-preview · xhigh · /work/project · Auto-review\n";
        let staged = format!("{header}{divider}❯ {prompt}\n{divider}{footer}");
        let accepted = format!("{header}❯ {prompt}\nWorking...\n{divider}❯\n{divider}{footer}");
        let accepted_current =
            format!("{header}❯ {prompt}\n◆ Working...\n{divider}❯\n{divider}{footer}");
        let error_redraw = format!("{header}{divider}❯ {prompt}\nError: retry\n{divider}{footer}");
        assert!(muse_prompt_in_composer(&staged, prompt));
        assert!(!muse_prompt_in_transcript(&staged, prompt));
        assert!(muse_prompt_in_transcript(&accepted, prompt));
        assert!(muse_prompt_in_transcript(&accepted_current, prompt));
        assert!(!muse_prompt_in_composer(&accepted, prompt));
        assert!(muse_prompt_in_composer(&error_redraw, prompt));
        assert!(!muse_prompt_in_transcript(&error_redraw, prompt));
        let repeated_staged =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯ {prompt}\n{divider}{footer}");
        let cleared_without_submit =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&repeated_staged, prompt), 1);
        assert_eq!(
            muse_prompt_transcript_count(&cleared_without_submit, prompt),
            1
        );
    }

    #[test]
    fn custom_muse_launch_frames_never_read_as_idle() {
        // The screens of test_custom_muse_launch_never_accepts_a_trust_prompt and
        // test_custom_muse_launch_rejects_bare_prompt_without_idle_footer.
        for screen in [
            "Do you trust this workspace?\n❯ Yes\n  No\n",
            "A choice dialog\n❯\n",
        ] {
            assert!(!muse_idle_composer(screen));
            assert!(!muse_verified_process_idle_composer(screen));
            assert!(!muse_verified_process_composer(screen));
            assert_eq!(muse_verified_pane_status("idle", screen), "working");
        }
    }

    // New: the proofs the Rust edition used to approximate.

    #[test]
    fn an_echoed_prompt_does_not_satisfy_the_transcript_proof() {
        let prompt = "summarize the open review threads";
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        // The answer quotes the prompt: no user turn shows it, so nothing was submitted.
        let echoed = format!(
            "{header}❯ an earlier prompt\n◆ You asked me to {prompt}, later.\n\
             {prompt}\n{divider}❯\n{divider}{footer}"
        );
        assert_eq!(muse_prompt_transcript_count(&echoed, prompt), 0);
        assert!(!muse_prompt_in_transcript(&echoed, prompt));
        // A turn that starts with the prompt but continues is a different prompt.
        let longer =
            format!("{header}❯ {prompt} and the rest\n◆ ok\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&longer, prompt), 0);
        // The prompt quoted later in the editor is not retained input.
        let quoted = format!("{header}{divider}❯ please {prompt}\n{divider}{footer}");
        assert!(!muse_prompt_in_composer(&quoted, prompt));
        let real = format!("{header}❯ {prompt}\n◆ Working...\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&real, prompt), 1);
        let thinking =
            format!("{header}❯ {prompt}\nThinking\u{2026}\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&thinking, prompt), 1);
    }

    #[test]
    fn a_hyphenated_word_wrapped_by_muse_still_matches_exactly() {
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        let frame = |editor: &str| format!("{header}{divider}{editor}\n{divider}{footer}");
        let prompt = "keep the self-contained proof";
        assert!(muse_prompt_is_exact_composer(
            &frame("❯ keep the self-\ncontained proof"),
            prompt
        ));
        assert!(muse_prompt_is_exact_composer(
            &frame("❯ keep the self-\n  contained   proof"),
            prompt
        ));
        // Muse 1.4.14 also wraps inside a word or a path, so any row break may join unspaced
        // (captured in testdata/muse-busy/muse-wrapped-draft.txt); the words must still match.
        assert!(muse_prompt_is_exact_composer(
            &frame("❯ keep the self\ncontained proof"),
            "keep the selfcontained proof"
        ));
        assert!(!muse_prompt_is_exact_composer(
            &frame("❯ keep the self\ncontained proof"),
            "keep the self contained proofs"
        ));
        // A spaced join of the hyphenated line is still the literal text.
        assert!(muse_prompt_is_exact_composer(&frame("❯ a -\nb"), "a - b"));
        assert!(!muse_prompt_is_exact_composer(&frame("❯"), prompt));
        assert!(!muse_prompt_is_exact_composer(&frame("❯keep"), "keep"));
        assert!(!muse_prompt_is_exact_composer(&frame("❯ keep"), " \t "));
        // Retained input starts with the prompt as whole words.
        assert!(muse_prompt_in_composer(
            &frame("❯ keep the self-\ncontained proof then more"),
            prompt
        ));
        assert!(!muse_prompt_in_composer(
            &frame("❯ keep the self-\ncontained proof, then more"),
            prompt
        ));
    }

    #[test]
    fn muse_status_order_is_idle_staged_working() {
        let divider = "─".repeat(40);
        let footer = "kiki · xhigh · /work · YOLO\n";
        let idle = format!("old transcript\n{divider}\n❯\n{divider}\n{footer}");
        let staged = idle.replace("❯\n", "❯ queued owner prompt\n");
        let paused = idle.replace(footer, &format!("Goal (paused)\n{footer}"));
        let paused_staged = staged.replace(footer, &format!("Goal (paused)\n{footer}"));
        let headed = format!("Muse Code 1.4.0\n{idle}");
        let headed_paused = format!("Muse Code 1.4.0\n{paused}");
        for reported in ["idle", "done"] {
            assert_eq!(muse_verified_pane_status(reported, &idle), "idle");
            assert_eq!(muse_verified_pane_status(reported, &staged), "staged");
            // A paused goal is no hold: an empty editor reads idle, a staged one staged.
            assert_eq!(muse_verified_pane_status(reported, &paused), "idle");
            assert_eq!(
                muse_verified_pane_status(reported, &paused_staged),
                "staged"
            );
            assert_eq!(muse_verified_pane_status(reported, &headed_paused), "idle");
        }
        for reported in ["working", "unknown", "blocked"] {
            // Headerless evidence counts only while Herdr agrees the pane is idle.
            assert_eq!(muse_verified_pane_status(reported, &idle), "working");
            assert_eq!(muse_verified_pane_status(reported, &staged), "working");
            assert_eq!(muse_verified_pane_status(reported, &paused), "working");
            // The versioned header is evidence on its own, but never pauses.
            assert_eq!(muse_verified_pane_status(reported, &headed), "idle");
            assert_eq!(muse_verified_pane_status(reported, &headed_paused), "idle");
        }
        let echo = format!("Goal (paused)\n{idle}");
        assert_eq!(muse_verified_pane_status("done", &echo), "idle");
    }

    #[test]
    fn the_paste_count_expands_tabs_and_drops_controls_as_muse_does() {
        // Measured on Muse 1.4.15 with a 1700-character body (#1085 muse-send-staged-unsubmitted):
        // a tab shows as 4 wherever it sits, CR, CRLF and LF as 1, FF, VT, BEL and DEL as 0.
        let body = "x".repeat(1700);
        for (text, shown) in [
            (body.clone(), 1700),
            (format!("\t{body}"), 1704),
            (format!("abc\t{body}"), 1707),
            (format!("a\tb\t{body}"), 1710),
            (format!("{body}\nab\tc"), 1708),
            (format!("{body}\rq"), 1702),
            (format!("{body}\r\nq"), 1702),
            (format!("{body}\u{1F600}"), 1701),
            (format!("{body}\u{0c}q"), 1701),
            (format!("{body}\u{0b}q"), 1701),
            (format!("{body}\u{07}q"), 1701),
            (format!("{body}\u{7f}q"), 1701),
        ] {
            assert_eq!(
                muse_paste_length(&text),
                shown,
                "{:?}",
                &text[1690.min(text.len())..]
            );
        }
        // The placeholder of a brief holding a tab is that brief's paste, and nothing else's.
        let divider = "─".repeat(40);
        let brief =
            format!("[from lead via agentctl · reply: agentctl send lead]\nstep\t1\n{body}");
        let staged = format!(
            "old\n{divider}\n❯ [Pasted Content {} chars]\n{divider}\nm · xhigh · /w · YOLO\n",
            brief.chars().count() + 3
        );
        assert!(muse_verified_process_paste_is_exact_composer(
            &staged, &brief
        ));
        assert!(!muse_verified_process_paste_is_exact_composer(
            &staged,
            &brief.replace('\t', " ")
        ));
    }

    #[test]
    fn what_a_stalled_muse_has_drawn_of_a_paste_is_classified() {
        let divider = "─".repeat(40);
        let frame = |editor: &str| {
            format!("◇ Thinking (3m 27s · esc to interrupt)\n{divider}\n{editor}\n{divider}\nm · xhigh · /w · YOLO\n")
        };
        let prompt = "[from coordinator via agentctl · reply: agentctl send coordinator]\n\
                      Lead approved the change, part one only, as your next task after the\n\
                      current one lands.";
        let drawn = |editor: &str| muse_verified_process_paste_drawn(&frame(editor), prompt);
        assert_eq!(drawn("❯"), MusePasteDrawn::Nothing);
        // The incident's partial draws: the sender header alone, then a row cut mid-word.
        assert_eq!(
            drawn("❯ [from coordinator via agentctl · reply: agentctl send coordinator]"),
            MusePasteDrawn::Part
        );
        assert_eq!(
            drawn("❯ [from coordinator via agentctl · reply: agentctl send coordinator]\n  Lead approved the ch"),
            MusePasteDrawn::Part
        );
        assert_eq!(
            drawn("❯ [from coordinator via agentctl · reply: agentctl send coordinator]\n  Lead approved the change, part one only, as your next task after the\n  current one lands."),
            MusePasteDrawn::Whole
        );
        // What late clearing keys left behind, and the next prompt glued onto it, are neither
        // this paste nor a start of it.
        assert_eq!(
            drawn("❯ [from coordinator via agentctl · reply: agentctl send coordinator][from coordinator via agentctl · reply: agentctl send coordinator]\n  Lead approved"),
            MusePasteDrawn::Other
        );
        assert_eq!(drawn("❯ an operator's own draft"), MusePasteDrawn::Other);
        // A placeholder stands only for a whole paste: another count is not this paste.
        assert_eq!(drawn("❯ [Pasted Content 12 chars]"), MusePasteDrawn::Other);
        assert_eq!(
            muse_verified_process_paste_drawn("no editor here\n", prompt),
            MusePasteDrawn::Unreadable
        );
    }

    #[test]
    fn the_issue_1323_wrapped_notice_is_drawn_and_a_submitted_copy_is_no_draft() {
        // #1323 muse-drawn-prompt-missed: the notice as Muse drew it in the editor —
        // long lines wrapped, `❯` on the first row, a two-space continuation indent, the
        // status footer below the bottom rule.
        let divider = "─".repeat(160);
        let prompt = "[from coordinator via agentctl · reply: agentctl send coordinator]\n\
                      coordinator -> all lanes on this host: the data disk is low and another \
                      team's builds are frozen. Until further notice:\n\
                      (1) Run at most ONE local validate or build at a time per lane, and never \
                      in parallel nested worktrees.\n\
                      (2) As soon as a validate or gate finishes (never mid-validate), delete the \
                      regenerable build output it left in your slot or nested worktrees: cargo \
                      target/ dirs (rs/target, vibe-talk/target, etc.) and page build trees. If \
                      your next validate would rebuild them anyway, you may keep the ones in \
                      your MAIN slot checkout, but delete extra nested-worktree copies.\n\
                      No reply needed; just do it at your next safe point.";
        let footer = "  watermelon-full-preview · xhigh · /work/project · Launch overrides\n";
        let drawn = format!(
            "◆ prior answer\n{divider}\n\
             ❯ [from coordinator via agentctl · reply: agentctl send coordinator]\n  \
             coordinator -> all lanes on this host: the data disk is low and another team's \
             builds are frozen. Until further notice:\n  \
             (1) Run at most ONE local validate or build at a time per lane, and never in \
             parallel nested worktrees.\n  \
             (2) As soon as a validate or gate finishes (never mid-validate), delete the \
             regenerable build output it left in your slot or nested worktrees: cargo target/\n  \
             dirs (rs/target, vibe-talk/target, etc.) and page build trees. If your next \
             validate would rebuild them anyway, you may keep the ones in your MAIN slot\n  \
             checkout, but delete extra nested-worktree copies.\n  \
             No reply needed; just do it at your next safe point.\n{divider}\n{footer}"
        );
        assert_eq!(
            muse_verified_process_paste_drawn(&drawn, prompt),
            MusePasteDrawn::Whole
        );
        assert!(muse_verified_process_paste_is_exact_composer(
            &drawn, prompt
        ));
        // The draft the stale-draft check vouches for is the editor region only, unwrapped.
        let draft = muse_verified_process_editor_draft(&drawn).expect("a draft");
        let crate::stale_draft::Draft::Text(rows) = &draft else {
            panic!("a literal draft, not a placeholder: {draft:?}");
        };
        assert!(crate::stale_draft::text_matches(rows, prompt));
        // A really empty editor is still no draw at all: the late_paste refusal stands.
        let empty = format!("◆ prior answer\n{divider}\n❯\n{divider}\n{footer}");
        assert_eq!(
            muse_verified_process_paste_drawn(&empty, prompt),
            MusePasteDrawn::Nothing
        );
        assert!(muse_verified_process_editor_draft(&empty).is_none());
        // The same prompt submitted — now a user turn in the transcript above an empty
        // editor — is never a staged draft, and the pane reads idle, not staged.
        let submitted = format!(
            "❯ [from coordinator via agentctl · reply: agentctl send coordinator]\n  \
             coordinator -> all lanes on this host: the data disk is low and another team's \
             builds are frozen. Until further notice:\n  \
             (1) Run at most ONE local validate or build at a time per lane, and never in \
             parallel nested worktrees.\n  \
             (2) As soon as a validate or gate finishes (never mid-validate), delete the \
             regenerable build output it left in your slot or nested worktrees: cargo target/\n  \
             dirs (rs/target, vibe-talk/target, etc.) and page build trees. If your next \
             validate would rebuild them anyway, you may keep the ones in your MAIN slot\n  \
             checkout, but delete extra nested-worktree copies.\n  \
             No reply needed; just do it at your next safe point.\n\n\
             ◆ Noted. Standing by.\n\n{divider}\n❯\n{divider}\n{footer}"
        );
        assert_eq!(
            muse_verified_process_paste_drawn(&submitted, prompt),
            MusePasteDrawn::Nothing
        );
        assert!(muse_verified_process_editor_draft(&submitted).is_none());
        assert_eq!(
            muse_verified_process_prompt_transcript_count(&submitted, prompt),
            1
        );
        assert_eq!(muse_verified_pane_status("idle", &submitted), "idle");
    }

    #[test]
    fn a_long_brief_drawn_in_part_then_whole_is_classified_at_each_step() {
        // A real-sized coordinator brief: several KB over many lines, literal while Muse is
        // still taking it in, then its placeholder.
        let divider = "─".repeat(40);
        let line = "- Gate: COMMIT FIRST, then run the selected gate, never piped; no edits while it runs.";
        let brief = format!(
            "[from coordinator via agentctl · reply: agentctl send coordinator]\n{}",
            vec![line; 60].join("\n")
        );
        assert!(brief.len() > 5000);
        let frame =
            |editor: &str| format!("{divider}\n{editor}\n{divider}\nm · xhigh · /w · YOLO\n");
        let part = format!(
            "❯ {}",
            brief.lines().take(7).collect::<Vec<_>>().join("\n  ")
        );
        assert_eq!(
            muse_verified_process_paste_drawn(&frame(&part), &brief),
            MusePasteDrawn::Part
        );
        let whole = format!("❯ [Pasted Content {} chars]", muse_paste_length(&brief));
        assert_eq!(
            muse_verified_process_paste_drawn(&frame(&whole), &brief),
            MusePasteDrawn::Whole
        );
    }

    #[test]
    fn a_bare_slash_command_under_its_open_menu_is_the_staged_paste() {
        // Measured live on Muse 1.4.16 (#1136 raw-message-bridge): a pasted bare slash
        // command opens the command menu under the editor line, so the editor is the
        // command plus its menu rows, never the command alone; Enter runs the menu's
        // first row. Only that shape, with the typed command as the first row, is staged.
        let divider = "─".repeat(40);
        let frame =
            |editor: &str| format!("{divider}\n{editor}\n{divider}\nm · xhigh · /w · YOLO\n");
        let menu = frame("❯ /help\n  /help  Show help");
        assert!(muse_verified_process_paste_is_exact_composer(
            &menu, "/help"
        ));
        assert_eq!(
            muse_verified_process_paste_drawn(&menu, "/help"),
            MusePasteDrawn::Whole
        );
        // The command alone (menu already closed) is still the literal paste.
        assert!(muse_verified_process_paste_is_exact_composer(
            &frame("❯ /help"),
            "/help"
        ));
        // A mere prefix names no command: Enter would run the highlighted row, not this.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &frame("❯ /h\n  /help  Show help"),
            "/h"
        ));
        // The typed command must be the menu's first row, the one Enter runs.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &frame("❯ /help\n  /history  Show history\n  /help  Show help"),
            "/help"
        ));
        // A command with arguments is not staged by its menu; it stages as literal text
        // or, when long, as its placeholder, as before.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &frame("❯ /goal clear\n  /goal  Set the goal"),
            "/goal clear"
        ));
        // Ordinary text that merely stands over slash-led rows is not a command.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &frame("❯ hello\n  /help  Show help"),
            "hello"
        ));
        // A menu row that does not open with a slash is not the command menu.
        assert!(!muse_verified_process_paste_is_exact_composer(
            &frame("❯ /model\n  watermelon-full-preview"),
            "/model"
        ));
    }

    #[test]
    fn a_placeholder_hint_is_an_empty_editor_idle_and_mid_run() {
        // #1773: Muse 1.4.23 draws a dim hint inside an empty composer; a text
        // capture returns it as plain text, where it read as a foreign draft.
        let divider = "─".repeat(40);
        let footer = "m · xhigh · /w · YOLO\n";
        let idle_frame =
            |editor: &str| format!("◆ prior answer\n{divider}\n{editor}\n{divider}\n{footer}");
        let busy_frame = |editor: &str| {
            format!(
                "◇ Thinking (3s · esc to interrupt)\n◆ Backgrounded · Run build · 12s\n\
                 {divider}\n{editor}\n{divider}\n{footer}"
            )
        };
        for hint in [
            "Ask to monitor a running build or log and get pinged on changes",
            "Tab queues or steers while a run is active",
        ] {
            assert_eq!(
                hint.chars().count(),
                if hint.starts_with("Ask") { 63 } else { 42 }
            );
            let idle = idle_frame(&format!("❯ {hint}"));
            assert!(muse_verified_process_idle_composer(&idle), "{hint}");
            assert!(
                muse_idle_composer(&format!("  Muse Code 1.4.23\n{idle}")),
                "{hint}"
            );
            assert!(
                muse_verified_process_editor_draft(&idle).is_none(),
                "{hint}"
            );
            assert_eq!(muse_verified_pane_status("idle", &idle), "idle", "{hint}");
            assert_eq!(
                muse_verified_process_paste_drawn(&idle, "a real prompt"),
                MusePasteDrawn::Nothing,
                "{hint}"
            );
            // Mid-run the same ghost is a working pane, never staged.
            let busy = busy_frame(&format!("❯ {hint}"));
            assert!(!muse_verified_process_idle_composer(&busy), "{hint}");
            assert!(
                muse_verified_process_editor_draft(&busy).is_none(),
                "{hint}"
            );
            assert_eq!(
                muse_verified_pane_status("idle", &busy),
                "working",
                "{hint}"
            );
            assert!(
                crate::harness_tui::muse::MuseTui::editor_empty(&busy),
                "{hint}"
            );
        }
        // A real human draft and an unknown long text still fail closed.
        for draft in [
            "❯ please review my change before you run anything",
            "❯ Ask to monitor a running build or log and get pinged on changes please",
            "❯ a long unknown sentence that no hint table knows about at all here",
        ] {
            let screen = idle_frame(draft);
            assert!(!muse_verified_process_idle_composer(&screen), "{draft}");
            assert!(
                muse_verified_process_editor_draft(&screen).is_some(),
                "{draft}"
            );
            assert_eq!(
                muse_verified_pane_status("idle", &screen),
                "staged",
                "{draft}"
            );
            assert_eq!(
                muse_verified_process_paste_drawn(&screen, "a real prompt"),
                MusePasteDrawn::Other,
                "{draft}"
            );
        }
        // An agentctl staged paste, literal or collapsed, is still a draft.
        let brief = "x".repeat(1700);
        let staged = idle_frame(&format!(
            "❯ [Pasted Content {} chars]",
            muse_paste_length(&brief)
        ));
        assert!(muse_verified_process_editor_draft(&staged).is_some());
        assert_eq!(muse_verified_pane_status("idle", &staged), "staged");
        assert_eq!(
            muse_verified_process_paste_drawn(&staged, &brief),
            MusePasteDrawn::Whole
        );
        let literal = idle_frame("❯ [from lead via agentctl · reply: agentctl send lead] hello");
        assert!(muse_verified_process_editor_draft(&literal).is_some());
        // Narrow panes wrap a long hint across rows, or cut it at the edge
        // (with or without an ellipsis): all three read empty; a short prefix
        // or edge-full unknown text still fails closed.
        let long_hint = "Ask to monitor a running build or log and get pinged on changes";
        for width in [40usize, 60] {
            let rule = "─".repeat(width);
            let frame =
                |editor: &str| format!("◆ prior answer\n{rule}\n{editor}\n{rule}\n{footer}");
            // Wrapped: break the hint (after the marker) into width-full rows.
            let mut rows: Vec<String> = Vec::new();
            let mut rest = format!("❯ {long_hint}");
            while rest.chars().count() > width {
                let cut: String = rest.chars().take(width).collect();
                rows.push(cut);
                rest = rest.chars().skip(width).collect();
            }
            rows.push(rest);
            let wrapped = frame(&rows.join("\n"));
            assert!(
                muse_verified_process_idle_composer(&wrapped),
                "wrapped {width}: {wrapped}"
            );
            assert!(
                muse_verified_process_editor_draft(&wrapped).is_none(),
                "wrapped {width}"
            );
            // Truncated at the edge: one width-full row, no ellipsis.
            let cut: String = format!("❯ {long_hint}").chars().take(width).collect();
            let truncated = frame(&cut);
            assert!(
                muse_verified_process_idle_composer(&truncated),
                "cut {width}: {truncated}"
            );
            assert!(
                muse_verified_process_editor_draft(&truncated).is_none(),
                "cut {width}"
            );
            // Unknown text filling the same edge is still a draft.
            let unknown = frame(
                &"❯ an unknown draft that fills this row!!"
                    .chars()
                    .take(width)
                    .collect::<String>(),
            );
            assert!(
                !muse_verified_process_idle_composer(&unknown),
                "unknown {width}"
            );
            assert!(
                muse_verified_process_editor_draft(&unknown).is_some(),
                "unknown {width}"
            );
        }
        // An ellipsis cut needs no edge, a short prefix is never enough.
        let ellipsised = idle_frame("❯ Ask to monitor a running build or log and get pinged…");
        assert!(muse_verified_process_idle_composer(&ellipsised));
        let short = idle_frame("❯ /goal pins a ses");
        assert!(!muse_verified_process_idle_composer(&short));
        assert!(muse_verified_process_editor_draft(&short).is_some());
        // Faint styling alone proves a ghost on an ANSI read, even for new text.
        let faint = format!(
            "◆ prior answer\n{divider}\n❯ \u{1b}[2ma future hint nobody listed\u{1b}[0m\n{divider}\n{footer}"
        );
        assert_eq!(muse_ansi_composer_is_empty(&faint), Some(true));
        // Live 1.4.23 styling (coordb evidence): blue glyph, truecolor-grey hint,
        // grey status line. The exact sequence must read empty; the same words in
        // default intensity are typed text and must not.
        let grey = format!(
            "◆ prior answer\n{divider}\n\
             \u{1b}[0m\u{1b}[38;2;90;160;255m❯ \u{1b}[0m\u{1b}[38;2;103;108;116m\
             Type @ to search and insert workspace file paths\u{1b}[0m\n{divider}\n\
             \u{1b}[38;2;103;108;116mm · xhigh · /w · YOLO\u{1b}[0m\n"
        );
        assert_eq!(muse_ansi_composer_is_empty(&grey), Some(true), "{grey:?}");
        assert!(crate::harness_tui::muse::MuseTui::editor_empty(&grey));
        let typed = grey.replace("\u{1b}[38;2;103;108;116mType @", "Type @");
        assert_eq!(
            muse_ansi_composer_is_empty(&typed),
            Some(false),
            "{typed:?}"
        );
        assert!(!crate::harness_tui::muse::MuseTui::editor_empty(&typed));
        let solid = format!(
            "◆ prior answer\n{divider}\n❯ a future hint nobody listed\n{divider}\n{footer}"
        );
        assert_eq!(muse_ansi_composer_is_empty(&solid), None);
        assert!(muse_verified_process_editor_draft(&solid).is_some());
    }

    #[test]
    fn python_string_semantics_are_reproduced() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), vec![""]);
        assert_eq!(splitlines("a\n\nb\n\n"), vec!["a", "", "b", ""]);
        assert_eq!(
            splitlines("a\r\nb\rc\u{0b}d\u{0c}e"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(
            splitlines("a\u{1c}b\u{1d}c\u{1e}d\u{85}e\u{2028}f\u{2029}g\u{1f}h"),
            vec!["a", "b", "c", "d", "e", "f", "g\u{1f}h"]
        );
        assert_eq!(normalize("\u{1f} a\u{a0}\u{3000}b\t\u{1c}"), "a b");
        assert_eq!(strip("\u{1d} ❯ \u{2009}"), "❯");
        // A line separator inside a prompt is a line break to both editions.
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work/project · Auto-review\n";
        let staged = format!("{header}{divider}❯ first\u{2028}second\n{divider}{footer}");
        assert!(muse_prompt_is_exact_composer(&staged, "first second"));
        assert!(muse_prompt_is_exact_composer(&staged, "first\u{1f}second"));
    }
}
