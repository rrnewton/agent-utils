//! Who a Slack mention names. `#318 slack-mention-names`.
//!
//! A user mention stays `<@U0123ABCD>` in a message's `content` on purpose
//! ([`super::mrkdwn`]): vibe-talk's mention syntax is the same, so a reply that quotes one notifies
//! the same person, and what is posted is exactly what was written. But the page drew the chip as
//! the member id, `@U0123ABCD`, and speech said only "a mention", so neither the reader nor the
//! voice agent learned who was addressed — in one channel, seven messages of sixteen.
//!
//! So every read of a Slack message also says who each of its user mentions names
//! ([`crate::model::Message::mentions`]), and the page draws the chip as `@Ada Lovelace`, speech
//! says "Ada Lovelace", and a model is shown the name beside the mention. The name comes from the
//! `users.info` cache every author's name already goes through, so a person who has written in the
//! channel costs nothing, and a legacy `<@U0123|ada>` names its person by its own label. The
//! people nobody has looked up yet are looked up as authors are, at most
//! [`MENTION_LOOKUPS_MAX`] of them for one read, best effort; a mention whose name is not known
//! is drawn as its id and said as "a mention", as before.
//!
//! A channel mention Slack sends without a name, `<#C0123>` or `<#C0123|>`, is named the same
//! way, from `conversations.info`, at most [`CHANNEL_LOOKUPS_MAX`] channels for one read, each
//! remembered for [`CHANNEL_NAME_TTL`], and written into the body as `#general`, as one that
//! carries its name already is. A user group's mention always carries its handle and is drawn
//! as `@ops`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use serde_json::Value;

use super::client::{HttpSlackClient, Patience, Raw};
use crate::model::MentionName;

/// Most people one read looks up for its mentions who have not been looked up already: a read of
/// a hundred messages naming as many strangers costs this many calls at most, and the rest are
/// named by a later read.
pub const MENTION_LOOKUPS_MAX: usize = 25;
/// Most channels one read looks up for its mentions that carry no name.
pub const CHANNEL_LOOKUPS_MAX: usize = 10;
/// How long a mentioned channel's name is trusted.
pub const CHANNEL_NAME_TTL: Duration = Duration::from_secs(3600);
/// How long a channel that could not be named is left unasked.
const CHANNEL_FAILURE_TTL: Duration = Duration::from_secs(300);
/// Most channels whose names are remembered at once.
const CHANNEL_NAMES_MAX: usize = 1000;
/// The longest name drawn for a mention, in characters.
pub const MENTION_NAME_CHARS: usize = 80;

/// What is remembered of each mentioned channel's name: the name, or `None` when it could not be
/// had, and when that was learned.
pub(super) type ChannelNames = std::sync::Mutex<HashMap<String, (Option<String>, Instant)>>;

/// Each `<…>` token of a Slack text, without its brackets, in order. The same reading
/// [`super::mrkdwn::to_markdown`] makes: a token ends at the first `>`, and a `<` before it means
/// the first was text.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || loop {
        let open = rest.find('<')?;
        let after = &rest[open + 1..];
        match after.find(['>', '<']) {
            Some(close) if after.as_bytes()[close] == b'>' => {
                rest = &after[close + 1..];
                return Some(&after[..close]);
            }
            _ => rest = after,
        }
    })
}

/// The people a Slack text mentions, `<@U…>` or a legacy `<@U…|label>`, each once, in the order
/// first mentioned, with the label a legacy mention carries.
pub(super) fn user_mentions(text: &str) -> Vec<(&str, Option<&str>)> {
    let mut seen = HashSet::new();
    tokens(text)
        .filter_map(|token| {
            let (target, label) = match token.split_once('|') {
                Some((target, label)) => (target, Some(label.trim()).filter(|l| !l.is_empty())),
                None => (token, None),
            };
            let id = target.strip_prefix('@')?;
            super::ids::is_user_id(id).then_some((id, label))
        })
        .filter(|(id, _)| seen.insert(*id))
        .collect()
}

