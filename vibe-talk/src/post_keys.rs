//! What became of each keyed post, for a while. `#448 send-dedupe`.
//!
//! The incident: the owner sent a long message to a Slack channel, cut in three parts. The post
//! took twelve seconds, the page showed it as failed, and the owner pressed Retry. The first post
//! had landed all the same, so the message reached the channel twice, and then a third time. The
//! page sent each attempt under the same idempotency key, but only a provider that honours keys
//! upstream ([`crate::chat::ChatClient::supports_idempotent_posts`]) ever looked at it, and Slack
//! has no such key. Nothing recognised the retry as the first post.
//!
//! So this server remembers every keyed post itself, whatever the provider, BY ITS KEY:
//!
//! * **A repeat of a post that landed** — the same key, channel and text — is answered with the
//!   first post's own answer and posts nothing, wherever it now says the post goes. So is a repeat
//!   of a post that landed in part: its 207, which hands back the unsent text, is the answer again,
//!   and the caller's next attempt sends only that text. A placement changed between attempts — a
//!   reply the page has since drawn into the thread it started, say — names the same post: it was
//!   posted once, where the first attempt said.
//! * **The unsent rest of a 207**, sent under the key it had, to the thread the 207 named, is the
//!   one other post a key may name: it goes, and the key names it from then on.
//! * **Any other reuse of a key** — other words, another channel — is refused (`key_reused`) and
//!   posts nothing. A caller that means a new message mints a new key.
//! * **A repeat that arrives while the first is still posting** waits for it and is then answered
//!   by the rules above: the two are coalesced into one post. The first one's post runs in a task
//!   of its own ([`crate::ops::reply_scoped`]), so it finishes and is remembered even when its
//!   caller has given up waiting.
//! * **A post that certainly posted nothing** — refused, or failed before any part could have
//!   landed — is forgotten, so sending it again is a fresh attempt, as it should be. So is one
//!   whose FIRST part's answer was lost with nothing confirmed after it: remembering it would answer
//!   every later attempt with the same uncertainty, and Retry could never send it. That part may
//!   have landed, and a retry may then post it twice; only a provider that recognises the key
//!   upstream ([`crate::chat::ChatClient::supports_idempotent_posts`]) can rule that out.
//!
//! [`PostKeys::lookup`] answers a caller that lost its answer without posting anything: the page
//! asks it after a send whose answer never came back (`POST .../reply/outcome`), and while a slow
//! send is under way, to say which part is being posted.
//!
//! For [`KEEP`], at most [`MAX_KEPT`] posts, in memory and in the store (`#471
//! replication-c5-post-keys`): what a post landed as is filed there when it settles ([`Kept`]) and
//! read back at start ([`PostKeys::restore`]), so a repeat after a restart is answered with the
//! first post's answer and posts nothing. A post in flight at a restart is not kept: it ended with
//! the process, and a page that asks afterwards is told it is unknown, which leaves its send
//! unconfirmed, as it must.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::model::{ChannelInfo, Message};

/// How long a post that landed is remembered.
pub const KEEP: Duration = Duration::from_secs(24 * 60 * 60);

/// The most posts remembered at once; the oldest is forgotten first.
pub const MAX_KEPT: usize = 4096;

/// One post as its caller asked for it: the key, and everything the key was given for. The memory
/// is by key; the rest says whether a later request under it repeats this post, continues it, or
/// reuses the key for something else.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    key: String,
    channel: String,
    thread: Option<String>,
    reply_to: Option<String>,
    text: String,
}

impl Fingerprint {
    /// The post `key` names in `channel`.
    #[must_use]
    pub fn new(
        key: &str,
        channel: &str,
        thread: Option<&str>,
        reply_to: Option<&str>,
        text: &str,
    ) -> Self {
        Self {
            key: key.to_owned(),
            channel: channel.to_owned(),
            thread: thread.map(str::to_owned),
            reply_to: reply_to.map(str::to_owned),
            text: text.to_owned(),
        }
    }
}

/// What a remembered post came to.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Every part landed.
    Posted {
        /// The channel it went to.
        channel: ChannelInfo,
        /// The first part.
        first: Message,
        /// Every part, in order.
        parts: Vec<Message>,
    },
    /// Some parts landed and then one failed: the 207 the post was answered with.
    Partial(Partial),
}

