//! Discord access, behind a trait so the rest of the server is testable without a live Discord.
//!
//! v0 needs exactly two operations: read the recent messages of a channel, and post one message
//! back. Both are pulled on demand — this server has no gateway connection and no webhook
//! receiver, so it never needs to be running for a message to survive.

pub mod fake;
pub mod http;
pub mod ratelimit;

use std::time::Duration;

use async_trait::async_trait;

use crate::model::{ChannelId, Message, MessageId};

/// A Discord read/post failure.
#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    /// The request never completed.
    #[error("discord request failed: {0}")]
    Transport(String),
    /// Discord answered with a non-success status.
    ///
    /// **Not 429.** A rate limit is [`DiscordError::RateLimited`] and only ever after the client
    /// has waited it out and failed; a bare 429 never reaches a caller.
    #[error("discord returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body, truncated by the caller.
        body: String,
    },
    /// Discord rate-limited the request and the wait did not clear inside the client's budget.
    ///
    /// The client obeys `Retry-After` — see [`ratelimit`] — so this is the *exhausted* case, and
    /// it is deliberately its own variant rather than an HTTP 429 in [`DiscordError::Status`]. A
    /// caller that can slow itself down needs to be able to tell "we are over quota, and here is
    /// how long is left" from "Discord answered 500", and its [`Display`](std::fmt::Display) names
    /// the rate limit so a log line does too.
    #[error("{0}")]
    RateLimited(ratelimit::RateLimitExhausted),
    /// The response did not have the shape this server expects.
    #[error("discord response could not be understood: {0}")]
    Shape(String),
    /// The server refused the operation before contacting Discord.
    #[error("refused: {0}")]
    Refused(String),
}

impl DiscordError {
    /// How long Discord still wants us to wait, when this failure is a rate limit.
    ///
    /// This is the seam that keeps the two notions of "slow down" in this crate from being two:
    /// [`crate::live`]'s per-channel backoff consults it, so the poll loop's wait can be longer
    /// than Discord asked for but never shorter. `None` for every other failure, which is the
    /// honest answer — a 500 carries no instruction about when to come back.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited(detail) => Some(detail.retry_after),
            _ => None,
        }
    }
}

/// The bot's own Discord user id, read out of its token.
///
/// A bot token is three dot-separated parts, and the FIRST is the bot's user id in base64url. So
/// the account this server posts as is knowable without a single API call, without a new trait
/// method for every client and fake to implement, and without a startup step that can fail.
///
/// # Why this matters at all
///
/// The channel view colours a message by who sent it, and the one account it must recognise is
/// this bridge's own -- those messages are the OWNER's words, posted on his behalf. The page used
/// to learn that id only as a side effect of the reader replying from the app or of the live feed
/// delivering a `self_posted` message. A reader who had done neither saw every message fall
/// through to the same "somebody else" colour, including their own, which is exactly the bug this
/// closes: with the bridge unidentified, it also counts as a second bot, and the "the only bot
/// that is not us" guess for the coding agent becomes a coin toss it declines to call.
///
/// Returns `None` rather than guessing when the token is not in that shape. Degrading to the old
/// learned-later behaviour is correct; inventing an id would mislabel somebody else's messages as
/// the owner's.
#[must_use]
pub fn self_user_id_from_token(token: &str) -> Option<String> {
    let first = token.trim().split('.').next()?;
    if first.is_empty() {
        return None;
    }
    let decoded = base64_url_decode(first)?;
    let id = String::from_utf8(decoded).ok()?;
    // A snowflake is decimal digits and nothing else. Anything else means this was not the shape
    // assumed, and a wrong id is worse than no id.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(id)
}

/// Decode unpadded base64url. Small and local: this is the only base64 this crate needs.
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    const fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0_u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let six = value(byte)?;
        buffer = (buffer << 6) | u32::from(six);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// Who Discord says this bot is, as Discord itself reports it.
///
/// Distinct from [`self_user_id_from_token`], which reads an id out of the token's own bytes
/// without asking anybody. That is free and cannot fail, and it is therefore also no evidence
/// whatsoever that the token still works: a revoked token still decodes. This struct is the
/// answer to a real call, so obtaining one is proof the credential was accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotIdentity {
    /// The bot's user snowflake. Public: it is on every message the bot has ever sent.
    pub id: String,
    /// The bot's username, so a report can say which application answered rather than only that
    /// one did. An operator with two bots needs to know they configured the token of the other.
    pub username: String,
}