/// The channels a Slack text mentions without naming them, `<#C…>` or `<#C…|>`, each once.
pub(super) fn unnamed_channels(text: &str) -> Vec<&str> {
    let mut seen = HashSet::new();
    tokens(text)
        .filter_map(|token| {
            let (target, label) = token.split_once('|').unwrap_or((token, ""));
            let id = target.strip_prefix('#')?;
            (label.trim().is_empty() && super::ids::is_conversation_id(id)).then_some(id)
        })
        .filter(|id| seen.insert(*id))
        .collect()
}

/// A name as a mention may be drawn and said: one line, no angle brackets or backticks that
/// could be read as markup, and at most [`MENTION_NAME_CHARS`] characters. `None` when nothing is
/// left. UNTRUSTED still: whoever owns the account chose it.
#[must_use]
pub fn clean_name(name: &str) -> Option<String> {
    let flat: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | '`') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    let joined = words.join(" ");
    let bounded: String = joined.chars().take(MENTION_NAME_CHARS).collect();
    let bounded = bounded.trim().to_owned();
    (!bounded.is_empty()).then_some(bounded)
}

/// Who each user mention of `text` names, from a legacy mention's own label or from `resolved`,
/// in the order first mentioned. A mention named by neither is left out: it is drawn as its id.
pub(super) fn names_for(
    text: &str,
    resolved: &HashMap<String, (String, bool)>,
) -> Vec<MentionName> {
    user_mentions(text)
        .into_iter()
        .filter_map(|(id, label)| {
            let name = resolved
                .get(id)
                .map(|(name, _)| name.as_str())
                .or(label.map(|label| label.trim_start_matches('@')))
                .and_then(clean_name)?;
            Some(MentionName {
                id: id.to_owned(),
                name,
            })
        })
        .collect()
}