/// How an [`Outcome`] is filed in the store: `Posted` without `first`, which is always
/// `parts[0]`. That copy was two fifths of every record. `#490 storage-io-audit`.
#[derive(serde::Serialize)]
enum FiledRef<'a> {
    Posted {
        channel: &'a ChannelInfo,
        parts: &'a [Message],
    },
    Partial(&'a Partial),
}

/// A filed [`Outcome`] as it is read back. A record filed before `#490 storage-io-audit` still
/// carries `first`, and is read as it was written.
#[derive(serde::Deserialize)]
enum Filed {
    Posted {
        channel: ChannelInfo,
        #[serde(default)]
        first: Option<Message>,
        parts: Vec<Message>,
    },
    Partial(Partial),
}

impl serde::Serialize for Outcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Posted { channel, parts, .. } => FiledRef::Posted { channel, parts },
            Self::Partial(partial) => FiledRef::Partial(partial),
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Outcome {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Filed::deserialize(deserializer)? {
            Filed::Posted {
                channel,
                first,
                parts,
            } => {
                let first = match first.or_else(|| parts.first().cloned()) {
                    Some(first) => first,
                    None => return Err(serde::de::Error::custom("a posted record has no part")),
                };
                Self::Posted {
                    channel,
                    first,
                    parts,
                }
            }
            Filed::Partial(partial) => Self::Partial(partial),
        })
    }
}

/// The facts of a post that landed in part; see [`crate::ops::OpError::PartiallyPosted`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[allow(missing_docs)]
pub struct Partial {
    pub posted: usize,
    pub unsent: String,
    pub cause: String,
    pub retryable: bool,
    pub resumable: bool,
    pub thread_id: Option<String>,
    pub uncertain: bool,
    pub refusal: Option<crate::send_refusal::SendRefusal>,
    pub first: Option<Message>,
}

/// What the store keeps of one settled post: the post its key names, what it came to, and when,
/// in Unix milliseconds. `#471 replication-c5-post-keys`. It holds the owner's own words, as the
/// transcript does, and only for [`KEEP`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    /// The post the key names.
    pub fingerprint: Fingerprint,
    /// What it came to.
    pub outcome: Outcome,
    /// When it settled.
    pub at_ms: i64,
}

impl Kept {
    /// The key this record is filed under.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.fingerprint.key
    }
}

/// How far a post in flight has got.
#[derive(Debug, Default)]
pub struct Progress {
    posted: AtomicUsize,
    parts: AtomicUsize,
}

impl Progress {
    /// The post is cut in `parts`.
    pub fn begin(&self, parts: usize) {
        self.parts.store(parts, Ordering::Relaxed);
    }

    /// One more part has landed.
    pub fn landed(&self) {
        self.posted.fetch_add(1, Ordering::Relaxed);
    }

    /// The parts landed so far, and the parts in all (0 before the post is cut).
    #[must_use]
    pub fn get(&self) -> (usize, usize) {
        (
            self.posted.load(Ordering::Relaxed),
            self.parts.load(Ordering::Relaxed),
        )
    }
}

/// What one key names: the post it was last given for, and how far that has got.
struct Slot {
    fingerprint: Fingerprint,
    state: State,
}

enum State {
    InFlight {
        settled: watch::Receiver<Option<Arc<Outcome>>>,
        progress: Arc<Progress>,
    },
    Done {
        outcome: Arc<Outcome>,
        at: Instant,
    },
}

impl Fingerprint {
    /// Whether `self` asks for the same words in the same channel as `other`, wherever it puts them.
    fn same_words(&self, other: &Self) -> bool {
        self.channel == other.channel && self.text == other.text
    }

    /// Whether `self` is the unsent rest of `before`, which landed in part as `partial` says: its
    /// text, to the thread the 207 named, with the reply target `before` had or none.
    fn continues(&self, before: &Self, partial: &Partial) -> bool {
        self.channel == before.channel
            && self.text == partial.unsent
            && self.thread == partial.thread_id
            && (self.reply_to.is_none() || self.reply_to == before.reply_to)
    }
}

