//! One upstream read for identical timeline requests, and a short memory of each newest answer.
//! `#278 timeline-single-flight`, remote-ops audit W4.
//!
//! # Why this exists
//!
//! Every `GET /api/v1/channels/{id}/timeline` used to be one read of the chat provider, however
//! many identical requests were in flight. Behind a bridge whose newest read rescans a whole space,
//! that multiplied: the page's two columns, its re-read after a live event and its poll each paid
//! the full price. Worse, the page gives up on a read after twenty seconds and asks again, while the
//! bridge's read runs on to its own bound of fifty. Giving up dropped this server's handler, and the
//! read with it, so the re-send started a second read behind the first: one profiled hour had 111
//! reads abandoned that way.
//!
//! Now identical requests share one read ([`TimelineFlights::read`]). The read is a detached task,
//! so it finishes whatever happens to the request that started it, and a re-send joins it. A newest
//! page, or a delta, that has just landed is kept for a few seconds (`chat.timeline_memo_ms`, three
//! by default, zero to turn the memory off), so a request arriving just after it is answered from it.
//!
//! # What is shared, and what is each caller's own
//!
//! Only what the provider answered: the backend's page, and for a generic forward read the position
//! that continues it ([`Fetched`]). The channel allowlist is checked before a request joins anything.
//! Everything after the read — the forward cursor envelope, the spoken forms, the noise marks,
//! read-ahead memory, rendering, thread names and the naming queue, read state, pins and the
//! bookmark — is worked out again for each caller from its own copy, so what a caller's scope
//! decides (a read token queues no naming) is decided for that caller alone.
//!
//! # Which requests are the same
//!
//! The same channel, view, thread, step back (`before`), forward cursor as the backend sees it, and
//! page size — and the same CHANGE GENERATION of the channel: how many changes the live feed has
//! delivered for it, how many storage purges there have been, and how many posts this server has
//! sent to it. A request made after any of those is a different request, so it never joins or
//! reuses a read that began before the change. That is what keeps the page's own re-reads honest:
//! it re-reads after a live message and after its own post, and the read that answers must be one
//! that began after them. A change upstream that this server is not told of is invisible for at
//! most the memory's few seconds, as it was invisible until the next read before.
//!
//! # What is kept, and the purge
//!
//! Each kept answer is channel text, in memory only, for the memory's few seconds; nothing reaches
//! the store. The storage purge calls [`TimelineFlights::clear`] (through
//! [`crate::read_ahead::ReadAhead::clear`]), which drops every kept answer and forgets every read
//! running: a request after the purge reads afresh, and a read that began before it keeps nothing
//! when it lands. A caller already waiting on such a read is still answered by it, as a page
//! already rendering is.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::chat::ChatError;
use crate::model::ChannelId;
use crate::threads::{TimelinePage, TimelineRequest, TimelineView};
use crate::timeline_forward::GenericPosition;

/// How long a newest timeline answer is kept when the configuration does not say: three seconds.
pub const DEFAULT_MEMO_MS: u64 = 3_000;

/// The longest `chat.timeline_memo_ms` may be: one minute. A memory longer than that would hide a
/// change nobody pushed for longer than the page's own poll interval.
pub const MAX_MEMO_MS: u64 = 60_000;

/// The most kept answers held at once. Beyond it every kept answer is dropped: they are only a
/// shortcut, and a read running is never dropped.
pub const MAX_KEPT: usize = 256;

/// What one upstream timeline read brought.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The backend's page, exactly as it answered.
    pub page: TimelinePage,
    /// For a generic forward read, the position that continues it.
    pub position: Option<GenericPosition>,
    /// When the read began, before the provider was asked: what read-ahead memory is told, since
    /// the text is no newer than that.
    pub began: std::time::Instant,
}

/// What a caller is handed: the shared read, or why it failed.
pub type Answer = Result<Arc<Fetched>, ChatError>;

/// How a caller came by its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shared {
    /// It started the read.
    Read,
    /// It joined a read already running.
    Joined,
    /// It was answered from a read that had just landed.
    Remembered,
}

impl Shared {
    /// The word for a log line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Joined => "joined",
            Self::Remembered => "remembered",
        }
    }
}

/// Which requests are the same request. See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    channel: ChannelId,
    view: std::mem::Discriminant<TimelineView>,
    thread: Option<String>,
    before: Option<String>,
    forward: Option<String>,
    limit: u16,
    generation: Generation,
}

impl Key {
    /// The key of `request` to `channel`, whose forward cursor, as the backend will be asked it, is
    /// `forward`: a native cursor or a generic position, written so two different ones never meet.
    #[must_use]
    pub fn new(channel: &ChannelId, request: &TimelineRequest, forward: Option<String>) -> Self {
        Self {
            channel: channel.clone(),
            view: std::mem::discriminant(&request.view),
            thread: request.thread_id.clone(),
            before: request.before.clone(),
            forward,
            limit: request.limit,
            generation: Generation::default(),
        }
    }
}

