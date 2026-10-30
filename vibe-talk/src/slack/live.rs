//! The live poller's read of a Slack conversation: its own new messages, and the thread replies
//! posted since its previous read. `#307 slack-reply-live`.
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
//! any page, and may say nothing of it: a read that brings new replies short of the latest is
//! followed by another on the next read, however long the burst. A read that brings no new reply
//! while more is to come proves nothing either: a feed may lag its root, a reply may have been
//! deleted since, and a lenient server may ignore `oldest`, answering the same first page with
//! `has_more` every time. The thread is read again, and given up after [`LIVE_LAG_READS`] such
//! reads in a row.
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
//!
//! Where a root stands is only where the poller starts: a thread whose replies before the seed
//! never reached the message cache (the server was restarting, or every read was refused) is read
//! from its root on a later read when the poller asks for it
//! ([`crate::chat::ChatClient::reread_thread`], `#708 vt-live-push`), as it does for every thread
//! whose root counts more replies than the cache holds.

use std::collections::{HashMap, HashSet};

use super::client::{HttpSlackClient, Raw};
use super::history::{nearest, Feed, Toward, SCAN_PAGE_LIMIT};
use super::ids;
use super::reactions;
use super::timeline::{cursor_micros, root_says_replies_after};
use crate::chat::{ChatError, LiveRead};
use crate::model::{ChannelId, MessageId, Reaction};

/// Most threads whose new replies one live read reads, one `conversations.replies` call each. The
/// rest are read by the next read, the longest waiting first.
pub const LIVE_THREAD_READS: usize = 4;
/// Reads of a thread in a row that bring no new reply while more is to come (Slack says `has_more`,
/// or its root's latest reply has not come back) before the thread is taken as read to its root.
/// A read that brings a new reply starts the count again.
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
    /// Reads in a row that brought no new reply while more was to come: Slack said `has_more`, or
    /// the root's `latest_reply` had not come back.
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

/// What the poller holds for one conversation. `#307 slack-reply-live`.
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
    /// The reaction set last seen for each message, by `ts`: `None` for a known message no copy
    /// has reported reactions for. `#378 live-reactions`. The newest page every live read
    /// already fetches carries the current sets, so a diff against this is the whole cost of
    /// seeing a reaction added in the native app.
    reactions: HashMap<String, Option<Vec<Reaction>>>,
}

impl LiveThreads {
    /// The places of a conversation whose newest page of history is `raws`, as it stands.
    /// `owner` reads the reaction sets the page reports, for the diff later reads make
    /// (`#378 live-reactions`): seeding learns them and reports no change.
    fn seeded(raws: &[Raw], roots: &[&Raw], owner: Option<&str>) -> Self {
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
        let reactions = raws
            .iter()
            .map(|raw| {
                (
                    raw.ts.clone(),
                    reactions::parse(raw.value.get("reactions"), owner),
                )
            })
            .collect();
        Self {
            floor,
            places,
            moved: Vec::new(),
            reactions,
        }
    }

    /// Diff the reaction sets `raws` — one read's copies, from one endpoint — report against the
    /// remembered ones, remembering the new sets, and return the messages whose set moved.
    /// `#378 live-reactions`.
    ///
    /// The same rules as the hub's memory ([`crate::live::LiveHub::reactions_changed`]): a
    /// message never seen is learned silently; a known message's first reported set and a set
    /// that moved are changes. A copy reporting nothing where a non-empty set stood is the last
    /// reaction removed ONLY under `absent_means_none` — where some copy in this same read
    /// carried a set, proving the endpoint reports them; a shim may supply them on history and
    /// not on replies, and its silent copies must never wipe chips (`#312 slack-reactions`
    /// keeps a merely absent field as "not reported" everywhere else). A removal's raw is
    /// stamped with the empty array it means, so the message built from it says "none" outright.
    fn reaction_changes(
        &mut self,
        raws: &[Raw],
        owner: Option<&str>,
        absent_means_none: bool,
    ) -> Vec<Raw> {
        let mut changed = Vec::new();
        for raw in raws {
            let set = reactions::parse(raw.value.get("reactions"), owner);
            let held = self.reactions.get(&raw.ts).cloned();
            match (&held, &set) {
                (Some(Some(before)), None) if absent_means_none && !before.is_empty() => {
                    let mut stamped = raw.clone();
                    if let Some(object) = stamped.value.as_object_mut() {
                        object.insert("reactions".to_owned(), serde_json::Value::Array(Vec::new()));
                    }
                    changed.push(stamped);
                    self.reactions.insert(raw.ts.clone(), Some(Vec::new()));
                }
                (Some(before), Some(now)) => {
                    if before.as_ref() != Some(now) {
                        changed.push(raw.clone());
                    }
                    self.reactions.insert(raw.ts.clone(), set);
                }
                (Some(_), None) => {}
                (None, _) => {
                    self.reactions.insert(raw.ts.clone(), set);
                }
            }
        }
        changed
    }