/// The two phases of one keyed send, in milliseconds. `#621 send-double-check`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SendTiming {
    /// Server accept: request start to durable claim recorded.
    pub server_ms: Option<u64>,
    /// Upstream: accept to provider outcome known.
    pub upstream_ms: Option<u64>,
}

/// Every keyed post in flight, and every one that landed within [`KEEP`].
pub struct PostKeys {
    slots: Mutex<HashMap<String, Slot>>,
    timings: Mutex<HashMap<String, (Instant, SendTiming)>>,
    keep: Duration,
    /// Moved by every [`PostKeys::forget_settled`]: a claim made before it settles into memory,
    /// but is not filed in the store. `#471 replication-c5-post-keys`.
    generation: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for PostKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostKeys")
            .field("kept", &self.lock().len())
            .finish_non_exhaustive()
    }
}

impl Default for PostKeys {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`PostKeys::claim`] found.
pub enum Claim {
    /// Nobody has this post: it is the caller's to make, and to settle.
    Mine(Pending),
    /// It landed, wholly or in part: this is its answer.
    Known(Arc<Outcome>),
    /// Somebody is posting under this key now: wait for them with [`Waiting::settled`], then claim
    /// again.
    Wait(Waiting),
    /// The key names another post, which this one neither repeats nor continues.
    Reused,
}

/// What [`PostKeys::lookup`] found. Posts nothing, claims nothing.
pub enum Lookup {
    /// It landed, wholly or in part: this is its answer.
    Known(Arc<Outcome>),
    /// It is being posted now.
    InFlight(Waiting),
    /// Not remembered: never asked for, forgotten because it certainly posted nothing or may have
    /// posted only its one uncertain part, or asked for before a restart.
    Unknown,
}

/// A post somebody else is making.
pub struct Waiting {
    settled: watch::Receiver<Option<Arc<Outcome>>>,
    progress: Arc<Progress>,
}

impl Waiting {
    /// Its answer, once it has one to remember; `None` when it was forgotten, which leaves the
    /// post to whoever claims it next.
    pub async fn settled(mut self) -> Option<Arc<Outcome>> {
        loop {
            if let Some(outcome) = self.settled.borrow_and_update().clone() {
                return Some(outcome);
            }
            if self.settled.changed().await.is_err() {
                return self.settled.borrow().clone();
            }
        }
    }

    /// How far it has got.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        self.progress.get()
    }
}

/// The claim on a post being made. Settled with what it came to; dropped unsettled — refused, or
/// its caller gone before anything was sent — it is forgotten and whoever waits for it may make it.
pub struct Pending {
    keys: Arc<PostKeys>,
    fingerprint: Fingerprint,
    settle: Option<watch::Sender<Option<Arc<Outcome>>>>,
    progress: Arc<Progress>,
    /// What the key named before this claim — the post whose unsent rest this is — put back if
    /// this one is forgotten, so the key still answers for the part that landed.
    before: Option<Box<Slot>>,
    /// [`PostKeys::generation`] when the claim was made.
    generation: u64,
}

impl Pending {
    /// The post this claim is for.
    #[must_use]
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Whether the store may file what this claim settles as: no purge has emptied the send log
    /// since it was made. `#471 replication-c5-post-keys`.
    #[must_use]
    pub fn fileable(&self) -> bool {
        self.keys.generation() == self.generation
    }

    /// How far the post has got, for [`PostKeys::lookup`] to report.
    #[must_use]
    pub fn progress(&self) -> Arc<Progress> {
        Arc::clone(&self.progress)
    }

    /// Remember what the post came to, and give it to whoever waits; `None` forgets it.
    pub fn settle(mut self, outcome: Option<Outcome>) {
        let Some(sender) = self.settle.take() else {
            return;
        };
        match outcome {
            Some(outcome) => {
                let outcome = Arc::new(outcome);
                self.keys.lock().insert(
                    self.fingerprint.key.clone(),
                    Slot {
                        fingerprint: self.fingerprint.clone(),
                        state: State::Done {
                            outcome: Arc::clone(&outcome),
                            at: Instant::now(),
                        },
                    },
                );
                sender.send_replace(Some(outcome));
            }
            None => self.forget(),
        }
    }

