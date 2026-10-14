//! The message cache: a disposable on-disk copy of channel messages. `#208 persistent-cache`.
//!
//! **Filled, not served yet** (milestone `cache-fill`). A server opens a cache only when its
//! configuration's `[message_cache]` table says `enabled = true` and the owner has not turned it
//! off ([`lifecycle`]). The cache then sits behind the server's one [`Sink`], which fills it from:
//!
//! - what write-scope and server-owned reads see. The read sites offer it under a [`FillGrant`],
//!   which a read-scope caller never has ([`grant`]), and each read claims only what it proves
//!   ([`observe`]);
//! - every change the live feed publishes, and every message the server posts.
//!
//! The sink fills a channel only while it is on the server's list, and sweeps away every channel
//! that is not. It fills a provider only once that provider's own client has said which account it
//! is, and the file agrees. When a provider refuses to let the server read a channel, a thread or
//! anything at all, the sink stops trusting what it holds of it at once, and erases that once the
//! refusal is confirmed ([`access`]). The store's summaries follow what the cache learns
//! ([`orphans`]). The owner's switch, the purge and every erase of the whole file are in
//! [`lifecycle`] and [`marker`].
//!
//! Nothing reads the cache back to answer anyone: what a caller is answered is exactly what it was
//! before the cache existed. Instead, each read a later milestone will answer from the cache is
//! shadowed ([`shadow`]). What the cache would have served is counted in memory, beside the
//! upstream answer, and reported in diagnostics. For operators, see the README's section "The
//! message cache".
//!
//! # A separate file, never tables in the store
//!
//! The cache is its own SQLite file beside the durable store, and is never `ATTACH`ed to it: the
//! store has no busy timeout, so a cache connection reading it would make the store's own writes
//! fail. It is disposable: erasing it loses nothing that cannot be read from the provider again,
//! and every erase is a whole-file or whole-channel operation (see [`MessageCache::erase_file`]).
//! Summaries are never in it.
//!
//! # One writer, a sequencer, and readers that check before they serve
//!
//! - Every write is an [`Observation`] offered with a [`Stamp`] taken when its upstream read
//!   STARTED. One thread applies them in the order they were offered: [`MessageCache::offer`]
//!   takes the sequencer, numbers the offer and queues it while holding it, so queue order is
//!   sequence order and the applied sequence is always contiguous.
//! - The queue is bounded by BYTES (32 MB), not by count. An offer that does not fit is dropped,
//!   counted, and marks its channel suspect; no producer ever waits on the cache.
//! - The writer commits after about 25 ms of work, or at once when a reader raises the flush flag.
//!   A read that must be current notes the sequence, raises the flag and waits (the barrier).
//! - Readers use two read-only connections. Each re-checks the file generation and the channel's
//!   epoch AFTER its SQL and before handing anything back, so a row read before an erase is never
//!   served after it.
//! - Anything that can take long — the rows of an erased channel, the age and revalidation pass,
//!   eviction, rebuilding the index text, compacting and merging the index, checkpoints — is
//!   background work the writer does in steps of about 25 ms BETWEEN batches. A batch is
//!   published to barrier waiters as soon as it commits, never after an unrelated erase or an
//!   eviction finishes. An erase itself is logical at once: the writer detaches the channel's
//!   rows from its name in one short transaction, so no read can reach them, and deletes them in
//!   later steps.
//!
//! # Calling it from async code
//!
//! Every read ([`MessageCache::newest`] and the rest), [`MessageCache::open`],
//! [`MessageCache::erase_file`] and [`MessageCache::quiesce_blocking`] block the calling thread:
//! a read on SQLite, a file erase on the writer's acknowledgement (up to a minute) and on reads in
//! flight. Call them from `tokio::task::spawn_blocking`, never directly on a runtime worker.
//! [`MessageCache::offer`] never blocks, and [`MessageCache::barrier`] is the async form of the
//! barrier. A read's closure must not do slow work outside SQL: a file erase waits for it.
//!
//! # What can never be written
//!
//! A row for a channel the current route snapshot does not list, a row older than the age limit,
//! coverage below it, an observation whose stamp is older than an erase, and anything at all that a
//! read-scope caller saw: such a caller is never given a stamp to write with, because every fill
//! begins at the [`Sink`] under a [`FillGrant`] and a read-scope caller has none ([`grant`]).
//! A deletion ([`Observation::removes_only`]) is the one observation applied to a channel off the
//! snapshot, and then only to messages the file already holds of it.
//!
//! # What the durable store must follow
//!
//! Summaries are never in the cache: they stay in the store, keyed by the text they summarise.
//! The cache is where the server learns that such text changed, so once a transaction that
//! deleted a message because it was deleted upstream, replaced a message's text, or erased a
//! channel has COMMITTED, the writer sends a [`WriterEvent`] to whoever subscribed
//! ([`MessageCache::subscribe_events`]), through a bounded queue it never waits on: a full one
//! drops the event and counts it. Rows the cache itself let go of — aged out, evicted, or erased
//! with the whole file — send nothing: they say nothing about the message.
//!
//! # Privacy
//!
//! `secure_delete` on every connection, FTS5 `secure-delete` from creation and on at every commit,
//! no `INSERT OR`-style replacing writes anywhere (a source test enforces it), the write-ahead log
//! truncated after every deletion, and a deleted message leaves only a [tombstone](rows) of ids
//! and times. Logs carry SQLite result codes and counts, never text, ids or hashes.
//!
//! A message deleted upstream, or edited, leaves the full-text index in place, at once. Deleting
//! a row in place rewrites the index page of every trigram it holds, about five milliseconds and
//! hundreds of kilobytes of log a row (measured), so a BULK deletion (a large erase or age pass,
//! eviction, a rebuild of the index text) deletes by marker instead, about a tenth of a
//! millisecond a row, with `secure-delete` switched off and back on inside its own transaction.
//! Until a compaction has rewritten the index, the deleted rows' entries stay in its old pages,
//! unreachable from any query; the compaction is owed in the file from the moment the markers
//! are written, runs in steps between batches, and is over only when FTS5's own structure record
//! says its merge has taken in every segment that existed when it began. A crash before then
//! leaves it owed, and the next open runs it.
//!
//! # Files and threads
//!
//! The writer, a checkpointer that copies the log back into the database concurrently (the
//! writer's commits never checkpoint), and two readers, each with its own connection.

pub mod access;
mod checkpoint;
pub mod coverage;
pub mod file;
pub mod grant;
pub mod lifecycle;
pub mod marker;
pub mod merge;
pub mod observe;
pub mod orphans;
mod reader;
pub mod rows;
pub mod schema;
pub mod search;
pub mod search_text;
pub mod shadow;
pub mod sink;
mod structure;
mod writer;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

pub use file::default_path;
pub use grant::{FillGrant, ServerFill};
pub use rows::{CachedMessage, ChanKey, Incoming, Scope};
pub use search_text::{PlainText, Rendered, SearchTextSource};
pub use sink::{Fill, Shadowed, Sink};

use crate::model::{ChannelId, MessageId};

/// The most bytes of queued observations the writer's queue holds before offers are dropped.
pub const QUEUE_BYTES: usize = 32 * 1024 * 1024;
/// How long the writer works before it commits a batch.
pub const BATCH: Duration = Duration::from_millis(25);
/// How long a deleted message's tombstone is kept, so a read that started before the deletion
/// cannot bring it back.
pub const TOMBSTONE_KEEP_MS: i64 = 7 * DAY_MS;
/// A row neither revalidated by an edit-and-delete-aware read nor observed for this long is
/// deleted, whatever else holds: "erased within a week" whatever the refill budget allows.
pub const REVALIDATE_WITHIN_MS: i64 = 7 * DAY_MS;
/// The most rows one eviction step deletes. Steps are bounded by [`STEP`] as well.
pub const DELETE_CHUNK: usize = 1_000;
/// How long one background step of the writer works before it commits and serves the queue.
pub const STEP: Duration = Duration::from_millis(25);
/// How long a file erase lets in-flight reads finish before it interrupts their statements, and
/// how often it interrupts again while one is still running.
pub const ERASE_GRACE: Duration = Duration::from_millis(200);

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Milliseconds since the Unix epoch, by the system clock.
#[must_use]
pub fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Local time, as the cache reads it. Injectable so tests can move a week in a line.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        wall_ms()
    }
}

/// Why a cache could not be opened, or an operation on it failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CacheError {
    /// The linked SQLite lacks something the cache needs. The server runs without a cache.
    #[error("the message cache is unavailable: {0}")]
    Unavailable(String),
    /// The file at the path is not one this server made. It is left exactly as it is.
    #[error("the message cache refuses its file: {0}")]
    Foreign(String),
    /// Another open holds the path.
    #[error("{0}")]
    Locked(String),
    /// The directory or file cannot be made private, or created.
    #[error("{0}")]
    Filesystem(String),
    /// SQLite failed; the text is a result code, never a message.
    #[error("the message cache failed: {0}")]
    Sqlite(String),
}

