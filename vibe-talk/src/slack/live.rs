//! The live poller's read of a Slack conversation: its own new messages, and the thread replies
//! posted since its previous read. `slack-reply-live`.
//!
//! `conversations.history` lists a conversation's own timeline and never a thread reply, so a
//! poller that only walks it forward delivers no reply at all. Each read here therefore starts from
//! the newest page of history, with no bound: one call, which answers the forward read too
//! whenever it reaches back to the poller's cursor (a busier stretch is then walked forward as
//! before, see [`super::history`]), and which carries every thread root on it with its
//! `reply_count` and `latest_reply`.
//!
//! # Which threads are read, and what each costs
//!
//! A root whose `latest_reply` has not moved past the newest reply the poller has had from its
//! thread costs no call, by the rule Flat's skip-list reads a root by
//! ([`super::timeline::root_says_replies_after`], `#282 slack-flat-skip`). A thread whose root moved
//! costs one `conversations.replies` call, with `oldest` at that reply, and whatever came after it
//! is reported. At most [`LIVE_THREAD_READS`] threads are read in one read; the rest wait for the
//! next read, oldest first, and so does a thread that had more new replies than one call returned,
//! even once its root has left the newest page. A root that counts replies but names no
//! `latest_reply` is read when its count changes, and once when it is first seen.
//!
//! A thread is taken as read to its root's `latest_reply` only when a message at that instant came
//! back and Slack answered `has_more: false`. A short page proves nothing, since a server may cap
//! any page. A thread whose latest reply does not come back (a feed that lags its root, or a reply
//! deleted since) is read again on the next read, and given up after [`LIVE_LAG_READS`] reads.
//!
//! # A read that fails
//!
//! A thread whose read fails waits for the next read and is read again from the same place, so a
//! failure loses no reply and repeats none. The read stops at the failure, leaving every thread it
//! has not read yet for the next read too, so an outage costs one call a read, not one a thread.
//! A thread whose last read failed then waits behind every thread whose last read did not, so one
//! thread that keeps failing holds up no other: the rest are read, and their replies reported,
//! before it is tried again, and threads that all keep failing take turns.
//!
//! Slack answers a failure on its side (`internal_error`, `request_timeout`,
//! `service_unavailable`, ...) as HTTP 200 with `ok: false`, as it answers a refusal, and a bridge
//! answers a command that failed or timed out the same way, so any failure is retried. A thread
//! that reads as gone (`404`: `thread_not_found`, `channel_not_found`, `message_not_found`) is no
//! exception, since a bridge may answer so for a command that failed for another reason; forgetting
//! its place would read it again from the seeding read and report every reply since then a second
//! time. A thread is given up, with a warning, only when its reads fail [`LIVE_FAILED_READS`] times
//! in a row other than for a moment (a lost connection, a rate limit, a 5xx), or
//! [`LIVE_TRANSIENT_FAILED_READS`] times in a row for any reason (a bridge whose command fails on
//! one thread's content may answer every read of it with a 5xx): it is taken as read to what its
//! root said, its replies left to the thread's and Flat's own reads.
//!
//! # What it does not report
//!
//! * **Edits and deletions.** A changed or deleted reply moves nothing on its root. They reach the
//!   page and the recent windows through those readers' own reads of the thread, of Flat and of the
//!   newest messages ([`super::timeline`]), as before.
//! * **Threads rooted before the newest page.** As in Flat, such a thread is read when it is
//!   opened.
//! * **A reply also sent to the channel** (`thread_broadcast`). It is in history, and arrives as
//!   one of the conversation's own messages.
//!
//! # Seeding
//!
//! The first read of a conversation notes each root on the newest page as it stands and reports no
//! reply, as the poller's first tick publishes nothing. From then on a reply newer than the newest
//! instant that read saw is news in any thread, including one whose root it did not see.

use std::collections::{HashMap, HashSet};

use super::client::{HttpSlackClient, Raw};
use super::history::{nearest, Feed, Toward, SCAN_PAGE_LIMIT};
use super::ids;
use super::timeline::{cursor_micros, root_says_replies_after};
use crate::chat::{ChatError, LiveRead};
use crate::model::{ChannelId, MessageId};

