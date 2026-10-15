//! Standard emoji shortcodes in a Slack message's text, shown as the emoji they name.
//! `#321 slack-emoji-shortcodes`.
//!
//! Slack writes every emoji in a message's `text` as its short name between colons — `:crab:`,
//! `:slightly_smiling_face:`, `:wave::skin-tone-3:` — and its own client draws the emoji. Read as
//! they came, the page showed the colons and the words, and the voice said "slightly smiling face"
//! between two colons. So [`shortcodes`] replaces each standard shortcode with its emoji before the
//! rest of the reader ([`super::mrkdwn::to_markdown`]) sees the text, from the same table of
//! Slack's names the reactions use ([`super::reactions::standard_emoji`]): the page draws the
//! emoji, Copy text copies it, and the voice says it.
//!
//! * **Only Slack's standard names.** Any other `:name:` is a workspace's custom emoji, and stays
//!   as it is written, colons and all: drawing its image would mean fetching it from Slack.
//! * **Not inside code** — a code span, or a block between ```` ``` ```` — where Slack shows the
//!   characters as written, nor inside a `<…>` link, mention or channel token.
//! * **Not inside a word or a number.** A shortcode touching a letter, a digit or an underscore on
//!   either side is left as written, so `10:100:20`, an IPv6 address such as `fe80::a:b` and a path
//!   such as `std::io::stdin` keep their characters. A colon between two shortcodes,
//!   `:crab::crab:`, joins them, and so does a skin tone, `:+1::skin-tone-2:`.
//!
//! Only what is READ changes. A post is sent exactly as written: `chat.postMessage` is given the
//! owner's text, colons and all, and Slack draws it in its own client as it always has.

use std::borrow::Cow;

/// The longest shortcode name looked up, in bytes: Slack's longest standard name is 54.
const MAX_NAME: usize = 64;

/// Whether `byte` may be part of a standard Slack emoji name: `a`–`z`, `0`–`9`, `_`, `-` and `+`.
fn name_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'+')
}

/// Whether `character` is part of a word or a number, which a shortcode may not touch.
fn word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// `:name:` at the start of `text`, as the length of `:name` (the closing colon not counted).
fn name_at(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let length = bytes[1..]
        .iter()
        .take(MAX_NAME + 1)
        .take_while(|&&byte| name_byte(byte))
        .count();
    (length > 0 && length <= MAX_NAME && bytes.get(1 + length) == Some(&b':')).then_some(1 + length)
}

/// `:skin-tone-N:` at the start of `text`, as its length.
fn tone_at(text: &str) -> Option<usize> {
    let rest = text.strip_prefix(":skin-tone-")?.as_bytes();
    (rest.len() >= 2 && rest[0].is_ascii_digit() && rest[1] == b':')
        .then_some(":skin-tone-".len() + 2)
}

