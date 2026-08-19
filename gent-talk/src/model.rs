//! The message and channel types the whole server speaks in.
//!
//! These are deliberately independent of Discord's wire format: [`crate::discord`] converts.

use serde::{Deserialize, Serialize};

/// A Discord channel snowflake, as a string, exactly as Discord renders it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub String);

impl ChannelId {
    /// Borrow the underlying snowflake text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Discord message snowflake, as a string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    /// Borrow the underlying snowflake text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Numeric value of the snowflake, when it parses.
    ///
    /// Discord snowflakes sort chronologically as integers but NOT as strings once they differ in
    /// length, so ordering must go through this rather than through string comparison.
    #[must_use]
    pub fn numeric(&self) -> Option<u64> {
        self.0.parse::<u64>().ok()
    }
}

/// A Discord user snowflake, as a string.
///
/// Kept distinct from [`MessageId`] and [`ChannelId`] so the three cannot be passed for one
/// another: they are all decimal snowflakes, and a wrong one is silently plausible.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    /// Borrow the underlying snowflake text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Render the Discord mention that actually notifies this user: `<@123…>`.
    ///
    /// Discord notifies on this exact syntax and on nothing else. Writing `@display-name` is plain
    /// text — it renders as words and pings nobody, which is the failure this exists to prevent.
    #[must_use]
    pub fn mention(&self) -> String {
        format!("<@{}>", self.0)
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One message as the rest of the server sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Snowflake of this message.
    pub id: MessageId,
    /// Channel the message was read from.
    pub channel_id: ChannelId,
    /// Display name of the author, as reported by Discord.
    pub author: String,
    /// Snowflake of the author.
    ///
    /// This travels with every rendered message ON PURPOSE, and the alternative — a
    /// "look up this display name" tool — was rejected. Two reasons, in order of importance:
    ///
    /// 1. **No lookup step, so nothing to guess.** The id arrives ATTACHED to the very message
    ///    being replied to, so a caller that wants to mention its author already holds the exact
    ///    snowflake. A lookup tool would put a fuzzy name match between the agent and a real
    ///    notification, and a hallucinated id pings a stranger.
    /// 2. **It bounds who can be mentioned.** The only ids a caller ever sees are those of people
    ///    and bots that have actually spoken in an allowlisted channel. A general user-lookup tool
    ///    would let the agent ping anyone in the server, which is a strictly larger capability
    ///    than this project wants to hand a voice model.
    ///
    /// Bots have ids too and they are included: addressing another coding agent by mention is a
    /// legitimate thing to want.
    pub author_id: UserId,
    /// Whether Discord flagged the author as a bot.
    pub author_is_bot: bool,
    /// The EXACT instant, ISO-8601, as reported by Discord. Compute with this one.
    ///
    /// Unrounded on purpose: sub-second precision is ordering information, and anything that has
    /// to decide which of two messages came first needs it. This field is never spoken; see
    /// [`Message::spoken_time`] for the one that is.
    pub timestamp: String,
    /// The same instant in the operator's configured zone, already formatted for speech:
    /// `09:51:25 EDT`. READ THIS ONE ALOUD.
    ///
    /// Two fields rather than one because they answer different questions, and because a single
    /// field would force whoever renders it to choose — which is exactly the choice that produced
    /// the bug. Handed an ISO string in UTC, the assistant said "thirteen fifty-one Eastern Time":
    /// it kept the digits and relabelled them. A value that is already correct when read verbatim
    /// requires no conversion and admits no such mistake.
    ///
    /// EMPTY means "not yet stamped". The Discord layer constructs it empty because it holds no
    /// configuration and therefore does not know the operator's zone; [`crate::ops`] is the only
    /// filler. Use [`Message::spoken`] to read it, which falls back to the exact instant rather
    /// than to a blank.
    #[serde(default)]
    pub spoken_time: String,
    /// Raw message body. UNTRUSTED: written by whoever is in the channel.
    pub content: String,
}

impl Message {
    /// The time label to render: the spoken form when it has been stamped, else the raw instant.
    ///
    /// Never blank, and never invented. A message that reached a renderer unstamped is a bug in
    /// the pipeline, but the reader should still learn when it was said.
    #[must_use]
    pub fn spoken(&self) -> &str {
        if self.spoken_time.is_empty() {
            &self.timestamp
        } else {
            &self.spoken_time
        }
    }
}

/// Sort a batch of messages oldest-first, by snowflake where possible.
///
/// Messages whose id does not parse keep their relative order and sort after the parseable ones,
/// so a malformed id degrades placement instead of corrupting the whole ordering.
pub fn sort_oldest_first(messages: &mut [Message]) {
    messages.sort_by_key(|m| (m.id.numeric().is_none(), m.id.numeric().unwrap_or(0)));
}

/// A channel this server is configured to read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// Snowflake of the channel.
    pub id: ChannelId,
    /// Human label used in speech and in the web app ("deepscry lead team").
    pub label: String,
    /// Whether posting into this channel is permitted at all.
    pub writable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str) -> Message {
        Message {
            id: MessageId(id.to_owned()),
            channel_id: ChannelId("c".to_owned()),
            author: "a".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "t".to_owned(),
            spoken_time: String::new(),
            content: String::new(),
        }
    }

    #[test]
    fn snowflakes_sort_numerically_not_lexically() {
        // Lexically "1000000000000000000" < "999999999999999999"; chronologically it is later.
        let mut messages = vec![msg("1000000000000000000"), msg("999999999999999999")];
        sort_oldest_first(&mut messages);
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["999999999999999999", "1000000000000000000"],
            "sort_oldest_first must order by snowflake value, not by string"
        );
    }

    #[test]
    fn unparseable_ids_sort_last_without_dropping_messages() {
        let mut messages = vec![msg("not-a-snowflake"), msg("42"), msg("7")];
        sort_oldest_first(&mut messages);
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["7", "42", "not-a-snowflake"]
        );
    }

    #[test]
    fn a_mention_is_the_syntax_discord_actually_notifies_on() {
        // The whole point of carrying the id: "@coding_agent" is plain text and pings nobody.
        assert_eq!(
            UserId("1532416065114607829".to_owned()).mention(),
            "<@1532416065114607829>"
        );
    }

    #[test]
    fn an_unstamped_message_still_reports_a_time_rather_than_a_blank() {
        let mut message = msg("7");
        message.timestamp = "2026-08-19T13:51:25+00:00".to_owned();
        assert_eq!(message.spoken(), "2026-08-19T13:51:25+00:00");
        message.spoken_time = "09:51:25 EDT".to_owned();
        assert_eq!(
            message.spoken(),
            "09:51:25 EDT",
            "once stamped, the spoken form is what a renderer must use"
        );
    }

    #[test]
    fn numeric_rejects_non_numeric_snowflakes() {
        assert_eq!(MessageId("12".to_owned()).numeric(), Some(12));
        assert_eq!(MessageId("12x".to_owned()).numeric(), None);
    }
}