/// Most threads whose new replies one live read reads, one `conversations.replies` call each. The
/// rest are read by the next read, the longest waiting first.
pub const LIVE_THREAD_READS: usize = 4;
/// Reads of a thread in a row that reach its end without its root's latest reply before that reply
/// is given up.
pub const LIVE_LAG_READS: u32 = 3;
/// Reads of a thread in a row that fail, other than for a moment, before its new replies are given
/// up. A failure that reads as a moment (a lost connection, a rate limit, a 5xx) counts only
/// towards [`LIVE_TRANSIENT_FAILED_READS`].
pub const LIVE_FAILED_READS: u32 = 3;
/// Reads of a thread in a row that fail for any reason, a moment's failure included, before its
/// new replies are given up. A bridge whose command fails on one thread's content answers every
/// read of it with a 5xx; without this bound that thread would cost a call on every read with no
/// end. An outage seldom reaches it: a read whose newest page of history fails reads no thread.
pub const LIVE_TRANSIENT_FAILED_READS: u32 = 20;
/// Most threads whose place is remembered in one conversation; the oldest roots go first.
const LIVE_REMEMBERED_ROOTS: usize = 1024;
/// Most conversations whose places are remembered; past it every conversation is seeded again.
const LIVE_REMEMBERED_CONVERSATIONS: usize = 1000;

/// Where the poller stands in one thread.
#[derive(Clone, Debug)]
struct Place {
    /// The newest message of the thread it has had, root excluded, in microseconds: the next
    /// read's `oldest`.
    through: i64,
    /// The root's `reply_count` when the thread was last read to its end, or seen not to have
    /// moved. Zero for a thread not yet read.
    count: u64,
    /// Reads in a row that reached the end of the thread without its root's `latest_reply`.
    misses: u32,
    /// Reads in a row that failed, for any reason.
    failed: u32,
    /// Of those, the reads that failed with a failure that does not read as a moment.
    failures: u32,
}

impl Place {
    /// A place at `through`, the root then counting `count` replies, with no read behind it.
    fn at(through: i64, count: u64) -> Self {
        Self {
            through,
            count,
            misses: 0,
            failed: 0,
            failures: 0,
        }
    }
}

/// A thread whose root moved, and whose new replies have not all been read yet.
#[derive(Clone, Debug)]
struct Moved {
    /// The root `ts`.
    root: String,
    /// The root's `latest_reply` when it was last seen, in microseconds, when it named one.
    latest: Option<i64>,
    /// The root's `reply_count` when it was last seen.
    count: u64,
}

/// What the poller holds for one conversation. `slack-reply-live`.
#[derive(Clone, Debug, Default)]
pub(super) struct LiveThreads {
    /// The newest instant the seeding read saw, in microseconds: a reply after it is news in a
    /// thread the poller holds no place in.
    floor: i64,
    /// Its place in each thread, by root `ts`.
    places: HashMap<String, Place>,
    /// Threads waiting to be read, the longest waiting first; a read takes those whose last read
    /// failed after the rest.
    moved: Vec<Moved>,
}

impl LiveThreads {
    /// The places of a conversation whose newest page of history is `raws`, as it stands.
    fn seeded(raws: &[Raw], roots: &[&Raw]) -> Self {
        let floor = raws
            .iter()
            .map(|raw| raw.micros)
            .chain(roots.iter().filter_map(|root| root.latest_reply_micros()))
            .max()
            .unwrap_or(0);
        let places = roots
            .iter()
            .map(|root| {
                // Its latest reply, the newest the root says it has; a root that names none stands
                // at the newest instant seen, which nothing in its thread is newer than.
                let through = root.latest_reply_micros().unwrap_or(floor);
                (root.ts.clone(), Place::at(through, root.reply_count()))
            })
            .collect();
        Self {
            floor,
            places,
            moved: Vec::new(),
        }
    }

    /// Note what each root on the newest page says, and queue the threads that moved behind those
    /// already waiting, in the order their latest replies came.
    fn note_roots(&mut self, roots: &[&Raw]) {
        let mut moved = Vec::new();
        for root in roots {
            if let Some(waiting) = self.moved.iter_mut().find(|thread| thread.root == root.ts) {
                waiting.latest = root.latest_reply_micros();
                waiting.count = root.reply_count();
                continue;
            }
            let known = self.places.contains_key(&root.ts);
            let place = self
                .places
                .entry(root.ts.clone())
                .or_insert(Place::at(self.floor.max(root.micros), 0));
            let news = match root_says_replies_after(root, place.through) {
                Some(news) => news,
                None => !known || place.count != root.reply_count(),
            };
            if news {
                moved.push(Moved {
                    root: root.ts.clone(),
                    latest: root.latest_reply_micros(),
                    count: root.reply_count(),
                });
            } else {
                place.count = root.reply_count();
            }
        }
        moved.sort_by_key(|thread| thread.latest.unwrap_or(i64::MAX));
        self.moved.extend(moved);
    }

