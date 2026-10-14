//! Several threads of one conversation in one call. remote-ops-audit W14 `#287 bulk-reads`.
//!
//! Slack has no read of several threads: `conversations.replies` reads one. What a read of
//! several saves is what the thread view spends on each thread, and what an instant makes
//! unnecessary:
//!
//! * **A thread's newest page** is one `conversations.replies` window read, whose first message is
//!   the root, where the thread view reads the root first and then the window: one call a thread
//!   of up to a page, instead of two. The answer is the thread view's own, cursors included, and
//!   it settles what Flat holds for the thread exactly as the thread view's read does
//!   (`#282 slack-flat-skip`): an edit it shows reaches Flat's next read. A thread too long for the
//!   window, or one a walk already proved a stretch of empty, is read by the thread view itself,
//!   as before.
//! * **What is newer than an instant** is a skip-list, by the rule Flat's skip-list reads a root by
//!   ([`super::timeline::root_says_replies_after`]). A root says when its latest reply was
//!   posted, so a thread whose root says nothing newer was added costs no read of its replies,
//!   and one whose root moved, or cannot say, costs one `conversations.replies` read with `oldest`
//!   at the instant. The roots come from one `conversations.history` page shared by every such ask
//!   (newest first, from the oldest root asked about), read at most a page fewer times than there
//!   are threads; a root it did not reach is read on its own, as the thread view reads it. Slack
//!   moves `latest_reply` only when a reply is added, so such an answer is additions only, and
//!   says so (see [`crate::chat::ChatClient::fetch_threads`]).
//!
//! A channel narrowed to one thread is read by the trait's default, the thread view per thread.

use std::collections::HashMap;

use futures_util::{stream, StreamExt as _};

use super::client::{refused, HttpSlackClient, Raw};
use super::history::{
    advance, far_future_micros, nearest, Feed, Toward, MAX_WINDOW_PAGES, SCAN_PAGE_LIMIT,
};
use super::ids;
use super::timeline::{root_says_replies_after, thread_ts, MAX_TIMELINE_LIMIT, TIMELINE_TIMEOUT};
use crate::chat::{ChatError, ThreadAnswers, ThreadAsk};
use crate::model::ChannelId;
use crate::threads::{TimelineDelta, TimelinePage};

impl HttpSlackClient {
    /// [`crate::chat::ChatClient::fetch_threads`], without the provider label on each error.
    pub(super) async fn threads(&self, channel: &ChannelId, asks: &[ThreadAsk]) -> ThreadAnswers {
        let target = match Self::channel_ref(channel) {
            Ok(target) => target,
            Err(error) => {
                return asks
                    .iter()
                    .map(|ask| (ask.id.clone(), Err(error.clone())))
                    .collect()
            }
        };
        if target.thread_ts.is_some() {
            return crate::chat::emulated_threads(self, channel, asks).await;
        }
        let conversation = target.conversation.as_str();
        let roots = self.newer_roots(conversation, asks).await;
        let reads: Vec<_> = asks
            .iter()
            .map(|ask| self.thread_answer(channel, conversation, ask, &roots))
            .collect();
        let answers: Vec<_> = stream::iter(reads)
            .buffered(crate::chat::THREADS_IN_FLIGHT)
            .collect()
            .await;
        asks.iter()
            .zip(answers)
            .map(|(ask, answer)| (ask.id.clone(), answer))
            .collect()
    }

