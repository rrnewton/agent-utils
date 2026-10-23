//! Why a chat service refused a post, in its words and in the reader's.
//!
//! `#444 send-refusal-reason`. The owner, replying in threads: "the message won't send after
//! multiple retries." Each retry was refused by the provider with HTTP 400 in a millisecond — its
//! answer said why — and this server logged nothing about it, and the page called every one
//! "Delivery unconfirmed. Retrying cannot post it twice.", which invites exactly the retry that
//! cannot succeed: the same words are refused the same way.
//!
//! So a provider's refusal — a 4xx, which says the post was not taken and that asking again asks
//! the same question ([`ChatError::is_transient`]) — becomes a [`SendRefusal`]: the status, the
//! provider's own reason, and a clause the page can put after "Google Chat refused this message:"
//! ([`SendRefusal::explanation`]). It travels in the 207's `refusal` and is logged at WARN by
//! [`log_failed_part`], with every other failed part, without the text of the post.

use serde::Serialize;

use crate::chat::ChatError;
use crate::model::ChannelInfo;

/// The longest provider reason kept, in characters. A bridge's reason is a sentence; a longer body
/// is an intermediary's page, and this ends up in a log and on a card.
pub const REASON_MAX_CHARS: usize = 300;

/// A provider's refusal of one part of a post. `#444 send-refusal-reason`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SendRefusal {
    /// The provider's HTTP status, a 4xx.
    pub status: u16,
    /// The provider's reason as it gave it: its error body's `message` (or `detail`, or `error`),
    /// else the body itself, cut to [`REASON_MAX_CHARS`].
    pub reason: String,
    /// The reason in the reader's words, to follow "<service> refused this message: ". Names a
    /// mention as the text wrote it (`it contains @Ada, which would notify someone`); a reason it
    /// has no words of its own for is the provider's.
    pub explanation: String,
}

impl SendRefusal {
    /// The refusal `error` is, when it is a provider's 4xx answer to posting `part`; `None` for a
    /// failure that may pass — a lost connection, a rate limit, a 5xx, a 408 — and for one that
    /// is not the provider's.
    #[must_use]
    pub fn of(error: &ChatError, part: &str) -> Option<Self> {
        let ChatError::Status { status, body } = error.cause() else {
            return None;
        };
        if !(400..500).contains(status) || error.is_transient() {
            return None;
        }
        let reason = reason_in(body);
        let explanation = explain(&reason, part);
        Some(Self {
            status: *status,
            reason,
            explanation,
        })
    }
}