/// The bounds a cache enforces on write, by eviction, and in its hourly pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Oldest message kept, in days.
    pub max_days: u16,
    /// Per-channel age limits by channel id, overriding `max_days`.
    pub channel_days: HashMap<String, u16>,
    /// Ceiling on the database plus `-wal` and `-shm`, in bytes.
    pub max_bytes: u64,
}

impl Limits {
    /// The bounds `[message_cache]` sets.
    #[must_use]
    pub fn from_config(config: &crate::config::MessageCacheConfig) -> Self {
        Self {
            max_days: config.max_days,
            channel_days: config
                .channels
                .iter()
                .map(|limit| (limit.id.0.clone(), limit.max_days))
                .collect(),
            max_bytes: u64::from(config.max_megabytes) * 1024 * 1024,
        }
    }

    /// The age cutoff for one channel at `now_ms`: rows created before it are neither written
    /// nor kept.
    #[must_use]
    pub fn cutoff_ms(&self, channel_id: &str, now_ms: i64) -> i64 {
        let days = self
            .channel_days
            .get(channel_id)
            .copied()
            .unwrap_or(self.max_days);
        now_ms.saturating_sub(i64::from(days) * DAY_MS)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::from_config(&crate::config::MessageCacheConfig::default())
    }
}

/// Everything [`MessageCache::open`] needs.
pub struct CacheOptions {
    /// The cache file.
    pub path: PathBuf,
    /// The durable store's file, which the cache must never be.
    pub store_path: Option<PathBuf>,
    /// The bounds to enforce.
    pub limits: Limits,
    /// How bodies are rendered for the index.
    pub search_text: Arc<dyn SearchTextSource>,
    /// Local time.
    pub clock: Arc<dyn Clock>,
}

impl CacheOptions {
    /// Options for `path` with the default bounds, the plain-text index and the system clock.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            store_path: None,
            limits: Limits::default(),
            search_text: Arc::new(PlainText),
            clock: Arc::new(SystemClock),
        }
    }
}

impl std::fmt::Debug for CacheOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheOptions")
            .field("path", &self.path)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// The channels and providers that may hold rows: the route snapshot the writer gates on, and
/// the allowlist [`MessageCache::retain_only`] sweeps to. Hidden channels are not in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteSnapshot {
    channels: HashSet<ChanKey>,
    providers: HashSet<String>,
}

impl RouteSnapshot {
    /// A snapshot of exactly these channels, and these configured provider keys.
    #[must_use]
    pub fn new(
        channels: impl IntoIterator<Item = ChanKey>,
        providers: impl IntoIterator<Item = String>,
    ) -> Self {
        let channels: HashSet<ChanKey> = channels.into_iter().collect();
        let mut providers: HashSet<String> = providers.into_iter().collect();
        providers.extend(channels.iter().map(|c| c.provider_key.clone()));
        Self {
            channels,
            providers,
        }
    }

    /// Whether a channel may hold rows.
    #[must_use]
    pub fn allows(&self, chan: &ChanKey) -> bool {
        self.channels.contains(chan)
    }

    /// Whether a provider key is configured.
    #[must_use]
    pub fn has_provider(&self, provider_key: &str) -> bool {
        self.providers.contains(provider_key)
    }
}

/// Where a write may land, captured when its upstream read started. The writer drops an
/// observation whose stamp is older than an erase of its file or channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    generation: u64,
    epoch: u64,
}

/// Where an exhaustive listing begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStart {
    /// The provider proved there is nothing older in the scope.
    ScopeStart,
    /// At this provider time, inclusive.
    At(i64),
}

/// Where an exhaustive listing ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEnd {
    /// At the scope's newest end: the read was of the newest page.
    Newest,
    /// Before this provider time, exclusive.
    Before(i64),
}

/// A provider's proof that a listing holds every message of its scope over a span. "The read
/// came back short" is never one: providers drop rows a client cannot render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proof {
    /// Where the proven span begins.
    pub from: ListStart,
    /// Where it ends.
    pub until: ListEnd,
    /// Whether the source could not have omitted rows.
    pub complete: bool,
    /// Whether edits and deletions are reflected too, not only creations.
    pub full: bool,
    /// What kind of read it was.
    pub source: coverage::Source,
}

/// A read of one scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The scope read.
    pub scope: Scope,
    /// What it returned.
    pub rows: Vec<Incoming>,
    /// Local time the read started; for a snapshot that says how old it is, the earlier of the
    /// two.
    pub observed_ms: i64,
    /// The provider's proof of exhaustion, when there is one. Without one, rows are only upserted.
    pub proof: Option<Proof>,
}

/// One thread record, from a thread list or derived from its messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRecord {
    /// The opaque thread id.
    pub thread_id: String,
    /// Its root message, when known.
    pub root_id: Option<MessageId>,
    /// Replies, when the provider says.
    pub reply_count: Option<u64>,
    /// Whether that count is exact.
    pub reply_count_exact: bool,
    /// Provider time of the latest activity known.
    pub last_activity_ms: i64,
}

/// A change from the live feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Push {
    /// A message created or edited.
    Upsert {
        /// The message.
        message: Box<Incoming>,
        /// Whether it was an edit rather than a creation.
        update: bool,
        /// Whether the sender marked the body as possibly a snippet.
        partial: bool,
    },
    /// A message deleted.
    Delete {
        /// Its id.
        id: MessageId,
        /// Its creation time, when the feed says.
        created_ms: Option<i64>,
    },
}

/// Something a read or the feed saw, offered to the writer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// A read of one scope, possibly exhaustive.
    Listing(Listing),
    /// Messages from a read that is not a listing of a scope: a delta, a posted message.
    Rows {
        /// The messages.
        rows: Vec<Incoming>,
        /// A scope they are listed in, beyond their home.
        scope: Option<Scope>,
        /// Local time the read started.
        observed_ms: i64,
    },
    /// Ids a complete delta reported deleted.
    Deleted {
        /// The ids.
        ids: Vec<MessageId>,
        /// Local time the read started.
        observed_ms: i64,
    },
    /// One live change, observed at `observed_ms` (local).
    Push {
        /// The change.
        push: Push,
        /// Local time it was published.
        observed_ms: i64,
    },
    /// Thread records.
    Threads {
        /// The records; one with a known count of zero replies is not a thread and is skipped.
        threads: Vec<ThreadRecord>,
        /// Local time the read started.
        observed_ms: i64,
    },
}

impl Observation {
    /// Whether this only takes messages out: a pushed deletion, or the ids a complete delta
    /// reports deleted. It writes nothing a provider's account could have read, so it may be
    /// applied whatever is known of that account, and to a channel the route snapshot does not
    /// name, as far as the file already holds it.
    #[must_use]
    pub fn removes_only(&self) -> bool {
        matches!(
            self,
            Self::Deleted { .. }
                | Self::Push {
                    push: Push::Delete { .. },
                    ..
                }
        )
    }

    /// Roughly how many bytes this holds while queued: every string a message carries, derived
    /// ones included, because an [`Incoming`] built field by field may still hold them.
    fn estimated_bytes(&self) -> usize {
        fn row(incoming: &Incoming) -> usize {
            let m = &incoming.message;
            let thread = m.thread.as_ref().map_or(0, |t| {
                64 + t.id.len() + t.root_message_id.as_ref().map_or(0, |r| r.0.len())
            });
            let reactions = m.reactions.as_ref().map_or(0, |list| {
                list.iter()
                    .map(|r| 48 + r.emoji.len() + r.custom_id.as_ref().map_or(0, String::len))
                    .sum()
            });
            let heading = m.heading.as_ref().map_or(0, |h| {
                [&h.tag, &h.time, &h.title]
                    .iter()
                    .map(|part| part.as_ref().map_or(0, String::len))
                    .sum::<usize>()
                    + 64
            });
            256 + m.content.len() * 2
                + m.author.len()
                + m.author_id.0.len()
                + m.id.0.len()
                + m.channel_id.0.len()
                + m.timestamp.len()
                + m.spoken_time.len()
                + m.edited_timestamp.as_ref().map_or(0, String::len)
                + m.spoken_content.len()
                + m.content_html.len()
                + m.reply_to.as_ref().map_or(0, |r| r.0.len())
                + thread
                + reactions
                + heading
        }
        match self {
            Self::Listing(listing) => 128 + listing.rows.iter().map(row).sum::<usize>(),
            Self::Rows { rows, .. } => 128 + rows.iter().map(row).sum::<usize>(),
            Self::Deleted { ids, .. } => 128 + ids.iter().map(|id| 64 + id.0.len()).sum::<usize>(),
            Self::Push { push, .. } => match push {
                Push::Upsert { message, .. } => 128 + row(message),
                Push::Delete { id, .. } => 128 + id.0.len(),
            },
            Self::Threads { threads, .. } => 128 + threads.len() * 128,
        }
    }
}

/// Something the writer did that the durable store's summaries must follow, sent once the
/// transaction that did it has committed. See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriterEvent {
    /// A message was deleted because it was deleted upstream (`live_hash` is `None`), or its text
    /// was replaced by newer text (`live_hash` is that text's [`crate::summarize::content_hash`]):
    /// a summary of any other text is of words the message no longer says.
    SummaryOrphan {
        /// The channel, by its own id: the store keys summaries without a provider.
        channel_id: ChannelId,
        /// The message.
        message_id: MessageId,
        /// The hash of the text the message says now, if it still exists.
        live_hash: Option<u64>,
    },
    /// A channel was erased from the file: swept off the route snapshot, or erased by name.
    ChannelErased {
        /// The channel, by its own id. The same id may still be listed under another provider.
        channel_id: ChannelId,
    },
}