/// `text` with every standard emoji shortcode outside code, tokens and words replaced by its emoji.
/// See the module note.
#[must_use]
pub(super) fn shortcodes(text: &str) -> Cow<'_, str> {
    if !text.contains(':') {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut at = 0;
    // Where the last shortcode, standard or custom, ended: a colon there may open the next one.
    let mut shortcode_end = usize::MAX;
    while at < bytes.len() {
        match bytes[at] {
            b'`' if text[at..].starts_with("```") => {
                at = text[at + 3..]
                    .find("```")
                    .map_or(at + 3, |close| at + 3 + close + 3);
            }
            b'`' => {
                let line_end = text[at + 1..].find('\n').map_or(text.len(), |n| at + 1 + n);
                at = text[at + 1..line_end]
                    .find('`')
                    .map_or(at + 1, |close| at + 1 + close + 1);
            }
            b'<' => {
                // A token as `to_markdown` reads one: up to the next `>`, unless a `<` comes first.
                at = match text[at + 1..].find(['>', '<']) {
                    Some(close) if bytes[at + 1 + close] == b'>' => at + 1 + close + 1,
                    _ => at + 1,
                };
            }
            b':' => {
                let Some(name_length) = name_at(&text[at..]) else {
                    at += 1;
                    continue;
                };
                let opens = match text[..at].chars().next_back() {
                    None => true,
                    Some(':') => shortcode_end == at,
                    Some(character) => !word(character),
                };
                let name = &text[at + 1..at + name_length];
                // Just past the name's closing colon, and past a skin tone written after it.
                let mut end = at + name_length + 1;
                let tone = tone_at(&text[end..]).map(|length| {
                    let tone = &text[end + 1..end + length - 1];
                    end += length;
                    tone
                });
                let closes = text[end..].chars().next().is_none_or(|next| !word(next));
                let emoji = if opens && closes {
                    match tone {
                        Some(tone) => super::reactions::standard_emoji(&format!("{name}::{tone}")),
                        None => super::reactions::standard_emoji(name),
                    }
                } else {
                    None
                };
                if let Some(emoji) = emoji {
                    out.push_str(&text[copied..at]);
                    out.push_str(&emoji);
                    copied = end;
                    at = end;
                    shortcode_end = end;
                } else if tone.is_some() {
                    // A custom emoji with a tone, or one in a word: left as written, tone and all.
                    if opens && closes {
                        shortcode_end = end;
                    }
                    at = end;
                } else {
                    // A custom emoji, or a name in a word: left as written. A custom emoji's closing
                    // colon may still open the next shortcode, `:partyblob::crab:`, so it is looked at
                    // again, as the colon before a name it is not.
                    if opens && closes {
                        shortcode_end = end;
                    }
                    at = end - 1;
                }
            }
            _ => at += 1,
        }
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::shortcodes;

    #[test]
    fn standard_shortcodes_become_the_emoji_they_name() {
        for (slack, shown) in [
            (":crab:", "🦀"),
            ("thanks :slightly_smiling_face:", "thanks \u{1F642}"),
            (":+1: merged", "👍 merged"),
            (":white_check_mark: done, :tada:!", "✅ done, 🎉!"),
            ("(:eyes:)", "(👀)"),
            ("line one\n:rocket: line two", "line one\n🚀 line two"),
            (":crab::crab:", "🦀🦀"),
            (":heavy_check_mark:", "\u{2714}\u{FE0F}"),
            (":flag-us: :hash:", "🇺🇸 #\u{FE0F}\u{20E3}"),
        ] {
            assert_eq!(shortcodes(slack), shown, "{slack:?}");
        }
    }

    #[test]
    fn a_skin_tone_is_applied_where_slack_writes_one() {
        assert_eq!(shortcodes(":wave::skin-tone-3:"), "👋🏼");
        assert_eq!(shortcodes(":+1::skin-tone-2: ok"), "👍🏻 ok");
        assert_eq!(shortcodes(":+1::skin-tone-6::+1:"), "👍🏿👍");
        // A tone on an emoji that takes none, or one Slack does not have: the emoji as it is.
        assert_eq!(shortcodes(":eyes::skin-tone-4:"), "👀");
        assert_eq!(shortcodes(":wave::skin-tone-9:"), "👋");
    }

    #[test]
    fn a_custom_emoji_stays_as_written() {
        assert_eq!(
            shortcodes(":party-parrot: shipped"),
            ":party-parrot: shipped"
        );
        assert_eq!(shortcodes(":simple_smile:"), ":simple_smile:");
        // Beside a standard one, which still becomes its emoji.
        assert_eq!(shortcodes(":party-parrot::crab:"), ":party-parrot:🦀");
        assert_eq!(shortcodes(":crab::party-parrot:"), "🦀:party-parrot:");
        assert_eq!(
            shortcodes(":partyblob::skin-tone-2: hi"),
            ":partyblob::skin-tone-2: hi",
            "a custom emoji's tone became an emoji of its own"
        );
        // Names Slack never uses are not names at all.
        assert_eq!(shortcodes(":Crab: :cr ab: ::"), ":Crab: :cr ab: ::");
    }

    #[test]
    fn code_and_tokens_are_left_as_written() {
        for text in [
            "`:crab:`",
            "run `echo :crab:` now",
            "```\n:crab:\n```",
            "see ```:tada:\n:tada:``` above",
            "<https://example.com/:crab:/x|a link>",
            "<#C123|:crab:>",
        ] {
            assert_eq!(shortcodes(text), text, "{text:?}");
        }
        // Outside the code, on the same line, a shortcode is still read.
        assert_eq!(shortcodes("`code` :crab:"), "`code` 🦀");
        assert_eq!(shortcodes("```a``` :crab:"), "```a``` 🦀");
        assert_eq!(shortcodes("<@U1> :crab:"), "<@U1> 🦀");
        // An unmatched backtick is a character, as Slack shows it.
        assert_eq!(shortcodes("it`s :crab:"), "it`s 🦀");
        assert_eq!(shortcodes("a ``` b :crab:"), "a ``` b 🦀");
    }

    #[test]
    fn a_shortcode_inside_a_word_or_a_number_is_left_as_written() {
        for text in [
            "10:100:20",
            "fe80::a:b",
            "2001:db8:a:b::1",
            "x::id::y",
            "std::io::stdin",
            "ratio:ok:",
            ":ok:go",
            "done:tada:",
            "_:crab:",
        ] {
            assert_eq!(shortcodes(text), text, "{text:?}");
        }
    }

    #[test]
    fn text_without_a_shortcode_is_not_copied() {
        assert!(matches!(
            shortcodes("no colons here"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            shortcodes("a: b: :custom: 10:30"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_line_of_colons_is_read_once() {
        for text in [
            ":".repeat(64 * 1024),
            ":a".repeat(32 * 1024),
            format!(":{}", "a".repeat(64 * 1024)),
            "`:".repeat(32 * 1024),
            "<:".repeat(32 * 1024),
        ] {
            let started = std::time::Instant::now();
            let _ = shortcodes(&text);
            let took = started.elapsed();
            assert!(
                took < std::time::Duration::from_secs(2),
                "64 KiB took {took:?}"
            );
        }
    }
}