    /// Give the key back to what it named before this claim, or to nothing.
    fn forget(&mut self) {
        let mut slots = self.keys.lock();
        match self.before.take() {
            Some(before) => {
                slots.insert(self.fingerprint.key.clone(), *before);
            }
            None => {
                slots.remove(&self.fingerprint.key);
            }
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.settle.take().is_some() {
            self.forget();
        }
    }
}

impl PostKeys {
    /// Nothing remembered, for [`KEEP`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_keep(KEEP)
    }

    /// Nothing remembered, each post that lands kept for `keep`.
    #[must_use]
    pub fn with_keep(keep: Duration) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            timings: Mutex::new(HashMap::new()),
            keep,
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Record the server-accept phase for `key`. `#621 send-double-check`.
    pub fn note_accepted(&self, key: &str, server_ms: u64) {
        let mut timings = self.timings.lock().unwrap_or_else(PoisonError::into_inner);
        timings.retain(|_, (at, _)| at.elapsed() < KEEP);
        if timings.len() >= MAX_KEPT {
            timings.clear();
        }
        let entry = timings
            .entry(key.to_owned())
            .or_insert_with(|| (Instant::now(), SendTiming::default()));
        entry.1.server_ms = Some(server_ms);
        entry.1.upstream_ms = None;
        entry.0 = Instant::now();
    }

    /// Record the upstream phase for `key`; returns the full timing. `#621 send-double-check`.
    pub fn note_settled(&self, key: &str) -> SendTiming {
        let mut timings = self.timings.lock().unwrap_or_else(PoisonError::into_inner);
        match timings.get_mut(key) {
            Some(entry) => {
                entry.1.upstream_ms = Some(entry.0.elapsed().as_millis() as u64);
                entry.1
            }
            None => SendTiming::default(),
        }
    }

    /// The recorded timing for `key`, if any. `#621 send-double-check`.
    #[must_use]
    pub fn timing(&self, key: &str) -> Option<SendTiming> {
        self.timings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .map(|(_, t)| *t)
    }

    /// Claim `fingerprint` to post it, or find it made or being made under its key.
    pub fn claim(self: &Arc<Self>, fingerprint: Fingerprint) -> Claim {
        let mut slots = self.lock();
        self.prune(&mut slots);
        let before = match slots.remove(&fingerprint.key) {
            None => None,
            Some(slot) => {
                let answer = match &slot.state {
                    State::InFlight { settled, progress } => Some(Claim::Wait(Waiting {
                        settled: settled.clone(),
                        progress: Arc::clone(progress),
                    })),
                    State::Done { outcome, .. } if fingerprint.same_words(&slot.fingerprint) => {
                        Some(Claim::Known(Arc::clone(outcome)))
                    }
                    State::Done { outcome, .. } => match outcome.as_ref() {
                        Outcome::Partial(partial)
                            if fingerprint.continues(&slot.fingerprint, partial) =>
                        {
                            None
                        }
                        _ => Some(Claim::Reused),
                    },
                };
                if let Some(answer) = answer {
                    slots.insert(fingerprint.key.clone(), slot);
                    return answer;
                }
                Some(Box::new(slot))
            }
        };
        let (sender, settled) = watch::channel(None);
        let progress = Arc::new(Progress::default());
        slots.insert(
            fingerprint.key.clone(),
            Slot {
                fingerprint: fingerprint.clone(),
                state: State::InFlight {
                    settled,
                    progress: Arc::clone(&progress),
                },
            },
        );
        Claim::Mine(Pending {
            keys: Arc::clone(self),
            fingerprint,
            settle: Some(sender),
            progress,
            before,
            generation: self.generation(),
        })
    }

    /// What became of the post `fingerprint` asks for, without claiming it: the post its key names,
    /// when that is the same words in the same channel.
    pub fn lookup(&self, fingerprint: &Fingerprint) -> Lookup {
        let mut slots = self.lock();
        self.prune(&mut slots);
        match slots.get(&fingerprint.key) {
            Some(slot) if slot.fingerprint.same_words(fingerprint) => match &slot.state {
                State::Done { outcome, .. } => Lookup::Known(Arc::clone(outcome)),
                State::InFlight { settled, progress } => Lookup::InFlight(Waiting {
                    settled: settled.clone(),
                    progress: Arc::clone(progress),
                }),
            },
            _ => Lookup::Unknown,
        }
    }