/// How many changes a channel had had when a request was made. See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Generation {
    pushes: u64,
    purges: u64,
    posts: u64,
    clears: u64,
}

enum Entry {
    Running {
        number: u64,
        answer: watch::Receiver<Option<Answer>>,
    },
    Kept {
        until: Instant,
        fetched: Arc<Fetched>,
    },
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Key, Entry>,
    posts: HashMap<ChannelId, u64>,
    clears: u64,
    next: u64,
}

impl Inner {
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, entry| match entry {
            Entry::Running { .. } => true,
            Entry::Kept { until, .. } => *until > now,
        });
        let kept = self
            .entries
            .values()
            .filter(|entry| matches!(entry, Entry::Kept { .. }))
            .count();
        if kept > MAX_KEPT {
            self.entries
                .retain(|_, entry| matches!(entry, Entry::Running { .. }));
        }
    }
}

/// The timeline reads running, and the newest answers just landed. See the module documentation.
#[derive(Default)]
pub struct TimelineFlights {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for TimelineFlights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineFlights").finish_non_exhaustive()
    }
}

impl TimelineFlights {
    /// Nothing running, nothing kept.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Note that this server has just sent a post to `channel`, whether or not it landed: no read
    /// that began before it answers a request made after it.
    pub fn posted(&self, channel: &ChannelId) {
        *self.lock().posts.entry(channel.clone()).or_default() += 1;
    }

    /// Drop every kept answer and forget every read running. For the storage purge.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.clears += 1;
    }

    /// How many answers are kept or reads running. For tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// Whether nothing is kept and nothing is running. What [`TimelineFlights::clear`] leaves.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The answer to the request `key` names: a kept one, the one running, or a read started now.
    ///
    /// `seen` is the channel's live-feed pushes and storage purges when the request was made
    /// ([`crate::recent::ReadMark::seen`]). `memo` is how long a read this call starts is kept once
    /// it lands; zero keeps nothing. `start` makes the read; it runs in a task of its own, and is
    /// never cancelled by any caller going away.
    pub async fn read<F, Fut>(
        self: &Arc<Self>,
        mut key: Key,
        seen: (u64, u64),
        memo: Duration,
        start: F,
    ) -> (Answer, Shared)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(TimelinePage, Option<GenericPosition>), ChatError>>
            + Send
            + 'static,
    {
        let (mut receiver, claim) = {
            let mut inner = self.lock();
            key.generation = Generation {
                pushes: seen.0,
                purges: seen.1,
                posts: inner.posts.get(&key.channel).copied().unwrap_or(0),
                clears: inner.clears,
            };
            inner.prune(Instant::now());
            match inner.entries.get(&key) {
                Some(Entry::Kept { fetched, .. }) => {
                    return (Ok(Arc::clone(fetched)), Shared::Remembered);
                }
                Some(Entry::Running { answer, .. }) => (answer.clone(), None),
                None => {
                    let number = inner.next;
                    inner.next += 1;
                    let (sender, receiver) = watch::channel(None);
                    inner.entries.insert(
                        key.clone(),
                        Entry::Running {
                            number,
                            answer: receiver.clone(),
                        },
                    );
                    let claim = Claim {
                        flights: Arc::clone(self),
                        key,
                        number,
                        memo,
                        sender: Some(sender),
                    };
                    (receiver, Some(claim))
                }
            }
        };
        let shared = match claim {
            Some(claim) => {
                let work = start();
                tokio::spawn(async move {
                    let began = std::time::Instant::now();
                    let answer = work.await.map(|(page, position)| {
                        Arc::new(Fetched {
                            page,
                            position,
                            began,
                        })
                    });
                    claim.finish(answer);
                });
                Shared::Read
            }
            None => Shared::Joined,
        };
        let answer = match receiver.wait_for(Option::is_some).await {
            Ok(answer) => answer.clone().unwrap_or_else(|| Err(ended())),
            Err(_) => Err(ended()),
        };
        (answer, shared)
    }

    /// Put a read's answer where later requests find it: kept for `memo`, or gone. Only the read
    /// still filed under `key` — one a purge has not forgotten — is filed again.
    fn settle(&self, key: &Key, number: u64, answer: &Answer, memo: Duration) {
        let mut inner = self.lock();
        let ours = matches!(
            inner.entries.get(key),
            Some(Entry::Running { number: running, .. }) if *running == number
        );
        if !ours {
            return;
        }
        match answer {
            Ok(fetched) if !memo.is_zero() => {
                inner.entries.insert(
                    key.clone(),
                    Entry::Kept {
                        until: Instant::now() + memo,
                        fetched: Arc::clone(fetched),
                    },
                );
            }
            _ => {
                inner.entries.remove(key);
            }
        }
    }
}