/// The account a provider's rows were read as, as the file records it: what a later start compares
/// the provider's answer to, so that rows read as one account are never served as another's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// The provider's own id for the account.
    pub account_id: String,
    /// The workspace the account belongs to, where the provider has workspaces.
    pub workspace_id: Option<String>,
    /// Local time the row was written, by the cache's clock.
    pub verified_ms: i64,
}

impl Account {
    /// Whether this is the account `account_id` in `workspace_id`: both must match.
    #[must_use]
    pub fn is(&self, account_id: &str, workspace_id: Option<&str>) -> bool {
        self.account_id == account_id && self.workspace_id.as_deref() == workspace_id
    }
}

/// What happened to an offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offered {
    /// Queued with this sequence number.
    Queued(u64),
    /// Dropped: the queue was full or closed. The channel is now suspect.
    Dropped,
    /// No cache file is open.
    Off,
}

/// The answer of a read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read<T> {
    /// What the cache holds, checked against erases after the SQL ran.
    Served(T),
    /// No cache file is open.
    Off,
    /// An erase happened while the read ran: nothing it found may be served.
    Moved,
    /// The channel (or the scope) may not be read from the cache: it is off the route snapshot,
    /// an erase of it is waiting to be applied, access to it is in doubt, or its provider is
    /// suspended.
    Blocked,
    /// SQLite failed; a result code.
    Failed(String),
}

impl<T> Read<T> {
    /// The served value, if any.
    pub fn served(self) -> Option<T> {
        match self {
            Self::Served(value) => Some(value),
            _ => None,
        }
    }

    /// The same answer, with a served value turned into another.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Read<U> {
        match self {
            Self::Served(value) => Read::Served(f(value)),
            Self::Off => Read::Off,
            Self::Moved => Read::Moved,
            Self::Blocked => Read::Blocked,
            Self::Failed(code) => Read::Failed(code),
        }
    }
}

/// Counts of what the cache holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Messages held.
    pub messages: u64,
    /// Channels with anything held.
    pub channels: u64,
    /// Tombstones held.
    pub tombstones: u64,
    /// Coverage intervals held.
    pub intervals: u64,
}

/// What a file erase removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EraseReport {
    /// Bytes the deleted files held.
    pub bytes: u64,
    /// Whether a fresh file was opened in its place.
    pub reopened: bool,
}

/// Powers of two above one microsecond that [`BatchDurations`] spans: 2^27 µs, about 134 s.
const BATCH_OCTAVES: u32 = 27;

/// How long the writer's batches took, each from its first observation to its commit, in
/// microseconds: counts in powers of two from one microsecond, each split into equal steps as the
/// upstream profile's are ([`crate::upstream_profile::Histogram`]), so a percentile read off it is
/// within about 6 % of the true value. Numbers only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchDurations(crate::upstream_profile::Histogram);

impl Default for BatchDurations {
    fn default() -> Self {
        Self(crate::upstream_profile::Histogram::new(1, BATCH_OCTAVES))
    }
}

impl BatchDurations {
    /// How many batches were counted.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.0.count()
    }

    /// The duration, in microseconds, below which a fraction `q` of the batches fall. 0 when none
    /// has been counted.
    #[must_use]
    pub fn quantile_us(&self, q: f64) -> u64 {
        // A histogram's quantile is a non-negative bucket midpoint, kept within what was seen.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let us = self.0.quantile(q).round() as u64;
        us
    }

    /// The histogram itself.
    #[must_use]
    pub fn histogram(&self) -> &crate::upstream_profile::Histogram {
        &self.0
    }

    pub(crate) fn record(&mut self, us: u64) {
        self.0.record(us);
    }
}

impl serde::Serialize for BatchDurations {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut out = serializer.serialize_struct("BatchDurations", 5)?;
        out.serialize_field("count", &self.count())?;
        out.serialize_field("p50_us", &self.quantile_us(0.5))?;
        out.serialize_field("p99_us", &self.quantile_us(0.99))?;
        out.serialize_field("max_us", &self.0.max())?;
        out.serialize_field("octaves_us", &self.0.octaves())?;
        out.end()
    }
}

/// Numbers only: what the writer has done since the handle opened.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct CacheStats {
    /// Observations and control operations applied.
    pub ops_applied: u64,
    /// Offers dropped because the queue was full or closed.
    pub ops_dropped: u64,
    /// Bytes of those offers.
    pub bytes_dropped: u64,
    /// Observations dropped because an erase came after their read started.
    pub ops_stale: u64,
    /// Observations dropped because their channel is not in the route snapshot.
    pub ops_unrouted: u64,
    /// Observations that failed in SQLite and were rolled back alone.
    pub ops_failed: u64,
    /// Batches of observations committed.
    pub batches: u64,
    /// The longest batch, from its first observation to its commit, in microseconds.
    pub batch_max_us: u64,
    /// Background steps run between batches: deletions, compaction, merges, checkpoints.
    pub steps: u64,
    /// The longest background step, in microseconds: what a barrier can wait behind.
    pub step_max_us: u64,
    /// Steps that merged the full-text index's segments (it never merges inside a batch).
    pub merge_steps: u64,
    /// The longest checkpoint on the checkpointer's own thread, in microseconds.
    pub checkpoint_max_us: u64,
    /// The longest checkpoint the writer ran itself, in microseconds: a step, which a barrier
    /// can wait behind.
    pub writer_checkpoint_max_us: u64,
    /// Message rows inserted or changed.
    pub rows_written: u64,
    /// Message rows offered that changed nothing and were not written.
    pub rows_unchanged: u64,
    /// Message rows refused for being older than the age limit.
    pub rows_too_old: u64,
    /// Message rows refused because a later deletion's tombstone holds the id.
    pub rows_held_by_tombstone: u64,
    /// Messages deleted and tombstoned.
    pub tombstoned: u64,
    /// Rows deleted to stay under the size limit.
    pub evicted_rows: u64,
    /// Eviction steps (one transaction each).
    pub eviction_chunks: u64,
    /// The longest eviction step, in microseconds.
    pub eviction_chunk_max_us: u64,
    /// Rows deleted by the age limit or for want of revalidation.
    pub expired_rows: u64,
    /// Channels erased (detached from their names; their rows go in later steps).
    pub erased_channels: u64,
    /// Message rows those erases have deleted so far.
    pub erased_rows: u64,
    /// Full-text index compactions: after deletions by marker (a large erase or age pass,
    /// eviction, a rebuild of the index text).
    pub compactions: u64,
    /// Steps those compactions took.
    pub compaction_steps: u64,
    /// The longest compaction, start to finish, in microseconds.
    pub compaction_max_us: u64,
    /// The longest single compaction slice (one transaction), in microseconds.
    pub compaction_slice_max_us: u64,
    /// FTS integrity checks that failed (and were repaired by a rebuild).
    pub integrity_failures: u64,
    /// `TRUNCATE` checkpoints that emptied the write-ahead log.
    pub checkpoints_truncated: u64,
    /// [`WriterEvent`]s sent to their subscriber.
    pub orphans_sent: u64,
    /// [`WriterEvent`]s dropped because their subscriber's queue was full or gone.
    pub orphans_dropped: u64,
    /// How long each batch took, as [`CacheStats::batch_max_us`] measures it: its p50 and p99.
    pub batch_us: BatchDurations,
}

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        #[derive(Debug, Default)]
        pub(crate) struct Stats {
            $(pub(crate) $name: AtomicU64,)*
            /// Every batch's duration, in microseconds. Locked by the writer once per batch, and
            /// by whoever reads the stats.
            pub(crate) batch_us: Mutex<BatchDurations>,
        }
        impl Stats {
            fn snapshot(&self) -> CacheStats {
                CacheStats {
                    $($name: self.$name.load(Ordering::Relaxed),)*
                    batch_us: lock(&self.batch_us).clone(),
                }
            }
        }
    };
}

counters!(
    ops_applied,
    ops_dropped,
    bytes_dropped,
    ops_stale,
    ops_unrouted,
    ops_failed,
    batches,
    batch_max_us,
    steps,
    step_max_us,
    merge_steps,
    checkpoint_max_us,
    writer_checkpoint_max_us,
    rows_written,
    rows_unchanged,
    rows_too_old,
    rows_held_by_tombstone,
    tombstoned,
    evicted_rows,
    eviction_chunks,
    eviction_chunk_max_us,
    expired_rows,
    erased_channels,
    erased_rows,
    compactions,
    compaction_steps,
    compaction_max_us,
    compaction_slice_max_us,
    integrity_failures,
    checkpoints_truncated,
    orphans_sent,
    orphans_dropped,
);

impl Stats {
    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn max(counter: &AtomicU64, value: u64) {
        counter.fetch_max(value, Ordering::Relaxed);
    }

