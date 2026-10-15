//! A group DM named by the people in it. `#317 slack-group-dm-names`.
//!
//! Slack names a group DM after its members' handles, `mpdm-ada--grace--alan-1`, and the channel
//! browser used to show exactly that, as `Group: ada, grace, alan`. In a workspace whose handles
//! are numbers that is `Group: 123456, 234567, 345678`: the owner could not tell one group DM from
//! another, nor find one by typing a person's name into the browser's search.
//!
//! So a group DM is named by its members' display names, as the people in it see each other:
//! `Group: Ada Lovelace, Grace Hopper`. The members come from `conversations.members`, and each
//! name from `users.info` through the cache every author's name already goes through
//! (`HttpSlackClient::user`), so a person already seen as an author costs nothing. The owner
//! (`owner_user_id`) and the account this client posts as are left out: a group DM is named by
//! who ELSE is in it, as Slack's own client names it.
//!
//! The name made here is the directory entry's name, which the app takes as the channel's label
//! when the owner adds it from the browser, so the channel picker, the channel's header and speech
//! all say it too.
//!
//! # When names cannot be had
//!
//! A member whose name cannot be looked up is shown by their user id, as a DM with such a person
//! is. When no member's name is known at all — the token lacks `users:read`, or
//! `conversations.members` is refused, or a bridge does not implement it — the name falls back to
//! the handles in the conversation's own name, exactly as before, so nothing is worse than it was.
//!
//! # What it costs
//!
//! One `conversations.members` call per group DM, asking for [`GROUP_MEMBER_PAGE`] members (a
//! group DM holds at most nine people) and following the cursor of a server that answers with
//! fewer, at most [`GROUP_MEMBER_CALLS`] calls, remembered for [`GROUP_MEMBERS_TTL`] (a refusal for
//! [`GROUP_REFUSAL_TTL`]); and at most that many `users.info` lookups, each remembered as an
//! author's is. Every call here is best effort: a rate limit gives up at once and the name falls
//! back, because a name is not worth holding up the browser for.

use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use serde_json::Value;

use super::client::{HttpSlackClient, Patience};

/// The members asked of `conversations.members`, which is also the most a group DM is named from.
/// Slack lets at most nine people share a group DM, so one page holds all of them.
pub const GROUP_MEMBER_PAGE: usize = 20;
/// Most `conversations.members` calls one group DM's members may take, for a server that answers
/// with fewer members a page than were asked for.
pub const GROUP_MEMBER_CALLS: usize = 3;
/// How long a group DM's members are trusted. Slack starts a new group DM rather than adding
/// somebody to an old one, so they rarely change; an hour is the author-name cache's own period.
pub const GROUP_MEMBERS_TTL: Duration = Duration::from_secs(3600);
/// How long a refused or unreadable members list is remembered before it is asked for again.
pub const GROUP_REFUSAL_TTL: Duration = Duration::from_secs(300);
/// Most group DMs whose members are remembered at once.
const GROUP_MEMBERS_MAX: usize = 1000;

/// `mpdm-ada--grace--alan-1` as `Group: ada, grace, alan`: the handles Slack names a group DM by.
/// `None` for a name that is not a group DM's.
pub(super) fn handles_name(name: &str) -> Option<String> {
    name.strip_prefix("mpdm-")
        .and_then(|rest| rest.rsplit_once('-').map(|(members, _)| members))
        .filter(|members| !members.is_empty())
        .map(|members| {
            format!(
                "Group: {}",
                members.split("--").collect::<Vec<_>>().join(", ")
            )
        })
}

/// The member ids in one `conversations.members` answer, in the order Slack gave them, and the
/// cursor of the next page when there is one.
fn member_ids(value: &Value) -> Option<(Vec<String>, Option<String>)> {
    let members = value.get("members")?.as_array()?;
    let ids = members
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|id| super::ids::is_user_id(id))
        .map(str::to_owned)
        .take(GROUP_MEMBER_PAGE)
        .collect();
    let next = value
        .get("response_metadata")
        .and_then(|metadata| metadata.get("next_cursor"))
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_owned);
    Some((ids, next))
}

