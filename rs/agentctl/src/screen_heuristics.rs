//! Harness-neutral screen heuristics: where our own text sits on a terminal screen, whether a
//! menu or dialog is open, and whether the screen has stopped moving.
//!
//! These read no harness's phrases or layout. They hold for any TUI that draws its input near the
//! bottom of the screen, so they serve two callers: the best-effort mode for a harness agentctl
//! knows nothing about ([`crate::unknown_harness`]), and every harness's admission path as a
//! fallback layer under the harness's own composer model.
//!
//! The central rule is the owner's: if words that fuzzy-match what we typed are still sitting at
//! the bottom of the screen, our prompt is still there. "Still sitting" is judged against the
//! screen as it was once the text was staged (its *static tail*): the text is a leftover only
//! while the rows after it are unchanged. A line-oriented TUI keeps a submitted line on screen
//! as transcript right above its next prompt, so the text alone at the bottom proves nothing;
//! a new prompt row, output, or a redrawn input box after it shows the submission.
//!
//! Matching ignores everything but letters and digits, lowercased, so line wrapping, box
//! borders, prompt glyphs, markdown rendering and quote styles do not matter.

use crate::submission::render_screen;

/// Non-blank rows, counted up from the last one, that make up the bottom of the screen.
pub const BOTTOM_ROWS: usize = 8;
/// Non-blank rows at the very bottom in which text counts as stranded input before anything
/// was typed: an input line and the box border and footer a TUI may draw under it.
pub const STRANDED_ROWS: usize = 3;
/// Letters and digits of the text's end used to find it on screen.
pub const PROBE_CHARS: usize = 32;
/// Probes shorter than this must end a row to match, so `hi` does not match inside `this`.
const SHORT_PROBE: usize = 8;
/// Option rows a numbered menu shows at least.
const MENU_MIN_OPTIONS: usize = 2;
/// Glyphs that mark the selected row of a menu.
const CURSOR_GLYPHS: &[char] = &['❯', '›', '>', '▶', '▸', '➜', '→', '●', '◉', '*'];
/// Yes/no prompts on the last row, lowercased.
const YES_NO: &[&str] = &[
    "(y/n)", "[y/n]", "(yes/no)", "[yes/no]", "(y/n/a)", "[y/n/a]",
];

/// One rendered row: its text, and whether any of its letters are drawn in reverse video or faint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Visible characters.
    pub text: String,
    /// Some non-blank cell is reverse video or faint.
    pub highlighted: bool,
}

/// Rendered rows of `screen`, trailing blank rows removed.
#[must_use]
pub fn rows(screen: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = render_screen(screen)
        .into_iter()
        .map(|(plain, solid)| Row {
            highlighted: plain
                .iter()
                .zip(&solid)
                .any(|(p, s)| !p.is_whitespace() && s.is_whitespace()),
            text: plain.iter().collect::<String>().trim_end().to_owned(),
        })
        .collect();
    while rows.last().is_some_and(|row| row.text.trim().is_empty()) {
        rows.pop();
    }
    rows
}

/// Index of the first of the last `count` non-blank rows.
fn bottom_start(rows: &[Row], count: usize) -> usize {
    let mut seen = 0;
    for (index, row) in rows.iter().enumerate().rev() {
        if !row.text.trim().is_empty() {
            seen += 1;
            if seen == count {
                return index;
            }
        }
    }
    0
}

/// `text` reduced to its letters and digits, lowercased.
#[must_use]
pub fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The end of `text` used to find it on screen, or `None` when it has no letters or digits.
#[must_use]
pub fn probe(text: &str) -> Option<String> {
    let squashed: Vec<char> = squash(text).chars().collect();
    if squashed.is_empty() {
        return None;
    }
    let start = squashed.len().saturating_sub(PROBE_CHARS);
    Some(squashed[start..].iter().collect())
}