    /// Take back what the store kept (`#471 replication-c5-post-keys`): each post settled within
    /// [`KEEP`] before `now_ms`, oldest first, so a key filed twice keeps its later answer. A key
    /// already remembered or in flight here is left as it is: what this process knows is newer.
    pub fn restore(&self, kept: Vec<Kept>, now_ms: i64) {
        let mut kept = kept;
        kept.sort_by_key(|record| record.at_ms);
        let mut slots = self.lock();
        let now = Instant::now();
        let mut restored: HashMap<String, Slot> = HashMap::new();
        for record in kept {
            let age = Duration::from_millis(u64::try_from(now_ms - record.at_ms).unwrap_or(0));
            if age >= self.keep {
                continue;
            }
            // Soon after the host started, the monotonic clock may not reach back that far: the
            // record is then taken as settled now, which keeps it a little longer, never shorter.
            let at = now.checked_sub(age).unwrap_or(now);
            restored.insert(
                record.fingerprint.key.clone(),
                Slot {
                    fingerprint: record.fingerprint,
                    state: State::Done {
                        outcome: Arc::new(record.outcome),
                        at,
                    },
                },
            );
        }
        for (key, slot) in restored {
            slots.entry(key).or_insert(slot);
        }
        self.prune(&mut slots);
    }

    /// Forget every post that landed, as the operator's purge does to the store, and move the
    /// generation so a post in flight now is remembered here when it settles but never filed in
    /// the store ([`Pending::fileable`]). Posts in flight stay, so a repeat still waits for them.
    /// `#471 replication-c5-post-keys`.
    pub fn forget_settled(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lock()
            .retain(|_, slot| matches!(slot.state, State::InFlight { .. }));
    }