/// Read and post access to Discord channels.
#[async_trait]
pub trait DiscordClient: Send + Sync {
    /// Ask Discord who this token belongs to.
    ///
    /// The smallest authenticated call there is: it names no channel, so it separates "the
    /// credential is bad" from "the credential is fine and this one channel is not reachable" —
    /// two failures that a channel read alone reports identically often enough to matter, and
    /// that have completely different fixes.
    ///
    /// Required rather than defaulted on purpose. A default would have to invent an answer, and
    /// an invented answer to "is this token valid" is the single worst thing this trait could
    /// return.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] exactly as a read does; [`crate::probe::classify`] turns it into
    /// the same vocabulary a channel failure speaks.
    async fn identity(&self) -> Result<BotIdentity, DiscordError>;

    /// Fetch one page of a channel, returned OLDEST FIRST.
    ///
    /// Discord itself returns newest-first; implementations normalize so that everything above
    /// this trait reads in conversation order.
    ///
    /// `before` and `after` are Discord's own cursors and are **mutually exclusive** — Discord
    /// does not define what sending both means, so an implementation must refuse rather than pick.
    /// Their asymmetry matters and is easy to get backwards: `before` walks BACKWARDS and yields
    /// the newest messages older than the cursor, whereas `after` yields the OLDEST messages newer
    /// than it. Inverting `after` produces a walk that looks right against a fake and skips
    /// messages against live Discord.
    ///
    /// This is the required method. [`DiscordClient::fetch_recent`] is a thin default over it, so
    /// a client cannot implement the uncursored case and silently ignore the cursor.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] when the request fails, is rejected, or cannot be parsed, and
    /// [`DiscordError::Refused`] when both cursors are supplied.
    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, DiscordError>;

    /// Fetch up to `limit` most recent messages, returned OLDEST FIRST.
    ///
    /// # Errors
    ///
    /// As [`DiscordClient::fetch_page`].
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, DiscordError> {
        self.fetch_page(channel, limit, None, None).await
    }

    /// Post `content` to `channel`, optionally as a reply to an existing message.
    ///
    /// # Errors
    ///
    /// Returns [`DiscordError`] when the post fails or is rejected.
    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, DiscordError>;
}

#[cfg(test)]
mod self_id_tests {
    use super::self_user_id_from_token;

    #[test]
    fn a_bot_token_carries_the_bots_own_user_id_in_its_first_segment() {
        // A real-shaped token: base64url("1000000000000000009") . timestamp . hmac. The id is what
        // lets the channel view tell the owner's own words from everybody else's on the first
        // render, with no API call and no waiting for him to reply.
        let token = "MTAwMDAwMDAwMDAwMDAwMDAwOQ.Gabcde.fghijklmnopqrstuvwxyz0123456789";
        assert_eq!(
            self_user_id_from_token(token).as_deref(),
            Some("1000000000000000009")
        );
    }

    #[test]
    fn anything_not_in_that_shape_is_none_rather_than_a_guess() {
        // A wrong id is worse than no id: it would paint somebody else's messages as the owner's.
        for token in [
            "",
            ".x.y",
            "not-base64!!!.x.y",
            // Decodes cleanly, but to letters rather than a snowflake.
            "aGVsbG8.x.y",
            // Decodes to something with a non-digit in the middle.
            "MTIzNDU2Nzg5YQ.x.y",
        ] {
            assert!(
                self_user_id_from_token(token).is_none(),
                "{token:?} produced an id"
            );
        }
    }

    #[test]
    fn the_dots_are_not_required_because_only_the_first_segment_is_read() {
        // Said out loud because it looks like an oversight and is not: a caller that hands over
        // just the first segment gets the same answer. Nothing here validates that a token is
        // WELL-FORMED -- that is Discord's job, and it answers 401. This only reads an id out of
        // one, and refuses when what it reads is not one.
        assert_eq!(
            self_user_id_from_token("MTAwMDAwMDAwMDAwMDAwMDAwOQ").as_deref(),
            Some("1000000000000000009")
        );
    }
}