impl HttpSlackClient {
    /// The people the user mentions of `raws` name who still need looking up, besides `authors`:
    /// every one already in the name cache, and at most [`MENTION_LOOKUPS_MAX`] who are not.
    pub(super) fn mentions_to_look_up<'r>(
        &self,
        raws: &'r [Raw],
        authors: &[&str],
    ) -> Vec<&'r str> {
        let mut seen: HashSet<&str> = authors.iter().copied().collect();
        let mut held = Vec::new();
        let mut fresh = Vec::new();
        for raw in raws {
            let text = raw.value.get("text").and_then(Value::as_str).unwrap_or("");
            for (id, label) in user_mentions(text) {
                if label.is_some() || !seen.insert(id) {
                    continue;
                }
                if self.knows_user(id) {
                    held.push(id);
                } else if fresh.len() < MENTION_LOOKUPS_MAX {
                    fresh.push(id);
                }
            }
        }
        held.extend(fresh);
        held
    }

    /// The names of the channels `raws` mention without naming, as far as they can be had: from
    /// memory, or from `conversations.info` for at most [`CHANNEL_LOOKUPS_MAX`] channels not
    /// asked about lately. Best effort: a channel that cannot be named keeps its id.
    pub(super) async fn mentioned_channels(&self, raws: &[Raw]) -> HashMap<String, String> {
        let mut wanted: Vec<&str> = Vec::new();
        for raw in raws {
            let text = raw.value.get("text").and_then(Value::as_str).unwrap_or("");
            for id in unnamed_channels(text) {
                if !wanted.contains(&id) {
                    wanted.push(id);
                }
            }
        }
        if wanted.is_empty() {
            return HashMap::new();
        }
        let mut names = HashMap::new();
        let mut ask = Vec::new();
        {
            let held = self
                .channel_names
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for id in wanted {
                match held.get(id) {
                    Some((name, at))
                        if at.elapsed()
                            < if name.is_some() {
                                CHANNEL_NAME_TTL
                            } else {
                                CHANNEL_FAILURE_TTL
                            } =>
                    {
                        if let Some(name) = name {
                            names.insert(id.to_owned(), name.clone());
                        }
                    }
                    _ if ask.len() < CHANNEL_LOOKUPS_MAX => ask.push(id.to_owned()),
                    _ => {}
                }
            }
        }
        let lookups: Vec<_> = ask
            .iter()
            .map(|id| async move { (id.clone(), self.channel_name(id).await) })
            .collect();
        let found: Vec<(String, Option<String>)> = futures_util::stream::iter(lookups)
            .buffered(4)
            .collect()
            .await;
        let mut held = self
            .channel_names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.len() + found.len() > CHANNEL_NAMES_MAX {
            held.retain(|_, (_, at)| at.elapsed() < CHANNEL_NAME_TTL);
            if held.len() + found.len() > CHANNEL_NAMES_MAX {
                held.clear();
            }
        }
        for (id, name) in found {
            if let Some(name) = &name {
                names.insert(id.clone(), name.clone());
            }
            held.insert(id, (name, Instant::now()));
        }
        names
    }

    /// One channel's name from `conversations.info`, best effort.
    async fn channel_name(&self, id: &str) -> Option<String> {
        let answer = self
            .call_with(
                "conversations.info",
                &[("channel", id.to_owned())],
                Patience::BestEffort,
            )
            .await;
        match answer {
            Ok(value) => {
                let channel = value.get("channel")?;
                if channel.get("id").and_then(Value::as_str) != Some(id) {
                    return None;
                }
                channel
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(clean_name)
            }
            Err(error) => {
                tracing::debug!(channel = id, %error, "a mentioned Slack channel could not be named");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{clean_name, names_for, unnamed_channels, user_mentions, MENTION_NAME_CHARS};
    use crate::model::MentionName;

    #[test]
    fn the_people_a_text_mentions_are_read_once_each_in_order() {
        assert_eq!(
            user_mentions("<@U0ADA0001> and <@W0GRACE01|grace>, <@U0ADA0001> again"),
            [("U0ADA0001", None), ("W0GRACE01", Some("grace"))]
        );
        // Not a person: a channel, a group, a link, a bot's B id, an unclosed bracket, text.
        assert!(user_mentions(
            "<#C0123ABCD|general> <!subteam^S01|@ops> <https://x.test|x> <@B0123> <@U01 a < b"
        )
        .is_empty());
        assert_eq!(user_mentions("<@U01|>"), [("U01", None)]);
    }

    #[test]
    fn only_a_channel_mention_without_a_name_needs_one() {
        assert_eq!(
            unnamed_channels("<#C0123ABCD> <#C0456EFGH|> <#C0789IJKL|named> <#C0123ABCD>"),
            ["C0123ABCD", "C0456EFGH"]
        );
        assert!(unnamed_channels("<#nope> <@U01>").is_empty());
    }

    #[test]
    fn a_name_is_one_line_with_nothing_to_read_as_markup() {
        assert_eq!(
            clean_name("  Ada\n<b>Lovelace</b> `x`  ").as_deref(),
            Some("Ada b Lovelace /b x")
        );
        assert_eq!(clean_name(" \t "), None);
        assert_eq!(
            clean_name(&"a".repeat(500)).map(|name| name.chars().count()),
            Some(MENTION_NAME_CHARS)
        );
    }

    #[test]
    fn a_mention_is_named_by_the_cache_then_by_its_own_label_and_otherwise_not_at_all() {
        let resolved: HashMap<String, (String, bool)> = [
            ("U0ADA0001".to_owned(), ("Ada Lovelace".to_owned(), false)),
            ("U0GRACE01".to_owned(), ("Grace Hopper".to_owned(), false)),
        ]
        .into();
        assert_eq!(
            names_for(
                "<@U0ADA0001> <@U0GRACE01|grace> <@W0ALAN001|@alan> <@U0GHOST01>",
                &resolved
            ),
            [
                MentionName {
                    id: "U0ADA0001".to_owned(),
                    name: "Ada Lovelace".to_owned()
                },
                MentionName {
                    id: "U0GRACE01".to_owned(),
                    name: "Grace Hopper".to_owned()
                },
                MentionName {
                    id: "W0ALAN001".to_owned(),
                    name: "alan".to_owned()
                },
            ]
        );
    }
}