    /// Count one committed batch that took `us` microseconds.
    pub(crate) fn batch(&self, us: u64) {
        Self::add(&self.batches, 1);
        Self::max(&self.batch_max_us, us);
        lock(&self.batch_us).record(us);
    }
}

/// File generation and per-channel epochs. A stamp holds both; an erase bumps one.
#[derive(Debug, Default)]
pub(crate) struct Epochs {
    generation: AtomicU64,
    channels: Mutex<HashMap<ChanKey, u64>>,
    providers: Mutex<HashMap<String, u64>>,
}

impl Epochs {
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A channel's epoch: its own count plus its provider's, so bumping either moves it.
    pub(crate) fn epoch(&self, chan: &ChanKey) -> u64 {
        let own = lock(&self.channels).get(chan).copied().unwrap_or(0);
        let provider = lock(&self.providers)
            .get(&chan.provider_key)
            .copied()
            .unwrap_or(0);
        own.wrapping_add(provider)
    }

    pub(crate) fn bump_channel(&self, chan: &ChanKey) {
        *lock(&self.channels).entry(chan.clone()).or_insert(0) += 1;
    }

    pub(crate) fn bump_provider(&self, provider_key: &str) {
        *lock(&self.providers)
            .entry(provider_key.to_owned())
            .or_insert(0) += 1;
    }

    /// A provider's own count: it moves on every erase of the provider.
    pub(crate) fn provider_epoch(&self, provider_key: &str) -> u64 {
        lock(&self.providers)
            .get(provider_key)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn stamp(&self, chan: &ChanKey) -> Stamp {
        Stamp {
            generation: self.generation(),
            epoch: self.epoch(chan),
        }
    }

    pub(crate) fn current(&self, chan: &ChanKey, stamp: Stamp) -> bool {
        stamp.generation == self.generation() && stamp.epoch == self.epoch(chan)
    }
}

/// The sequencer and the applied mark the barrier waits on.
#[derive(Debug)]
pub(crate) struct Sequencer {
    /// The last sequence number handed out. Held while queueing, so queue order is sequence order.
    enqueued: Mutex<u64>,
    applied: Mutex<u64>,
    applied_changed: Condvar,
    applied_watch: tokio::sync::watch::Sender<u64>,
    pub(crate) flush: AtomicBool,
    /// Whether the writer has background work left. Published before the applied mark moves past
    /// the operation that made the work, so a waiter that saw the mark sees the work too.
    busy: Mutex<bool>,
    busy_changed: Condvar,
}

impl Default for Sequencer {
    fn default() -> Self {
        Self {
            enqueued: Mutex::new(0),
            applied: Mutex::new(0),
            applied_changed: Condvar::new(),
            applied_watch: tokio::sync::watch::Sender::new(0),
            flush: AtomicBool::new(false),
            busy: Mutex::new(false),
            busy_changed: Condvar::new(),
        }
    }
}

impl Sequencer {
    pub(crate) fn set_busy(&self, busy: bool) {
        let mut held = lock(&self.busy);
        if *held != busy {
            *held = busy;
            self.busy_changed.notify_all();
        }
    }

    /// Wait until the writer has no background work left, or `deadline`.
    fn wait_idle(&self, deadline: Instant) -> bool {
        let mut busy = lock(&self.busy);
        while *busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            busy = self
                .busy_changed
                .wait_timeout(busy, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    pub(crate) fn mark_applied(&self, seq: u64) {
        let mut applied = lock(&self.applied);
        if seq > *applied {
            *applied = seq;
            self.applied_watch.send_replace(seq);
            self.applied_changed.notify_all();
        }
    }

    fn applied(&self) -> u64 {
        *lock(&self.applied)
    }

    /// Everything queued so far has been applied or discarded: after the writer of an erased
    /// file has gone, nothing it held will ever be applied.
    fn catch_up(&self) {
        let enqueued = *lock(&self.enqueued);
        self.mark_applied(enqueued);
    }
}

/// The byte budget of the queue.
#[derive(Debug, Default)]
pub(crate) struct ByteBudget {
    used: AtomicUsize,
}

impl ByteBudget {
    fn try_take(&self, bytes: usize) -> bool {
        let mut used = self.used.load(Ordering::SeqCst);
        loop {
            let Some(total) = used.checked_add(bytes).filter(|&t| t <= QUEUE_BYTES) else {
                return false;
            };
            match self
                .used
                .compare_exchange_weak(used, total, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(actual) => used = actual,
            }
        }
    }

    pub(crate) fn release(&self, bytes: usize) {
        let mut used = self.used.load(Ordering::SeqCst);
        loop {
            match self.used.compare_exchange_weak(
                used,
                used.saturating_sub(bytes),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(actual) => used = actual,
            }
        }
    }
}

/// What the handle and the writer share.
pub(crate) struct Shared {
    pub(crate) epochs: Epochs,
    pub(crate) seq: Sequencer,
    pub(crate) budget: ByteBudget,
    pub(crate) routes: RwLock<Option<Arc<RouteSnapshot>>>,
    /// Channels whose head is not trusted, since the local time an offer for them was dropped.
    pub(crate) suspects: Mutex<HashMap<ChanKey, i64>>,
    pub(crate) stats: Stats,
    pub(crate) limits: Limits,
    pub(crate) search_text: Arc<dyn SearchTextSource>,
    pub(crate) clock: Arc<dyn Clock>,
    /// Whether every row's `search_text` was built by the current [`SearchTextSource::version`].
    pub(crate) search_text_current: AtomicBool,
    /// Reads running now. A `TRUNCATE` checkpoint is attempted only at zero.
    pub(crate) readers_in_use: AtomicUsize,
    /// Frames of the write-ahead log the checkpointer had not copied back when its last pass
    /// ended: the writer checkpoints the log itself when this is small, so that its own copy,
    /// and its sync, are cheap.
    pub(crate) log_backlog: AtomicI64,
    /// Erases asked for and not yet applied, with the sequence number that applies them
    /// (`u64::MAX` until it is known, or when the writer could not be asked).
    pub(crate) erasing: Mutex<HashMap<Erasing, u64>>,
    /// The latest sweep: until the writer has applied it, only channels routed continuously since
    /// before the oldest sweep still pending may be read.
    pub(crate) sweep: Mutex<Option<Sweep>>,
    /// Erases the writer could not carry out. What they cover stays blocked until the file is
    /// erased: a cache that cannot forget a channel must not serve it either.
    pub(crate) erase_failed: Mutex<HashSet<Erasing>>,
    /// What may not be served while access to it is in doubt: see [`MessageCache::block_channel`].
    pub(crate) blocks: Mutex<Blocks>,
    /// Where the writer sends its [`WriterEvent`]s, once something has subscribed. Bounded, and
    /// only ever `try_send`: the writer never waits on it.
    pub(crate) events: Mutex<Option<tokio::sync::mpsc::Sender<WriterEvent>>>,
    /// A test's hook into [`MessageCache::read`], run at each [`ReadPoint`].
    #[cfg(test)]
    pub(crate) read_hook: Mutex<Option<ReadHook>>,
}

/// Channels, scopes and providers whose rows are kept but not served: access to them is in
/// doubt. In memory only.
#[derive(Debug, Default)]
pub(crate) struct Blocks {
    channels: HashSet<ChanKey>,
    /// By channel and `scope_key`.
    scopes: HashSet<(ChanKey, String)>,
    providers: HashSet<String>,
}

/// What an erase waiting to be applied covers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Erasing {
    Channel(ChanKey),
    Provider(String),
    /// A sweep to the route snapshot.
    Sweep,
}

/// A sweep waiting to be applied.
#[derive(Clone, Debug)]
pub(crate) struct Sweep {
    /// The sequence number that applies it (`u64::MAX` until known).
    seq: u64,
    /// The channels routed both before and after every sweep still pending: the only ones whose
    /// rows no pending sweep can be about to erase.
    stable: HashSet<ChanKey>,
}

/// Where in [`MessageCache::read`] a test's hook runs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadPoint {
    /// Before anything is checked.
    Start,
    /// The channel's epoch has been noted.
    EpochTaken,
    /// The channel was not blocked, and the SQL is about to run.
    Checked,
    /// The SQL has run; nothing has been checked since.
    SqlDone,
}

#[cfg(test)]
pub(crate) type ReadHook = Box<dyn FnMut(ReadPoint, &MessageCache) + Send>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An envelope on the writer's queue.
pub(crate) struct Envelope {
    pub(crate) seq: u64,
    pub(crate) bytes: usize,
    pub(crate) op: Op,
}

/// What the writer can be asked to do.
pub(crate) enum Op {
    Observe {
        chan: ChanKey,
        stamp: Stamp,
        observation: Box<Observation>,
    },
    Erase {
        chans: Vec<ChanKey>,
    },
    EraseProvider {
        provider_key: String,
    },
    EraseScope {
        chan: ChanKey,
        scope_key: String,
    },
    RetainOnly {
        routes: Arc<RouteSnapshot>,
    },
    SetAccount {
        provider_key: String,
        account_id: String,
        workspace_id: Option<String>,
    },
    Maintain {
        kind: Maintenance,
    },
    Close {
        ack: mpsc::Sender<()>,
    },
    /// Report the writer connection's pragmas, for the per-open assertion.
    #[cfg(test)]
    Probe {
        reply: mpsc::Sender<Vec<Vec<(String, String)>>>,
    },
    /// Hold the writer still until `resume` answers or is dropped: a test's way to order an erase
    /// against a read exactly.
    #[cfg(test)]
    Pause {
        paused: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
    },
}

/// The pragmas the per-open assertion reads on every connection.
#[cfg(test)]
pub(crate) const PROBED: &[&str] = &[
    "secure_delete",
    "synchronous",
    "journal_size_limit",
    "busy_timeout",
    "temp_store",
    "foreign_keys",
    "journal_mode",
    "application_id",
    "auto_vacuum",
];

#[cfg(test)]
pub(crate) fn probe(connection: &rusqlite::Connection) -> Vec<(String, String)> {
    PROBED
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                schema::pragma_text(connection, name).unwrap_or_else(|e| schema::sqlite_code(&e)),
            )
        })
        .collect()
}

