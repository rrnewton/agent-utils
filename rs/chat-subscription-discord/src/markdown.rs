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

/// Split `text` into Discord-sized chunks of at most `limit` characters, preferring line
/// breaks. Returns `None` when the text contains a fenced block that would be split: a split
/// relay post's later chunk carries no fence opener, so the helper refuses rather than
/// breaking an envelope.
#[must_use]
pub fn chunk(text: &str, limit: usize) -> Option<Vec<String>> {
    let limit = limit.max(1);
    if text.chars().count() <= limit {
        return Some(vec![text.to_owned()]);
    }
    if text.contains("```") {
        return None;
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for line in text.split_inclusive('\n') {
        let line_chars = line.chars().count();
        if count + line_chars > limit && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        if line_chars > limit {
            for c in line.chars() {
                if count == limit {
                    chunks.push(std::mem::take(&mut current));
                    count = 0;
                }
                current.push(c);
                count += 1;
            }
        } else {
            current.push_str(line);
            count += line_chars;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Some(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addressing::mention_ids;

    const FENCED: &str = "relay deepscry@a -> deepscry@b: hi <@1545796614826889226>\n\
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
        assert!(escaped.starts_with("relay deepscry@a \\-\\> deepscry@b: hi <\u{200B}@"));
    }

    #[test]
    fn chunking_prefers_lines_and_refuses_to_split_a_fence() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(
            chunk(text, 10),
            Some(vec!["aaaa\nbbbb\n".to_owned(), "cccc\n".to_owned()])
        );
        assert_eq!(chunk("abcdefgh", 3).map(|c| c.len()), Some(3));
        assert_eq!(chunk(FENCED, 20), None);
        assert_eq!(chunk(FENCED, 2000).map(|c| c.len()), Some(1));
    }
}