    /// Take the thread as read to what its root said: its latest reply, and its count.
    fn settle(&mut self, thread: &Moved) {
        if let Some(place) = self.places.get_mut(&thread.root) {
            if let Some(latest) = thread.latest {
                place.through = place.through.max(latest);
            }
            place.count = thread.count;
            place.misses = 0;
            place.failed = 0;
            place.failures = 0;
        }
    }

    /// Forget the places of the oldest roots past [`LIVE_REMEMBERED_ROOTS`], never one waiting.
    /// A root that left the newest page comes back to it only when newer messages are deleted.
    fn bound(&mut self) {
        let excess = self.places.len().saturating_sub(LIVE_REMEMBERED_ROOTS);
        if excess == 0 {
            return;
        }
        let waiting: HashSet<&str> = self
            .moved
            .iter()
            .map(|thread| thread.root.as_str())
            .collect();
        let mut oldest: Vec<(i64, String)> = self
            .places
            .keys()
            .filter(|root| !waiting.contains(root.as_str()))
            .map(|root| (ids::ts_micros(root).unwrap_or(0), root.clone()))
            .collect();
        oldest.sort_unstable();
        for (_, root) in oldest.into_iter().take(excess) {
            self.places.remove(&root);
        }
    }
}

impl HttpSlackClient {
    /// [`crate::chat::ChatClient::fetch_live`], without the provider label.
    pub(super) async fn live(
        &self,
        channel: &ChannelId,
        limit: u16,
        after: Option<&MessageId>,
    ) -> Result<LiveRead, ChatError> {
        let target = Self::channel_ref(channel)?;
        if target.thread_ts.is_some() {
            // A channel narrowed to one thread: its own forward read is the thread, replies and all.
            let messages = self.read_page(channel, limit, None, after).await?;
            return Ok(LiveRead {
                messages,
                ..LiveRead::default()
            });
        }
        let after = after.map(cursor_micros).transpose()?;
        let conversation = target.conversation.as_str();
        let want = usize::from(limit.max(1));
        let newest = self
            .page(
                Feed::History(conversation),
                None,
                None,
                false,
                SCAN_PAGE_LIMIT,
                None,
            )
            .await?;
        let own = match after {
            // The newest page reaches back to the cursor: it holds every message after it.
            Some(after)
                if !newest.has_more || newest.raws.iter().any(|raw| raw.micros <= after) =>
            {
                let newer: Vec<Raw> = newest
                    .raws
                    .iter()
                    .filter(|raw| raw.micros > after)
                    .cloned()
                    .collect();
                nearest(&newer, want, Toward::Newer)
            }
            Some(after) => self.history_oldest_after(conversation, after, want).await?,
            None => {
                let real = nearest(&newest.raws, want, Toward::Older);
                if real.is_empty() && newest.has_more {
                    self.history_newest(conversation, None, want).await?.real
                } else {
                    real
                }
            }
        };
        let roots: Vec<&Raw> = newest
            .raws
            .iter()
            .filter(|raw| raw.is_thread_root())
            .collect();
        if !roots.is_empty() || own.iter().any(Raw::is_thread_root) {
            self.mark_threaded(conversation);
        }
        let held = after.and_then(|_| self.live_threads(conversation));
        let (replies, replies_behind) = match held {
            Some(mut held) => {
                held.note_roots(&roots);
                let (replies, behind) = self.read_moved(conversation, &mut held).await;
                held.bound();
                self.keep_live_threads(conversation, held);
                (replies, behind)
            }
            // Seeding, or a conversation whose places were forgotten: as it stands, and no reply.
            None => {
                self.keep_live_threads(conversation, LiveThreads::seeded(&newest.raws, &roots));
                (Vec::new(), false)
            }
        };
        Ok(LiveRead {
            messages: self.messages(channel, &own).await,
            replies: self.messages(channel, &replies).await,
            replies_behind,
        })
    }

