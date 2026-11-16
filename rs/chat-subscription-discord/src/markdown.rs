//! Discord markdown and mention handling for posted text.
//!
//! The Discord counterpart of the relay's Slack link-markup handling
//! (`agent-teams::relay::undo_slack_link_markup`). The two providers differ in kind:
//!
//! * Slack REWRITES stored text (`foo.rs` becomes `<http://foo.rs|foo.rs>`), so the relay must
//!   undo markup on receipt before the body hash verifies.
//! * Discord stores and returns content byte-for-byte. It only RENDERS markdown and mentions.
//!   A relay envelope inside its fenced block needs no rescue on the Discord path (and the
//!   relay already escapes `@`, `<`, `>` inside the fence, so a body cannot form a mention).
//!
//! What Discord does need is care with the VISIBLE text outside fences: a `<@ID>`,
//! `@everyone` or `@here` there would notify people, and (for a careless reader) look like
//! addressing a bot. [`neutralize_mentions`] defuses those while leaving ordinary markdown
//! alone (agents write markdown on purpose); [`escape_markdown`] makes text render literally;
//! [`unescape_markdown`] is its exact inverse. All three leave fenced code blocks unchanged,
//! so an `agent-teams-envelope` block always reaches the receiver byte-for-byte.
//!
//! The outbound helper also sends `allowed_mentions: {"parse": []}` on every post, so even a
//! mention that slipped through cannot ping; neutralising is the visible half of that guard.

/// Characters Discord markdown treats specially.
const MARKDOWN_SPECIALS: &[char] = &[
    '\\', '*', '_', '~', '`', '|', '>', '#', '-', '[', ']', '(', ')',
];

/// A zero-width space, inserted to break mention syntax without changing what a reader sees.
const ZWSP: char = '\u{200B}';

/// Apply `transform` to the text outside fenced (```) code blocks only.
fn outside_fences(text: &str, transform: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    let mut prose = String::new();
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            if !in_fence {
                out.push_str(&transform(&prose));
                prose.clear();
            }
            in_fence = !in_fence;
            out.push_str(line);
        } else if in_fence {
            out.push_str(line);
        } else {
            prose.push_str(line);
        }
    }
    // An unclosed fence is left as written; trailing prose is transformed.
    out.push_str(&transform(&prose));
    out
}

/// Break `<@ID>`, `<@!ID>`, `<@&ID>`, `@everyone` and `@here` outside fences.
#[must_use]
pub fn neutralize_mentions(text: &str) -> String {
    outside_fences(text, |prose| {
        prose
            .replace("<@", &format!("<{ZWSP}@"))
            .replace("@everyone", &format!("@{ZWSP}everyone"))
            .replace("@here", &format!("@{ZWSP}here"))
    })
}

/// Backslash-escape markdown specials outside fences so the text renders literally, and
/// neutralise mentions.
#[must_use]
pub fn escape_markdown(text: &str) -> String {
    let escaped = outside_fences(text, |prose| {
        let mut out = String::with_capacity(prose.len());
        for c in prose.chars() {
            if MARKDOWN_SPECIALS.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    });
    neutralize_mentions(&escaped)
}

/// The exact inverse of [`escape_markdown`]: remove the zero-width mention breaks and the
/// backslash before each markdown special, outside fences.
#[must_use]
pub fn unescape_markdown(text: &str) -> String {
    outside_fences(text, |prose| {
        let prose = prose
            .replace(&format!("<{ZWSP}@"), "<@")
            .replace(&format!("@{ZWSP}everyone"), "@everyone")
            .replace(&format!("@{ZWSP}here"), "@here");
        let mut out = String::with_capacity(prose.len());
        let mut chars = prose.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.peek().copied() {
                    if MARKDOWN_SPECIALS.contains(&next) {
                        out.push(next);
                        chars.next();
                        continue;
                    }
                }
            }
            out.push(c);
        }
        out
    })
}