    /// The generation [`PostKeys::forget_settled`] moves.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many posts are remembered or in flight.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing is remembered or in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget what landed longer ago than `keep`, and the oldest beyond [`MAX_KEPT`]. A post in
    /// flight is never forgotten here: its claim settles it.
    fn prune(&self, slots: &mut HashMap<String, Slot>) {
        let now = Instant::now();
        slots.retain(|_, slot| match slot.state {
            State::Done { at, .. } => now.duration_since(at) < self.keep,
            State::InFlight { .. } => true,
        });
        if slots.len() <= MAX_KEPT {
            return;
        }
        let mut landed: Vec<(Instant, String)> = slots
            .iter()
            .filter_map(|(key, slot)| match slot.state {
                State::Done { at, .. } => Some((at, key.clone())),
                State::InFlight { .. } => None,
            })
            .collect();
        landed.sort_by_key(|(at, _)| *at);
        let excess = slots.len() - MAX_KEPT;
        for (_, key) in landed.into_iter().take(excess) {
            slots.remove(&key);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(key: &str, text: &str) -> Fingerprint {
        Fingerprint::new(key, "C1", None, None, text)
    }

    fn partial(posted: usize) -> Outcome {
        Outcome::Partial(Partial {
            posted,
            unsent: "rest".to_owned(),
            cause: "slow".to_owned(),
            retryable: true,
            resumable: true,
            thread_id: None,
            uncertain: true,
            refusal: None,
            first: None,
        })
    }

    #[test]
    fn a_restored_post_is_known_and_an_old_one_or_a_newer_claim_is_not_replaced() {
        // `#471 replication-c5-post-keys`.
        let keys = Arc::new(PostKeys::new());
        let now = 1_790_000_000_000;
        let kept = |key: &str, text: &str, posted: usize, at_ms: i64| Kept {
            fingerprint: fingerprint(key, text),
            outcome: partial(posted),
            at_ms,
        };
        // A claim made in this process before the restore is left alone.
        let Claim::Mine(_held) = keys.claim(fingerprint("live", "now")) else {
            panic!("a fresh post was not the caller's");
        };
        keys.restore(
            vec![
                kept("k", "hello", 1, now - 1_000),
                kept("old", "gone", 1, now - 25 * 60 * 60 * 1000),
                kept("live", "now", 2, now - 1_000),
            ],
            now,
        );
        assert!(matches!(
            keys.claim(fingerprint("k", "hello")),
            Claim::Known(outcome) if matches!(outcome.as_ref(), Outcome::Partial(p) if p.posted == 1)
        ));
        assert!(matches!(
            keys.claim(fingerprint("old", "gone")),
            Claim::Mine(_)
        ));
        assert!(matches!(
            keys.claim(fingerprint("live", "now")),
            Claim::Wait(_)
        ));
        // The record round-trips as the store keeps it.
        let record = serde_json::to_string(&kept("k", "hello", 1, now)).expect("JSON");
        let back: Kept = serde_json::from_str(&record).expect("parses");
        assert_eq!(back.key(), "k");
    }

    #[test]
    fn a_posted_record_is_filed_without_its_first_part_twice_and_an_older_one_still_reads() {
        // `#490 storage-io-audit`. A record as `#471 replication-c5-post-keys` filed it, with
        // `first` beside `parts`.
        let part = |id: &str, text: &str| {
            serde_json::json!({
                "id": id, "channel_id": "C1", "author": "owner", "author_id": "U1",
                "author_is_bot": false, "timestamp": "2026-09-15T12:00:00Z", "content": text,
            })
        };
        let older = serde_json::json!({
            "fingerprint": {"key": "k", "channel": "C1", "thread": null, "reply_to": null,
                            "text": "one two"},
            "outcome": {"Posted": {
                "channel": {"id": "C1", "label": "lead team", "writable": true},
                "first": part("1", "one"),
                "parts": [part("1", "one"), part("2", "two")],
            }},
            "at_ms": 1_790_000_000_000_i64,
        })
        .to_string();
        let kept: Kept = serde_json::from_str(&older).expect("an older record reads");
        let Outcome::Posted { first, parts, .. } = &kept.outcome else {
            panic!("not a posted record");
        };
        assert_eq!((first.id.0.as_str(), parts.len()), ("1", 2));

        let filed = serde_json::to_string(&kept).expect("JSON");
        assert!(!filed.contains("\"first\""), "{filed}");
        assert!(filed.len() < older.len());
        let back: Kept = serde_json::from_str(&filed).expect("parses");
        let Outcome::Posted { first, parts, .. } = &back.outcome else {
            panic!("not a posted record");
        };
        assert_eq!(first, &parts[0], "the first part is the first of the parts");
        assert_eq!(parts.len(), 2);

        let empty = filed.replace(&serde_json::to_string(parts).expect("JSON"), "[]");
        assert!(
            serde_json::from_str::<Kept>(&empty).is_err(),
            "a posted record with no part at all is refused, not given an invented one"
        );
        let null = older.replacen(
            &serde_json::to_string(&part("1", "one")).expect("JSON"),
            "null",
            1,
        );
        assert!(null.contains("\"first\":null"), "{null}");
        let back: Kept = serde_json::from_str(&null).expect("a null first reads");
        assert!(matches!(&back.outcome, Outcome::Posted { first, .. } if first.id.0 == "1"));

        // A partial post is filed as it always was.
        let partial = Kept {
            fingerprint: fingerprint("p", "rest"),
            outcome: partial(1),
            at_ms: 1_790_000_000_000,
        };
        let filed = serde_json::to_string(&partial).expect("JSON");
        assert!(
            filed.contains("\"outcome\":{\"Partial\":{\"posted\":1,"),
            "{filed}"
        );
        let back: Kept = serde_json::from_str(&filed).expect("parses");
        assert!(
            matches!(&back.outcome, Outcome::Partial(p) if p.posted == 1 && p.unsent == "rest")
        );
    }

    #[test]
    fn the_purge_forgets_what_landed_and_a_claim_made_before_it_is_not_filed() {
        // `#471 replication-c5-post-keys`.
        let keys = Arc::new(PostKeys::new());
        let Claim::Mine(done) = keys.claim(fingerprint("done", "landed")) else {
            panic!("a fresh post was not the caller's");
        };
        done.settle(Some(partial(1)));
        let Claim::Mine(flying) = keys.claim(fingerprint("flying", "now")) else {
            panic!("a fresh post was not the caller's");
        };
        assert!(flying.fileable());
        keys.forget_settled();
        assert!(
            !flying.fileable(),
            "a post in flight at the purge would be filed after it"
        );
        assert!(matches!(
            keys.claim(fingerprint("done", "landed")),
            Claim::Mine(_)
        ));
        assert!(
            matches!(keys.claim(fingerprint("flying", "now")), Claim::Wait(_)),
            "a post in flight is still waited for"
        );
        let Claim::Mine(after) = keys.claim(fingerprint("after", "later")) else {
            panic!("a fresh post was not the caller's");
        };
        assert!(after.fileable());
    }

    #[tokio::test]
    async fn a_settled_post_is_known_and_wakes_whoever_waits() {
        let keys = Arc::new(PostKeys::new());
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "hello")) else {
            panic!("a fresh post was not the caller's");
        };
        let Claim::Wait(waiting) = keys.claim(fingerprint("k", "hello")) else {
            panic!("a repeat in flight was not told to wait");
        };
        let waiter = tokio::spawn(waiting.settled());
        pending.settle(Some(partial(1)));
        assert!(matches!(
            waiter.await.unwrap().as_deref(),
            Some(Outcome::Partial(_))
        ));
        assert!(matches!(
            keys.claim(fingerprint("k", "hello")),
            Claim::Known(_)
        ));
        // Other words under the same key are another post.
        assert!(matches!(
            keys.claim(fingerprint("k", "rest")),
            Claim::Mine(_)
        ));
    }

