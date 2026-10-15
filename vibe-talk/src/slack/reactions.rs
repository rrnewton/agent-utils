//! The reactions Slack reports on a message, as the provider-neutral tallies the page draws.
//! `#312 slack-reactions`.
//!
//! `conversations.history` and `conversations.replies` list each message's reactions as
//! `reactions: [{name, count, users}]`, and every read of a message goes through
//! [`super::client::Raw::to_message`], so history, Main, Flat, a thread, a lookup and the live
//! poller all carry them alike.
//!
//! * **The emoji.** Slack names a reaction by its emoji's short name, never by the emoji itself:
//!   `+1`, `eyes`, `white_check_mark`, and `+1::skin-tone-2` with a skin tone. A standard name is
//!   shown as its emoji, from the table `emoji_names.tsv` holds (Slack's own names, from iamcal's
//!   emoji-data; see `vibe-talk/scripts/slack_emoji_names.py`). Any other name is a workspace's
//!   custom emoji: it is kept as a custom emoji's name, which the page draws as `:name:`, since
//!   drawing its image would mean fetching it from Slack.
//! * **Whether the owner reacted**, from `users` holding the deployment's `owner_user_id`. Slack
//!   may cut that list short of `count` on a message many reacted to, so it is "the owner is among
//!   the users Slack listed": never true when the owner did not react, and false when no owner id
//!   is configured.
//! * **Absent is not reported, not none.** Slack omits `reactions` on a message nobody reacted
//!   to, and a compatible bridge that cannot supply them omits it on every message; the two cannot
//!   be told apart, so an absent field is [`None`], as it was before reactions were read, and a
//!   live copy without it does not take away chips a read brought; see
//!   [`crate::model::Message::reactions`].

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::discord::http::{MAX_REACTIONS, MAX_REACTION_CHARS};
use crate::model::Reaction;

/// Slack's standard emoji names: `name<TAB>code points[<TAB>s]`, generated, with a header of
/// `#` lines naming its source and licence.
const NAMES: &str = include_str!("emoji_names.tsv");

/// Slack's `skin-tone-2` to `skin-tone-6`: the Fitzpatrick modifiers U+1F3FB to U+1F3FF.
const TONE_MODIFIERS: [char; 5] = [
    '\u{1F3FB}',
    '\u{1F3FC}',
    '\u{1F3FD}',
    '\u{1F3FE}',
    '\u{1F3FF}',
];

/// One standard name: its emoji's code points, and whether a skin tone applies to it.
#[derive(Clone, Copy, Debug)]
struct Standard {
    unified: &'static str,
    tones: bool,
}

/// The table, parsed once.
fn names() -> &'static HashMap<&'static str, Standard> {
    static PARSED: OnceLock<HashMap<&'static str, Standard>> = OnceLock::new();
    PARSED.get_or_init(|| {
        NAMES
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let name = fields.next()?;
                let unified = fields.next()?;
                let tones = fields.next() == Some("s");
                Some((name, Standard { unified, tones }))
            })
            .collect()
    })
}

/// The characters `unified` names: hex code points joined by `-`. `None` for anything else.
fn characters(unified: &str) -> Option<Vec<char>> {
    unified
        .split('-')
        .map(|point| u32::from_str_radix(point, 16).ok().and_then(char::from_u32))
        .collect()
}

/// The emoji a Slack reaction name stands for, when it is a standard name: `+1` is 👍, and
/// `+1::skin-tone-2` is 👍🏻. `None` for a custom emoji's name.
///
/// One tone is applied as the generator checked it against the data: its modifier after the first
/// code point, dropping a U+FE0F there. A tone on an emoji that takes none, an unknown tone, or two
/// tones (a pair of people) leaves the emoji as it is: the right emoji in the default tone is a
/// better answer than none.
#[must_use]
pub(super) fn standard_emoji(name: &str) -> Option<String> {
    let mut parts = name.split("::");
    let base = parts.next()?;
    let standard = names().get(base)?;
    let mut chars = characters(standard.unified)?;
    let tones: Vec<&str> = parts.collect();
    let tone = match tones.as_slice() {
        [one] => one
            .strip_prefix("skin-tone-")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(2))
            .and_then(|at| TONE_MODIFIERS.get(at)),
        _ => None,
    };
    if let Some(&modifier) = tone.filter(|_| standard.tones) {
        if chars.get(1) == Some(&'\u{FE0F}') {
            chars.remove(1);
        }
        chars.insert(1, modifier);
    }
    Some(chars.into_iter().collect())
}