    /// Learn the reaction sets `raws` report without reporting any change: for the messages a
    /// read already reports as new, whose sets ride with them. `#378 live-reactions`.
    fn learn_reactions(&mut self, raws: &[Raw], owner: Option<&str>) {
        for raw in raws {
            let set = reactions::parse(raw.value.get("reactions"), owner);
            match set {
                Some(_) => {
                    self.reactions.insert(raw.ts.clone(), set);
                }
                None => {
                    self.reactions.entry(raw.ts.clone()).or_insert(None);
                }
            }
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

    /// Read `root`'s replies after `after` (microseconds; `None` for every reply) on the next
    /// read, whatever this took the thread as read to. `#708 vt-live-push`. The thread is read
    /// until its root's latest reply as this last saw it has come back, as any moved thread is.
    fn reread(&mut self, root: &str, after: Option<i64>) {
        let from = after.unwrap_or_else(|| ids::ts_micros(root).unwrap_or(0));
        let floor = self.floor;
        let place = self
            .places
            .entry(root.to_owned())
            .or_insert(Place::at(floor, 0));
        let latest = (place.through > from).then_some(place.through);
        place.through = place.through.min(from);
        place.misses = 0;
        let count = place.count;
        if !self.moved.iter().any(|thread| thread.root == root) {
            self.moved.push(Moved {
                root: root.to_owned(),
                latest,
                count,
            });
        }
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

    /// Forget the places of the oldest roots past [`LIVE_REMEMBERED_ROOTS`], never one waiting,
    /// and the oldest reaction sets past the same bound (`#378 live-reactions`): a set forgotten
    /// is re-learned silently by the next read that reports it.
    /// A root that left the newest page comes back to it only when newer messages are deleted.
    fn bound(&mut self) {
        let excess = self.reactions.len().saturating_sub(LIVE_REMEMBERED_ROOTS);
        if excess > 0 {
            let mut oldest: Vec<(i64, String)> = self
                .reactions
                .keys()
                .map(|ts| (ids::ts_micros(ts).unwrap_or(0), ts.clone()))
                .collect();
            oldest.sort_unstable();
            for (_, ts) in oldest.into_iter().take(excess) {
                self.reactions.remove(&ts);
            }
        }
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
        let owner = self.owner_id();
        let held = after.and_then(|_| self.live_threads(conversation));
        let (replies, replies_behind, changed) = match held {
            Some(mut held) => {
                held.note_roots(&roots);
                let (replies, behind, thread_seen) = self.read_moved(conversation, &mut held).await;
                // `#378 live-reactions`. The newest page this read already fetched, and every
                // thread page it read, carry current reaction sets: diff them, less the messages
                // this read already reports as new, whose sets ride with them. The two endpoints
                // are diffed as separate reads, each proving for itself whether it reports
                // reactions, and a root in both is taken from history only.
                let reported: HashSet<&str> = own
                    .iter()
                    .chain(&replies)
                    .map(|raw| raw.ts.as_str())
                    .collect();
                let on_page: HashSet<&str> =
                    newest.raws.iter().map(|raw| raw.ts.as_str()).collect();
                let select = |raws: &[Raw]| -> Vec<Raw> {
                    raws.iter()
                        .filter(|raw| !reported.contains(raw.ts.as_str()))
                        .cloned()
                        .collect()
                };
                let diff = |held: &mut LiveThreads, batch: &[Raw]| {
                    let reports = batch.iter().any(|raw| {
                        reactions::parse(raw.value.get("reactions"), owner.as_deref()).is_some()
                    });
                    held.reaction_changes(batch, owner.as_deref(), reports)
                };
                let mut changed = diff(&mut held, &select(&newest.raws));
                let thread_only: Vec<Raw> = thread_seen
                    .iter()
                    .filter(|raw| !on_page.contains(raw.ts.as_str()))
                    .cloned()
                    .collect();
                changed.extend(diff(&mut held, &select(&thread_only)));
                // The reported messages' sets are learned too, so their next move is a change.
                let fresh: Vec<Raw> = own.iter().chain(&replies).cloned().collect();
                held.learn_reactions(&fresh, owner.as_deref());
                held.bound();
                self.keep_live_threads(conversation, held);
                (replies, behind, changed)
            }
            // Seeding, or a conversation whose places were forgotten: as it stands, and no reply.
            None => {
                self.keep_live_threads(
                    conversation,
                    LiveThreads::seeded(&newest.raws, &roots, owner.as_deref()),
                );
                (Vec::new(), false, Vec::new())
            }
        };
        Ok(LiveRead {
            messages: self.messages(channel, &own).await,
            replies: self.messages(channel, &replies).await,
            replies_behind,
            changed: self.messages(channel, &changed).await,
        })
    }

    /// Read the new replies of the threads waiting in `held`, at most [`LIVE_THREAD_READS`] of
    /// them, one call each, and none after a read that fails. Returns the replies to report,
    /// oldest first, whether any thread is still waiting, and every raw those pages carried —
    /// roots and already-known replies included, whose current reaction sets the caller diffs
    /// (`#378 live-reactions`).
    async fn read_moved(
        &self,
        conversation: &str,
        held: &mut LiveThreads,
    ) -> (Vec<Raw>, bool, Vec<Raw>) {
        let mut waiting = std::mem::take(&mut held.moved);
        // A thread whose last read failed waits behind every thread whose last read did not, in
        // the order they were waiting, so one that keeps failing holds up no other.
        waiting.sort_by_key(|thread| {
            held.places
                .get(&thread.root)
                .is_some_and(|place| place.failed > 0)
        });
        let mut found: Vec<Raw> = Vec::new();
        let mut seen: Vec<Raw> = Vec::new();
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
                    seen.extend(page.raws.iter().cloned());
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
                    let progressed = !fresh.is_empty();
                    if let Some(newest) = fresh.iter().map(|raw| raw.micros).max() {
                        place.through = newest;
                        place.misses = 0;
                    }
                    let reached = thread.latest.is_none_or(|latest| place.through >= latest);
                    found.extend(
                        fresh
                            .into_iter()
                            .filter(|raw| raw.is_real() && !raw.is_broadcast()),
                    );
                    // More is to come when Slack says so, or when the root's latest reply has not
                    // come back. A read that brought something new is followed by another however
                    // long the burst. One that brought nothing proves neither: a server may ignore
                    // `oldest` and answer the same first page with `has_more` every time, and a
                    // feed may lag its root. Such reads count, and a run of them ends the wait.
                    if page.has_more || !reached {
                        if progressed {
                            held.moved.push(thread);
                            continue;
                        }
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
        (found, !held.moved.is_empty(), seen)
    }

    /// [`crate::chat::ChatClient::reread_thread`] for `conversation`, which the poller has seeded:
    /// whether the thread will be read. `#708 vt-live-push`.
    pub(super) fn reread_live_thread(
        &self,
        conversation: &str,
        root: &str,
        after: Option<i64>,
    ) -> bool {
        if ids::ts_micros(root).is_none() {
            return false;
        }
        let mut held = self
            .live_threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(threads) = held.get_mut(conversation) else {
            return false;
        };
        threads.reread(root, after);
        true
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