    #[test]
    fn a_key_names_one_post_wherever_a_repeat_now_puts_it_and_refuses_other_words() {
        let keys = Arc::new(PostKeys::new());
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "hello")) else {
            panic!();
        };
        pending.settle(Some(partial(1)));
        // The same words, now said to go into a thread: the same post.
        let moved = Fingerprint::new("k", "C1", Some("t1"), None, "hello");
        assert!(matches!(keys.claim(moved.clone()), Claim::Known(_)));
        assert!(matches!(keys.lookup(&moved), Lookup::Known(_)));
        // Other words, or another channel: refused.
        assert!(matches!(keys.claim(fingerprint("k", "bye")), Claim::Reused));
        let elsewhere = Fingerprint::new("k", "C2", None, None, "hello");
        assert!(matches!(keys.claim(elsewhere.clone()), Claim::Reused));
        assert!(matches!(keys.lookup(&elsewhere), Lookup::Unknown));
        // The rest the 207 handed back is the one other post the key may name; forgotten, the key
        // still answers for the part that landed.
        let Claim::Mine(rest) = keys.claim(fingerprint("k", "rest")) else {
            panic!("the unsent rest was refused");
        };
        drop(rest);
        assert!(matches!(
            keys.claim(fingerprint("k", "hello")),
            Claim::Known(_)
        ));
    }

    #[tokio::test]
    async fn a_forgotten_or_dropped_claim_leaves_the_post_to_the_next_caller() {
        let keys = Arc::new(PostKeys::new());
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "a")) else {
            panic!();
        };
        let Claim::Wait(waiting) = keys.claim(fingerprint("k", "a")) else {
            panic!();
        };
        let waiter = tokio::spawn(waiting.settled());
        drop(pending);
        assert!(waiter.await.unwrap().is_none());
        assert!(matches!(
            keys.lookup(&fingerprint("k", "a")),
            Lookup::Unknown
        ));
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "a")) else {
            panic!("a dropped claim still held the post");
        };
        pending.settle(None);
        assert!(keys.is_empty());
    }

    #[test]
    fn what_landed_is_forgotten_after_its_keep() {
        let keys = Arc::new(PostKeys::with_keep(Duration::ZERO));
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "a")) else {
            panic!();
        };
        pending.settle(Some(partial(1)));
        assert!(matches!(
            keys.lookup(&fingerprint("k", "a")),
            Lookup::Unknown
        ));
    }

    #[test]
    fn progress_is_reported_while_in_flight() {
        let keys = Arc::new(PostKeys::new());
        let Claim::Mine(pending) = keys.claim(fingerprint("k", "a")) else {
            panic!();
        };
        let progress = pending.progress();
        progress.begin(3);
        progress.landed();
        let Lookup::InFlight(waiting) = keys.lookup(&fingerprint("k", "a")) else {
            panic!();
        };
        assert_eq!(waiting.progress(), (1, 3));
    }
}