/// The fence opener every relay block starts with.
const RELAY_FENCE_OPENER: &str = "```agent-teams-";
/// The line that closes a fence a chunk boundary interrupts.
const FENCE_CLOSE: &str = "```\n";

/// Split `text` into Discord-sized chunks of at most `limit` characters, preferring line
/// breaks.
///
/// A fenced block that crosses a chunk boundary is closed at the end of one chunk and reopened,
/// with its original opener line (and so its language), at the start of the next, so every
/// chunk renders on its own. The one exception is a relay block (a line opening an
/// `agent-teams-` fence): its receiver needs the whole block in one post, so text that needs
/// splitting and contains one returns `None`, and the helper refuses it rather than break an
/// envelope.
#[must_use]
pub fn chunk(text: &str, limit: usize) -> Option<Vec<String>> {
    let limit = limit.max(1);
    if text.chars().count() <= limit {
        return Some(vec![text.to_owned()]);
    }
    if text
        .lines()
        .any(|line| line.trim_start().starts_with(RELAY_FENCE_OPENER))
    {
        return None;
    }
    let mut chunker = Chunker {
        limit,
        chunks: Vec::new(),
        current: String::new(),
        count: 0,
        base: 0,
        fence: None,
    };
    for line in text.split_inclusive('\n') {
        chunker.line(line);
    }
    if chunker.count > chunker.base || chunker.chunks.is_empty() {
        chunker.chunks.push(chunker.current);
    }
    Some(chunker.chunks)
}

struct Chunker {
    limit: usize,
    chunks: Vec<String>,
    current: String,
    /// Characters in `current`.
    count: usize,
    /// Characters of the reopened fence opener at the start of `current` (0 when none).
    base: usize,
    /// The opener line of the fence `current` is inside, if any.
    fence: Option<String>,
}

impl Chunker {
    /// Room to keep free for closing an open fence: the close line and a possible newline.
    fn reserve(&self) -> usize {
        if self.fence.is_some() {
            FENCE_CLOSE.len() + 1
        } else {
            0
        }
    }

    fn fits(&self, chars: usize) -> bool {
        self.count + chars + self.reserve() <= self.limit
    }

    /// End the current chunk (closing an open fence) and start the next (reopening it).
    fn break_chunk(&mut self) {
        if self.fence.is_some() {
            if !self.current.ends_with('\n') {
                self.current.push('\n');
            }
            self.current.push_str(FENCE_CLOSE);
        }
        self.chunks.push(std::mem::take(&mut self.current));
        self.count = 0;
        self.base = 0;
        if let Some(open) = &self.fence {
            self.current.push_str(open);
            self.count = open.chars().count();
            self.base = self.count;
        }
    }

    fn push_text(&mut self, text: &str) {
        let chars = text.chars().count();
        if !self.fits(chars) && self.count > self.base {
            self.break_chunk();
        }
        if self.fits(chars) {
            self.current.push_str(text);
            self.count += chars;
            return;
        }
        // Longer than a whole chunk: split it by characters.
        for c in text.chars() {
            if !self.fits(1) && self.count > self.base {
                self.break_chunk();
            }
            self.current.push(c);
            self.count += 1;
        }
    }