impl HttpSlackClient {
    /// Read a group DM's member ids: pages of `conversations.members` until there are no more, or
    /// [`GROUP_MEMBER_PAGE`] members, or [`GROUP_MEMBER_CALLS`] calls. A server may answer a page
    /// with fewer members than were asked for. `None` when any page is refused or unreadable,
    /// rather than a name made from some of the people in it.
    async fn read_group_members(&self, id: &str) -> Option<Vec<String>> {
        let mut members: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..GROUP_MEMBER_CALLS {
            let mut params = vec![
                ("channel", id.to_owned()),
                ("limit", GROUP_MEMBER_PAGE.to_string()),
            ];
            if let Some(cursor) = &cursor {
                params.push(("cursor", cursor.clone()));
            }
            let answer = self
                .call_with("conversations.members", &params, Patience::BestEffort)
                .await;
            let (page, next) = match answer.as_ref().map(member_ids) {
                Ok(Some(page)) => page,
                Ok(None) => {
                    tracing::debug!(
                        conversation = id,
                        "a Slack group DM's members answer had no members list; naming it by its \
                         handles"
                    );
                    return None;
                }
                Err(error) => {
                    tracing::debug!(
                        conversation = id,
                        %error,
                        "a Slack group DM's members could not be read; naming it by its handles"
                    );
                    return None;
                }
            };
            for member in page {
                if !members.contains(&member) {
                    members.push(member);
                }
            }
            match next {
                Some(next)
                    if members.len() < GROUP_MEMBER_PAGE && Some(&next) != cursor.as_ref() =>
                {
                    cursor = Some(next);
                }
                _ => break,
            }
        }
        members.truncate(GROUP_MEMBER_PAGE);
        Some(members)
    }

    /// The name a group DM is listed by: its other members' display names, or, when none of them
    /// can be named, the handles in `channel_name`, or failing that its id.
    pub(super) async fn group_name(&self, id: &str, channel_name: Option<&str>) -> String {
        let fallback = || {
            channel_name
                .map(|name| handles_name(name).unwrap_or_else(|| name.to_owned()))
                .unwrap_or_else(|| id.to_owned())
        };
        let Some(members) = self.group_members(id).await else {
            return fallback();
        };
        let owner = self.owner_id();
        let me = self.self_id();
        let others: Vec<String> = members
            .into_iter()
            .filter(|member| {
                Some(member.as_str()) != owner.as_deref() && Some(member.as_str()) != me.as_deref()
            })
            .collect();
        let lookups: Vec<_> = others.iter().map(|member| self.user(member)).collect();
        let found: Vec<Option<(String, bool)>> = futures_util::stream::iter(lookups)
            .buffered(4)
            .collect()
            .await;
        if found.iter().all(Option::is_none) {
            return fallback();
        }
        let names: Vec<String> = others
            .iter()
            .zip(found)
            .map(|(member, found)| found.map_or_else(|| member.clone(), |(name, _)| name))
            .collect();
        format!("Group: {}", names.join(", "))
    }

    /// A group DM's member ids, from memory or one `conversations.members` page; `None` when
    /// Slack would not say. A refusal is remembered for [`GROUP_REFUSAL_TTL`], as a failed name
    /// lookup is, so a token without the scope does not cost a call each time the browser opens.
    async fn group_members(&self, id: &str) -> Option<Vec<String>> {
        {
            let held = self
                .group_members
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((members, at)) = held.get(id) {
                let ttl = if members.is_some() {
                    GROUP_MEMBERS_TTL
                } else {
                    GROUP_REFUSAL_TTL
                };
                if at.elapsed() < ttl {
                    return members.clone();
                }
            }
        }
        let members = self.read_group_members(id).await;
        let mut held = self
            .group_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.len() >= GROUP_MEMBERS_MAX {
            held.retain(|_, (_, at)| at.elapsed() < GROUP_MEMBERS_TTL);
            if held.len() >= GROUP_MEMBERS_MAX {
                held.clear();
            }
        }
        held.insert(id.to_owned(), (members.clone(), Instant::now()));
        members
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{handles_name, member_ids, GROUP_MEMBER_PAGE};

    #[test]
    fn a_group_dms_own_name_lists_its_handles() {
        assert_eq!(
            handles_name("mpdm-ada--grace--alan-1").as_deref(),
            Some("Group: ada, grace, alan")
        );
        assert_eq!(
            handles_name("mpdm-123456--234567-1").as_deref(),
            Some("Group: 123456, 234567")
        );
        assert_eq!(handles_name("general"), None);
        assert_eq!(handles_name("mpdm--1"), None);
    }

    #[test]
    fn only_user_ids_are_taken_from_a_members_answer_and_never_more_than_a_page() {
        let mut many: Vec<_> = (0..40).map(|n| json!(format!("U{n:08}"))).collect();
        many.insert(1, json!("not an id"));
        many.insert(2, json!(7));
        let (members, next) = member_ids(&json!({"members": many})).expect("members");
        assert_eq!(members.len(), GROUP_MEMBER_PAGE);
        assert_eq!(members[0], "U00000000");
        assert_eq!(members[1], "U00000001");
        assert_eq!(next, None);
        let (_, next) = member_ids(
            &json!({"members": [], "response_metadata": {"next_cursor": "dXNlcjpVMDYxTkZUVDI="}}),
        )
        .expect("members");
        assert_eq!(next.as_deref(), Some("dXNlcjpVMDYxTkZUVDI="));
        assert_eq!(member_ids(&json!({"ok": true})), None);
    }
}