/// A provider error body's own sentence: the `message`, `detail` or `error` string of a JSON
/// object, as Discord and the bridges that speak its API write one; else the body, on one line.
fn reason_in(body: &str) -> String {
    let said = serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|value| {
            ["message", "detail", "error"].iter().find_map(|key| {
                value
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| body.trim().to_owned());
    one_line_capped(&said)
}

/// `text` on one line, cut to [`REASON_MAX_CHARS`].
fn one_line_capped(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut kept: String = one_line.chars().take(REASON_MAX_CHARS).collect();
    if one_line.chars().count() > REASON_MAX_CHARS {
        kept.push('…');
    }
    kept
}

/// `reason` in the reader's words, about `part`, the text that was refused.
fn explain(reason: &str, part: &str) -> String {
    let lower = reason.to_lowercase();
    if lower.contains("mention") {
        return match crate::mention_defuse::first_notifying(part) {
            Some(mention) => format!("it contains {mention}, which would notify someone"),
            None => "it contains mention syntax, which would notify someone".to_owned(),
        };
    }
    if lower.contains("nul byte") || lower.contains("null byte") {
        return "it contains a NUL character, which cannot be sent".to_owned();
    }
    if lower.contains("at most") || lower.contains("too long") || lower.contains("length") {
        return format!("it is too long for one message ({reason})");
    }
    if reason.is_empty() {
        return "it gave no reason".to_owned();
    }
    reason.to_owned()
}

/// Log a part of a post that failed, without its text: at WARN, with the provider's reason when
/// it refused the part. `#444 send-refusal-reason`.
///
/// `thread` is where the part was going, `None` for the conversation itself. `index` and `parts`
/// place the part in a post that was cut, `part` is its text, of which only the size is logged.
pub fn log_failed_part(
    info: &ChannelInfo,
    thread: Option<&str>,
    index: usize,
    parts: usize,
    part: &str,
    error: &ChatError,
    refusal: Option<&SendRefusal>,
) {
    let provider = match error {
        ChatError::Provider { provider, .. } => provider.as_str(),
        _ => info.provider.as_deref().unwrap_or("chat"),
    };
    let place = if thread.is_some() { "thread" } else { "main" };
    let content_bytes = part.len();
    let content_chars = part.chars().count();
    let part = index + 1;
    match refusal {
        Some(refusal) => tracing::warn!(
            provider,
            status = refusal.status,
            channel = %info.id,
            place,
            reason = %refusal.reason,
            content_bytes,
            content_chars,
            part,
            parts,
            "the chat service refused a post; sending the same text again will be refused too"
        ),
        None => tracing::warn!(
            provider,
            channel = %info.id,
            place,
            error = %failure_in_brief(error),
            transient = error.is_transient(),
            may_have_delivered = error.may_have_delivered(),
            content_bytes,
            content_chars,
            part,
            parts,
            "a post failed"
        ),
    }
}

/// A failure that is not a refusal, for the log: the status and the provider's reason, cut short,
/// rather than the whole body — an intermediary's error page, or a body that repeats the post.
fn failure_in_brief(error: &ChatError) -> String {
    match error.cause() {
        ChatError::Status { status, body } => format!("HTTP {status}: {}", reason_in(body)),
        other => one_line_capped(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: u16, body: &str) -> ChatError {
        ChatError::Status {
            status,
            body: body.to_owned(),
        }
        .with_provider("Google Chat")
    }

    #[test]
    fn a_refused_mention_is_named_as_the_text_wrote_it() {
        let error = status(
            400,
            r#"{"message": "content contains mention syntax that Google Chat would expand"}"#,
        );
        let refusal =
            SendRefusal::of(&error, "> @Ada Lovelace said so\n\nagreed").expect("refusal");
        assert_eq!(refusal.status, 400);
        assert_eq!(
            refusal.reason,
            "content contains mention syntax that Google Chat would expand"
        );
        assert_eq!(
            refusal.explanation,
            "it contains @Ada, which would notify someone"
        );
        let token = SendRefusal::of(&error, "cc <users/42>").expect("refusal");
        assert_eq!(
            token.explanation,
            "it contains <users/42>, which would notify someone"
        );
        let unseen = SendRefusal::of(&error, "nothing that matches").expect("refusal");
        assert_eq!(
            unseen.explanation,
            "it contains mention syntax, which would notify someone"
        );
    }

    #[test]
    fn other_refusals_keep_the_providers_words() {
        let long = status(
            400,
            r#"{"message": "content must be at most 16384 UTF-8 bytes"}"#,
        );
        assert_eq!(
            SendRefusal::of(&long, "x").expect("refusal").explanation,
            "it is too long for one message (content must be at most 16384 UTF-8 bytes)"
        );
        let nul = status(400, r#"{"message": "content must not contain a NUL byte"}"#);
        assert_eq!(
            SendRefusal::of(&nul, "x").expect("refusal").explanation,
            "it contains a NUL character, which cannot be sent"
        );
        let denied = status(403, r#"{"message": "Missing Permissions", "code": 50013}"#);
        assert_eq!(
            SendRefusal::of(&denied, "x").expect("refusal").explanation,
            "Missing Permissions"
        );
        let plain = status(400, "Bad Request\n");
        assert_eq!(
            SendRefusal::of(&plain, "x").expect("refusal").reason,
            "Bad Request"
        );
        let empty = status(400, "");
        assert_eq!(
            SendRefusal::of(&empty, "x").expect("refusal").explanation,
            "it gave no reason"
        );
    }

    #[test]
    fn a_long_reason_is_cut_and_put_on_one_line() {
        let body = format!("{{\"message\": \"bad\\nthing {}\"}}", "x".repeat(400));
        let refusal = SendRefusal::of(&status(400, &body), "x").expect("refusal");
        assert!(
            refusal.reason.starts_with("bad thing "),
            "{}",
            refusal.reason
        );
        assert_eq!(refusal.reason.chars().count(), REASON_MAX_CHARS + 1);
        assert!(!refusal.reason.contains('\n'));
    }

    #[test]
    fn a_failure_is_logged_in_brief() {
        let page = status(502, &format!("<html>{}</html>", "x".repeat(2000)));
        let brief = failure_in_brief(&page);
        assert!(brief.starts_with("HTTP 502: <html>"), "{brief}");
        assert_eq!(
            brief.chars().count(),
            "HTTP 502: ".len() + REASON_MAX_CHARS + 1
        );
        assert_eq!(
            failure_in_brief(&status(503, r#"{"message": "upstream send timed out"}"#)),
            "HTTP 503: upstream send timed out"
        );
        assert_eq!(
            failure_in_brief(&ChatError::Transport("connection\nreset".to_owned())),
            "chat request failed: connection reset"
        );
    }

    #[test]
    fn only_a_providers_answer_that_will_not_change_is_a_refusal() {
        for passing in [
            status(408, "timeout"),
            status(429, "slow down"),
            status(500, r#"{"message": "boom"}"#),
            status(502, "bad gateway"),
            ChatError::Transport("reset".to_owned()),
            ChatError::Refused("ours".to_owned()),
            ChatError::Shape("odd".to_owned()),
        ] {
            assert_eq!(SendRefusal::of(&passing, "x"), None, "{passing}");
        }
        // Unlabelled, as a provider client's own error is before it is labelled.
        let bare = ChatError::Status {
            status: 400,
            body: r#"{"message": "no"}"#.to_owned(),
        };
        assert_eq!(SendRefusal::of(&bare, "x").expect("refusal").reason, "no");
    }
}