    /// The roots of the threads asked about after an instant, as one shared
    /// `conversations.history` read found them: newest first from the oldest root asked about,
    /// page by page until every root is found, the history ends, or the read has made one page
    /// fewer than there are such threads, so it never costs more than reading each root would. A
    /// single such thread reads its own root instead, and a failed read finds nothing: each
    /// thread then reads its own root, and answers that read's failure if it fails too.
    async fn newer_roots(&self, conversation: &str, asks: &[ThreadAsk]) -> HashMap<String, Raw> {
        let mut wanted: HashMap<String, i64> = HashMap::new();
        for ask in asks.iter().filter(|ask| ask.newer_than_ms.is_some()) {
            if let Ok(thread) = thread_ts(&ask.id) {
                if let Some(micros) = ids::ts_micros(&thread) {
                    wanted.insert(thread, micros);
                }
            }
        }
        let mut found = HashMap::new();
        let Some(oldest) = wanted.values().min().copied() else {
            return found;
        };
        let mut cursor: Option<String> = None;
        for _ in 1..wanted.len() {
            let page = match self
                .page(
                    Feed::History(conversation),
                    Some(oldest),
                    None,
                    true,
                    SCAN_PAGE_LIMIT,
                    cursor.as_deref(),
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::debug!(%error, "the roots of several threads could not be read together");
                    break;
                }
            };
            for raw in page.raws {
                if wanted.contains_key(&raw.ts) {
                    found.insert(raw.ts.clone(), raw);
                }
            }
            if found.len() == wanted.len() || !page.has_more {
                break;
            }
            match advance(cursor, page.next_cursor) {
                Ok(next) => cursor = next,
                Err(_) => break,
            }
        }
        found
    }

    /// One thread of [`Self::threads`].
    async fn thread_answer(
        &self,
        channel: &ChannelId,
        conversation: &str,
        ask: &ThreadAsk,
        roots: &HashMap<String, Raw>,
    ) -> Result<TimelinePage, ChatError> {
        let thread = thread_ts(&ask.id)?;
        let read = async {
            match ask.newer_than_ms {
                None => {
                    self.thread_newest(channel, conversation, &thread, ask)
                        .await
                }
                Some(newer_than_ms) => {
                    self.thread_newer(channel, conversation, &thread, ask, newer_than_ms, roots)
                        .await
                }
            }
        };
        tokio::time::timeout(TIMELINE_TIMEOUT, read)
            .await
            .map_err(|_| {
                refused("the thread read exceeded its time budget; nothing was returned")
            })?
    }

    /// The thread view's newest page of `thread`, from one `conversations.replies` window read
    /// whose first message is the root: exactly what the thread view answers, which reads the root
    /// and then the same window. A thread longer than that window, or one a walk already proved a
    /// stretch of empty, is read by the thread view itself.
    async fn thread_newest(
        &self,
        channel: &ChannelId,
        conversation: &str,
        thread: &str,
        ask: &ThreadAsk,
    ) -> Result<TimelinePage, ChatError> {
        let request = ask.request();
        let limit = usize::from(ask.limit.clamp(1, MAX_TIMELINE_LIMIT));
        let key = format!("replies:{conversation}:{thread}");
        if self.proven_from(&key, i64::MAX).is_some() {
            return self.timeline(channel, &request).await;
        }
        let (primary, complete) = self
            .window(
                Feed::Replies(conversation, thread),
                None,
                Some(far_future_micros()),
                MAX_WINDOW_PAGES,
            )
            .await?;
        if !complete {
            return self.timeline(channel, &request).await;
        }
        // What the thread view's root read checks: Slack answers a thread's messages root first.
        let root = primary.first().ok_or_else(|| ChatError::Status {
            status: 404,
            body: "conversations.replies answered with no root message".to_owned(),
        })?;
        if root.ts != thread {
            return Err(refused(
                "that ts is not the root of a thread in the configured channel",
            ));
        }
        self.mark_threaded(conversation);
        let mut raws = nearest(&primary, limit + 1, Toward::Older);
        let has_more = raws.len() > limit;
        if has_more {
            raws.drain(..raws.len() - limit);
        }
        // What the thread view's read does with the same messages (`#282 slack-flat-skip`): Flat
        // drops what it holds for the thread where this read shows otherwise, and a Flat read of it
        // already under way holds nothing it read.
        self.check_flat_against_thread(conversation, thread, &raws);
        let next_before = if has_more {
            Some(self.cursor(channel, &request, raws[0].ts.clone())?)
        } else {
            None
        };
        let mut page = TimelinePage {
            messages: self.messages(channel, &raws).await,
            thread: Some(self.summary(channel, root).await),
            has_threads: true,
            has_more,
            next_before,
            ..TimelinePage::default()
        };
        // As the thread view's newest page: a forward read from the newest message delivered.
        let newest = page
            .messages
            .last()
            .and_then(|message| super::ts_from_message_id(&message.id))
            .unwrap_or_else(|| ids::micros_ts(0));
        page.next_after = Some(self.forward_cursor(channel, &request, newest)?);
        Ok(page)
    }

    /// What of `thread` was created after `newer_than_ms`, as a delta of additions: nothing read
    /// when its root says no reply came after the instant, and otherwise one
    /// `conversations.replies` read with `oldest` there, at most `limit`, the oldest first, with
    /// `delta.more` when there were more. The root is the shared history read's, or its own read.
    async fn thread_newer(
        &self,
        channel: &ChannelId,
        conversation: &str,
        thread: &str,
        ask: &ThreadAsk,
        newer_than_ms: i64,
        roots: &HashMap<String, Raw>,
    ) -> Result<TimelinePage, ChatError> {
        let root = match roots.get(thread) {
            Some(root) => root.clone(),
            None => self.thread_root(conversation, thread).await?,
        };
        self.mark_threaded(conversation);
        let limit = usize::from(ask.limit.clamp(1, MAX_TIMELINE_LIMIT));
        // The last microsecond of the instant's millisecond: everything after it is newer.
        let after = newer_than_ms.saturating_mul(1_000).saturating_add(999);
        // A root with replies but no `latest_reply` says nothing about them, so it is read.
        let moved = root_says_replies_after(&root, after).unwrap_or(true);
        let mut raws = if moved {
            self.replies_oldest_after(conversation, thread, Some(after), limit + 1)
                .await?
        } else {
            Vec::new()
        };
        if root.micros > after && root.is_real() {
            raws.push(root.clone());
        }
        raws.sort_by_key(|raw| raw.micros);
        raws.dedup_by_key(|raw| raw.micros);
        let more = raws.len() > limit;
        raws.truncate(limit);
        Ok(TimelinePage {
            messages: self.messages(channel, &raws).await,
            thread: Some(self.summary(channel, &root).await),
            has_threads: true,
            delta: Some(TimelineDelta {
                more,
                complete: false,
                deleted: Vec::new(),
                removed_threads: Vec::new(),
            }),
            ..TimelinePage::default()
        })
    }
}