    /// Read the new replies of the threads waiting in `held`, at most [`LIVE_THREAD_READS`] of
    /// them, one call each, and none after a read that fails. Returns the replies to report,
    /// oldest first, and whether any thread is still waiting.
    async fn read_moved(&self, conversation: &str, held: &mut LiveThreads) -> (Vec<Raw>, bool) {
        let mut waiting = std::mem::take(&mut held.moved);
        // A thread whose last read failed waits behind every thread whose last read did not, in
        // the order they were waiting, so one that keeps failing holds up no other.
        waiting.sort_by_key(|thread| {
            held.places
                .get(&thread.root)
                .is_some_and(|place| place.failed > 0)
        });
        let mut found: Vec<Raw> = Vec::new();
        let mut reads = 0;
        // The thread whose read failed, which goes behind every other thread still waiting.
        let mut failing = None;
        for thread in waiting {
            if failing.is_some() || reads == LIVE_THREAD_READS {
                held.moved.push(thread);
                continue;
            }
            let floor = held.floor;
            let through = held
                .places
                .entry(thread.root.clone())
                .or_insert(Place::at(floor, 0))
                .through;
            reads += 1;
            let page = self
                .page(
                    Feed::Replies(conversation, &thread.root),
                    Some(through),
                    None,
                    false,
                    SCAN_PAGE_LIMIT,
                    None,
                )
                .await;
            match page {
                Ok(page) => {
                    // A server may repeat the root on every replies page; it is not news.
                    let fresh: Vec<Raw> = page
                        .raws
                        .into_iter()
                        .filter(|raw| raw.ts != thread.root && raw.micros > through)
                        .collect();
                    let place = held
                        .places
                        .get_mut(&thread.root)
                        .expect("a place was made for every thread read");
                    place.failed = 0;
                    place.failures = 0;
                    if let Some(newest) = fresh.iter().map(|raw| raw.micros).max() {
                        place.through = newest;
                    }
                    let reached = thread.latest.is_none_or(|latest| place.through >= latest);
                    found.extend(
                        fresh
                            .into_iter()
                            .filter(|raw| raw.is_real() && !raw.is_broadcast()),
                    );
                    if page.has_more {
                        held.moved.push(thread);
                        continue;
                    }
                    if !reached {
                        place.misses += 1;
                        if place.misses < LIVE_LAG_READS {
                            held.moved.push(thread);
                            continue;
                        }
                    }
                    held.settle(&thread);
                }
                Err(error) => {
                    // Slack answers a failure on its side (`internal_error`, `request_timeout`,
                    // ...) as HTTP 200 with `ok: false`, and so does a bridge for a command that
                    // failed or timed out, so a failure that does not read as a moment may be one.
                    // A `404` is no proof either: a bridge may answer `thread_not_found` for a
                    // command that failed for another reason. The thread waits for the next read
                    // either way, keeping its place, so no reply is reported twice; only one that
                    // keeps failing is given up. Nothing more is read now, so an outage costs one
                    // call a read.
                    let place = held
                        .places
                        .get_mut(&thread.root)
                        .expect("a place was made for every thread read");
                    place.failed += 1;
                    if !error.is_transient() {
                        place.failures += 1;
                    }
                    if place.failures < LIVE_FAILED_READS
                        && place.failed < LIVE_TRANSIENT_FAILED_READS
                    {
                        tracing::debug!(
                            conversation,
                            thread = %thread.root,
                            %error,
                            failed = place.failed,
                            failures = place.failures,
                            "the new replies of a Slack thread are left for the next live read"
                        );
                        failing = Some(thread);
                    } else {
                        tracing::warn!(
                            conversation,
                            thread = %thread.root,
                            %error,
                            failed = place.failed,
                            failures = place.failures,
                            "the new replies of a Slack thread could not be read for the live \
                             stream; they show when the thread or the flat view is read"
                        );
                        held.settle(&thread);
                    }
                }
            }
        }
        held.moved.extend(failing);
        found.sort_by_key(|raw| raw.micros);
        found.dedup_by_key(|raw| raw.micros);
        (found, !held.moved.is_empty())
    }

    /// What the poller holds for `conversation`, when it has seeded it.
    fn live_threads(&self, conversation: &str) -> Option<LiveThreads> {
        self.live_threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(conversation)
            .cloned()
    }

    fn keep_live_threads(&self, conversation: &str, threads: LiveThreads) {
        let mut held = self
            .live_threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.len() >= LIVE_REMEMBERED_CONVERSATIONS && !held.contains_key(conversation) {
            held.clear();
        }
        held.insert(conversation.to_owned(), threads);
    }
}
