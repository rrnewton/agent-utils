//! [`crate::chat::ChatClient::fetch_messages`] for Slack: a run of timestamps a call.
//! remote-ops-audit W11 `#287 bulk-reads`.
//!
//! A Slack message id is its `ts`, an instant, so the ids asked for can be read in runs. Sorted,
//! one call reads the stretch from the first id still unresolved to the last, pinned with
//! `oldest`, `latest` and `inclusive`, from the end its feed starts at: history newest first, a
//! thread's replies oldest first. Slack answers every message of a pinned stretch in order, so the
//! part of the stretch one page covered was read without a gap, and every id inside that part is
//! either on the page or not in that feed at all. The next call starts at the first id the page
//! did not reach.
//!
//! Ids within a page of each other cost one call between them, and ids further apart one each:
//! never more calls than ids, never a row outside the stretch the ids span, and an id proved absent
//! only by a page that covered it. A message the page has but this server would not show there (a
//! system event among the channel's own messages, a reply of another thread, one with no author)
//! is [`Lookup::Unseen`], so the caller looks it up as it did before and gets the same answer.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::client::{HttpSlackClient, Raw};
use super::history::{Feed, SCAN_PAGE_LIMIT};
use super::ids;
use crate::chat::{ChatError, Lookup};
use crate::model::{ChannelId, MessageId};

impl HttpSlackClient {
    /// [`crate::chat::ChatClient::fetch_messages`], without the provider label.
    pub(super) async fn lookup(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        wanted: &[MessageId],
    ) -> Result<Vec<Lookup>, ChatError> {
        let target = Self::channel_ref(channel)?;
        // In a thread, its replies. Otherwise what `fetch_page` reads: the conversation's history,
        // or the replies of the thread a thread-scoped channel is narrowed to.
        let scoped = match thread {
            Some(thread) => Some(self.member_thread(&target, thread)?),
            None => target.thread_ts.clone(),
        };
        let feed = match scoped.as_deref() {
            Some(ts) => Feed::Replies(&target.conversation, ts),
            None => Feed::History(&target.conversation),
        };
        let newest_first = matches!(feed, Feed::History(_));
        let mut pending: BTreeSet<i64> = wanted.iter().filter_map(ids::message_id_micros).collect();
        let mut found: HashMap<i64, Raw> = HashMap::new();
        let mut absent: HashSet<i64> = HashSet::new();
        let mut calls = 0;
        while let (Some(&first), Some(&last)) = (pending.first(), pending.last()) {
            // Each call resolves at least the id at the end it reads from; this only bounds a
            // server that answers otherwise.
            if calls == wanted.len() {
                break;
            }
            calls += 1;
            // A microsecond past each end, so a server that ignores `inclusive` answers both.
            let page = self
                .page(
                    feed,
                    Some(first - 1),
                    Some(last + 1),
                    true,
                    SCAN_PAGE_LIMIT,
                    None,
                )
                .await?;
            let reached = page.raws.iter().map(|raw| raw.micros);
            let covered = if !page.has_more {
                Some((first, last))
            } else if newest_first {
                reached.min().map(|low| (low, last))
            } else {
                reached.max().map(|high| (first, high))
            };
            // A page that says there is more, yet reached no id: nothing is proved, and asking
            // again would ask the same question.
            let Some((low, high)) = covered.filter(|(low, high)| low <= high) else {
                break;
            };
            let inside: Vec<i64> = pending.range(low..=high).copied().collect();
            for at in inside {
                pending.remove(&at);
                match page.raws.iter().find(|raw| raw.micros == at) {
                    Some(raw) => {
                        found.insert(at, raw.clone());
                    }
                    None => {
                        absent.insert(at);
                    }
                }
            }
        }
        // What this server would show where it looked: a thread's own messages; the channel's
        // real messages, as `fetch_page` keeps them.
        let shown: Vec<Raw> = found
            .into_values()
            .filter(|raw| match (thread, scoped.as_deref()) {
                (Some(_), Some(scope)) => {
                    raw.ts == scope || raw.thread_ts().as_deref() == Some(scope)
                }
                _ => raw.is_real(),
            })
            .collect();
        let messages = self.messages(channel, &shown).await;
        Ok(wanted
            .iter()
            .map(|id| {
                let Some(at) = ids::message_id_micros(id) else {
                    return Lookup::Unseen;
                };
                if absent.contains(&at) {
                    return Lookup::Absent;
                }
                messages
                    .iter()
                    .find(|message| message.id == *id)
                    .map_or(Lookup::Unseen, |message| {
                        Lookup::Found(Box::new(message.clone()))
                    })
            })
            .collect())
    }
}