fn ended() -> ChatError {
    ChatError::Transport("the shared timeline read ended without an answer".to_owned())
}

/// The right, and the duty, to answer one read. Dropped unfinished — the task torn down with the
/// runtime, or a panic in the provider — it still answers every caller waiting, with a failure,
/// and takes the read off the list, so nothing waits on a read that will never land.
struct Claim {
    flights: Arc<TimelineFlights>,
    key: Key,
    number: u64,
    memo: Duration,
    sender: Option<watch::Sender<Option<Answer>>>,
}

impl Claim {
    fn finish(mut self, answer: Answer) {
        self.flights
            .settle(&self.key, self.number, &answer, self.memo);
        if let Some(sender) = self.sender.take() {
            sender.send_replace(Some(answer));
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let answer = Err(ended());
            self.flights
                .settle(&self.key, self.number, &answer, Duration::ZERO);
            sender.send_replace(Some(answer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key(view: TimelineView) -> Key {
        Key::new(
            &ChannelId("1".to_owned()),
            &TimelineRequest {
                view,
                ..TimelineRequest::default()
            },
            None,
        )
    }

    type Read = std::pin::Pin<
        Box<dyn Future<Output = Result<(TimelinePage, Option<GenericPosition>), ChatError>> + Send>,
    >;

    fn counting(reads: &Arc<AtomicUsize>, answer: Result<(), ChatError>) -> impl FnOnce() -> Read {
        let reads = Arc::clone(reads);
        move || {
            Box::pin(async move {
                reads.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(500)).await;
                answer.map(|()| (TimelinePage::default(), None))
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_is_shared_with_every_caller_waiting_and_never_kept() {
        let flights = Arc::new(TimelineFlights::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let refused = || {
            Err(ChatError::Status {
                status: 502,
                body: "bad gateway".to_owned(),
            })
        };
        let memo = Duration::from_secs(3);
        let ((a, how_a), (b, how_b)) = tokio::join!(
            flights.read(
                key(TimelineView::Main),
                (0, 0),
                memo,
                counting(&reads, refused())
            ),
            flights.read(
                key(TimelineView::Main),
                (0, 0),
                memo,
                counting(&reads, refused())
            ),
        );
        assert!(a.is_err() && b.is_err());
        assert_eq!((how_a, how_b), (Shared::Read, Shared::Joined));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert!(flights.is_empty(), "a failure was kept");
        let (again, how) = flights
            .read(
                key(TimelineView::Main),
                (0, 0),
                memo,
                counting(&reads, Ok(())),
            )
            .await;
        assert!(again.is_ok());
        assert_eq!(how, Shared::Read);
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_push_a_post_or_a_purge_starts_a_new_read() {
        let flights = Arc::new(TimelineFlights::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let memo = Duration::from_secs(3);
        let read = |seen| {
            flights.read(
                key(TimelineView::Flat),
                seen,
                memo,
                counting(&reads, Ok(())),
            )
        };
        assert_eq!(read((0, 0)).await.1, Shared::Read);
        assert_eq!(read((0, 0)).await.1, Shared::Remembered);
        assert_eq!(read((1, 0)).await.1, Shared::Read, "after a push");
        flights.posted(&ChannelId("1".to_owned()));
        assert_eq!(read((1, 0)).await.1, Shared::Read, "after a post");
        flights.posted(&ChannelId("elsewhere".to_owned()));
        assert_eq!(
            read((1, 0)).await.1,
            Shared::Remembered,
            "a post to another channel"
        );
        flights.clear();
        assert!(flights.is_empty());
        assert_eq!(read((1, 0)).await.1, Shared::Read, "after the purge");
        assert_eq!(reads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_running_when_the_purge_comes_answers_its_callers_and_keeps_nothing() {
        let flights = Arc::new(TimelineFlights::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let memo = Duration::from_secs(3);
        let waiting = flights.read(
            key(TimelineView::Main),
            (0, 0),
            memo,
            counting(&reads, Ok(())),
        );
        let purge = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flights.clear();
        };
        let ((answer, _), ()) = tokio::join!(waiting, purge);
        assert!(answer.is_ok());
        assert!(
            flights.is_empty(),
            "the purge was undone by a read that began before it"
        );
    }

    fn provider_panics() -> TimelinePage {
        panic!("the provider panicked")
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_whose_task_dies_answers_with_a_failure_and_is_forgotten() {
        let flights = Arc::new(TimelineFlights::new());
        let start = || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok((provider_panics(), None))
        };
        let (answer, how) = flights
            .read(
                key(TimelineView::Main),
                (0, 0),
                Duration::from_secs(3),
                start,
            )
            .await;
        assert_eq!(how, Shared::Read);
        assert!(matches!(answer, Err(ChatError::Transport(_))), "{answer:?}");
        assert!(flights.is_empty());
    }
}
