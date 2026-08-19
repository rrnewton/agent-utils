//! Turning one verbose agent message into one speakable line.
//!
//! This is deliberately extractive and deterministic: no model call, no network, no cost. The
//! related-work review found that a shipped product solved the same problem with a terse system
//! prompt rather than a summarization pipeline, so the job here is only to make a channel
//! *listing* short enough to be spoken. When the owner wants the real thing, he asks for the full
//! message and gets it verbatim.

use serde::Serialize;

use crate::model::{Message, UserId};

/// Default width of a spoken digest line.
pub const DEFAULT_SUMMARY_CHARS: usize = 160;

/// One entry of a channel digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DigestEntry {
    /// Snowflake, so the caller can ask for this exact message in full.
    pub id: String,
    /// Author display name.
    pub author: String,
    /// Author snowflake, so a caller can mention them with `<@id>` without a lookup step.
    /// See [`crate::model::Message::author_id`] for why this rides along instead.
    pub author_id: UserId,
    /// ISO-8601 timestamp as Discord reported it.
    pub timestamp: String,
    /// One speakable line. UNTRUSTED text, condensed.
    pub summary: String,
    /// Character length of the full message, so the caller can tell "short" from "a wall".
    pub full_length: usize,
    /// Whether the summary dropped anything.
    pub truncated: bool,
}

/// Condense a message body to at most `max_chars` characters of speakable text.
///
/// Fenced code, inline code, and URLs are replaced with short placeholders, because reading them
/// aloud is never what was wanted. The text that remains is the author's, unaltered.
#[must_use]
pub fn condense(content: &str, max_chars: usize) -> String {
    let flattened = flatten(content);
    if flattened.is_empty() {
        return "(no text)".to_owned();
    }
    truncate_on_word_boundary(&flattened, max_chars)
}

/// Build a digest entry for one message.
#[must_use]
pub fn digest_entry(message: &Message, max_chars: usize) -> DigestEntry {
    let summary = condense(&message.content, max_chars);
    DigestEntry {
        id: message.id.0.clone(),
        author: message.author.clone(),
        author_id: message.author_id.clone(),
        timestamp: message.timestamp.clone(),
        truncated: summary.ends_with('…'),
        summary,
        full_length: message.content.chars().count(),
    }
}

/// Build a digest for a batch of messages, in the order given.
#[must_use]
pub fn digest(messages: &[Message], max_chars: usize) -> Vec<DigestEntry> {
    messages
        .iter()
        .map(|m| digest_entry(m, max_chars))
        .collect()
}

/// Replace code and links with placeholders, then collapse all whitespace to single spaces.
fn flatten(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    // Fenced blocks first: they are the biggest source of unspeakable text.
    while let Some(start) = rest.find("```") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        match after.find("```") {
            Some(end) => {
                out.push_str(" [code] ");
                rest = &after[end + 3..];
            }
            None => {
                // Unterminated fence: treat the remainder as code rather than reading it aloud.
                out.push_str(" [code] ");
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);

    let mut result = String::with_capacity(out.len());
    for word in out.split_whitespace() {
        let cleaned = if word.starts_with("http://") || word.starts_with("https://") {
            "[link]"
        } else {
            word
        };
        let cleaned = cleaned.trim_matches('`');
        if cleaned.is_empty() {
            continue;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(cleaned);
    }
    result
}

fn truncate_on_word_boundary(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return "…".to_owned();
    }
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    // Reserve one character for the ellipsis.
    let budget = max_chars - 1;
    let mut kept = String::new();
    for word in text.split(' ') {
        let extra = if kept.is_empty() { 0 } else { 1 };
        if kept.chars().count() + extra + word.chars().count() > budget {
            break;
        }
        if extra == 1 {
            kept.push(' ');
        }
        kept.push_str(word);
    }
    if kept.is_empty() {
        // One enormous word: cut it mid-word rather than returning nothing.
        kept = text.chars().take(budget).collect();
    }
    kept.push('…');
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelId, MessageId};

    fn message(content: &str) -> Message {
        Message {
            id: MessageId("1000000000000000001".to_owned()),
            channel_id: ChannelId("c".to_owned()),
            author: "coder-bot".to_owned(),
            author_id: UserId("2000000000000000001".to_owned()),
            author_is_bot: true,
            timestamp: "2026-08-18T12:00:00+00:00".to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn a_short_message_is_returned_verbatim() {
        assert_eq!(condense("build is green", 160), "build is green");
    }

    #[test]
    fn fenced_code_is_replaced_not_read_aloud() {
        let summary = condense("here is the fix:\n```rust\nfn main() {}\n```\nlanded", 160);
        assert_eq!(summary, "here is the fix: [code] landed");
        assert!(!summary.contains("fn main"));
    }

    #[test]
    fn an_unterminated_fence_does_not_leak_the_rest_of_the_message() {
        let summary = condense("look:\n```\nsecrets and noise\nmore noise", 160);
        assert!(
            !summary.contains("noise"),
            "unterminated fence leaked: {summary}"
        );
        assert!(summary.contains("[code]"));
    }

    #[test]
    fn urls_become_a_placeholder() {
        let summary = condense(
            "see https://github.com/rrnewton/agent-utils/pull/12 for detail",
            160,
        );
        assert_eq!(summary, "see [link] for detail");
    }

    #[test]
    fn newlines_and_runs_of_space_collapse() {
        assert_eq!(condense("one\n\n   two\tthree", 160), "one two three");
    }

    #[test]
    fn empty_content_is_labelled_not_blank() {
        assert_eq!(condense("   \n  ", 160), "(no text)");
    }

    #[test]
    fn long_text_is_cut_at_a_word_boundary_within_budget() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let summary = condense(text, 20);
        assert!(summary.chars().count() <= 20, "over budget: {summary:?}");
        assert!(summary.ends_with('…'), "no ellipsis: {summary:?}");
        assert_eq!(summary, "alpha bravo charlie…");
        assert!(
            !summary.contains("charli…"),
            "truncation must not split a word: {summary:?}"
        );
    }

    #[test]
    fn a_single_enormous_word_is_still_cut() {
        let summary = condense(&"x".repeat(100), 10);
        assert_eq!(summary.chars().count(), 10);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn digest_entry_reports_the_full_length_and_truncation_flag() {
        let long = "word ".repeat(200);
        let entry = digest_entry(&message(&long), 40);
        assert!(entry.truncated);
        assert_eq!(entry.full_length, long.chars().count());
        assert_eq!(entry.author, "coder-bot");
        assert_eq!(entry.id, "1000000000000000001");
        assert_eq!(
            entry.author_id.as_str(),
            "2000000000000000001",
            "a digest line must carry the id, or replying by mention needs a lookup"
        );

        let short = digest_entry(&message("done"), 40);
        assert!(!short.truncated);
        assert_eq!(short.summary, "done");
    }

    #[test]
    fn digest_preserves_input_order_and_length() {
        let messages = vec![message("first"), message("second")];
        let entries = digest(&messages, 40);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }
}