    fn line(&mut self, line: &str) {
        if !line.trim_start().starts_with("```") {
            self.push_text(line);
            return;
        }
        let chars = line.chars().count();
        if self.fence.is_some() {
            // A closing line needs no reserve: it closes the fence itself.
            if self.count + chars > self.limit && self.count > self.base {
                self.break_chunk();
            }
            self.current.push_str(line);
            self.count += chars;
            self.fence = None;
        } else {
            // Keep the opener with at least one line of its block.
            if !self.fits(chars + FENCE_CLOSE.len()) && self.count > self.base {
                self.break_chunk();
            }
            self.current.push_str(line);
            self.count += chars;
            let mut open = line.to_owned();
            if !open.ends_with('\n') {
                open.push('\n');
            }
            self.fence = Some(open);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addressing::mention_ids;

    const FENCED: &str = "relay fleet@a -> fleet@b: hi <@1545796614826889226>\n\
```agent-teams-envelope\n{\"body\":\"a_b *c* <@1>\"}\n```\n";

    #[test]
    fn mentions_are_broken_outside_fences_only() {
        let out = neutralize_mentions(FENCED);
        let (visible, fence) = out.split_once("```").expect("fence kept");
        assert!(mention_ids(visible).is_empty(), "{visible}");
        assert!(!visible.contains("@everyone"));
        assert_eq!(
            fence,
            "agent-teams-envelope\n{\"body\":\"a_b *c* <@1>\"}\n```\n"
        );
        assert!(
            neutralize_mentions("ping @everyone and @here")
                .split(ZWSP)
                .count()
                == 3
        );
    }

    #[test]
    fn escape_round_trips_and_keeps_fences_byte_for_byte() {
        for text in [
            FENCED,
            "plain *bold* _it_ ~~s~~ `code` | > # - [x](y) back\\slash",
            "@everyone look at <@!42>",
            "unclosed\n```\nfence *stays*",
        ] {
            let escaped = escape_markdown(text);
            assert_eq!(unescape_markdown(&escaped), text, "{escaped}");
        }
        let escaped = escape_markdown(FENCED);
        assert!(escaped.ends_with("```agent-teams-envelope\n{\"body\":\"a_b *c* <@1>\"}\n```\n"));
        assert!(escaped.starts_with("relay fleet@a \\-\\> fleet@b: hi <\u{200B}@"));
    }

    #[test]
    fn chunking_prefers_lines_and_refuses_to_split_a_fence() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(
            chunk(text, 10),
            Some(vec!["aaaa\nbbbb\n".to_owned(), "cccc\n".to_owned()])
        );
        assert_eq!(chunk("abcdefgh", 3).map(|c| c.len()), Some(3));
        assert_eq!(chunk(FENCED, 20), None, "a relay envelope is never split");
        assert_eq!(chunk(FENCED, 2000).map(|c| c.len()), Some(1));
    }

    /// Every chunk is within the limit and leaves no fence open.
    fn assert_fenced_split(text: &str, limit: usize) -> Vec<String> {
        let chunks = chunk(text, limit).expect("an ordinary fence splits");
        assert!(chunks.len() > 1, "the text needed splitting");
        for (index, part) in chunks.iter().enumerate() {
            assert!(
                part.chars().count() <= limit,
                "chunk {index}: {}",
                part.chars().count()
            );
            let fences = part
                .lines()
                .filter(|line| line.trim_start().starts_with("```"))
                .count();
            assert_eq!(fences % 2, 0, "chunk {index} leaves a fence open:\n{part}");
        }
        chunks
    }

    #[test]
    fn a_long_reply_with_an_ordinary_fence_is_split_and_reopened() {
        let code: String = (0..120)
            .map(|n| format!("let value_{n} = {n};\n"))
            .collect();
        let text = format!("Here is the patch:\n```rust\n{code}```\nThat is all.\n");
        assert!(text.chars().count() > 2000);
        let chunks = assert_fenced_split(&text, 2000);
        assert!(chunks[0].starts_with("Here is the patch:\n```rust\n"));
        assert!(chunks[0].ends_with("\n```\n"), "closed at the break");
        assert!(
            chunks[1].starts_with("```rust\n"),
            "reopened with its language"
        );
        assert!(chunks.last().expect("last").ends_with("That is all.\n"));
        // Stripping the inserted close+reopen pairs restores the original exactly.
        let mut joined = chunks.join("");
        joined = joined.replace("```\n```rust\n", "");
        assert_eq!(joined, text);
        // A tiny limit and a single over-long fenced line still terminate within the limit.
        let long_line = format!("```\n{}\n```\n", "x".repeat(300));
        assert_fenced_split(&long_line, 50);
    }
}
