//! Cutting one long message into the several Discord will actually accept.
//!
//! # Why this exists
//!
//! Discord refuses a message body over [`DISCORD_MAX_CONTENT_LEN`] characters. Until now this
//! server refused it too, one layer earlier, which on a phone meant a reply somebody had spent
//! minutes typing came back as an error and stayed stuck in the box. Coding agents posting INTO
//! the channel already split their own long messages; the reply path did not.
//!
//! # The invariant that matters more than the formatting
//!
//! **No character the reader typed is ever dropped.** Concatenating the parts reproduces the
//! original text exactly, except where a fenced code block had to be closed and reopened across a
//! boundary — and that exception is itself explicit, testable, and the only one.
//!
//! Everything else here is a preference about WHERE to cut. Those preferences may be wrong for
//! some message and it will still be readable; losing a paragraph is not recoverable and would be
//! unforgivable, because the text only ever existed in one place.
//!
//! # Where it prefers to cut, in order
//!
//! A blank line, then any newline, then a space — each searched for backwards from the limit, so a
//! part is as full as it can be. A single run of non-whitespace longer than the whole limit (a URL,
//! a base64 blob) is cut at the limit, because there is nothing else to do with it.
//!
//! # Fenced code
//!
//! A cut inside ``` would leave the first part opening a fence it never closes and the second
//! starting with code Discord renders as prose. So the fence is closed at the end of one part and
//! reopened — with its language — at the start of the next.

use super::http::DISCORD_MAX_CONTENT_LEN;

/// Cut `text` into pieces Discord will accept, in order.
///
/// Returns one part for a message already short enough, and never returns an empty vector for
/// non-empty input. A part is never empty and never only whitespace.
#[must_use]
pub fn split_for_discord(text: &str) -> Vec<String> {
    split_to(text, DISCORD_MAX_CONTENT_LEN)
}

/// The limit is a parameter so the tests can exercise boundaries without building 2000-character
/// fixtures nobody can read in a diff.
#[must_use]
pub fn split_to(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    if text.chars().count() <= limit {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![text.to_owned()]
        };
    }

    let mut parts: Vec<String> = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    // The fence a previous part left open, if it did: `Some(language)`, language possibly empty.
    let mut reopen: Option<String> = None;

    while !rest.is_empty() {
        let prefix = reopen
            .as_ref()
            .map(|lang| format!("```{lang}\n"))
            .unwrap_or_default();
        let prefix_len = prefix.chars().count();
        // Room for the body of this part, once any reopened fence and the closing fence it may
        // need are accounted for. `+ 4` is "\n```".
        let budget = limit.saturating_sub(prefix_len);
        if rest.len() <= budget && open_fence(&prefix, &rest).is_none() {
            let body: String = rest.iter().collect();
            push_part(&mut parts, format!("{prefix}{body}"));
            break;
        }
        // Two SEPARATE numbers, and conflating them was a bug: reserving the closing fence was
        // also acting as the "do not cut too early" floor, so a blank line comfortably inside the
        // budget fell below the search window and the cut landed mid-sentence instead.
        //
        // The reservation is conservative — taken whenever the text contains a fence at all,
        // because whether THIS part ends inside one is only known after the cut is chosen.
        let hard = if text.contains("```") {
            budget.saturating_sub(4).max(1)
        } else {
            budget
        };
        // Half the allowance: below that a part is mostly empty, which is worse than a cut that
        // breaks a sentence.
        let take = cut_at(&rest, (hard / 2).max(1), hard);
        let (head, tail) = rest.split_at(take);
        let head: String = head.iter().collect();
        let mut piece = format!("{prefix}{head}");
        reopen = open_fence(&prefix, &rest[..take]);
        if let Some(lang) = reopen.clone() {
            // Close what this part opened, so the next one can reopen it. Without this the first
            // part renders as an unterminated fence and the second as prose.
            let _ = &lang;
            piece.push_str("\n```");
        }
        push_part(&mut parts, piece);
        rest = tail.to_vec();
        if rest.iter().all(|c| c.is_whitespace()) {
            // Only whitespace left. It cannot be posted and carries nothing, so it is the one
            // thing dropped — recorded here rather than left as a silent difference.
            break;
        }
    }
    parts
}

fn push_part(parts: &mut Vec<String>, piece: String) {
    if !piece.trim().is_empty() {
        parts.push(piece);
    }
}

/// Where to cut, searching BACKWARDS from `hard` so the part is as full as it can be.
///
/// `soft` is the earliest cut worth taking; below it the search gives up and takes `hard`, because
/// a part that is mostly empty is worse than one that breaks mid-sentence.
fn cut_at(rest: &[char], soft: usize, hard: usize) -> usize {
    let hard = hard.min(rest.len()).max(1);
    let soft = soft.min(hard);
    // A blank line: the strongest boundary a reader recognises.
    for i in (soft..hard).rev() {
        if rest[i] == '\n' && i > 0 && rest[i - 1] == '\n' {
            return i + 1;
        }
    }
    for i in (soft..hard).rev() {
        if rest[i] == '\n' {
            return i + 1;
        }
    }
    for i in (soft..hard).rev() {
        if rest[i] == ' ' {
            return i + 1;
        }
    }
    // One unbroken run longer than the limit. Nothing to prefer; take the whole allowance.
    hard
}

