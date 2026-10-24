//! Slack identifiers and the bridge resource names built from them.
//!
//! The bridge identifies a message, a thread and a reaction by provider-neutral resource strings
//! and checks that a send receipt is `<channel>/messages/<id>` and a reaction receipt is
//! `<message>/reactions/<id>`. This provider therefore uses:
//!
//! | Bridge field | Slack form |
//! | --- | --- |
//! | channel | `C0123456789` (the conversation ID itself) |
//! | message | `C0123456789/messages/1700000000.000100` (channel and message `ts`) |
//! | thread | `C0123456789/threads/1700000000.000100` (channel and the thread root's `ts`) |
//! | reaction | `C0123456789/messages/1700000000.000100/reactions/robot_face` |
//! | sender | the Slack user ID, `U…` or `W…` |

use std::fmt;

/// A Slack conversation ID: `C`, `G` or `D` followed by uppercase letters and digits.
#[must_use]
pub fn valid_channel_id(value: &str) -> bool {
    slack_id(value, b"CGD")
}

/// A Slack user ID as it appears on a message: `U` or `W` followed by uppercase letters/digits.
#[must_use]
pub fn valid_user_id(value: &str) -> bool {
    slack_id(value, b"UW")
}

/// A Slack file ID: `F` followed by uppercase letters and digits.
#[must_use]
pub fn valid_file_id(value: &str) -> bool {
    slack_id(value, b"F")
}

/// A bot or user ID used to recognise this app's own posts: `U`, `W` or `B` prefixed.
#[must_use]
pub fn valid_author_id(value: &str) -> bool {
    slack_id(value, b"UWB")
}

fn slack_id(value: &str, prefixes: &[u8]) -> bool {
    let bytes = value.as_bytes();
    (9..=24).contains(&bytes.len())
        && prefixes.contains(&bytes[0])
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// A Slack message timestamp: 1 to 12 digits, a dot, and exactly six digits.
#[must_use]
pub fn valid_ts(value: &str) -> bool {
    let Some((seconds, micros)) = value.split_once('.') else {
        return false;
    };
    (1..=12).contains(&seconds.len())
        && micros.len() == 6
        && seconds.bytes().all(|byte| byte.is_ascii_digit())
        && micros.bytes().all(|byte| byte.is_ascii_digit())
}

/// A registered Slack app ID (`A…`). Every outbound write requires one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppId(String);

impl AppId {
    /// Validate an app ID: `A` followed by 8 to 23 uppercase letters or digits.
    ///
    /// # Errors
    ///
    /// Returns a bounded description when the value is not an app ID. A user ID (`U…`) is
    /// refused here, so a user identity cannot be configured as the posting identity.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if slack_id(&value, b"A") {
            Ok(Self(value))
        } else {
            Err("a Slack app id is `A` followed by 8 to 23 uppercase letters or digits".to_owned())
        }
    }

    /// Borrow the app ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `<channel>/messages/<ts>`.
#[must_use]
pub fn message_resource(channel_id: &str, ts: &str) -> String {
    format!("{channel_id}/messages/{ts}")
}

/// `<channel>/threads/<root ts>`.
#[must_use]
pub fn thread_resource(channel_id: &str, root_ts: &str) -> String {
    format!("{channel_id}/threads/{root_ts}")
}

/// Split `<channel>/<kind>/<ts>` into its channel and timestamp, validating both.
#[must_use]
pub fn parse_resource<'a>(value: &'a str, kind: &str) -> Option<(&'a str, &'a str)> {
    let (channel, rest) = value.split_once('/')?;
    let ts = rest.strip_prefix(kind)?.strip_prefix('/')?;
    (valid_channel_id(channel) && valid_ts(ts)).then_some((channel, ts))
}

/// Microseconds since the Unix epoch for a valid Slack timestamp.
#[must_use]
pub fn ts_micros(ts: &str) -> Option<u128> {
    if !valid_ts(ts) {
        return None;
    }
    let (seconds, micros) = ts.split_once('.')?;
    Some(seconds.parse::<u128>().ok()? * 1_000_000 + micros.parse::<u128>().ok()?)
}

/// A Slack timestamp string for a Unix time in milliseconds, for `--oldest` bounds.
#[must_use]
pub fn ts_from_millis(millis: u64) -> String {
    format!("{}.{:06}", millis / 1_000, (millis % 1_000) * 1_000)
}

/// RFC 3339 UTC rendering of a Slack timestamp, keeping its microseconds.
#[must_use]
pub fn ts_rfc3339(ts: &str) -> Option<String> {
    let micros = ts_micros(ts)?;
    let seconds = i64::try_from(micros / 1_000_000).ok()?;
    let fraction = micros % 1_000_000;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{fraction:06}Z",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60
    ))
}

// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// Undo the three escapes Slack applies to message text (`&amp;`, `&lt;`, `&gt;`).
#[must_use]
pub fn unescape_text(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The Slack reaction name for a reaction emoji the bridge sends.
///
/// The bridge sends reactions as Unicode emoji, while Slack names reactions by shortcode. A
/// shortcode, with or without colons, passes through unchanged. The bridge's built-in
/// acknowledgement, receipt and rejection map through the Slack names defined beside them in
/// [`chat_subscription::reactions`], so those defaults have one definition; a small table covers
/// other common Unicode emoji a bridge may configure. Anything else returns `None` and the
/// reaction is refused as not applied rather than guessed.
#[must_use]
pub fn reaction_name(emoji: &str) -> Option<String> {
    let trimmed = emoji.trim();
    let shortcode = trimmed.trim_matches(':');
    if !shortcode.is_empty()
        && shortcode.len() <= 64
        && shortcode.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'+' | b':')
        })
    {
        return Some(shortcode.to_owned());
    }
    let normalized = trimmed.trim_end_matches('\u{fe0f}');
    if let Some(reaction) = chat_subscription::reactions::DEFAULT_REACTIONS
        .iter()
        .find(|reaction| reaction.emoji == normalized)
    {
        return Some(reaction.slack_name.to_owned());
    }
    let name = match normalized {
        "🤖" => "robot_face",
        "✔" => "heavy_check_mark",
        "👍" => "+1",
        "👌" => "ok_hand",
        "⏳" => "hourglass_flowing_sand",
        "🎉" => "tada",
        "❌" => "x",
        "⚠" => "warning",
        "🚀" => "rocket",
        "🙏" => "pray",
        "📝" => "memo",
        "🔄" => "arrows_counterclockwise",
        _ => return None,
    };
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers() {
        assert!(valid_channel_id("C0C91RW5FAR"));
        assert!(!valid_channel_id("U0C91RW5FAR"));
        assert!(!valid_channel_id("c0c91rw5far"));
        assert!(valid_user_id("U012ABCDEF"));
        assert!(valid_ts("1700000000.000100"));
        assert!(!valid_ts("1700000000.0001"));
        assert!(AppId::new("A0BRVKV248L").is_ok());
        assert!(AppId::new("U0BRVKV248L").is_err());
        assert!(AppId::new("").is_err());
    }

    #[test]
    fn resources_round_trip() {
        let message = message_resource("C0C91RW5FAR", "1700000000.000100");
        assert_eq!(
            parse_resource(&message, "messages"),
            Some(("C0C91RW5FAR", "1700000000.000100"))
        );
        assert_eq!(parse_resource(&message, "threads"), None);
        assert_eq!(parse_resource("spaces/x/messages/y", "messages"), None);
    }

    #[test]
    fn timestamps_render_as_rfc3339() {
        assert_eq!(
            ts_rfc3339("1700000000.000100").as_deref(),
            Some("2023-11-14T22:13:20.000100Z")
        );
        assert_eq!(
            ts_rfc3339("0.000000").as_deref(),
            Some("1970-01-01T00:00:00.000000Z")
        );
        assert_eq!(
            ts_rfc3339("951782400.500000").as_deref(),
            Some("2000-02-29T00:00:00.500000Z")
        );
        assert_eq!(ts_from_millis(1_700_000_000_123), "1700000000.123000");
    }

    #[test]
    fn reactions_map_to_shortcodes() {
        assert_eq!(reaction_name("🤖").as_deref(), Some("robot_face"));
        assert_eq!(reaction_name("✅").as_deref(), Some("white_check_mark"));
        assert_eq!(reaction_name("❌").as_deref(), Some("x"));
        assert_eq!(reaction_name("👀").as_deref(), Some("eyes"));
        // An emoji presentation selector does not change the name.
        assert_eq!(
            reaction_name("✅\u{fe0f}").as_deref(),
            Some("white_check_mark")
        );
        assert_eq!(reaction_name(":eyes:").as_deref(), Some("eyes"));
        assert_eq!(reaction_name("thumbsup").as_deref(), Some("thumbsup"));
        assert_eq!(reaction_name("🦀"), None);
        assert_eq!(unescape_text("a &lt;b&gt; &amp;amp;"), "a <b> &amp;");
    }

    #[test]
    fn the_bridge_default_reactions_map_through_their_shared_slack_names() {
        for reaction in chat_subscription::reactions::DEFAULT_REACTIONS {
            assert_eq!(
                reaction_name(reaction.emoji).as_deref(),
                Some(reaction.slack_name)
            );
        }
    }
}