/// The tallies of a Slack message's `reactions` field, for an owner whose Slack id is `owner`.
///
/// Absent, null, or anything that is not an array is [`None`]: this copy says nothing about
/// reactions. Lenient inside the array, as the Discord reading is, because no reaction is worth
/// failing a read for: an entry without a name, or with a count of none, is skipped, and a missing
/// `count` is the length of `users`. Slack's order is kept, cut to [`MAX_REACTIONS`], and a name to
/// [`MAX_REACTION_CHARS`] characters.
#[must_use]
pub(super) fn parse(value: Option<&Value>, owner: Option<&str>) -> Option<Vec<Reaction>> {
    let items = value?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|item| one(item, owner))
            .take(MAX_REACTIONS)
            .collect(),
    )
}

fn one(item: &Value, owner: Option<&str>) -> Option<Reaction> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(|name| name.trim().trim_matches(':'))
        .filter(|name| !name.is_empty())?;
    let users = item.get("users").and_then(Value::as_array);
    let count = item
        .get("count")
        .and_then(Value::as_u64)
        .or_else(|| users.map(|users| users.len() as u64))
        .filter(|count| *count > 0)?;
    let owner_reacted = owner.is_some_and(|owner| {
        users.is_some_and(|users| users.iter().any(|user| user.as_str() == Some(owner)))
    });
    let (emoji, custom) = match standard_emoji(name) {
        Some(emoji) => (emoji, false),
        None => (name.chars().take(MAX_REACTION_CHARS).collect(), true),
    };
    Some(Reaction {
        emoji,
        custom,
        // A Slack custom emoji is known by its name alone; there is no other id to keep apart two
        // of one name, and two of one name cannot exist in one workspace.
        custom_id: None,
        count: u32::try_from(count).unwrap_or(u32::MAX),
        owner_reacted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shown(reactions: &[Reaction]) -> Vec<(&str, bool, u32, bool)> {
        reactions
            .iter()
            .map(|r| (r.emoji.as_str(), r.custom, r.count, r.owner_reacted))
            .collect()
    }

    #[test]
    fn slack_names_become_the_emoji_they_name() {
        for (name, emoji) in [
            ("+1", "👍"),
            ("thumbsup", "👍"),
            ("eyes", "👀"),
            ("white_check_mark", "✅"),
            ("heavy_check_mark", "\u{2714}\u{FE0F}"),
            ("thinking_face", "🤔"),
            ("face_with_rolling_eyes", "🙄"),
            ("tada", "🎉"),
            ("100", "💯"),
            ("flag-us", "🇺🇸"),
            ("hash", "#\u{FE0F}\u{20E3}"),
            ("woman-raising-hand", "\u{1F64B}\u{200D}\u{2640}\u{FE0F}"),
        ] {
            assert_eq!(standard_emoji(name).as_deref(), Some(emoji), "{name}");
        }
    }

    #[test]
    fn a_skin_tone_is_applied_where_slack_names_one() {
        assert_eq!(standard_emoji("+1::skin-tone-2").as_deref(), Some("👍🏻"));
        assert_eq!(standard_emoji("wave::skin-tone-6").as_deref(), Some("👋🏿"));
        // Inside a sequence the tone follows the first code point, and replaces a U+FE0F there.
        assert_eq!(
            standard_emoji("woman-raising-hand::skin-tone-3").as_deref(),
            Some("\u{1F64B}\u{1F3FC}\u{200D}\u{2640}\u{FE0F}")
        );
        assert_eq!(
            standard_emoji("v::skin-tone-4").as_deref(),
            Some("\u{270C}\u{1F3FD}")
        );
        // A tone on an emoji that takes none, an unknown tone, or two: the emoji as it is.
        assert_eq!(standard_emoji("eyes::skin-tone-2").as_deref(), Some("👀"));
        assert_eq!(standard_emoji("+1::skin-tone-9").as_deref(), Some("👍"));
        assert_eq!(standard_emoji("+1::skin-tone-1").as_deref(), Some("👍"));
        assert_eq!(
            standard_emoji("+1::skin-tone-2::skin-tone-3").as_deref(),
            Some("👍")
        );
    }

    #[test]
    fn a_name_slack_does_not_have_is_a_custom_emoji() {
        assert_eq!(standard_emoji("party-parrot"), None);
        assert_eq!(standard_emoji("simple_smile"), None);
        assert_eq!(standard_emoji(""), None);
        assert_eq!(standard_emoji("::skin-tone-2"), None);
    }

    #[test]
    fn every_line_of_the_table_names_an_emoji() {
        let table = names();
        assert!(table.len() > 1900, "{} names", table.len());
        for (name, standard) in table {
            assert!(
                characters(standard.unified).is_some_and(|chars| !chars.is_empty()),
                "{name} names no characters: {}",
                standard.unified
            );
        }
    }

    #[test]
    fn reactions_are_read_with_counts_and_whether_the_owner_is_among_them() {
        let value = json!([
            {"name": "eyes", "count": 2, "users": ["UADA00001", "UOWNER001"]},
            {"name": "white_check_mark", "count": 1, "users": ["UADA00001"]},
            {"name": "party-parrot", "count": 3, "users": ["UOWNER001"]},
            {"name": "+1::skin-tone-2", "count": 1, "users": ["UBOB00001"]},
        ]);
        let reactions = parse(Some(&value), Some("UOWNER001")).expect("an array is an answer");
        assert_eq!(
            shown(&reactions),
            vec![
                ("👀", false, 2, true),
                ("✅", false, 1, false),
                ("party-parrot", true, 3, true),
                ("👍🏻", false, 1, false),
            ]
        );
        assert!(reactions.iter().all(|r| r.custom_id.is_none()));
        // With no owner configured, nobody is the owner.
        let unknown = parse(Some(&value), None).expect("an array is an answer");
        assert!(unknown.iter().all(|r| !r.owner_reacted), "{unknown:?}");
    }

    #[test]
    fn an_absent_field_is_not_reported_and_an_empty_one_is_none() {
        assert_eq!(parse(None, Some("UOWNER001")), None);
        assert_eq!(parse(Some(&Value::Null), Some("UOWNER001")), None);
        assert_eq!(parse(Some(&json!({"eyes": 1})), Some("UOWNER001")), None);
        assert_eq!(parse(Some(&json!([])), Some("UOWNER001")), Some(Vec::new()));
    }

    #[test]
    fn a_malformed_entry_is_skipped_rather_than_failing_the_read() {
        let value = json!([
            {"count": 1, "users": ["U1"]},
            {"name": "", "count": 1},
            {"name": "eyes", "count": 0, "users": []},
            {"name": "eyes"},
            {"name": "tada", "users": ["U1", "UOWNER001"]},
            {"name": ":fire:", "count": 1, "users": "UOWNER001"},
            {"name": "rocket", "count": 5_000_000_000_u64},
            "eyes",
        ]);
        let reactions = parse(Some(&value), Some("UOWNER001")).expect("an array is an answer");
        assert_eq!(
            shown(&reactions),
            vec![
                ("🎉", false, 2, true),
                ("🔥", false, 1, false),
                ("🚀", false, u32::MAX, false),
            ]
        );
    }

    #[test]
    fn reactions_are_bounded_in_number_and_length() {
        let many: Vec<Value> = (0..MAX_REACTIONS + 10)
            .map(|i| json!({"name": format!("custom-{i}"), "count": 1}))
            .collect();
        let reactions = parse(Some(&Value::Array(many)), None).expect("an array is an answer");
        assert_eq!(reactions.len(), MAX_REACTIONS);
        assert_eq!(reactions[0].emoji, "custom-0", "Slack's order is kept");
        let long = json!([{"name": "x".repeat(MAX_REACTION_CHARS * 3), "count": 1}]);
        let reactions = parse(Some(&long), None).expect("an array is an answer");
        assert_eq!(reactions[0].emoji.chars().count(), MAX_REACTION_CHARS);
    }
}
