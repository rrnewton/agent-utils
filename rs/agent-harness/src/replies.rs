//! Reply blocks, and whether a block read from a screen renders one read from a transcript.
//!
//! An agent answering a chat writes each reply between a line `<CHAT_REPLY_ID>` and a line
//! `</CHAT_REPLY_ID>`. In a transcript those lines appear exactly as written. A terminal draws
//! the same block with Markdown markers turned into styles (`**bold**` as bold, `` `code` `` as
//! code, `# Heading` as a styled line), tables as box drawing, links as their text, and every
//! long line wrapped at the terminal's width. [`rendered_equivalent`] tells whether a screen
//! block is such a rendering of a transcript block, so the transcript's text can stand in for it.

/// One complete reply block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyBlock {
    /// The ID between `<CHAT_REPLY_` and `>`.
    pub identifier: String,
    /// The lines between the two marker lines, without the blank lines at either end and
    /// without the indentation every line shares.
    pub body: String,
}

/// The ID of a marker line `<CHAT_REPLY_ID>` (or `</CHAT_REPLY_ID>` when `closing`), alone on
/// its line apart from surrounding whitespace.
fn marker(line: &str, closing: bool) -> Option<&str> {
    let prefix = if closing {
        "</CHAT_REPLY_"
    } else {
        "<CHAT_REPLY_"
    };
    let identifier = line.trim().strip_prefix(prefix)?.strip_suffix('>')?;
    (!identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then_some(identifier)
}

/// The code-fence run a line opens or closes: its character (`` ` `` or `~`) and length, when
/// the line starts one of at least three.
fn fence(line: &str) -> Option<(char, usize)> {
    let line = line.trim_start();
    let character = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = line.chars().take_while(|c| *c == character).count();
    let info = &line[length..];
    (length >= 3 && !(character == '`' && info.contains('`'))).then_some((character, length))
}

/// Whether `line` closes the fence `open`.
fn closes(line: &str, open: (char, usize)) -> bool {
    let line = line.trim();
    let length = line.chars().take_while(|c| *c == open.0).count();
    length >= open.1 && line.chars().all(|c| c == open.0)
}

/// A reply block being read: its ID, its lines so far, and a code fence open inside it.
type OpenBlock<'a> = (&'a str, Vec<&'a str>, Option<(char, usize)>);

/// Every complete reply block in `text`, in order.
///
/// A block opens at a marker line outside any code fence and closes at the closing marker line
/// of the same ID; it needs at least one nonblank line between them. Inside a block, code fences
/// are part of the reply, and a closing marker inside one of them does not close the block. An
/// opening marker inside an open block starts a new block in its place. A block with no closing
/// marker is not complete and is left out.
#[must_use]
pub fn complete_blocks(text: &str) -> Vec<ReplyBlock> {
    let mut blocks = Vec::new();
    let mut outside_fence: Option<(char, usize)> = None;
    // The open block's ID, its lines, and a code fence open inside it.
    let mut open: Option<OpenBlock<'_>> = None;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some((identifier, lines, inner)) = open.as_mut() {
            if let Some(run) = *inner {
                if closes(line, run) {
                    *inner = None;
                }
                lines.push(line);
                continue;
            }
            if marker(line, true) == Some(*identifier) {
                let body = dedent(lines);
                if !body.trim().is_empty() {
                    blocks.push(ReplyBlock {
                        identifier: (*identifier).to_owned(),
                        body,
                    });
                }
                open = None;
                continue;
            }
            if let Some(next) = marker(line, false) {
                open = Some((next, Vec::new(), None));
                continue;
            }
            *inner = fence(line);
            lines.push(line);
            continue;
        }
        if let Some(run) = outside_fence {
            if closes(line, run) {
                outside_fence = None;
            }
            continue;
        }
        if let Some(identifier) = marker(line, false) {
            open = Some((identifier, Vec::new(), None));
        } else if let Some(run) = fence(line) {
            outside_fence = Some(run);
        }
    }
    blocks
}