/// The row on which the last match of `text`'s probe ends, searching rows from `from` on.
fn locate_from(rows: &[Row], text: &str, from: usize) -> Option<usize> {
    let probe: Vec<char> = probe(text)?.chars().collect();
    let mut chars: Vec<char> = Vec::new();
    let mut owners: Vec<usize> = Vec::new();
    let mut row_ends: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(from) {
        for c in squash(&row.text).chars() {
            chars.push(c);
            owners.push(index);
        }
        row_ends.push(chars.len());
    }
    if probe.len() > chars.len() {
        return None;
    }
    (0..=chars.len() - probe.len()).rev().find_map(|start| {
        let end = start + probe.len();
        let matched = chars[start..end] == probe[..]
            && (probe.len() >= SHORT_PROBE || row_ends.contains(&end));
        matched.then(|| owners[end - 1])
    })
}

/// Row index at which `text` ends within the bottom of the screen, if it is there.
#[must_use]
pub fn locate_in_bottom(screen: &str, text: &str) -> Option<usize> {
    let rows = rows(screen);
    locate_from(&rows, text, bottom_start(&rows, BOTTOM_ROWS))
}

/// Whether `text` shows anywhere on `screen` above its bottom rows: echoed into the transcript.
#[must_use]
pub fn echoed_above(screen: &str, text: &str) -> bool {
    let rows = rows(screen);
    let bottom = bottom_start(&rows, BOTTOM_ROWS);
    locate_from(&rows[..bottom], text, 0).is_some()
}

/// Whether `text` sits in the last [`STRANDED_ROWS`] non-blank rows: typed earlier and never
/// submitted, as far as a screen with no reference can tell.
#[must_use]
pub fn stranded(screen: &str, text: &str) -> bool {
    let rows = rows(screen);
    let start = bottom_start(&rows, STRANDED_ROWS);
    locate_from(&rows, text, bottom_start(&rows, BOTTOM_ROWS)).is_some_and(|end| end >= start)
}

/// A row with clock digits and spinner glyphs masked, for comparing screens over time. Each run
/// of digits becomes one `0`, so a counter that gains a digit (9 to 10) does not count as a change.
fn masked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '0'..='9' => {
                if !out.ends_with('0') {
                    out.push('0');
                }
            }
            '\u{2800}'..='\u{28ff}' => out.push(' '),
            c if crate::harness::running_glyphs().contains(&c) => out.push(' '),
            c => out.push(c),
        }
    }
    out.trim_end().to_owned()
}

