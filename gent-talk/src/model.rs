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

/// One message as the rest of the server sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Snowflake of this message.
    pub id: MessageId,
    /// Channel the message was read from.
    pub channel_id: ChannelId,
    /// Display name of the author, as reported by Discord.
    pub author: String,
    /// Whether Discord flagged the author as a bot.
    pub author_is_bot: bool,
    /// ISO-8601 timestamp string, as reported by Discord.
    pub timestamp: String,
    /// Raw message body. UNTRUSTED: written by whoever is in the channel.
    pub content: String,
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
            author_is_bot: false,
            timestamp: "t".to_owned(),
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
    fn numeric_rejects_non_numeric_snowflakes() {
        assert_eq!(MessageId("12".to_owned()).numeric(), Some(12));
        assert_eq!(MessageId("12x".to_owned()).numeric(), None);
    }
}