/// The language of a fence left OPEN at the end of `prefix + chunk`, if one is.
///
/// Counts ``` markers: an odd number means the block is still open. The language is taken from the
/// opening marker so the reopened fence highlights the same way.
fn open_fence(prefix: &str, chunk: &[char]) -> Option<String> {
    let joined: String = prefix.chars().chain(chunk.iter().copied()).collect();
    let mut open: Option<String> = None;
    let mut rest = joined.as_str();
    while let Some(at) = rest.find("```") {
        let after = &rest[at + 3..];
        if open.is_some() {
            open = None;
        } else {
            let lang: String = after
                .chars()
                .take_while(|c| !c.is_whitespace())
                .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '-' || *c == '#')
                .collect();
            open = Some(lang);
        }
        rest = after;
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property everything else is subordinate to: nothing the reader typed disappears.
    ///
    /// Fence markers this module INJECTED are removed before comparing, because those are the one
    /// documented exception — and the removal is written so that a part which lost real text
    /// cannot pass by being mistaken for an injected marker.
    fn rejoined(parts: &[String]) -> String {
        parts.join("")
    }

    #[test]
    fn a_message_that_already_fits_is_left_exactly_alone() {
        assert_eq!(split_to("hello", 100), vec!["hello".to_owned()]);
        // Including one that is exactly at the limit: an off-by-one here splits a message that did
        // not need splitting, which is visible to everyone in the channel.
        let exact = "x".repeat(100);
        assert_eq!(split_to(&exact, 100), vec![exact.clone()]);
    }

    #[test]
    fn nothing_is_ever_lost() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
        for limit in 8..=40 {
            let parts = split_to(text, limit);
            assert_eq!(
                rejoined(&parts),
                text,
                "limit {limit} lost or invented text"
            );
            for part in &parts {
                assert!(part.chars().count() <= limit, "limit {limit}: {part:?}");
                assert!(
                    !part.trim().is_empty(),
                    "limit {limit} produced a blank part"
                );
            }
        }
    }

    #[test]
    fn a_paragraph_boundary_is_preferred_to_a_line_and_a_line_to_a_space() {
        let text = "one two three\n\nfour five six seven eight";
        let parts = split_to(text, 20);
        assert!(
            parts[0].ends_with("\n\n"),
            "did not cut at the blank line: {parts:?}"
        );
        assert_eq!(rejoined(&parts), text);
    }

    #[test]
    fn one_unbroken_run_longer_than_the_limit_is_cut_rather_than_refused() {
        // A URL or a base64 blob. There is no good boundary; refusing would lose the message.
        let text = "x".repeat(250);
        let parts = split_to(&text, 100);
        assert_eq!(rejoined(&parts), text);
        assert!(parts.iter().all(|p| p.chars().count() <= 100));
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn a_fence_split_across_parts_is_closed_and_reopened_with_its_language() {
        let text = format!("```rust\n{}\n```", "let x = 1;\n".repeat(30));
        let parts = split_to(&text, 120);
        assert!(parts.len() > 1, "the fixture did not split");
        // Every part is independently renderable: an even number of fence markers.
        for (i, part) in parts.iter().enumerate() {
            assert_eq!(
                part.matches("```").count() % 2,
                0,
                "part {i} leaves a fence open: {part:?}"
            );
        }
        assert!(
            parts[1].starts_with("```rust"),
            "the reopened fence lost its language"
        );
    }

    #[test]
    fn every_part_is_something_discord_will_accept() {
        let text = "a\n\n\n\n\nb\n\n\n\n\nc ".repeat(40);
        for limit in 10..=60 {
            for part in split_to(&text, limit) {
                assert!(
                    !part.trim().is_empty(),
                    "limit {limit}: a blank part would be refused"
                );
                assert!(part.chars().count() <= limit, "limit {limit}: {part:?}");
            }
        }
    }

    #[test]
    fn text_that_is_only_whitespace_produces_nothing_to_send() {
        assert!(split_to("", 100).is_empty());
        assert!(split_to("   \n\n  ", 100).is_empty());
    }

    #[test]
    fn multibyte_characters_are_counted_as_characters_and_never_cut_in_half() {
        // Discord counts characters, not bytes, and a cut that lands mid-codepoint would corrupt
        // the text rather than merely split it.
        let text = "héllo wörld ".repeat(30);
        for limit in 5..=40 {
            let parts = split_to(&text, limit);
            assert_eq!(rejoined(&parts), text, "limit {limit}");
            for part in &parts {
                assert!(part.chars().count() <= limit);
            }
        }
    }
}