/// The screen as compared for quiescence: rendered text with clock digits and spinner glyphs
/// masked, so a ticking timer or spinner does not count as movement.
#[must_use]
pub fn quiet_key(screen: &str) -> String {
    rows(screen)
        .iter()
        .map(|row| masked(&row.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The screen once our text was staged, kept to tell a leftover from a submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticTail {
    /// Rows after the text (masked) when the text was found; `None` when the text was never
    /// found and the whole bottom is compared instead.
    after_text: Option<Vec<String>>,
    /// The bottom rows (masked).
    bottom: Vec<String>,
}

impl StaticTail {
    /// Record `screen`, on which `text` was just staged.
    #[must_use]
    pub fn capture(screen: &str, text: &str) -> Self {
        let rows = rows(screen);
        let start = bottom_start(&rows, BOTTOM_ROWS);
        Self {
            after_text: locate_from(&rows, text, start).map(|end| tail_after(&rows, end)),
            bottom: rows[start..].iter().map(|row| masked(&row.text)).collect(),
        }
    }

    /// Whether the text was found on the staged screen.
    #[must_use]
    pub fn anchored(&self) -> bool {
        self.after_text.is_some()
    }

    /// Whether `text` still sits at the bottom of `screen` as it did when staged: found there
    /// with the same rows after it, or, for text never found, the same bottom rows.
    #[must_use]
    pub fn leftover(&self, screen: &str, text: &str) -> bool {
        let rows = rows(screen);
        let start = bottom_start(&rows, BOTTOM_ROWS);
        match &self.after_text {
            Some(after) => {
                locate_from(&rows, text, start).is_some_and(|end| &tail_after(&rows, end) == after)
            }
            None => rows[start..]
                .iter()
                .map(|row| masked(&row.text))
                .eq(self.bottom.iter().cloned()),
        }
    }
}

/// Whether the rows after `text` at the bottom of `screen` are the rows `before` ended with:
/// the text went in above the screen's static tail (an input line, a box border, a footer)
/// rather than being followed by new output. `None` when the text is not at the bottom.
#[must_use]
pub fn above_static_tail(before: &str, screen: &str, text: &str) -> Option<bool> {
    let rows_now = rows(screen);
    let end = locate_from(&rows_now, text, bottom_start(&rows_now, BOTTOM_ROWS))?;
    let after = tail_after(&rows_now, end);
    let before: Vec<String> = rows(before).iter().map(|row| masked(&row.text)).collect();
    Some(after.len() <= before.len() && before[before.len() - after.len()..] == after[..])
}

fn tail_after(rows: &[Row], end: usize) -> Vec<String> {
    rows[end + 1..]
        .iter()
        .map(|row| masked(&row.text))
        .collect()
}

/// A visible menu option row: its number, and whether a cursor glyph marks it.
fn option_row(text: &str) -> Option<(usize, bool)> {
    let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c == '│' || c == '|');
    let cursor = trimmed.starts_with(CURSOR_GLYPHS);
    let body = trimmed.trim_start_matches(CURSOR_GLYPHS).trim_start();
    let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let rest = &body[digits.len()..];
    let label = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    label
        .chars()
        .any(char::is_alphanumeric)
        .then(|| (digits.parse().ok(), cursor))
        .and_then(|(number, cursor)| number.map(|n| (n, cursor)))
}

/// `screen` without the numbered option rows (and blank rows) it ends with.
#[must_use]
pub fn without_trailing_options(screen: &str) -> String {
    let mut rows = rows(screen);
    while rows
        .last()
        .is_some_and(|row| row.text.trim().is_empty() || option_row(&row.text).is_some())
    {
        rows.pop();
    }
    rows.iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The line proving that `screen` shows an open menu or dialog at its bottom, which no key may
/// be sent into: numbered options 1..n with a cursor glyph or highlight on one of them, a yes/no
/// prompt on the last row, or any known harness's approval phrase. Rows that are part of
/// `ignore` (our own staged text, which may hold a numbered list) are skipped.
#[must_use]
pub fn menu_open(screen: &str, ignore: Option<&str>) -> Option<String> {
    let rows = rows(screen);
    let start = bottom_start(&rows, BOTTOM_ROWS + 4);
    let ours = ignore.map(squash).unwrap_or_default();
    let ours_contains = |text: &str| {
        let row = squash(text);
        !row.is_empty() && !ours.is_empty() && ours.contains(&row)
    };
    let mut numbers = Vec::new();
    let mut marked: Option<&str> = None;
    for row in &rows[start..] {
        if ours_contains(&row.text) {
            continue;
        }
        if let Some((number, cursor)) = option_row(&row.text) {
            numbers.push(number);
            if cursor || row.highlighted {
                marked.get_or_insert(row.text.trim());
            }
        }
    }
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.len() >= MENU_MIN_OPTIONS && numbers == (1..=numbers.len()).collect::<Vec<_>>() {
        if let Some(line) = marked {
            return Some(line.to_owned());
        }
    }
    if let Some(last) = rows.last().filter(|row| !ours_contains(&row.text)) {
        let lower = last.text.to_lowercase();
        if YES_NO.iter().any(|prompt| lower.contains(prompt)) {
            return Some(last.text.trim().to_owned());
        }
    }
    rows[start..].iter().find_map(|row| {
        let lower = row.text.to_lowercase();
        (!ours_contains(&row.text)
            && crate::harness::approval_patterns()
                .iter()
                .any(|phrase| lower.contains(phrase)))
        .then(|| row.text.trim().to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squash_ignores_wrapping_borders_and_glyphs() {
        assert_eq!(squash("│ › Hello,\n│   World! │"), "helloworld");
        assert_eq!(probe("...").as_deref(), None);
        assert_eq!(probe(&"ab".repeat(40)).unwrap().len(), PROBE_CHARS);
    }

    #[test]
    fn locates_wrapped_text_in_the_bottom_only() {
        let screen = "old line\nmore\n> please review the\n  change in foo.rs\n";
        assert_eq!(
            locate_in_bottom(screen, "please review the change in foo.rs"),
            Some(3)
        );
        assert_eq!(locate_in_bottom(screen, "something else"), None);
        let mut tall = String::from("> please review the change in foo.rs\n");
        for index in 0..20 {
            tall.push_str(&format!("output {index}\n"));
        }
        assert_eq!(
            locate_in_bottom(&tall, "please review the change in foo.rs"),
            None
        );
        assert!(echoed_above(&tall, "please review the change in foo.rs"));
    }

    #[test]
    fn short_probes_must_end_a_row() {
        assert_eq!(locate_in_bottom("this is it\n> ok", "hi"), None);
        assert_eq!(locate_in_bottom("this is it\n> hi", "hi"), Some(1));
    }

    #[test]
    fn leftover_is_judged_by_the_rows_after_the_text() {
        let text = "run the gate and report back";
        let staged = "transcript\n> run the gate and report back\n[idle] 12:00:01\n";
        let tail = StaticTail::capture(staged, text);
        assert!(tail.anchored());
        // The same screen, clock ticked: still a leftover.
        assert!(tail.leftover(
            "transcript\n> run the gate and report back\n[idle] 12:00:07\n",
            text
        ));
        // A line-oriented TUI keeps the line as transcript and prints a new prompt after it.
        assert!(!tail.leftover(
            "transcript\n> run the gate and report back\nworking...\n> \n[busy] 12:00:09\n",
            text
        ));
        // A boxed TUI clears its box.
        assert!(!tail.leftover("transcript\n> \n[busy] 12:00:09\n", text));
    }

    #[test]
    fn menus_need_numbered_options_and_a_cursor() {
        let menu = "Pick one\n❯ 1. Yes\n  2. No, and tell it what to do\n";
        assert_eq!(menu_open(menu, None).as_deref(), Some("❯ 1. Yes"));
        let list = "Plan:\n1. read the code\n2. write tests\n> ";
        assert_eq!(menu_open(list, None), None);
        let highlighted = "  1. Allow\n\u{1b}[7m  2. Deny\u{1b}[0m\n";
        assert_eq!(menu_open(highlighted, None).as_deref(), Some("2. Deny"));
        assert!(menu_open("Overwrite file? (y/N)", None).is_some());
        assert!(menu_open("Overwrite file? [y/n]", None).is_some());
        assert!(menu_open("Do you want to proceed?\n", None).is_some());
        // Our own numbered list sitting in the input is not a menu.
        let ours = "steps:\n> 1. build it\n  2. test it";
        let text = "steps:\n1. build it\n2. test it";
        assert!(menu_open(&format!("transcript\n{ours}"), Some(text)).is_none());
    }

    #[test]
    fn quiet_key_masks_clocks_and_spinners() {
        assert_eq!(
            quiet_key("⠋ working 12s\n> "),
            quiet_key("⠙ working 13s\n> ")
        );
        // A counter that gains a digit is still the same screen.
        assert_eq!(quiet_key("[idle] 9 reads"), quiet_key("[idle] 10 reads"));
        assert_ne!(quiet_key("a\n> "), quiet_key("b\n> "));
    }

    #[test]
    fn stranded_text_is_in_the_last_rows() {
        let text = "please continue with the review";
        assert!(stranded("out\n> please continue with the review\n", text));
        let mut later = String::from("> please continue with the review\n");
        later.push_str("one\ntwo\nthree\nfour\n> ");
        assert!(!stranded(&later, text));
    }
}
