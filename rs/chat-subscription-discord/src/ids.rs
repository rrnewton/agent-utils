//! Discord identifiers and the bridge's resource-name encoding.
//!
//! Discord identifies users, channels and messages by *snowflakes*: unsigned 64-bit integers
//! written as decimal strings. Every comparison in this crate is between parsed snowflakes,
//! never between substrings of text.
//!
//! The bridge treats message and thread identifiers as opaque strings. This crate encodes them
//! like the Slack provider does, so log lines read the same way:
//! `<channel>/messages/<message>` and `<channel>/threads/<root message>`.

use std::fmt;

/// One Discord snowflake identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Snowflake(u64);

impl Snowflake {
    /// Parse a canonical decimal snowflake: 1-20 ASCII digits, no sign, no whitespace, no
    /// leading zero (Discord never emits one), and within `u64`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > 20 || !bytes.iter().all(u8::is_ascii_digit) {
            return None;
        }
        if bytes.len() > 1 && bytes[0] == b'0' {
            return None;
        }
        text.parse::<u64>()
            .ok()
            .filter(|value| *value != 0)
            .map(Self)
    }

    /// Construct from a raw integer (tests and fixtures).
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// The raw integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Snowflake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The bridge message identifier for one Discord message.
#[must_use]
pub fn message_resource(channel: Snowflake, message: Snowflake) -> String {
    format!("{channel}/messages/{message}")
}

/// The bridge thread identifier rooted at one Discord message.
#[must_use]
pub fn thread_resource(channel: Snowflake, root: Snowflake) -> String {
    format!("{channel}/threads/{root}")
}

/// Parse `<channel>/messages/<message>` back into its parts.
#[must_use]
pub fn parse_message_resource(resource: &str) -> Option<(Snowflake, Snowflake)> {
    parse_pair(resource, "/messages/")
}

/// Parse `<channel>/threads/<root>` back into its parts.
#[must_use]
pub fn parse_thread_resource(resource: &str) -> Option<(Snowflake, Snowflake)> {
    parse_pair(resource, "/threads/")
}

fn parse_pair(resource: &str, separator: &str) -> Option<(Snowflake, Snowflake)> {
    let (channel, rest) = resource.split_once(separator)?;
    Some((Snowflake::parse(channel)?, Snowflake::parse(rest)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snowflakes_parse_strictly() {
        assert_eq!(
            Snowflake::parse("1545796614826889226"),
            Some(Snowflake::from_u64(1_545_796_614_826_889_226))
        );
        for bad in [
            "",
            " 1",
            "1 ",
            "+1",
            "-1",
            "01",
            "0",
            "1a",
            "99999999999999999999",
        ] {
            assert_eq!(Snowflake::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn resources_round_trip() {
        let channel = Snowflake::from_u64(1_566_504_459_499_217_097);
        let message = Snowflake::from_u64(42);
        assert_eq!(
            parse_message_resource(&message_resource(channel, message)),
            Some((channel, message))
        );
        assert_eq!(
            parse_thread_resource(&thread_resource(channel, message)),
            Some((channel, message))
        );
        assert_eq!(
            parse_thread_resource(&message_resource(channel, message)),
            None
        );
    }
}