/// `lines` without the blank lines at either end and without their shared indentation.
fn dedent(lines: &[&str]) -> String {
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    let (Some(first), Some(last)) = (first, last) else {
        return String::new();
    };
    let lines = &lines[first..=last];
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            if line.len() >= indent && line.is_char_boundary(indent) {
                line[indent..].trim_end()
            } else {
                line.trim()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Characters Markdown uses inside a line for emphasis, code, strike-through, tables and links,
/// which a terminal draws as style or layout rather than as text.
const INLINE_MARKUP: &[char] = &['*', '_', '`', '~', '|', '[', ']', '(', ')'];

/// Bullets that open a list item, as Markdown writes them and as terminals draw them.
const BULLETS: &[char] = &['-', '*', '+', '•', '·', '●', '◦', '▪', '‣'];

/// Whether `line` is drawn as a rule or not at all: a table's delimiter row such as
/// `|---|:---:|`, a thematic break such as `---` or `***`, or a code fence's opening or closing
/// line, with or without its info string.
fn layout_line(line: &str) -> bool {
    let line = line.trim();
    let table = line.contains("---")
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
    let rule = line.chars().count() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|mark| line.chars().all(|c| c == *mark || c == ' '));
    table || rule || fence(line).is_some()
}

/// `line` without what opens it as a quote, heading or list item: `>` markers, `#` markers, a
/// bullet, or a number or letter followed by `.` or `)`, each followed by a space. A terminal
/// redraws or renumbers these.
fn line_content(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let before = rest;
        if let Some(after) = rest.strip_prefix('>') {
            rest = after.trim_start();
        }
        let hashes = rest.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && rest[hashes..].starts_with(' ') {
            rest = rest[hashes..].trim_start();
        }
        if let Some(after) = rest
            .strip_prefix(BULLETS)
            .filter(|after| after.starts_with(' '))
        {
            rest = after.trim_start();
        }
        let marker = rest
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .count()
            .max(usize::from(
                rest.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && rest[1..].starts_with(['.', ')']),
            ));
        if marker > 0
            && rest[marker..].starts_with(['.', ')'])
            && rest[marker + 1..].starts_with(' ')
        {
            rest = rest[marker + 1..].trim_start();
        }
        if rest == before {
            return rest;
        }
    }
}

/// What survives every way a terminal renders `text`: its characters in order, without
/// whitespace, box drawing (U+2500 to U+257F), inline Markdown markup, the markers that open a
/// quote, heading or list item, or lines a terminal draws as rules (table delimiter rows,
/// thematic breaks, code fence lines). Letters, digits, other punctuation such as `-` and `+`
/// inside a line, and symbols such as a progress bar's `█` and `░` stay.
#[must_use]
pub fn rendered_key(text: &str) -> String {
    let mut key = String::new();
    for line in text.lines().filter(|line| !layout_line(line)) {
        let mut chars = line_content(line).chars().peekable();
        while let Some(c) = chars.next() {
            // A backslash escape (`\|`, `\_`) is drawn as the character it escapes, which then
            // counts as any such character does.
            let c = match chars.peek() {
                Some(next) if c == '\\' && next.is_ascii_punctuation() => {
                    let escaped = *next;
                    chars.next();
                    escaped
                }
                _ => c,
            };
            if !c.is_whitespace()
                && !('\u{2500}'..='\u{257f}').contains(&c)
                && !INLINE_MARKUP.contains(&c)
            {
                key.push(c);
            }
        }
    }
    key
}

/// Whether `text` holds a Markdown or box-drawn table row, whose cells a terminal can wrap
/// across rows, reordering its characters.
#[must_use]
pub fn has_table(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        (line.starts_with('|') && line.ends_with('|') && line.len() > 1)
            || (line.starts_with('│') && line.ends_with('│'))
    })
}

/// [`rendered_key`] of `text` with its Markdown link targets left out: the same for the text as
/// written and as a terminal draws it when it shows each link's label only.
#[must_use]
pub fn label_key(text: &str) -> String {
    rendered_key(&without_link_targets(text))
}