/// Housekeeping the writer runs on a timer, or when asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maintenance {
    /// The hourly pass: the age limit, expired tombstones, rows not revalidated within a week,
    /// the FTS integrity check, a bounded incremental vacuum, and a checkpoint.
    Hourly,
    /// The weekly bounded FTS merge.
    Weekly,
    /// Rebuild `search_text` for every row now, rather than in idle moments.
    RebuildSearchText,
    /// Check the size limit now.
    Bounds,
}

/// One open file: its writer thread and its two readers.
struct Inner {
    generation: u64,
    tx: Mutex<Option<mpsc::Sender<Envelope>>>,
    readers: reader::Readers,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Inner {
    /// Queue an envelope. `false` when the writer has gone.
    fn send(&self, envelope: Envelope) -> bool {
        lock(&self.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(envelope).is_ok())
    }

    fn join_writer(&self) {
        drop(lock(&self.tx).take());
        if let Some(handle) = lock(&self.writer).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.join_writer();
    }
}

/// The cache: one per process, for one file.
pub struct MessageCache {
    shared: Arc<Shared>,
    inner: RwLock<Option<Arc<Inner>>>,
    path: PathBuf,
    /// Held across the whole of a file erase, reopening included: two at once (the owner's
    /// "off" racing a purge) must not unlink the file the other has just opened.
    erase_lock: Mutex<()>,
    _lock: file::PathLock,
}

impl std::fmt::Debug for MessageCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageCache")
            .field("path", &self.path)
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl MessageCache {
    /// Open — creating if absent — the cache at `options.path`.
    ///
    /// Checks the linked SQLite, refuses the store's own file, makes or checks the directory,
    /// takes the path's lock, and recognises the file: one of ours is migrated, a half-created
    /// one is initialised, an unusable one of ours is rebuilt, and anything else is refused and
    /// left alone.
    ///
    /// # Errors
    ///
    /// [`CacheError`], saying which of those failed.
    pub fn open(options: CacheOptions) -> Result<Self, CacheError> {
        schema::capability_check().map_err(CacheError::Unavailable)?;
        let path = options.path;
        let refuse_store =
            || CacheError::Filesystem("the message cache path is the store's own file".to_owned());
        if options
            .store_path
            .as_deref()
            .is_some_and(|store| file::same_file(store, &path))
        {
            return Err(refuse_store());
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            file::ensure_private_dir(dir).map_err(CacheError::Filesystem)?;
        }
        // Again, now that the directory exists and a symbolic link in it can be resolved.
        if options
            .store_path
            .as_deref()
            .is_some_and(|store| file::same_file(store, &path))
        {
            return Err(refuse_store());
        }
        let lock = file::PathLock::acquire(&path).map_err(CacheError::Locked)?;
        let shared = Arc::new(Shared {
            epochs: Epochs::default(),
            seq: Sequencer::default(),
            budget: ByteBudget::default(),
            routes: RwLock::new(None),
            suspects: Mutex::new(HashMap::new()),
            stats: Stats::default(),
            limits: options.limits,
            search_text: options.search_text,
            clock: options.clock,
            search_text_current: AtomicBool::new(true),
            readers_in_use: AtomicUsize::new(0),
            log_backlog: AtomicI64::new(i64::MAX),
            erasing: Mutex::new(HashMap::new()),
            sweep: Mutex::new(None),
            erase_failed: Mutex::new(HashSet::new()),
            blocks: Mutex::new(Blocks::default()),
            events: Mutex::new(None),
            #[cfg(test)]
            read_hook: Mutex::new(None),
        });
        let inner = open_inner(&path, &shared)?;
        Ok(Self {
            shared,
            inner: RwLock::new(Some(Arc::new(inner))),
            path,
            erase_lock: Mutex::new(()),
            _lock: lock,
        })
    }

    /// The cache file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a file is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn current(&self) -> Option<Arc<Inner>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Take a stamp for a read of `chan` that is about to start.
    #[must_use]
    pub fn stamp(&self, chan: &ChanKey) -> Stamp {
        self.shared.epochs.stamp(chan)
    }

    /// Local time by the cache's own clock, in milliseconds since the Unix epoch: what a read
    /// that is about to start notes as its observation time, so that the times it offers and the
    /// writer's own cutoffs are read from one clock.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        self.shared.clock.now_ms()
    }