/// `text` with the targets of Markdown links, `[label](target)`, removed, as a terminal that
/// shows only a link's label draws it.
fn without_link_targets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("](") {
        let (before, after) = rest.split_at(index + 1);
        out.push_str(before);
        match after[1..].find(')') {
            Some(end) if !after[1..=end].contains('\n') => rest = &after[end + 2..],
            _ => {
                out.push('(');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn sorted(key: &str) -> Vec<char> {
    let mut chars: Vec<char> = key.chars().collect();
    chars.sort_unstable();
    chars
}

/// Whether `screen`, a reply block's text as read from a terminal, is a rendering of `source`,
/// the same block's text from a transcript.
///
/// They are when their [`rendered_key`]s are the same (Markdown markers, bullets, box drawing and
/// wrapping differ), with or without the source's link targets, or, when the source holds a table
/// ([`has_table`]), the same characters in another order, as when its cells wrap across rows.
/// Text with nothing but markup and whitespace is never judged a rendering.
#[must_use]
pub fn rendered_equivalent(screen: &str, source: &str) -> bool {
    let shown = rendered_key(screen);
    if shown.is_empty() {
        return false;
    }
    let candidates = [
        rendered_key(source),
        rendered_key(&without_link_targets(source)),
    ];
    if candidates.contains(&shown) {
        return true;
    }
    if !has_table(source) {
        return false;
    }
    let shown = sorted(&shown);
    candidates.iter().any(|key| sorted(key) == shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(identifier: &str, body: &str) -> ReplyBlock {
        ReplyBlock {
            identifier: identifier.into(),
            body: body.into(),
        }
    }

    #[test]
    fn complete_blocks_are_found_with_their_markdown_intact() {
        let text = "Intro.\n<CHAT_REPLY_007>\n\n**Done.** See `x`:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n</CHAT_REPLY_007>\nAfter.\n  <CHAT_REPLY_abc_2>  \n    indented\n      more\n  </CHAT_REPLY_abc_2>\n";
        assert_eq!(
            complete_blocks(text),
            [
                block(
                    "007",
                    "**Done.** See `x`:\n\n| a | b |\n|---|---|\n| 1 | 2 |"
                ),
                block("abc_2", "indented\n  more"),
            ]
        );
    }

    #[test]
    fn quoted_partial_empty_and_inline_blocks_are_left_out() {
        let text = [
            "```text",
            "<CHAT_REPLY_001>",
            "quoted in a fence",
            "</CHAT_REPLY_001>",
            "```",
            "<CHAT_REPLY_002>",
            "   ",
            "</CHAT_REPLY_002>",
            "<CHAT_REPLY_003>Inline</CHAT_REPLY_003>",
            "<CHAT_REPLY_004>",
            "replaced by the next opening",
            "<CHAT_REPLY_005>",
            "kept",
            "</CHAT_REPLY_004>",
            "</CHAT_REPLY_005>",
            "<CHAT_REPLY_006>",
            "never closed",
        ]
        .join("\n");
        assert_eq!(
            complete_blocks(&text),
            [block("005", "kept\n</CHAT_REPLY_004>")]
        );
    }

    #[test]
    fn a_code_fence_inside_a_block_belongs_to_the_reply() {
        let text = "<CHAT_REPLY_009>\nRun:\n```sh\necho '</CHAT_REPLY_009>'\n</CHAT_REPLY_009>\n```\nThat's it.\n</CHAT_REPLY_009>\r\n";
        assert_eq!(
            complete_blocks(text),
            [block(
                "009",
                "Run:\n```sh\necho '</CHAT_REPLY_009>'\n</CHAT_REPLY_009>\n```\nThat's it."
            )]
        );
    }

    #[test]
    fn a_screen_rendering_matches_its_source_and_other_text_does_not() {
        let source = "**Mitigated; please reload.** See `vibe-talk`'s [status page](https://example.com/s).\n\n| Leg | This test |\n|---|---:|\n| admitted → delivered | 5.6s |\n\n## Next\n- one\n- two";
        // Wrapped at the terminal's width, styles drawn, the table boxed, the link shown as its
        // label only.
        let wrapped = "Mitigated; please reload. See vibe-talk's status\npage.\n\n┌──────────────────────┬───────────┐\n│ Leg                  │ This test │\n├──────────────────────┼───────────┤\n│ admitted → delivered │      5.6s │\n└──────────────────────┴───────────┘\n\nNext\n• one\n• two";
        assert!(rendered_equivalent(wrapped, source));
        // A table whose cells wrapped across rows reads its letters in another order.
        let cells = "│ admitted → │ 5.6s │\n│ delivered  │      │";
        assert!(rendered_equivalent(
            cells,
            "| admitted → delivered | 5.6s |"
        ));
        // With the link target shown, it still matches.
        assert!(rendered_equivalent(
            "status page (https://example.com/s)",
            "[status page](https://example.com/s)"
        ));
        assert!(!rendered_equivalent("Mitigated; please reload.", source));
        assert!(!rendered_equivalent("- **`#`** -", "---"));
        assert_eq!(
            label_key("See [the page](https://x.test/p)."),
            "Seethepage."
        );
        // Symbols that are text stay: progress bars that differ are different text.
        assert!(!rendered_equivalent(
            "Rollout: ███░░░░░░░",
            "Rollout: ████████░░"
        ));
        assert_eq!(rendered_key("| a | b |\n|---|:--:|\n| 1 | 2 |"), "ab12");
        // A fence's info string, a renumbered list and nested bullets are layout.
        assert!(rendered_equivalent(
            "Pushed. Verify with:\n  git log -1",
            "Pushed. Verify with:\n```sh\ngit log -1\n```"
        ));
        assert!(rendered_equivalent(
            "1. first step\n2. second step\n  ◦ detail",
            "1. first step\n1. second step\n   - detail"
        ));
        // Signs inside a line are text, and only a table's characters may be reordered.
        assert!(!rendered_equivalent("Delta: -3", "Delta: +3"));
        assert!(!rendered_equivalent("21 of 30 done", "12 of 30 done"));
        // An escape is drawn as the character it escapes.
        assert!(rendered_equivalent(
            "a|b and foo_bar",
            "a\\|b and foo\\_bar"
        ));
        assert!(has_table("x\n| a |\ny") && !has_table("a | b"));
        assert_eq!(
            rendered_key("> ## Title\n- a-b\n10. c+d\n---"),
            "Titlea-bc+d"
        );
        assert_eq!(without_link_targets("a](b"), "a](b");
        assert_eq!(without_link_targets("[x](y)\n[z](w\n)"), "[x]\n[z](w\n)");
    }
}