    /// Replace the route snapshot the writer gates on, and sweep every channel and provider not
    /// in it out of the file.
    ///
    /// Every channel leaving the snapshot has its epoch bumped at once, so a fill already running
    /// for it is dropped. Until the writer has applied the sweep, a channel is readable only if it
    /// was routed before it too: one removed and added back before the sweep lands still has rows
    /// the sweep is about to erase, and they must not be served in between. Before the first
    /// sweep of a process has landed, no channel is readable at all.
    pub fn retain_only(&self, routes: RouteSnapshot) {
        let routes = Arc::new(routes);
        let previous = self
            .shared
            .routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut removed: HashSet<ChanKey> = lock(&self.shared.epochs.channels)
            .keys()
            .filter(|chan| !routes.allows(chan))
            .cloned()
            .collect();
        if let Some(previous) = &previous {
            removed.extend(
                previous
                    .channels
                    .iter()
                    .filter(|chan| !routes.allows(chan))
                    .cloned(),
            );
        }
        for chan in &removed {
            self.shared.epochs.bump_channel(chan);
        }
        let applied = self.shared.seq.applied();
        // Held to the end, so two sweeps cannot interleave. Recorded before the new snapshot is
        // installed, so no read can see the new routes without the sweep that goes with them.
        let mut sweep = lock(&self.shared.sweep);
        let stable: HashSet<ChanKey> = match sweep.as_ref() {
            Some(pending) if pending.seq > applied => pending
                .stable
                .iter()
                .filter(|chan| routes.allows(chan))
                .cloned()
                .collect(),
            _ => previous.as_ref().map_or_else(HashSet::new, |previous| {
                previous
                    .channels
                    .iter()
                    .filter(|chan| routes.allows(chan))
                    .cloned()
                    .collect()
            }),
        };
        *sweep = Some(Sweep {
            seq: u64::MAX,
            stable,
        });
        *self
            .shared
            .routes
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&routes));
        let seq = self.control(Op::RetainOnly { routes });
        if let Some(sweep) = sweep.as_mut() {
            sweep.seq = seq.unwrap_or(u64::MAX);
        }
    }

    /// Offer an observation. Never waits: a full queue drops it and marks the channel suspect.
    pub fn offer(&self, chan: ChanKey, stamp: Stamp, observation: Observation) -> Offered {
        let bytes = observation.estimated_bytes();
        let guard = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let Some(inner) = guard.as_ref() else {
            return Offered::Off;
        };
        let mut enqueued = lock(&self.shared.seq.enqueued);
        if !self.shared.budget.try_take(bytes) {
            drop(enqueued);
            self.dropped(&chan, bytes);
            return Offered::Dropped;
        }
        let seq = *enqueued + 1;
        let envelope = Envelope {
            seq,
            bytes,
            op: Op::Observe {
                chan: chan.clone(),
                stamp,
                observation: Box::new(observation),
            },
        };
        if inner.send(envelope) {
            *enqueued = seq;
            Offered::Queued(seq)
        } else {
            drop(enqueued);
            self.shared.budget.release(bytes);
            self.dropped(&chan, bytes);
            Offered::Dropped
        }
    }

    fn dropped(&self, chan: &ChanKey, bytes: usize) {
        Stats::add(&self.shared.stats.ops_dropped, 1);
        Stats::add(
            &self.shared.stats.bytes_dropped,
            u64::try_from(bytes).unwrap_or(u64::MAX),
        );
        self.mark_suspect(chan);
    }

    /// Something seen of `chan` was not offered: its head is not trusted until a read of its
    /// newest end that starts after now has landed. In memory only, as every suspicion is.
    pub fn mark_suspect(&self, chan: &ChanKey) {
        lock(&self.shared.suspects).insert(chan.clone(), self.shared.clock.now_ms());
    }

    /// Queue a control operation: exempt from the byte budget, because an erase must never be
    /// the thing a full queue drops. `None` when no file is open or its writer has gone.
    fn control(&self, op: Op) -> Option<u64> {
        let inner = self.current()?;
        self.control_on(&inner, op)
    }

    fn control_on(&self, inner: &Inner, op: Op) -> Option<u64> {
        let mut enqueued = lock(&self.shared.seq.enqueued);
        let seq = *enqueued + 1;
        if inner.send(Envelope { seq, bytes: 0, op }) {
            *enqueued = seq;
            Some(seq)
        } else {
            None
        }
    }

    /// Erase channels: bump their epochs, so new reads skip them and fills already running are
    /// dropped, then queue the erase. A channel with nothing held costs nothing.
    ///
    /// The writer applies an erase as soon as it reaches it, in one short transaction that
    /// detaches the channel's rows from its name, so no read can reach them; it deletes the rows
    /// themselves in later steps, between batches.
    pub fn erase_channels(&self, chans: &[ChanKey]) {
        if chans.is_empty() {
            return;
        }
        // Blocked for reads before the epoch moves, and until the writer has applied the erase:
        // a read that starts in between must not find the rows still there and serve them.
        let mut pending = lock(&self.shared.erasing);
        for chan in chans {
            pending.insert(Erasing::Channel(chan.clone()), u64::MAX);
            self.shared.epochs.bump_channel(chan);
        }
        drop(pending);
        let seq = self.control(Op::Erase {
            chans: chans.to_vec(),
        });
        // A writer that could not be asked never applies it: stay blocked.
        let mut pending = lock(&self.shared.erasing);
        for chan in chans {
            pending.insert(Erasing::Channel(chan.clone()), seq.unwrap_or(u64::MAX));
        }
    }

    /// Erase everything one provider's channels hold, and its account row.
    pub fn erase_provider(&self, provider_key: &str) {
        let key = Erasing::Provider(provider_key.to_owned());
        lock(&self.shared.erasing).insert(key.clone(), u64::MAX);
        self.shared.epochs.bump_provider(provider_key);
        let seq = self.control(Op::EraseProvider {
            provider_key: provider_key.to_owned(),
        });
        lock(&self.shared.erasing).insert(key, seq.unwrap_or(u64::MAX));
    }

    /// Erase one scope of a channel, such as a thread whose access is lost: each message whose
    /// home is the scope is deleted as a deletion upstream would delete it (a tombstone stays, and
    /// the store's summaries of it follow), and the scope's memberships, coverage, head and thread
    /// record are dropped. The channel's other scopes keep what they hold.
    ///
    /// The channel is blocked for reads until the writer has applied it, and its epoch moves, so a
    /// fill of the channel already running is dropped.
    pub fn erase_scope(&self, chan: &ChanKey, scope: &Scope) {
        let key = Erasing::Channel(chan.clone());
        lock(&self.shared.erasing).insert(key.clone(), u64::MAX);
        self.shared.epochs.bump_channel(chan);
        let seq = self.control(Op::EraseScope {
            chan: chan.clone(),
            scope_key: scope.key(),
        });
        lock(&self.shared.erasing).insert(key, seq.unwrap_or(u64::MAX));
    }

    /// Serve nothing of `chan` until [`MessageCache::unblock`]: access to it is in doubt. Its rows
    /// are kept, and observations of it are still applied: a successful read of it is what lifts
    /// a doubt. In memory only.
    pub fn block_channel(&self, chan: &ChanKey) {
        lock(&self.shared.blocks).channels.insert(chan.clone());
    }

    /// Serve nothing of one scope of `chan`, such as a thread, until [`MessageCache::unblock`];
    /// its other scopes are served as before. As [`MessageCache::block_channel`] otherwise.
    pub fn block_scope(&self, chan: &ChanKey, scope: &Scope) {
        lock(&self.shared.blocks)
            .scopes
            .insert((chan.clone(), scope.key()));
    }

    /// Lift a block: of the whole channel when `scope` is `None`, or of that one scope.
    pub fn unblock(&self, chan: &ChanKey, scope: Option<&Scope>) {
        let mut blocks = lock(&self.shared.blocks);
        match scope {
            None => {
                blocks.channels.remove(chan);
            }
            Some(scope) => {
                blocks.scopes.remove(&(chan.clone(), scope.key()));
            }
        }
    }

    /// Serve nothing of any of `provider_key`'s channels until [`MessageCache::resume_provider`]:
    /// the provider refused the credential. In memory only.
    pub fn suspend_provider(&self, provider_key: &str) {
        lock(&self.shared.blocks)
            .providers
            .insert(provider_key.to_owned());
    }

    /// Serve `provider_key`'s channels again, blocks of single channels and scopes aside.
    pub fn resume_provider(&self, provider_key: &str) {
        lock(&self.shared.blocks).providers.remove(provider_key);
    }

    /// Whether a scope of `chan` is blocked on its own. The channel's own blocks are
    /// [`MessageCache::blocked`]'s.
    fn scope_blocked(&self, chan: &ChanKey, scope_key: &str) -> bool {
        lock(&self.shared.blocks)
            .scopes
            .contains(&(chan.clone(), scope_key.to_owned()))
    }

    /// Record that `provider_key`'s rows are read as the account `account_id`, in `workspace_id`
    /// where the provider has workspaces. A control operation, so a full queue never drops it; the
    /// writer applies it only while the route snapshot names the provider, as it does rows. The
    /// sequence number that applies it, or `None` when no file is open or its writer has gone.
    pub fn set_account(
        &self,
        provider_key: &str,
        account_id: &str,
        workspace_id: Option<&str>,
    ) -> Option<u64> {
        self.control(Op::SetAccount {
            provider_key: provider_key.to_owned(),
            account_id: account_id.to_owned(),
            workspace_id: workspace_id.map(str::to_owned),
        })
    }

    /// The account the file says `provider_key`'s rows were read as, if it says one.
    ///
    /// [`Read::Blocked`] while an erase of the provider is waiting to be applied, or failed: the
    /// row it would find is the one the erase is about to delete. [`Read::Moved`] when the
    /// provider or the file was erased while the read ran.
    #[must_use]
    pub fn account(&self, provider_key: &str) -> Read<Option<Account>> {
        let epoch = self.shared.epochs.provider_epoch(provider_key);
        if self.provider_erasing(provider_key) {
            return Read::Blocked;
        }
        let read = self.read(None, |connection| reader::account(connection, provider_key));
        if self.shared.epochs.provider_epoch(provider_key) != epoch {
            return Read::Moved;
        }
        if self.provider_erasing(provider_key) {
            return Read::Blocked;
        }
        read
    }

    /// The file generation now. It moves on every file erase, so whatever was checked against
    /// one generation is known to be about a file that is gone once it has moved.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.epochs.generation()
    }

    /// Whether an erase of `provider_key` is waiting to be applied, or failed.
    fn provider_erasing(&self, provider_key: &str) -> bool {
        let key = Erasing::Provider(provider_key.to_owned());
        if lock(&self.shared.erase_failed).contains(&key) {
            return true;
        }
        let applied = self.shared.seq.applied();
        let mut pending = lock(&self.shared.erasing);
        pending.retain(|_, seq| *seq > applied);
        pending.contains_key(&key)
    }

    /// Whether reads of a channel are blocked: it is off the route snapshot (or there is none
    /// yet); a sweep that may erase it is pending; an erase of it or its provider has been asked
    /// for and not yet applied; or one failed; or access to it is in doubt, or its provider is
    /// suspended.
    fn blocked(&self, chan: &ChanKey) -> bool {
        {
            let blocks = lock(&self.shared.blocks);
            if blocks.channels.contains(chan) || blocks.providers.contains(&chan.provider_key) {
                return true;
            }
        }
        let routed = self
            .shared
            .routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|routes| routes.allows(chan));
        if !routed {
            return true;
        }
        let applied = self.shared.seq.applied();
        let failed = lock(&self.shared.erase_failed).clone();
        let channel = Erasing::Channel(chan.clone());
        let provider = Erasing::Provider(chan.provider_key.clone());
        if failed.contains(&channel) || failed.contains(&provider) {
            return true;
        }
        let sweep_failed = failed.contains(&Erasing::Sweep);
        if let Some(sweep) = lock(&self.shared.sweep).as_ref() {
            if (sweep.seq > applied || sweep_failed) && !sweep.stable.contains(chan) {
                return true;
            }
        }
        let mut pending = lock(&self.shared.erasing);
        pending.retain(|_, seq| *seq > applied);
        pending.contains_key(&channel) || pending.contains_key(&provider)
    }

    /// Ask the writer for a maintenance pass now.
    pub fn maintain(&self, kind: Maintenance) {
        self.control(Op::Maintain { kind });
    }

    /// Wait until everything offered so far has been applied, up to `timeout`. Raises the flush
    /// flag, only when there is something to wait for, so a long batch commits at once. `false`
    /// on timeout, or when no file is open.
    ///
    /// This waits for operations, not for the background work they leave: an erase is applied
    /// (its rows unreachable) before its rows are deleted, and an offer is applied before the
    /// eviction it may set off. [`MessageCache::quiesce_blocking`] waits for that work too.
    pub fn barrier_blocking(&self, timeout: Duration) -> bool {
        self.barrier_to_blocking(self.enqueued_seq(), timeout)
    }

    /// [`MessageCache::barrier_blocking`] for what was offered up to the sequence number `target`,
    /// noted earlier with [`MessageCache::enqueued_seq`]: what a read that must be current as of
    /// that moment waits for, and nothing offered since.
    pub fn barrier_to_blocking(&self, target: u64, timeout: Duration) -> bool {
        if !self.is_open() {
            return false;
        }
        let deadline = Instant::now() + timeout;
        let mut applied = lock(&self.shared.seq.applied);
        if *applied < target {
            self.shared.seq.flush.store(true, Ordering::SeqCst);
        }
        while *applied < target {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            applied = self
                .shared
                .seq
                .applied_changed
                .wait_timeout(applied, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// [`MessageCache::barrier_blocking`] for async callers.
    pub async fn barrier(&self, timeout: Duration) -> bool {
        if !self.is_open() {
            return false;
        }
        let target = *lock(&self.shared.seq.enqueued);
        if self.shared.seq.applied() >= target {
            return true;
        }
        self.shared.seq.flush.store(true, Ordering::SeqCst);
        let mut watch = self.shared.seq.applied_watch.subscribe();
        tokio::time::timeout(timeout, watch.wait_for(|applied| *applied >= target))
            .await
            .is_ok_and(|seen| seen.is_ok())
    }

    /// Wait until everything offered so far has been applied AND the writer has finished the
    /// background work it left: the rows of erased channels deleted, eviction and compaction
    /// done, the index merged, deleted text checkpointed out of the write-ahead log. For tests,
    /// benchmarks and an orderly shutdown; never on a request's path. `false` on timeout, or
    /// when no file is open.
    pub fn quiesce_blocking(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.barrier_blocking(timeout) && self.shared.seq.wait_idle(deadline)
    }

    /// Whether the writer has background work left.
    #[must_use]
    pub fn background_busy(&self) -> bool {
        *lock(&self.shared.seq.busy)
    }

    /// The highest sequence number applied.
    #[must_use]
    pub fn applied_seq(&self) -> u64 {
        self.shared.seq.applied()
    }

    /// The last sequence number handed out: everything offered so far has it or a lower one.
    #[must_use]
    pub fn enqueued_seq(&self) -> u64 {
        *lock(&self.shared.seq.enqueued)
    }

    /// Have the writer send its [`WriterEvent`]s to the returned receiver from now on, through a
    /// queue of `capacity` events; a full one drops what does not fit, and counts it
    /// ([`CacheStats::orphans_dropped`]). A later subscription takes the place of this one.
    #[must_use]
    pub fn subscribe_events(&self, capacity: usize) -> tokio::sync::mpsc::Receiver<WriterEvent> {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity.max(1));
        *lock(&self.shared.events) = Some(sender);
        receiver
    }

    /// Whether a channel's head is untrusted because an offer for it was dropped and no read of
    /// its newest end that started afterwards has landed.
    #[must_use]
    pub fn is_suspect(&self, chan: &ChanKey) -> bool {
        lock(&self.shared.suspects).contains_key(chan)
    }

    /// Whether every row's `search_text` was built by the current rendering rules. A search
    /// over a file that is still being rebuilt is partial.
    #[must_use]
    pub fn search_text_current(&self) -> bool {
        self.shared.search_text_current.load(Ordering::SeqCst)
    }

    /// What the writer has done, in numbers.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        self.shared.stats.snapshot()
    }

    /// The bytes the cache occupies on disk, its working files included.
    #[must_use]
    pub fn bytes_on_disk(&self) -> u64 {
        file::bytes_on_disk(&self.path)
    }

    /// Erase the whole file, then open a fresh one if `reopen`.
    ///
    /// 1. Uninstall the open file and bump the generation: new reads go upstream at once, and
    ///    every queued observation is stale.
    /// 2. Ask the writer to close. It discards what is queued, finishes its transaction, and
    ///    closes its connection.
    /// 3. Wait for reads in flight; after [`ERASE_GRACE`], interrupt their statements, and again
    ///    every [`ERASE_GRACE`] while one is still running.
    /// 4. Close both readers, then unlink the database and its working files and fsync the
    ///    directory.
    /// 5. If `reopen`, create and install a fresh file.
    ///
    /// The whole of it, reopening included, holds one lock: a second erase waits for the first
    /// to finish, and then erases whatever the first left.
    ///
    /// # Errors
    ///
    /// Only from reopening; the erase itself always completes.
    pub fn erase_file(&self, reopen: bool) -> Result<EraseReport, CacheError> {
        let _serial = lock(&self.erase_lock);
        let taken = {
            let mut guard = self.inner.write().unwrap_or_else(PoisonError::into_inner);
            let taken = guard.take();
            self.shared.epochs.bump_generation();
            taken
        };
        if let Some(inner) = taken {
            let (ack, acked) = mpsc::channel();
            if self.control_on(&inner, Op::Close { ack }).is_some() {
                let _ = acked.recv_timeout(Duration::from_secs(60));
            }
            inner.join_writer();
            let mut last_interrupt = Instant::now();
            while Arc::strong_count(&inner) > 1 {
                if last_interrupt.elapsed() >= ERASE_GRACE {
                    inner.readers.interrupt_all();
                    last_interrupt = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            drop(inner);
        }
        self.shared.seq.catch_up();
        self.shared.seq.set_busy(false);
        self.shared.budget.used.store(0, Ordering::SeqCst);
        let bytes = file::unlink_family(&self.path);
        // Nothing in the old file can be served any more, and a fresh one holds nothing: what
        // was waiting to be erased, or could not be, is moot.
        lock(&self.shared.erasing).clear();
        lock(&self.shared.erase_failed).clear();
        if let Some(sweep) = lock(&self.shared.sweep).as_mut() {
            sweep.seq = 0;
        }
        if reopen {
            let fresh = open_inner(&self.path, &self.shared)?;
            *self.inner.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(fresh));
        }
        Ok(EraseReport {
            bytes,
            reopened: reopen,
        })
    }

    #[cfg(test)]
    fn hook(&self, point: ReadPoint) {
        let taken = lock(&self.shared.read_hook).take();
        if let Some(mut hook) = taken {
            hook(point, self);
            let mut slot = lock(&self.shared.read_hook);
            if slot.is_none() {
                *slot = Some(hook);
            }
        }
    }

    /// Run a read against one of the reader connections, and serve its result only if no erase
    /// of the file (or of `chan`, when given) happened while it ran.
    ///
    /// The order matters. The channel's epoch is noted FIRST and only then is the channel checked
    /// for a pending erase: an erase marks itself pending before it bumps the epoch, so whatever
    /// moment it lands at, either the check sees it pending, or the epoch noted here is older than
    /// the one the read finds when its SQL is done. Checked the other way round, an erase landing
    /// between the two would leave a stale read looking current once the writer had applied it.
    pub(crate) fn read<T>(
        &self,
        chan: Option<&ChanKey>,
        work: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Read<T> {
        #[cfg(test)]
        self.hook(ReadPoint::Start);
        let Some(inner) = self.current() else {
            return Read::Off;
        };
        let generation = inner.generation;
        let epoch = chan.map(|c| self.shared.epochs.epoch(c));
        #[cfg(test)]
        self.hook(ReadPoint::EpochTaken);
        if chan.is_some_and(|c| self.blocked(c)) {
            return Read::Blocked;
        }
        if self.shared.epochs.generation() != generation {
            return Read::Moved;
        }
        #[cfg(test)]
        self.hook(ReadPoint::Checked);
        self.shared.readers_in_use.fetch_add(1, Ordering::SeqCst);
        let result = inner.readers.with(work);
        self.shared.readers_in_use.fetch_sub(1, Ordering::SeqCst);
        drop(inner);
        #[cfg(test)]
        self.hook(ReadPoint::SqlDone);
        let moved = self.shared.epochs.generation() != generation
            || chan.is_some_and(|c| Some(self.shared.epochs.epoch(c)) != epoch);
        if moved {
            return Read::Moved;
        }
        if chan.is_some_and(|c| self.blocked(c)) {
            return Read::Blocked;
        }
        match result {
            Ok(value) => Read::Served(value),
            Err(error) => Read::Failed(schema::sqlite_code(&error)),
        }
    }

    /// [`MessageCache::read`] of one scope of `chan`, blocked as well while that scope is, before
    /// the SQL and after it.
    fn read_scope<T>(
        &self,
        chan: &ChanKey,
        scope_key: &str,
        work: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Read<T> {
        if self.scope_blocked(chan, scope_key) {
            return Read::Blocked;
        }
        let read = self.read(Some(chan), work);
        if self.scope_blocked(chan, scope_key) {
            return Read::Blocked;
        }
        read
    }

    /// A scope's newest `limit` messages, oldest first.
    #[must_use]
    pub fn newest(&self, chan: &ChanKey, scope: &Scope, limit: usize) -> Read<Vec<CachedMessage>> {
        let scope_key = scope.key();
        self.read_scope(chan, &scope_key, |connection| {
            reader::newest(connection, chan, &scope_key, limit)
        })
    }

    /// One message, if the cache holds it. Blocked while the scope it belongs to is.
    #[must_use]
    pub fn message(&self, chan: &ChanKey, id: &MessageId) -> Read<Option<CachedMessage>> {
        let read = self.read(Some(chan), |connection| {
            reader::message_and_home(connection, chan, id)
        });
        if let Read::Served(Some((_, home))) = &read {
            if self.scope_blocked(chan, home) {
                return Read::Blocked;
            }
        }
        read.map(|found| found.map(|(message, _)| message))
    }

    /// Whether the cache holds a tombstone for a message.
    #[must_use]
    pub fn tombstoned(&self, chan: &ChanKey, id: &MessageId) -> Read<bool> {
        self.read(Some(chan), |connection| {
            reader::tombstoned(connection, chan, id)
        })
    }

    /// The coverage intervals of one scope, oldest first.
    #[must_use]
    pub fn coverage(&self, chan: &ChanKey, scope: &Scope) -> Read<Vec<coverage::Interval>> {
        let scope_key = scope.key();
        self.read_scope(chan, &scope_key, |connection| {
            reader::coverage(connection, chan, &scope_key)
        })
    }

    /// The head of one scope, if a read of its newest end has landed.
    #[must_use]
    pub fn head(&self, chan: &ChanKey, scope: &Scope) -> Read<Option<coverage::Head>> {
        let scope_key = scope.key();
        self.read_scope(chan, &scope_key, |connection| {
            reader::head(connection, chan, &scope_key)
        })
    }

    /// The scopes a message is listed in. Blocked while any of them is.
    #[must_use]
    pub fn memberships(&self, chan: &ChanKey, id: &MessageId) -> Read<Vec<Scope>> {
        let read = self.read(Some(chan), |connection| {
            reader::memberships(connection, chan, id)
        });
        if let Read::Served(scopes) = &read {
            if scopes
                .iter()
                .any(|scope| self.scope_blocked(chan, &scope.key()))
            {
                return Read::Blocked;
            }
        }
        read
    }

    /// A thread record. Blocked while the thread's scope is.
    #[must_use]
    pub fn thread(&self, chan: &ChanKey, thread_id: &str) -> Read<Option<ThreadRecord>> {
        let scope_key = Scope::Thread(thread_id.to_owned()).key();
        self.read_scope(chan, &scope_key, |connection| {
            reader::thread(connection, chan, thread_id)
        })
    }

    /// The writer connection's pragmas, the checkpointer's, then each reader's.
    #[cfg(test)]
    pub(crate) fn probe_pragmas(&self) -> Vec<Vec<(String, String)>> {
        let mut all = Vec::new();
        let (reply, answer) = mpsc::channel();
        self.control(Op::Probe { reply });
        all.extend(
            answer
                .recv_timeout(Duration::from_secs(10))
                .expect("writer answered"),
        );
        if let Some(inner) = self.current() {
            all.extend(inner.readers.each(|c| Ok(probe(c))).expect("readers"));
        }
        all
    }

    /// Hold the writer still until the returned sender is dropped or sent to. Returns once the
    /// writer has stopped.
    #[cfg(test)]
    pub(crate) fn pause_writer(&self) -> mpsc::Sender<()> {
        let (paused, stopped) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        self.control(Op::Pause {
            paused,
            resume: resumed,
        })
        .expect("a writer to pause");
        stopped
            .recv_timeout(Duration::from_secs(10))
            .expect("the writer paused");
        resume
    }

    /// Stop the writer as if it had died: the file stays open for reading, and nothing offered or
    /// asked for is applied any more.
    #[cfg(test)]
    pub(crate) fn stop_writer(&self) {
        if let Some(inner) = self.current() {
            let (ack, acked) = mpsc::channel();
            if self.control_on(&inner, Op::Close { ack }).is_some() {
                let _ = acked.recv_timeout(Duration::from_secs(10));
            }
            inner.join_writer();
        }
    }

    /// Run `hook` at every [`ReadPoint`] of every read, until replaced.
    #[cfg(test)]
    pub(crate) fn set_read_hook(&self, hook: Option<ReadHook>) {
        *lock(&self.shared.read_hook) = hook;
    }

    /// The bytes the queue's budget holds now.
    #[cfg(test)]
    pub(crate) fn queued_bytes(&self) -> usize {
        self.shared.budget.used.load(Ordering::SeqCst)
    }

    /// What the whole file holds, in counts.
    #[must_use]
    pub fn counts(&self) -> Read<Counts> {
        self.read(None, reader::counts)
    }

    /// What a shadow evaluation reads of `chan` ([`shadow::Query`]), all of it in one snapshot of
    /// the file, so that a batch committed while it runs is either wholly in it or not at all.
    /// Checked as every read is, and [`Read::Blocked`] while the scope it reads, or the scope the
    /// message it found belongs to, is.
    #[must_use]
    pub fn shadow_view(&self, chan: &ChanKey, query: &shadow::Query) -> Read<shadow::View> {
        let work = |connection: &rusqlite::Connection| reader::shadow_view(connection, chan, query);
        let read = match query.scope() {
            Some(scope) => self.read_scope(chan, &scope.key(), work),
            None => self.read(Some(chan), work),
        };
        if let Read::Served(view) = &read {
            if view
                .home
                .as_ref()
                .is_some_and(|home| self.scope_blocked(chan, &home.key()))
            {
                return Read::Blocked;
            }
        }
        read
    }

    /// The milliseconds of message time the coverage of every listed channel's own timeline (its
    /// `main` and `flat` scopes, not its threads) spans, summed over the channels: how much
    /// history the file has proven it holds whole. For the diagnostics report.
    #[must_use]
    pub fn covered_ms(&self) -> Read<i64> {
        self.read(None, reader::covered_ms)
    }

    /// How many channels and scopes are blocked now, access to them in doubt. Suspended
    /// providers are not counted here.
    #[must_use]
    pub fn blocks(&self) -> usize {
        let blocks = lock(&self.shared.blocks);
        blocks.channels.len() + blocks.scopes.len()
    }

    /// The earliest message time any scope's coverage starts at, across the whole file, in
    /// milliseconds since the Unix epoch: for the status Settings shows. `None` with no coverage.
    #[must_use]
    pub fn oldest_covered(&self) -> Read<Option<i64>> {
        self.read(None, reader::oldest_covered)
    }
}

impl Drop for MessageCache {
    fn drop(&mut self) {
        let taken = self
            .inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(taken);
    }
}

/// Open the file at `path` for `shared`: create it `0600` if absent, recognise it, rebuild one of
/// ours that cannot be used, start its writer, and open its readers.
fn open_inner(path: &Path, shared: &Arc<Shared>) -> Result<Inner, CacheError> {
    let mut rebuilt = false;
    let connection = loop {
        file::create_if_absent(path).map_err(CacheError::Filesystem)?;
        let header = schema::read_header(path).map_err(|e| {
            CacheError::Filesystem(format!("cannot read {}: {}", path.display(), e.kind()))
        })?;
        let connection = writer::open_connection(path)?;
        match schema::prepare_writer(&connection, header, schema::STEPS) {
            Ok(()) => break connection,
            Err(schema::Refusal::Foreign(reason)) => return Err(CacheError::Foreign(reason)),
            Err(schema::Refusal::Sqlite(code)) => return Err(CacheError::Sqlite(code)),
            Err(schema::Refusal::Rebuild(reason)) => {
                drop(connection);
                if rebuilt {
                    return Err(CacheError::Sqlite(reason));
                }
                tracing::warn!(
                    reason = reason.as_str(),
                    "the message cache file cannot be used by this build; erasing and rebuilding it"
                );
                file::unlink_family(path);
                rebuilt = true;
            }
        }
    };
    for member in file::family(path).iter().take(3) {
        file::restrict(member);
    }
    let current = writer::search_text_version(&connection)
        .is_some_and(|version| version == shared.search_text.version());
    shared.search_text_current.store(current, Ordering::SeqCst);
    let readers = reader::Readers::open(path)?;
    let (tx, rx) = mpsc::channel::<Envelope>();
    let generation = shared.epochs.generation();
    let thread_shared = Arc::clone(shared);
    let thread_path = path.to_owned();
    let (started, ready) = mpsc::channel::<()>();
    let handle = std::thread::Builder::new()
        .name("message-cache-writer".to_owned())
        .spawn(move || {
            writer::run(
                connection,
                &rx,
                &thread_shared,
                &thread_path,
                generation,
                &started,
            );
        })
        .map_err(|e| {
            CacheError::Filesystem(format!("cannot start the cache writer: {}", e.kind()))
        })?;
    // The writer's first steps (turning FTS secure-delete back on, finding work a crash left)
    // are done before anything can be offered.
    let _ = ready.recv_timeout(Duration::from_secs(30));
    Ok(Inner {
        generation,
        tx: Mutex::new(Some(tx)),
        readers,
        writer: Mutex::new(Some(handle)),
    })
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod privacy_tests;
#[cfg(test)]
mod race_tests;
#[cfg(test)]
mod tests;
