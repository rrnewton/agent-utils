//! The server's one handle on the message cache: a [`Sink`]. `#208 persistent-cache`.
//!
//! The handle is always present, in every server state, whether or not a cache is open behind it,
//! so a read site never has to ask whether the cache exists before it asks whether it may fill
//! it. Every fill starts with [`Sink::begin`], and only under a [`FillGrant`]: a read-scope caller
//! has none (see [`super::grant`]), so it never reaches the cache at all.
//!
//! # The route snapshot, and the startup guard
//!
//! The cache may hold rows only for the channels the server answers for, each under the provider
//! that owns it. [`Sink::set_routes`] is told that list every time it changes
//! (`AppState::refresh_routes`), and turns it into the [`RouteSnapshot`] the cache's writer gates
//! on and sweeps to: every channel and provider key off it is erased from the file.
//!
//! A sweep is only as good as the list it sweeps to. At startup the list is put together from the
//! configuration AND from the store (the channels added in the app, and the configured ones the
//! owner hid), and a store that failed to answer leaves it short: sweeping to it would erase every
//! channel the owner added. So nothing is sent to the cache until [`Sink::arm`] has been called,
//! which the server does only once both lists came back from the store. Unarmed, the sink keeps
//! the latest list and does nothing with it: no snapshot reaches the cache, so the cache sweeps
//! nothing and its writer refuses every row, and every fill [`Sink::begin`] begins offers
//! deletions alone, for the life of the process (see below).
//!
//! # Offering what a read saw
//!
//! A [`Fill`] is begun before the read and offered once, after it, through the method for the
//! kind of read it was. [`super::observe`] decides what that read proves, by the shape of the
//! channel's provider; the fill supplies the observation time, the earlier of when the read began
//! and the `as_of` the provider dated its answer with, so a change pushed while the read ran, or
//! after the provider's copy was taken, is never undone by it. The rows offered are the provider's
//! own, before anything prepares them for a reader.
//!
//! # Changes from the live feed, and posts
//!
//! The live hub offers every change it publishes ([`Sink::offer_push`]) under the server's own
//! grant, while it holds the channel's feed lock and before any subscriber is sent it: a page that
//! saw a deletion and then reads the cache can never find the message still there for want of the
//! deletion being queued. The lock order is the feed lock, then the cache's sequencer, and nothing
//! here waits. A message this server posts is offered as it comes back from the provider
//! ([`Fill::offer_posted`]), also under the server's own grant.
//!
//! # Whose account the rows were read as
//!
//! Rows read with one account's credential must never be served as another's. The file records,
//! per provider, the account its rows were read as ([`super::Account`]), and nothing of a provider
//! is filled until that provider's own client ([`crate::providers::ProviderEntry::client`], never
//! the router, which answers for its default provider alone) has said which account it is, in
//! this process and against this file:
//!
//! - no account recorded yet: this one is written, and fills begin once it reads back;
//! - the same account (and, where the provider has workspaces, the same workspace): fills begin;
//! - another account: everything the file holds for the provider is erased first, then the new
//!   account is written;
//! - no answer (a transport failure, a 5xx, anything that is not an account): nothing is erased
//!   and nothing filled, and it is asked again after a wait that doubles each time, up to
//!   [`VERIFY_RETRY_CAP`].
//!
//! **Deletions are the exception.** A deletion pushed by the live feed, or reported by a complete
//! delta, writes nothing an account could have read: it only takes a message out. So it is offered
//! whatever is known of the account (unchecked, suspended, or the sink not armed), and a mismatch
//! found later erases the provider anyway. Leaving it out would leave the deleted message in the
//! file, to be served once the account is checked. Everything else such a fill would have offered
//! is dropped, and the channel is marked suspect ([`MessageCache::mark_suspect`]), in memory and in
//! the file: what was dropped may have been an edit, a deletion or a creation, so no row of the
//! channel seen before is vouched for until a read that began afterwards has seen it again, and no
//! scope's head until a read of that scope's newest end that began afterwards has landed.
//!
//! It is asked whenever a cache is put behind an armed sink, when the sink is armed with one
//! behind it, when the providers change, and after any erase of the whole file: a fill checks the
//! file's generation, so one erased since the account was checked is checked again before any
//! fill reaches it.
//!
//! # When the provider stops letting the server read
//!
//! While a cache is behind an armed sink, the sink listens to the router's
//! [`super::access::AccessWatch`], which every provider client reports each operation's outcome
//! to (plan section 6.3):
//!
//! - **a channel or a thread in doubt** (a 403 or 404 from a read of it) is not served from the
//!   cache until a read of it succeeds again. Ten minutes on, the sink reads it again itself; the
//!   same refusal confirms the loss, and the sink erases the channel and the store's summaries of
//!   it, or erases the thread, each of whose messages the store's summaries then follow. A loss
//!   confirmed this way is erased whoever noticed it first, a read-scope request included: the
//!   erase removes what was read and writes no text (a thread's leaves only the ids and times of
//!   its messages, as tombstones);
//! - **a credential refused** (any 401) suspends the provider: nothing of it is served or filled.
//!   Its own client is asked which account it is every [`SUSPENDED_RECHECK`], and the suspension
//!   ends when the answer matches the file (or is recorded, the file holding none). Slack's
//!   `account_inactive` or `token_revoked`, or a refusal still heard [`CREDENTIAL_GRACE`] after the
//!   first, erases everything held for the provider.
//!
//! # The store's summaries
//!
//! A cache put behind the sink ([`Sink::install`]) is followed by a task that deletes the store's
//! summaries of messages the cache learns were edited or deleted, and of channels it erases that
//! are on the server's list under no provider at all ([`super::orphans`]). With no cache, there is
//! no task, and no summary is ever deleted on the cache's account.
//!
//! # What it serves, and what it would have served
//!
//! While a read the cache could answer goes upstream, [`Sink::shadow`] asks the cache what it
//! holds for the same question, off the request's path, and once upstream has answered compares
//! the two ([`super::shadow`]). Whoever asked, read scope included: an evaluation reads and
//! counts in memory, and writes nothing. With no cache open, nothing is read; the ask is
//! counted, as `off`, only when the configuration has a `[message_cache]` table, and costs
//! nothing otherwise.
//!
//! With `[message_cache] serve` on, the newest window and the newest timeline page ask through
//! [`Sink::try_serve`] instead: the same evaluation, awaited on the request's path for at most
//! [`SERVE_WAIT`], and answered from its snapshot when the rule serves and the caller holds the
//! write scope (its read sites decide that). The read it stood in for runs upstream behind
//! the answer — one at a time per read, under [`Sink::claim_refresh`] — and its answer is
//! handed to the evaluation for the same comparison, then corrects what was served.
//!
//! A provider's feed vouches for a channel only while an adapter is actually pushing for that
//! provider ([`Sink::fed_since_for`]): the sink notes the latest push per provider as it
//! offers it, here under the live hub's feed lock.
//!
//! # What it says about itself
//!
//! [`Sink::report`] is the `message_cache` section of the diagnostics report: whether a cache is
//! open, what the file holds and its writer has done, in numbers, each provider's standing, and the
//! shadow's counts. [`log_every_hour`] writes the same figures, totals since the process started,
//! on one `info` line an hour. Both say nothing on a server whose configuration has no
//! `[message_cache]` table, and neither ever carries a channel id, a message id or a word of text.
//!
//! A comparison that finds anything writes one line of its own (`#497 cache-shadow-missing-row`),
//! a warning for a difference and an `info` line for messages past the end of the answer's claim:
//! the channel, the scope, the read it rests on, and each message it found different by id and
//! times, with what offered it to the cache ([`Arrivals`]) and what the file holds of it. The one
//! cache log line that names a channel or a message; never a word of text, and at most
//! [`NOTES_PER_HOUR`] of each kind an hour.
//!
//! # Who puts a cache behind it
//!
//! The server does, at startup, when the configuration enables the cache and the owner has not
//! turned it off ([`super::lifecycle`]). Otherwise the slot stays empty and every fill is
//! refused.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;

use super::access::{AccessListener, Suspect};
use super::arrivals::Arrivals;
use super::grant::{FillGrant, ServerFill};
use super::lifecycle::Lifecycle;
use super::observe::{self, PageRead, Shape, TimelineRead};
use super::orphans::{self, Summaries};
use super::shadow::{self, Ask, Decision, Differing, Miss, Upstream, View};
use super::{
    lock, Account, CacheStats, ChanKey, Counts, MessageCache, Observation, Offered, Read,
    RouteSnapshot, Scope, Stamp,
};
use crate::chat::{ChatClient, ChatError, RecentRead};
use crate::config::{Config, ProviderKind, RecentView};
use crate::live::LiveMessage;
use crate::model::{ChannelId, ChannelInfo, Message, MessageId};
use crate::providers::ChatRouter;
use crate::threads::{TimelinePage, TimelineRequest, TimelineView};

/// How long a provider that gave no account is left before it is asked again. Each further
/// failure doubles the wait, up to [`VERIFY_RETRY_CAP`].
pub const VERIFY_RETRY_FIRST: Duration = Duration::from_secs(5);
/// The longest wait between two questions to a provider that gives no account.
pub const VERIFY_RETRY_CAP: Duration = Duration::from_secs(600);
/// How long a verification waits for the writer to apply what it asked before it reads the
/// account back. Longer, and it asks again later.
const ACCOUNT_WAIT: Duration = Duration::from_secs(10);
/// How often a provider that refused the credential is asked which account it is.
pub const SUSPENDED_RECHECK: Duration = Duration::from_secs(600);
/// How long a provider may go on refusing the credential before what is held for it is erased.
pub const CREDENTIAL_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// The message cache as the server sees it: an empty slot, or an open cache, and the channels it
/// may hold.
#[derive(Debug)]
pub struct Sink {
    /// Shared with the tasks the sink starts, and listened through: see the module documentation.
    state: Arc<State>,
    /// What opens, switches and erases the cache file behind the sink, once the server has
    /// started it ([`super::lifecycle::start`]). Never in a test that installs a cache itself.
    lifecycle: OnceLock<Arc<Lifecycle>>,
}

impl Default for Sink {
    fn default() -> Self {
        Self {
            state: Arc::new_cyclic(|me| State {
                me: me.clone(),
                ..State::default()
            }),
            lifecycle: OnceLock::new(),
        }
    }
}

/// What the sink knows, shared with the tasks it starts.
#[derive(Debug, Default)]
struct State {
    /// The sink's own state, for what it starts from a listener's call.
    me: Weak<State>,
    slot: RwLock<Option<Arc<MessageCache>>>,
    /// Taken before the slot whenever both are held, never the other way round.
    routes: Mutex<Routes>,
    /// Every channel id on the server's list, under whatever provider: what the task that keeps
    /// the store's summaries in step asks before it forgets an erased channel's.
    listed: Arc<RwLock<HashSet<ChannelId>>>,
    /// The server's summaries, for that task. Attached once; with none attached, no task runs.
    summaries: OnceLock<Summaries>,
    /// How far that task has got with the cache installed now.
    orphans: Mutex<OrphanProgress>,
    /// Whether each provider's account has been checked against the file. Taken after the routes
    /// whenever both are held.
    identities: Mutex<Identities>,
    /// What the shadow has counted, and how far its evaluations have got.
    shadow: Arc<ShadowProgress>,
    /// Whether reads may be answered from the cache (`[message_cache] serve`): set by
    /// [`super::lifecycle::start`] from the configuration, and by a test that installs a cache
    /// itself. `false` is "fill and measure, answer nothing". `#384 cache-serve`.
    serve: AtomicBool,
    /// When an adapter last pushed a change for each provider, by the cache's clock: whether
    /// that provider's feed may vouch for what it has not reported ([`Sink::fed_since_for`]).
    pushed: Mutex<HashMap<String, i64>>,
    /// The background refreshes of served answers now running, one per read: a second serve of
    /// the same read starts no second upstream read. `#384 cache-serve`.
    refreshing: Mutex<HashSet<RefreshKey>>,
    /// How each recent message reached the cache, by whom it was offered: what the shadow's
    /// diagnostics name beside a message its comparison found different. Memory only. `#497
    /// cache-shadow-missing-row`.
    arrivals: Arc<Arrivals>,
}

/// What the shadow has counted since the process started, and how many of its evaluations have
/// read the cache and been counted, of how many begun.
#[derive(Debug, Default)]
struct ShadowProgress {
    tallies: shadow::Tallies,
    begun: AtomicU64,
    read: AtomicU64,
    counted: AtomicU64,
    /// How many diagnostic lines the shadow has written this hour. `#497 cache-shadow-missing-row`.
    notes: Mutex<Notes>,
}

/// The most lines the shadow's diagnostics write an hour of each kind: comparisons that found a
/// difference, and comparisons that found only messages written past the end of their answer's
/// claim. `#497 cache-shadow-missing-row`.
pub const NOTES_PER_HOUR: u32 = 30;

/// The most messages of each kind one diagnostic line names; it counts the rest.
const NOTED_ROWS: usize = 8;

/// The kinds of diagnostic line, each with its own hourly allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteKind {
    /// A comparison found a difference: a `warn` line.
    Differs,
    /// A comparison found only messages written past the end of its answer's claim: `info`.
    Past,
}

/// The diagnostics' hourly allowance: see [`NOTES_PER_HOUR`].
#[derive(Debug, Default)]
struct Notes {
    since: Option<Instant>,
    written: [u32; 2],
    held_back: [u64; 2],
}

impl Notes {
    /// Whether a line of `kind` may be written at `now`: then how many of its kind were held back
    /// since the last one written, which it says.
    fn take(&mut self, kind: NoteKind, now: Instant) -> Option<u64> {
        if self
            .since
            .is_none_or(|since| now.duration_since(since) >= Duration::from_secs(60 * 60))
        {
            self.since = Some(now);
            self.written = [0; 2];
        }
        let kind = kind as usize;
        if self.written[kind] >= NOTES_PER_HOUR {
            self.held_back[kind] += 1;
            return None;
        }
        self.written[kind] += 1;
        Some(std::mem::take(&mut self.held_back[kind]))
    }
}

/// Each provider's standing against the cache behind the sink.
#[derive(Debug, Default)]
struct Identities {
    /// Moved on every fresh start of verification: a task of an earlier round changes nothing.
    round: u64,
    /// By provider key. A provider not here is unverified.
    providers: HashMap<String, Standing>,
    /// The providers a verification task of this round is running for.
    running: HashSet<String>,
}

/// A provider's standing, when it has one; one with none is unverified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// The provider's account matched the one the file records (or was recorded), while the file
    /// was at this generation.
    Verified { generation: u64 },
    /// The provider refused the credential, first at `since`; `erased` once what was held for it
    /// has been erased for that.
    Suspended {
        since: tokio::time::Instant,
        erased: bool,
    },
}

/// Whether one provider's channels may be filled: its account has been checked against the file.
/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Not checked yet, against the file open now: nothing of the provider is filled.
    Unverified,
    /// Checked: fills may begin.
    Verified,
    /// The provider refused the credential: nothing of it is served or filled until its account
    /// is checked again.
    Suspended,
}

impl Verification {
    /// The word the diagnostics report uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Verified => "verified",
            Self::Suspended => "suspended",
        }
    }
}

/// The `message_cache` section of `GET /api/v1/diagnostics` ([`Sink::report`]). Numbers, provider
/// keys, and words this server chooses: never a channel id, a message id or text.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CacheReport {
    /// Whether a cache file is open.
    pub available: bool,
    /// Whether the configuration enables the cache and the owner has not turned it off.
    pub enabled: bool,
    /// Why no cache is open, when none is: `config_disabled`, `switched_off`, or a code saying
    /// what failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// Bytes the file occupies, its working files included.
    pub bytes: u64,
    /// Messages the file holds.
    pub messages: u64,
    /// Channels with anything held.
    pub channels: u64,
    /// Deleted messages' tombstones held.
    pub tombstones: u64,
    /// Coverage intervals held.
    pub intervals: u64,
    /// Days of message time the coverage of each listed channel's own timeline spans (its `main`
    /// and `flat` scopes, not its threads), summed over channels.
    pub covered_days: f64,
    /// What the writer has done since the file opened, drops and batch durations included; absent
    /// with no file open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<CacheStats>,
    /// Each provider, and whether its account has been checked against the file, or it refused
    /// the credential.
    pub providers: Vec<ProviderStanding>,
    /// Channels and threads not served while access to them is in doubt.
    pub blocked: u64,
    /// What the shadow has counted since the process started, per consumer: what the cache would
    /// have served ([`super::shadow`]).
    pub shadow: Vec<shadow::ConsumerCounts>,
}

/// One provider's standing against the cache file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderStanding {
    /// The provider key, as in the configuration.
    pub provider: String,
    /// `verified`, `unverified` or `suspended`: see [`Verification`].
    pub verification: &'static str,
}

/// How many of the writer's events the summaries task has followed, against the count the cache
/// had sent when the task began.
#[derive(Debug, Default)]
struct OrphanProgress {
    handled: Arc<AtomicU64>,
    sent_before: u64,
}

/// What the sink knows about the channels the server answers for.
#[derive(Debug, Default)]
struct Routes {
    /// Whether the list may be sent to the cache: see the module documentation.
    armed: bool,
    /// The providers the list was routed by, whose clients are asked which account they are.
    router: Option<Arc<ChatRouter>>,
    /// The latest list, as the cache names its channels; `None` until the first is set.
    snapshot: Option<RouteSnapshot>,
    /// Each channel a fill may be begun for: the provider that owns it, and how that provider
    /// lists it.
    fills: HashMap<ChannelId, Route>,
}

/// How one channel is read: its provider's key, the shape of that provider, whether its recent
/// reads go through a bridge's Flat view, and whether the server polls its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Route {
    key: String,
    shape: Shape,
    recent_via_flat: bool,
    /// The server polls the provider (`live_poll_seconds`): the channel's live feed is then the
    /// poller's, which publishes creations alone, and the ingest route refuses an adapter's pushes
    /// for it. Nothing ever delivers a deletion or an edit of the channel, whatever its provider's
    /// shape, so the feed vouches for none of its messages ([`shadow::Facts::fed_since_ms`]).
    polled: bool,
}

/// One served read, for coalescing its background refresh ([`Sink::claim_refresh`]): the
/// channel, and the read in words the caller chooses (its view or window and its size). A
/// refresh of a different size is a different read. `#384 cache-serve`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefreshKey {
    /// The channel read.
    pub channel: ChannelId,
    /// Which read of it: never logged, compared for equality only.
    pub read: String,
}

impl RefreshKey {
    /// The read [`ServeOutcome::Served`] stands in for: one channel's newest window of `limit`.
    #[must_use]
    pub fn window(channel: ChannelId, limit: u16) -> Self {
        Self {
            channel,
            read: format!("window:{limit}"),
        }
    }

    /// One channel's newest timeline page of `view` (and `thread`), of `limit`.
    #[must_use]
    pub fn timeline(
        channel: ChannelId,
        view: TimelineView,
        thread: Option<&str>,
        limit: u16,
    ) -> Self {
        Self {
            channel,
            read: format!("timeline:{view:?}:{}:{limit}", thread.unwrap_or_default()),
        }
    }
}

/// A running background refresh's claim ([`Sink::claim_refresh`]): dropped, the read may be
/// refreshed again. `#384 cache-serve`.
#[derive(Debug)]
pub struct RefreshClaim {
    state: Weak<State>,
    key: RefreshKey,
}

impl Drop for RefreshClaim {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            lock(&state.refreshing).remove(&self.key);
        }
    }
}

/// When a read whose finds the live hub publishes began ([`Fill::read_start`]): the channel as the
/// cache named it then, the stamp taken then, and the local time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadStart {
    chan: ChanKey,
    stamp: Stamp,
    observed_ms: i64,
}

impl ReadStart {
    /// The local time the read began, by the cache's clock: the version of what it found.
    /// `#473 replication-c7-server-versions`.
    #[must_use]
    pub fn observed_ms(&self) -> i64 {
        self.observed_ms
    }
}

/// One upstream read's permission to offer what it sees, taken by [`Sink::begin`] before the read
/// starts.
///
/// It carries what has to be noted BEFORE the read: the stamp that lets the writer drop what the
/// read saw if the channel or the file is erased while it runs, and the local time the read
/// started, which is what decides whether what it saw is newer than what the cache already holds.
///
/// A fill begun before the provider's account was checked against the file offers deletions
/// alone ([`Fill::writes_rows`]): see the module documentation.
///
/// It holds the cache it was begun on weakly. A read can take a long time (a `/count` walk, a
/// poller's seed), and the owner may turn the cache off and on again meanwhile: the handle the
/// fill began on must then go, and its lock on the file with it, or the fresh file could not
/// open. A fill whose cache has gone offers nothing; its stamp was for that file anyway.
#[derive(Debug)]
pub struct Fill {
    grant: FillGrant,
    channel: ChannelId,
    chan: ChanKey,
    stamp: Stamp,
    observed_ms: i64,
    cache: Weak<MessageCache>,
    shape: Shape,
    recent_via_flat: bool,
    rows: bool,
    arrivals: Weak<Arrivals>,
}

impl Fill {
    /// The grant the fill was begun under.
    #[must_use]
    pub fn grant(&self) -> FillGrant {
        self.grant
    }

    /// The channel being read.
    #[must_use]
    pub fn channel(&self) -> &ChannelId {
        &self.channel
    }

    /// The channel as the cache names it: the provider that owns it, and its id.
    #[must_use]
    pub fn chan(&self) -> &ChanKey {
        &self.chan
    }

    /// The stamp taken when the fill began.
    #[must_use]
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }

    /// The local time the read started, by the cache's clock.
    #[must_use]
    pub fn observed_ms(&self) -> i64 {
        self.observed_ms
    }

    /// When the read began, and its stamp: what a change the live hub publishes from it is offered
    /// under ([`Sink::offer_push`]).
    #[must_use]
    pub fn read_start(&self) -> ReadStart {
        ReadStart {
            chan: self.chan.clone(),
            stamp: self.stamp,
            observed_ms: self.observed_ms,
        }
    }

    /// The cache the fill offers to, while it is still installed or held elsewhere.
    #[must_use]
    pub fn cache(&self) -> Option<Arc<MessageCache>> {
        self.cache.upgrade()
    }

    /// How the channel's provider lists it.
    #[must_use]
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// Whether the fill offers what the read saw. `false` when the provider's account had not
    /// been checked against the file when it began, or the sink was not armed: it then offers
    /// deletions alone, and marks the channel suspect for anything else.
    #[must_use]
    pub fn writes_rows(&self) -> bool {
        self.rows
    }

    /// Offer a `/timeline` page, raw, as the provider answered `request`. Whether every
    /// observation it made was queued: a full queue, or a fill that writes no rows, leaves some
    /// out (and the channel in doubt).
    pub fn offer_timeline(
        self,
        request: &TimelineRequest,
        read: TimelineRead,
        page: &TimelinePage,
    ) -> bool {
        let observed_ms = self.observed(page.as_of.as_deref());
        let observations = observe::timeline(self.shape, request, read, page, observed_ms);
        self.offer(observations)
    }

    /// Offer one step back of the full-history walk: `page`, read with `request`'s `before`, the
    /// continuation of a page whose oldest message was written at `below_ms`
    /// ([`observe::older`]). `#595 cache-full-history`.
    /// Whether every observation was queued, as [`Fill::offer_timeline`] says.
    pub fn offer_older(
        self,
        request: &TimelineRequest,
        page: &TimelinePage,
        below_ms: i64,
    ) -> bool {
        let observed_ms = self.observed(page.as_of.as_deref());
        let observations = observe::older(self.shape, request, page, below_ms, observed_ms);
        self.offer(observations)
    }

    /// Record where the full-history walk of the channel stands, under the stamp of the read it
    /// rests on: begin this fill before that read. `#595 cache-full-history`.
    pub fn offer_frontier(self, frontier: super::Frontier) -> bool {
        self.offer(vec![Observation::Frontier(frontier)])
    }

    /// Offer a read of the channel's most recent messages through `fetch_recent_dated`, which a
    /// bridge configured so answers from its Flat view.
    ///
    /// The only way a recent read is offered. `fetch_recent` on such a bridge answers from the same
    /// copy and drops its `as_of`, so what it returned would be offered as of when the read began,
    /// however old the copy: an edit or a deletion pushed in between would be undone by it.
    pub fn offer_recent_dated(self, read: &RecentRead) {
        let observed_ms = self.observed(read.as_of.as_deref());
        let observations = observe::window(self.shape, read, self.recent_via_flat, observed_ms);
        self.offer(observations);
    }

    /// Offer one channel's answer to a read of several channels at once
    /// ([`crate::chat::ChatClient::fetch_recent_many`]), dated as a recent read is: never as a
    /// read through a bridge's Flat view ([`observe::window_among_several`]).
    pub fn offer_recent_among_several(self, read: &RecentRead) {
        let observed_ms = self.observed(read.as_of.as_deref());
        let observations = observe::window_among_several(self.shape, read, observed_ms);
        self.offer(observations);
    }

    /// Offer one `/page` step: the messages the provider returned inside the requested span,
    /// before any was cut to the page's size, and whether one was.
    pub fn offer_page(self, read: PageRead, messages: &[Message], truncated: bool) {
        let observations = observe::page(self.shape, read, messages, truncated, self.observed_ms);
        self.offer(observations);
    }

    /// Offer every message a `/count` walk read, from the newest page down.
    pub fn offer_walk(self, messages: &[Message]) {
        let observations = observe::walk(self.shape, messages, self.observed_ms);
        self.offer(observations);
    }

    /// Offer every message a count made of the provider's own range reads read, and whether the
    /// provider proved its last read reached the count's `since` ([`observe::range_walk`]).
    pub fn offer_range_walk(self, messages: &[Message], exhausted: bool) {
        let observations = observe::range_walk(self.shape, messages, exhausted, self.observed_ms);
        self.offer(observations);
    }

    /// Offer the messages a lookup by id found, raw, in the thread `thread_id` when it looked in
    /// one ([`observe::looked_up`]).
    pub fn offer_looked_up(self, messages: &[Message], thread_id: Option<&str>) {
        let observations = observe::looked_up(self.shape, messages, thread_id, self.observed_ms);
        self.offer(observations);
    }

    /// Offer one message read by id from the thread `thread_id`.
    pub fn offer_single(self, message: &Message, thread_id: &str) {
        let observations = observe::single(self.shape, message, thread_id, self.observed_ms);
        self.offer(observations);
    }

    /// Offer one change from the live feed, observed when the fill began: the poller's, when the
    /// fill was begun for it, and otherwise an adapter's ([`observe::PushedBy`]).
    pub fn offer_push(self, live: &LiveMessage) {
        let by = if self.grant.server_fill() == Some(ServerFill::Poller) {
            observe::PushedBy::Poller
        } else {
            observe::PushedBy::Adapter
        };
        let observation = observe::pushed(self.shape, live, by, self.observed_ms);
        self.offer(vec![observation]);
    }

    /// Offer what the live poller's walk forward from its cursor read: every message its pages
    /// returned, the thread replies its provider read apart (`#307 slack-reply-live`), `after_ms`
    /// the provider time of the cursor it walked from, and `reached_end` whether it ended on a page
    /// shorter than it asked for, rather than at its page bound.
    pub fn offer_forward(
        self,
        messages: &[Message],
        replies: &[Message],
        after_ms: i64,
        reached_end: bool,
    ) {
        let observations = observe::forward(
            self.shape,
            messages,
            replies,
            after_ms,
            reached_end,
            self.observed_ms,
        );
        self.offer(observations);
    }

    /// Offer one message this server posted, into the thread `thread_id` when it went to one.
    pub fn offer_posted(self, message: &Message, thread_id: Option<&str>) {
        let observations = observe::posted(self.shape, message, thread_id, self.observed_ms);
        self.offer(observations);
    }

    /// When the read saw the channel: when it began, or the provider's `as_of` when that is
    /// earlier. `as_of` is the wall clock of a provider on this host.
    fn observed(&self, as_of: Option<&str>) -> i64 {
        as_of
            .and_then(crate::clock::instant_ms)
            .map_or(self.observed_ms, |as_of| as_of.min(self.observed_ms))
    }

    /// Queue every observation, in order, under the stamp taken when the read began. A full queue
    /// drops one and marks the channel suspect; nothing here waits.
    ///
    /// A fill that writes no rows offers deletions alone. Anything else it leaves out marks the
    /// channel suspect, as a full queue's drop does: it may have been an edit the file now lacks.
    ///
    /// What is offered is noted by id, with who offered it, for the shadow's diagnostics: one
    /// brief lock, never a wait ([`Arrivals::note`]).
    fn offer(self, observations: Vec<Observation>) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        let arrivals = self.arrivals.upgrade();
        let offered_ms = cache.now_ms();
        let mut every = true;
        for observation in observations {
            if self.rows || observation.removes_only() {
                // Noted only once queued: an offer dropped never reached the file.
                let noted = arrivals.as_ref().map(|arrivals| {
                    (
                        arrivals,
                        Arrivals::noted(&observation, self.grant.label(), offered_ms),
                    )
                });
                if let Offered::Queued(_) = cache.offer(self.chan.clone(), self.stamp, observation)
                {
                    if let Some((arrivals, noted)) = noted {
                        arrivals.record(&self.chan, noted);
                    }
                } else {
                    every = false;
                }
            } else {
                every = false;
                cache.mark_suspect(&self.chan);
            }
        }
        every
    }
}

/// One shadow evaluation under way, begun by [`Sink::shadow`]: hand it upstream's answer with
/// [`Shadowed::compare`]. One dropped without it, because the read failed, is counted with no
/// comparison. Either way it is decided and counted in a detached task: nothing waits for it.
#[derive(Debug)]
pub struct Shadowed {
    task: Option<Box<ShadowTask>>,
}

#[derive(Debug)]
struct ShadowTask {
    ask: Ask,
    /// The channel, as the cache names it, and the scope the evaluation read (a message's is
    /// found by the read): for the diagnostics.
    chan: ChanKey,
    scope: Option<Scope>,
    /// Why the request read, for the diagnostics.
    reason: crate::read_reason::ReadReason,
    /// Whether the answer was handed back from the cache ([`Sink::try_serve`]).
    served: bool,
    arrivals: Weak<Arrivals>,
    feed: shadow::Feed,
    live_fed: bool,
    /// [`MessageCache::fed_since_ms`] when the ask was made.
    fed_since_ms: Option<i64>,
    asked_ms: i64,
    /// When the request asked, by the monotonic clock, and how its reader was answered and how
    /// long after that: noted when the answer is ready, timed when the evaluation is counted.
    /// `#502 cache-serve-coverage`.
    began: Instant,
    answered: Option<(shadow::Answered, Duration)>,
    /// The evaluation reading the cache on the blocking pool, until awaited: by the counting
    /// task, or on the request's own path by [`Sink::try_serve`], which must know the verdict
    /// before it answers.
    reading: Option<tokio::task::JoinHandle<Evaluated>>,
    /// What the evaluation read, once awaited.
    evaluated: Option<Evaluated>,
    progress: Arc<ShadowProgress>,
    cache: Weak<MessageCache>,
}

/// What an evaluation read.
#[derive(Debug)]
struct Evaluated {
    read: Read<View>,
    barrier: bool,
    /// The latest drop of an offer for the channel in this process, when it read.
    dropped_ms: Option<i64>,
    /// The highest sequence number applied once it had read.
    applied_after: u64,
    /// How long after the ask it had read: what an answer from the cache would have waited.
    read_took: Duration,
}

impl Evaluated {
    /// No cache to read.
    fn off() -> Self {
        Self {
            read: Read::Off,
            barrier: false,
            dropped_ms: None,
            applied_after: 0,
            read_took: Duration::ZERO,
        }
    }
}

impl Shadowed {
    /// Compare what the cache would have answered with `upstream`'s answer, once the evaluation
    /// has read.
    ///
    /// Call it as soon as upstream has answered and BEFORE the request offers the cache what it
    /// read: what was queued by then is noted, and an evaluation that read past it may have read
    /// the request's own write, so it is counted `late`, with no verdict.
    pub fn compare(mut self, upstream: Upstream) {
        if let Some(mut task) = self.task.take() {
            task.answered_by(shadow::Answered::Upstream);
            let enqueued = task.cache.upgrade().map(|cache| cache.enqueued_seq());
            (*task).settle(Some((upstream, enqueued)));
        }
    }

    /// Note upstream's first answer as [`Shadowed::compare`] would, what was queued by now with
    /// it, and leave the comparison to the caller, which may yet find the answer elsewhere: a
    /// lookup by id that missed a reply which the server then finds in the thread it knows the
    /// reply to be in (`#319 slack-reply-by-id`). Compared against that first answer, the reply the
    /// request then answers with would be counted as a row upstream no longer lists.
    ///
    /// Call it where [`Shadowed::compare`] would be called: as soon as upstream has answered, and
    /// before the request offers the cache what it read.
    #[must_use]
    pub fn hold(mut self, upstream: Upstream) -> HeldShadow {
        let task = self.task.take();
        let enqueued = task
            .as_ref()
            .and_then(|task| task.cache.upgrade())
            .map(|cache| cache.enqueued_seq());
        HeldShadow {
            task,
            first: Some(upstream),
            enqueued,
        }
    }
}

impl Drop for Shadowed {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            (*task).settle(None);
        }
    }
}

/// A shadow evaluation whose comparison waits for the request's final answer
/// ([`Shadowed::hold`]): compared with what the request found elsewhere
/// ([`HeldShadow::found`]), or with upstream's first answer when it found nothing more
/// ([`HeldShadow::nothing_more`]). Dropped without either, because the further read failed, it is
/// counted with no comparison. Whichever, what was queued when upstream first answered decides
/// whether the evaluation read the request's own writes.
#[derive(Debug)]
pub struct HeldShadow {
    task: Option<Box<ShadowTask>>,
    first: Option<Upstream>,
    enqueued: Option<u64>,
}

impl HeldShadow {
    /// The request answered with what it found elsewhere: compare with that.
    pub fn found(mut self, upstream: Upstream) {
        if let Some(mut task) = self.task.take() {
            task.answered_by(shadow::Answered::Upstream);
            (*task).settle(Some((upstream, self.enqueued)));
        }
    }

    /// The request found nothing more: compare with upstream's first answer, as
    /// [`Shadowed::compare`] would have.
    pub fn nothing_more(mut self) {
        if let (Some(mut task), Some(first)) = (self.task.take(), self.first.take()) {
            task.answered_by(shadow::Answered::Upstream);
            (*task).settle(Some((first, self.enqueued)));
        }
    }
}

impl Drop for HeldShadow {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            (*task).settle(None);
        }
    }
}

/// How long [`Sink::try_serve`] waits for its evaluation before the request goes upstream as
/// it always did: the barrier's own wait, the cache read, and room to spare, against a
/// provider read of seconds. A slower evaluation still counts, off the path, as a shadow.
pub const SERVE_WAIT: Duration = Duration::from_millis(300);

/// What [`Sink::try_serve`] decided. `#384 cache-serve`.
#[derive(Debug)]
pub enum ServeOutcome {
    /// Answer from the cache: the snapshot read, and the handle the background refresh hands
    /// upstream's answer to, so the serve is compared and counted like any shadow.
    Served(Box<ServedAnswer>),
    /// Go upstream as without serving, with this ask's shadow (when there is one) to hand the
    /// answer to, exactly as [`Sink::shadow`] returned it.
    NotServed(Option<Shadowed>),
}

/// An answer to hand back from the cache. `#384 cache-serve`.
#[derive(Debug)]
pub struct ServedAnswer {
    /// What the evaluation read of the channel, in one snapshot: the rows to answer with, and
    /// the head and coverage the rule served on.
    pub view: View,
    /// Hand the refreshing read's answer here once upstream has spoken.
    pub pending: PendingServe,
}

/// A served answer's comparison, waiting for the background refresh's word from upstream:
/// [`PendingServe::compare`] with it, or dropped when the refresh failed, counted with no
/// comparison either way settled. `#384 cache-serve`.
#[derive(Debug)]
pub struct PendingServe {
    task: Option<Box<ShadowTask>>,
}

impl PendingServe {
    /// Compare what was served with what the refresh read upstream, as
    /// [`Shadowed::compare`] does before that read is offered to the cache.
    pub fn compare(mut self, upstream: Upstream) {
        if let Some(task) = self.task.take() {
            let enqueued = task.cache.upgrade().map(|cache| cache.enqueued_seq());
            (*task).settle(Some((upstream, enqueued)));
        }
    }
}

impl Drop for PendingServe {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            (*task).settle(None);
        }
    }
}

impl ShadowTask {
    /// Note that the request's reader has its answer now, `answered` so; the first note stands.
    fn answered_by(&mut self, answered: shadow::Answered) {
        if self.answered.is_none() {
            self.answered = Some((answered, self.began.elapsed()));
        }
    }

    /// Await the evaluation's read of the cache, once: what counting decides on.
    async fn evaluate(&mut self) {
        if self.evaluated.is_none() {
            if let Some(reading) = self.reading.take() {
                self.evaluated = reading.await.ok();
            }
        }
    }

    /// [`ShadowTask::evaluate`] given at most `wait`: what [`Sink::try_serve`] awaits before
    /// answering from the cache. On timeout the read is still running and its handle is kept,
    /// so the evaluation is still counted off the path exactly as a shadow's is — a slower
    /// evaluation is evidence about the cache, and dropping it from the counts would hide
    /// precisely that.
    ///
    /// Returns whether the evaluation has read.
    async fn evaluate_for(&mut self, wait: Duration) -> bool {
        if self.evaluated.is_some() {
            return true;
        }
        let Some(reading) = self.reading.as_mut() else {
            return false;
        };
        match tokio::time::timeout(wait, reading).await {
            Ok(read) => {
                self.evaluated = read.ok();
                self.reading = None;
                true
            }
            Err(_) => false,
        }
    }

    /// Decide, compare and count, in a detached task.
    fn settle(self, upstream: Option<(Upstream, Option<u64>)>) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(self.count(upstream)));
        } else {
            // No runtime to count it on; a request always has one.
            self.progress.counted.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn count(mut self, upstream: Option<(Upstream, Option<u64>)>) {
        self.evaluate().await;
        let Self {
            ask,
            chan,
            scope,
            reason,
            served,
            arrivals,
            feed,
            live_fed,
            fed_since_ms,
            asked_ms,
            answered,
            evaluated,
            progress,
            cache,
            ..
        } = self;
        let consumer = ask.consumer();
        if let Some((answered, took)) = answered {
            progress.tallies.timed(consumer, answered, took);
        }
        if let Some(Evaluated {
            read,
            barrier,
            dropped_ms,
            applied_after,
            read_took,
        }) = evaluated
        {
            let (decision, view) = match read {
                Read::Served(view) => {
                    let past = upstream
                        .as_ref()
                        .and_then(|(_, enqueued)| *enqueued)
                        .is_some_and(|enqueued| applied_after > enqueued);
                    if past || shadow::late(&view, asked_ms) {
                        (Decision::Late, None)
                    } else {
                        let facts = shadow::Facts {
                            asked_ms,
                            live_fed,
                            fed_since_ms,
                            dropped_ms,
                            barrier,
                            feed,
                        };
                        (shadow::decide(&ask, &view, &facts), Some(view))
                    }
                }
                Read::Off => (Decision::Miss(Miss::Off), None),
                Read::Blocked | Read::Moved | Read::Failed(_) => {
                    (Decision::Miss(Miss::Blocked), None)
                }
            };
            progress.tallies.decided(consumer, decision);
            if let (Decision::Miss(Miss::Gap), Some(view)) = (decision, &view) {
                if let Some(causes) = shadow::gap_causes(&ask, view) {
                    progress.tallies.gapped(consumer, &causes);
                }
            }
            // Timed only beside an upstream answer, as the reads it is set against are.
            if decision == Decision::Serve && !served && upstream.is_some() {
                progress
                    .tallies
                    .timed(consumer, shadow::Answered::Would, read_took);
            }
            if let (Decision::Serve, Some(view), Some((upstream, _))) = (decision, &view, &upstream)
            {
                if let Some((comparison, detail)) =
                    shadow::compare_detail(&ask, view, upstream, asked_ms)
                {
                    progress.tallies.compared(consumer, &comparison);
                    if comparison != shadow::Comparison::default() {
                        let context = NoteContext {
                            consumer,
                            chan: &chan,
                            scope: scope.as_ref().or(view.home.as_ref()),
                            reason,
                            served,
                            asked_ms,
                            live_fed,
                            view,
                        };
                        note(&progress, &context, &comparison, &detail, &arrivals, &cache).await;
                    }
                }
            }
        }
        progress.counted.fetch_add(1, Ordering::SeqCst);
    }
}

/// What one diagnostic line says beside a comparison's counts. `#497 cache-shadow-missing-row`.
struct NoteContext<'a> {
    consumer: shadow::Consumer,
    chan: &'a ChanKey,
    scope: Option<&'a Scope>,
    reason: crate::read_reason::ReadReason,
    served: bool,
    asked_ms: i64,
    live_fed: bool,
    view: &'a View,
}

/// A time as an offset from the ask, `+1200ms` or `-35000ms`, or `none`: short, and read at a
/// glance against the read the answer rests on. Provider times and local ones alike, both wall
/// clocks.
fn since_ask(at: Option<i64>, asked_ms: i64) -> String {
    at.map_or_else(
        || "none".to_owned(),
        |at| format!("{:+}ms", at.saturating_sub(asked_ms)),
    )
}

/// What the read scope's coverage says of instant `at`: above the head, in its unsettled band
/// (held by the latest read of the newest end alone), inside a claim (its source and when it was
/// listed), or nowhere.
fn claim_at(view: &View, at: i64, asked_ms: i64) -> String {
    if let Some(head) = view.head {
        if at > head.head_ms {
            return "above-head".to_owned();
        }
        if at >= head.settled_top() {
            return "band".to_owned();
        }
    }
    view.coverage
        .iter()
        .find(|interval| interval.t0_ms <= at && at < interval.t1_ms)
        .map_or_else(
            || "uncovered".to_owned(),
            |interval| {
                format!(
                    "{}@{}",
                    interval.source.as_str(),
                    since_ask(Some(interval.listed_ms), asked_ms)
                )
            },
        )
}

/// What the file holds of a message the answer lacked, read after the comparison: held (when last
/// seen, and in which scopes), tombstoned, absent, or not read.
fn file_row(cache: &MessageCache, chan: &ChanKey, id: &MessageId, asked_ms: i64) -> String {
    match cache.message(chan, id).served() {
        Some(Some(row)) => {
            let scopes = cache
                .memberships(chan, id)
                .served()
                .unwrap_or_default()
                .iter()
                .map(Scope::key)
                .collect::<Vec<_>>()
                .join("+");
            format!(
                "held(observed={},scopes={scopes})",
                since_ask(Some(row.observed_ms), asked_ms)
            )
        }
        Some(None) => match cache.tombstoned(chan, id).served() {
            Some(true) => "tombstoned".to_owned(),
            Some(false) => "absent".to_owned(),
            None => "unread".to_owned(),
        },
        None => "unread".to_owned(),
    }
}

/// One message of a diagnostic line: its id, its times as offsets from the ask, where the read
/// scope's coverage puts it, how it reached the cache, and what the file holds of it.
fn describe(
    row: &Differing,
    context: &NoteContext<'_>,
    arrivals: Option<&Arrivals>,
    files: &HashMap<String, String>,
) -> String {
    let asked = context.asked_ms;
    let mut out = format!("{} created={}", row.id, since_ask(row.created_ms, asked));
    if let Some(created) = row.created_ms {
        out.push_str(&format!(
            " claim={}",
            claim_at(context.view, created, asked)
        ));
    }
    if let Some(held) = row.held {
        out.push_str(&format!(
            " observed={} vouched={}{}",
            since_ask(Some(held.observed_ms), asked),
            since_ask(held.vouched_ms, asked),
            if held.partial { " partial" } else { "" }
        ));
    }
    match arrivals.and_then(|arrivals| arrivals.of(context.chan, &row.id)) {
        Some(arrived) => {
            let one = |at: super::arrivals::Arrival| {
                format!(
                    "{}/{}@{}",
                    at.by,
                    at.kind,
                    since_ask(Some(at.offered_ms), asked)
                )
            };
            out.push_str(&format!(" arrived={}", one(arrived.first)));
            if arrived.latest != arrived.first {
                out.push_str(&format!(",latest={}", one(arrived.latest)));
            }
        }
        None => out.push_str(" arrived=never"),
    }
    if let Some(file) = files.get(&row.id) {
        out.push_str(&format!(" file={file}"));
    }
    out
}

/// At most [`NOTED_ROWS`] messages, described, and a count of the rest.
fn describe_all(
    rows: &[Differing],
    context: &NoteContext<'_>,
    arrivals: Option<&Arrivals>,
    files: &HashMap<String, String>,
) -> String {
    let mut parts: Vec<String> = rows
        .iter()
        .take(NOTED_ROWS)
        .map(|row| describe(row, context, arrivals, files))
        .collect();
    if rows.len() > NOTED_ROWS {
        parts.push(format!("+{} more", rows.len() - NOTED_ROWS));
    }
    parts.join("; ")
}

/// Write one line about a comparison that found anything: a `warn` for a difference, an `info`
/// for messages written only past the end of the answer's claim ([`shadow::claim_end`]). Content-free: ids, times,
/// counts and words this server chooses, never a word of text. At most [`NOTES_PER_HOUR`] of
/// each, the rest counted and said by the next. `#497 cache-shadow-missing-row`.
async fn note(
    progress: &ShadowProgress,
    context: &NoteContext<'_>,
    comparison: &shadow::Comparison,
    detail: &shadow::Detail,
    arrivals: &Weak<Arrivals>,
    cache: &Weak<MessageCache>,
) {
    let kind = if comparison.agreed() {
        NoteKind::Past
    } else {
        NoteKind::Differs
    };
    let Some(held_back) = lock(&progress.notes).take(kind, Instant::now()) else {
        return;
    };
    // What the file holds of what the answer lacked, read now: a later write (the request's own,
    // even) may show in it, and its times say so.
    let lacked: Vec<MessageId> = detail
        .missing
        .iter()
        .chain(&detail.stale)
        .chain(&detail.newer)
        .take(3 * NOTED_ROWS)
        .map(|row| MessageId(row.id.clone()))
        .collect();
    let files = match cache.upgrade() {
        Some(cache) if !lacked.is_empty() => {
            let chan = context.chan.clone();
            let asked_ms = context.asked_ms;
            tokio::task::spawn_blocking(move || {
                lacked
                    .iter()
                    .map(|id| (id.0.clone(), file_row(&cache, &chan, id, asked_ms)))
                    .collect::<HashMap<_, _>>()
            })
            .await
            .unwrap_or_default()
        }
        _ => HashMap::new(),
    };
    let arrivals = arrivals.upgrade();
    let arrivals = arrivals.as_deref();
    let asked = context.asked_ms;
    let view = context.view;
    let head = view.head;
    let rows = |list: &[Differing]| describe_all(list, context, arrivals, &files);
    macro_rules! line {
        ($level:ident, $message:literal) => {
            tracing::$level!(
                consumer = context.consumer.label(),
                served = context.served,
                provider = %context.chan.provider_key,
                channel = %context.chan.channel_id,
                scope = %context.scope.map_or_else(|| "none".to_owned(), Scope::key),
                read_reason = context.reason.as_str(),
                live_fed = context.live_fed,
                asked_ms = asked,
                version = %since_ask(detail.version_ms, asked),
                claim_end = %since_ask(detail.claim_end_ms, asked),
                head = %since_ask(head.map(|head| head.head_ms), asked),
                settled_top = %since_ask(head.map(|head| head.settled_top()), asked),
                checked_from = %since_ask(view.head_checked_from_ms, asked),
                window_rows = view.rows.len(),
                window_oldest = %since_ask(view.rows.first().map(|row| row.created_ms), asked),
                window_newest = %since_ask(view.rows.last().map(|row| row.created_ms), asked),
                compared_from = %since_ask(detail.from_ms, asked),
                compared_until = %since_ask(detail.until_ms, asked),
                upstream_rows = detail.upstream_rows,
                upstream_oldest = %since_ask(detail.upstream_oldest_ms, asked),
                upstream_newest = %since_ask(detail.upstream_newest_ms, asked),
                upstream_bounded = detail.upstream_bounded,
                missing_rows = comparison.missing,
                stale_rows = comparison.stale,
                newer_rows = comparison.newer,
                extra_rows = comparison.extra,
                content_differs = comparison.differs,
                missing = %rows(&detail.missing),
                stale = %rows(&detail.stale),
                newer = %rows(&detail.newer),
                extra = %rows(&detail.extra),
                differs = %rows(&detail.differs),
                held_back,
                $message
            )
        };
    }
    match kind {
        NoteKind::Differs => line!(
            warn,
            "the message cache's would-be answer differed from its provider's; times are \
             offsets from the ask"
        ),
        NoteKind::Past => line!(
            info,
            "the message cache's would-be answer lacked only messages written past the end of \
             what it claims (stale: before the ask; newer: after it); times are offsets from the \
             ask"
        ),
    }
}

/// Every consumer's read latencies, one `;`-separated entry per consumer and way of answering
/// that timed any read: `timeline_fallback cache n=53 p50_ms=0.4 p90_ms=0.9 p99_ms=2.1`. `none`
/// when nothing was timed. Names and numbers only. `#502 cache-serve-coverage`.
#[must_use]
pub fn read_timings(consumers: &[shadow::ConsumerCounts]) -> String {
    // Microseconds to milliseconds with one decimal, as a figure to read.
    #[allow(clippy::cast_precision_loss)]
    let ms = |us: u64| (us as f64 / 100.0).round() / 10.0;
    let entries: Vec<String> = consumers
        .iter()
        .flat_map(|counts| {
            shadow::Answered::ALL.iter().filter_map(move |&answered| {
                let summary = counts.latency.of(answered);
                (summary.n > 0).then(|| {
                    format!(
                        "{} {} n={} p50_ms={} p90_ms={} p99_ms={}",
                        counts.consumer,
                        answered.label(),
                        summary.n,
                        ms(summary.p50_us),
                        ms(summary.p90_us),
                        ms(summary.p99_us)
                    )
                })
            })
        })
        .collect();
    if entries.is_empty() {
        "none".to_owned()
    } else {
        entries.join("; ")
    }
}

/// The hourly line's `gap` misses by cause (`#510 cache-serve-every-read`): one entry per
/// consumer that had any, `<consumer> <cause>=<count> ...`, joined by `; `; `none` when none had.
#[must_use]
pub fn gap_causes(consumers: &[shadow::ConsumerCounts]) -> String {
    let entries: Vec<String> = consumers
        .iter()
        .filter(|counts| counts.gaps != shadow::GapCounts::default())
        .map(|counts| {
            let causes: Vec<String> = shadow::GapCounts::LABELS
                .iter()
                .zip(counts.gaps.each())
                .map(|(label, n)| format!("{label}={n}"))
                .collect();
            format!("{} {}", counts.consumer, causes.join(" "))
        })
        .collect();
    if entries.is_empty() {
        "none".to_owned()
    } else {
        entries.join("; ")
    }
}

/// Wait up to `wait` for what was offered up to `target` to be applied to the cache `weak` names,
/// holding the cache only while looking, so one taken out from behind the sink meanwhile closes as
/// soon as its other holders let it go. Raises the writer's flush flag while there is anything to
/// wait for, as every barrier does.
fn barrier(weak: &Weak<MessageCache>, target: u64, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        let Some(cache) = weak.upgrade() else {
            return false;
        };
        if cache.barrier_to_blocking(target, Duration::ZERO) {
            return true;
        }
        drop(cache);
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl Sink {
    /// A sink with no cache behind it: every fill is refused.
    #[must_use]
    pub fn off() -> Self {
        Self::default()
    }

    /// Attach what opens, switches and erases the cache file: once per sink, by
    /// [`super::lifecycle::start`].
    pub(crate) fn attach_lifecycle(&self, lifecycle: Arc<Lifecycle>) {
        let _ = self.lifecycle.set(lifecycle);
    }

    /// Say whether reads may be answered from the cache (`[message_cache] serve`): set by
    /// [`super::lifecycle::start`] from the configuration, and by a test that installs a cache
    /// itself. `#384 cache-serve`.
    pub fn set_serve(&self, serve: bool) {
        self.state.serve.store(serve, Ordering::SeqCst);
    }

    /// Whether reads may be answered from the cache. A cache must also be open behind the sink;
    /// [`Sink::try_serve`] checks both. `#384 cache-serve`.
    #[must_use]
    pub fn serve_enabled(&self) -> bool {
        self.state.serve.load(Ordering::SeqCst)
    }

    /// Since when every change `route`'s channel's live feed offered has reached the file, for
    /// the shadow's facts — now judged per provider (`#384 cache-serve`, requirement (g) of
    /// `#297 cache-coverage-races`). A polled channel's feed vouches for nothing, as before. Any
    /// other channel's vouches only while an adapter is actually pushing for its provider: the
    /// latest push recorded for the provider (any of its channels) no more than
    /// [`shadow::FED_PUSH_RECENT_MS`] ago by the cache's clock. `[ingest]` merely being enabled
    /// proves nothing: an adapter that never connects delivers no deletion.
    fn fed_since_for(&self, route: &Route, cache: &MessageCache) -> Option<i64> {
        if route.polled {
            return None;
        }
        let last = lock(&self.state.pushed).get(&route.key).copied()?;
        (cache.now_ms().saturating_sub(last) <= shadow::FED_PUSH_RECENT_MS)
            .then(|| cache.fed_since_ms())
    }

    /// Claim the background refresh of one served read, or `None` when one is already running:
    /// a page reloaded twice starts one upstream read, not two. The claim is given up when the
    /// returned guard is dropped. `#384 cache-serve`.
    #[must_use]
    pub fn claim_refresh(&self, key: RefreshKey) -> Option<RefreshClaim> {
        lock(&self.state.refreshing)
            .insert(key.clone())
            .then(|| RefreshClaim {
                state: Arc::downgrade(&self.state),
                key,
            })
    }

    /// What opens, switches and erases the cache file, once the server has started it.
    #[must_use]
    pub fn lifecycle(&self) -> Option<&Arc<Lifecycle>> {
        self.lifecycle.get()
    }

    /// Have the store's summaries kept in step with every cache installed from now on: see
    /// [`super::orphans`]. Once per sink; the server attaches its own before it installs a cache.
    pub fn attach_summaries(&self, summaries: Summaries) {
        let _ = self.state.summaries.set(summaries);
    }

    /// Put `cache` behind the sink. If the sink is armed, the latest route snapshot is sent to it
    /// at once: the cache serves and accepts nothing until it has one.
    ///
    /// When the server's summaries are attached and a runtime is running, this first starts the
    /// task that deletes the store's summaries the cache learns are stale ([`super::orphans`]),
    /// so it hears of the very first sweep. It ends when the cache does.
    ///
    /// Every provider is unverified against the new cache, and once the sink is armed, each is
    /// asked which account it is: see the module documentation.
    pub fn install(&self, cache: Arc<MessageCache>) {
        let state = &self.state;
        if let (Some(summaries), Ok(runtime)) =
            (state.summaries.get(), tokio::runtime::Handle::try_current())
        {
            let handled = Arc::new(AtomicU64::new(0));
            *state.orphans.lock().unwrap_or_else(PoisonError::into_inner) = OrphanProgress {
                handled: Arc::clone(&handled),
                sent_before: cache.stats().orphans_sent,
            };
            runtime.spawn(orphans::run(
                cache.subscribe_events(orphans::EVENT_QUEUE),
                summaries.clone(),
                Arc::clone(&state.listed),
                handled,
            ));
        }
        {
            let routes = state.routes();
            *state.slot.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&cache));
            if routes.armed {
                if let Some(snapshot) = &routes.snapshot {
                    cache.retain_only(snapshot.clone());
                }
            }
        }
        State::verify_all(state);
        State::listen(state);
    }

    /// Take the cache out from behind the sink, leaving it empty, and stop listening to the
    /// providers' access signals. The cache itself is closed when the last handle on it is
    /// dropped.
    pub fn uninstall(&self) -> Option<Arc<MessageCache>> {
        let taken = {
            let _routes = self.state.routes();
            self.state
                .slot
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        };
        State::listen(&self.state);
        taken
    }

    /// The cache behind the sink, if any.
    #[must_use]
    pub fn cache(&self) -> Option<Arc<MessageCache>> {
        self.state.cache()
    }

    /// Allow the route list to reach the cache, and send the latest one now. Called once the
    /// whole list is known: see the module documentation. With a cache behind the sink, each
    /// provider is then asked which account it is.
    pub fn arm(&self) {
        {
            let mut routes = self.state.routes();
            routes.armed = true;
            if let (Some(cache), Some(snapshot)) = (self.cache(), &routes.snapshot) {
                cache.retain_only(snapshot.clone());
            }
        }
        State::verify_all(&self.state);
        State::listen(&self.state);
    }

    /// Whether [`Sink::arm`] has been called.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.state.routes().armed
    }

    /// The channels the server answers for have changed to `channels`: name each under the
    /// provider `router` routes it to, and, once armed, send the cache the new snapshot, which
    /// sweeps every other channel and every provider key `router` does not have out of the file.
    ///
    /// A channel the router routes to no provider is left out: nothing can be read from it, so
    /// nothing may be kept for it either. Hidden channels are not in `channels` at all. A channel
    /// whose provider `config` does not describe stays in the snapshot but is never filled: what
    /// its reads prove depends on how that provider lists it.
    ///
    /// A `router` other than the last one is a new set of providers: each is asked which account
    /// it is before anything of it is filled.
    pub fn set_routes(&self, channels: &[ChannelInfo], router: &Arc<ChatRouter>, config: &Config) {
        let mut fills = HashMap::with_capacity(channels.len());
        let mut chans = Vec::with_capacity(channels.len());
        for channel in channels {
            let Some(key) = router.key_for(&channel.id) else {
                continue;
            };
            chans.push(ChanKey::new(key.clone(), channel.id.as_str()));
            if let Some(provider) = config.provider(&key) {
                let recent_via_flat = match &provider.kind {
                    ProviderKind::Discord(discord) => discord.recent.view == RecentView::Flat,
                    ProviderKind::Slack(_) => false,
                };
                let polled = router
                    .entry(&key)
                    .is_some_and(|entry| entry.live_poll_seconds > 0);
                let route = Route {
                    shape: Shape::of(provider, channel.id.as_str()),
                    key,
                    recent_via_flat,
                    polled,
                };
                fills.insert(channel.id.clone(), route);
            }
        }
        let snapshot = RouteSnapshot::new(
            chans,
            router.entries().iter().map(|entry| entry.key.clone()),
        );
        *self
            .state
            .listed
            .write()
            .unwrap_or_else(PoisonError::into_inner) =
            channels.iter().map(|channel| channel.id.clone()).collect();
        let replaced = {
            let mut routes = self.state.routes();
            routes.fills = fills;
            routes.snapshot = Some(snapshot.clone());
            let replaced = (!routes
                .router
                .as_ref()
                .is_some_and(|known| Arc::ptr_eq(known, router)))
            .then(|| routes.router.replace(Arc::clone(router)));
            if routes.armed {
                if let Some(cache) = self.cache() {
                    cache.retain_only(snapshot);
                }
            }
            replaced
        };
        if let Some(previous) = replaced {
            if let Some(previous) = previous {
                previous.access().listen(None);
            }
            State::verify_all(&self.state);
            State::listen(&self.state);
        }
    }

    /// Begin a fill of `channel` under `grant`, before its upstream read starts. `None` means
    /// nothing the read sees is offered: there is no cache, or the channel is not one the server
    /// answers for.
    ///
    /// When the sink is not armed, or the provider's account has not been checked against the
    /// file open now (or the provider is suspended), the fill offers deletions alone
    /// ([`Fill::writes_rows`] is `false`): see the module documentation.
    #[must_use]
    pub fn begin(&self, grant: &FillGrant, channel: &ChannelId) -> Option<Fill> {
        let cache = self.cache()?;
        let (route, armed) = {
            let routes = self.state.routes();
            (routes.fills.get(channel)?.clone(), routes.armed)
        };
        let rows = armed && State::may_fill(&self.state, &route.key, &cache);
        let chan = ChanKey::new(route.key, channel.as_str());
        let stamp = cache.stamp(&chan);
        let observed_ms = cache.now_ms();
        Some(Fill {
            grant: *grant,
            channel: channel.clone(),
            chan,
            stamp,
            observed_ms,
            cache: Arc::downgrade(&cache),
            shape: route.shape,
            recent_via_flat: route.recent_via_flat,
            rows,
            arrivals: Arc::downgrade(&self.state.arrivals),
        })
    }

    /// [`Sink::begin`] under the grant a request's caller holds: none at read scope.
    #[must_use]
    pub fn begin_for(&self, caller: crate::auth::Scope, channel: &ChannelId) -> Option<Fill> {
        FillGrant::for_caller(caller).and_then(|grant| self.begin(&grant, channel))
    }

    /// Offer one change the live hub is publishing to `channel`, from `producer`: the poller, or
    /// an adapter's pushed event. `read`, when given, is when the read that found it began (the
    /// poller's): the change is offered as of then, or now if that is earlier, so a deletion or an
    /// edit the cache learned of while that read was on its way back is not undone by it; and under
    /// that read's stamp, so an erase made while it was on its way back drops the change as it
    /// drops the read's own offers. (A channel the cache names otherwise now, its provider changed
    /// meanwhile, is offered under its name now.)
    ///
    /// Called by [`crate::live::LiveHub`] while it holds the channel's feed lock and BEFORE any
    /// subscriber is sent the change, so a change a subscriber could have seen is already queued,
    /// with a sequence number, by the time anyone who saw it reads the cache. So it must never
    /// wait and never call back into the hub: the lock order is the feed lock, then the cache's
    /// sequencer. It takes the sink's own locks and the sequencer briefly and returns; a full
    /// queue drops the offer and marks the channel suspect, and the publish goes on regardless.
    ///
    /// A deletion is offered whatever is known of the provider's account; a creation or an edit
    /// of a provider not checked against the file is dropped, and marks the channel suspect.
    pub(crate) fn offer_push(
        &self,
        channel: &ChannelId,
        live: &LiveMessage,
        producer: ServerFill,
        read: Option<ReadStart>,
    ) {
        if let Some(mut fill) = self.begin(&FillGrant::server(producer), channel) {
            // An adapter pushed for this provider just now: what lets its feed vouch for a
            // channel of the provider (`Sink::fed_since_for`). The poller's publishes are its
            // own reads and prove no adapter. Noted under the feed lock like the offer, and as
            // briefly: one map entry, never a wait.
            if producer == ServerFill::Ingest {
                lock(&self.state.pushed).insert(fill.chan.provider_key.clone(), fill.observed_ms);
            }
            if let Some(read) = read {
                fill.observed_ms = fill.observed_ms.min(read.observed_ms);
                if read.chan == fill.chan {
                    fill.stamp = read.stamp;
                }
            }
            fill.offer_push(live);
        }
    }

    /// Begin a shadow evaluation of `ask` of `channel`, before the request's own read goes
    /// upstream: what the cache would have answered, read on the blocking pool while upstream
    /// answers ([`super::shadow`]). Hand the returned [`Shadowed`] upstream's answer; nothing waits
    /// for either.
    ///
    /// `None` when there is nothing to evaluate. With no cache open nothing is read, and the ask is
    /// counted, as `off`, only when the configuration has a `[message_cache]` table; a channel the
    /// cache would not read (the sink not armed, or the channel not on its list) is counted as
    /// `blocked` and not read either; and an ask the cache would never answer is not counted at
    /// all. `live_fed` is asked only when the cache is to be read.
    #[must_use]
    pub fn shadow(
        &self,
        channel: &ChannelId,
        ask: Ask,
        live_fed: impl FnOnce() -> bool,
    ) -> Option<Shadowed> {
        let consumer = ask.consumer();
        let progress = &self.state.shadow;
        let cache = self.cache().filter(|cache| cache.is_open());
        // No cache and no table: nothing is looked at, nothing counted.
        if cache.is_none()
            && !self
                .lifecycle()
                .is_some_and(|lifecycle| lifecycle.configured())
        {
            return None;
        }
        let (route, armed) = {
            let routes = self.state.routes();
            (routes.fills.get(channel).cloned(), routes.armed)
        };
        // An ask the cache would never answer is not one it could miss, whether or not it is open.
        let query = match &route {
            Some(route) => Some(shadow::query(&ask, route.shape, route.recent_via_flat)?),
            None => None,
        };
        let Some(cache) = cache else {
            progress
                .tallies
                .decided(consumer, Decision::Miss(Miss::Off));
            return None;
        };
        let (Some(route), Some(query), true) = (route, query, armed) else {
            progress
                .tallies
                .decided(consumer, Decision::Miss(Miss::Blocked));
            return None;
        };
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let live_fed = live_fed();
        // Both noted now, before the request's own read starts: the time the rule decides at, and
        // the offers the barrier waits for, none of them the request's own.
        let asked_ms = cache.now_ms();
        // A polled channel's feed never delivers a deletion or an edit: it vouches for nothing.
        // Any other channel's vouches only while an adapter is pushing for its provider.
        let fed_since_ms = self.fed_since_for(&route, &cache);
        let chan = ChanKey::new(route.key, channel.as_str());
        let noted = (chan.clone(), query.scope().cloned());
        let target = cache.enqueued_seq();
        let weak = Arc::downgrade(&cache);
        drop(cache);
        progress.begun.fetch_add(1, Ordering::SeqCst);
        let reading = Arc::clone(progress);
        let reads = weak.clone();
        let began = Instant::now();
        let evaluated = runtime.spawn_blocking(move || {
            let barrier = barrier(&reads, target, shadow::BARRIER);
            let evaluated = reads.upgrade().map_or_else(Evaluated::off, |cache| {
                let dropped_ms = cache.last_drop_ms(&chan);
                let read = cache.shadow_view(&chan, &query);
                Evaluated {
                    read,
                    barrier,
                    dropped_ms,
                    applied_after: cache.applied_seq(),
                    read_took: began.elapsed(),
                }
            });
            reading.read.fetch_add(1, Ordering::SeqCst);
            evaluated
        });
        Some(Shadowed {
            task: Some(Box::new(ShadowTask {
                ask,
                chan: noted.0,
                scope: noted.1,
                reason: crate::read_reason::ReadReason::current(),
                served: false,
                arrivals: Arc::downgrade(&self.state.arrivals),
                feed: shadow::Feed::of(route.shape),
                live_fed,
                fed_since_ms,
                asked_ms,
                began,
                answered: None,
                reading: Some(evaluated),
                evaluated: None,
                progress: Arc::clone(progress),
                cache: weak,
            })),
        })
    }

    /// Ask the cache to answer `ask` of `channel` at once (`#384 cache-serve`): the serving
    /// form of [`Sink::shadow`]. Call it where `shadow` is called — before any upstream read —
    /// and only it, never both for one ask: it begins the ask's one evaluation, awaits it for
    /// at most [`SERVE_WAIT`], and decides by the same rule the shadow counts by.
    ///
    /// [`ServeOutcome::Served`] only when serving is on ([`Sink::serve_enabled`]), the ask is
    /// one this milestone may answer ([`shadow::serve_eligible`]), the channel's provider's
    /// account has been checked against the file in this process (rows read as one account are
    /// never handed back as another's, verified again after the snapshot read, in case either
    /// changed under it), the snapshot holds nothing a read begun at or after the ask wrote
    /// (counting would call such an evaluation `late` and compare nothing: a serve escapes no
    /// comparison), and the rule decides [`Decision::Serve`] on the snapshot read. Nothing a
    /// serve hands out holds back the next: a served timeline page carries a backward cursor of
    /// the server's own ([`crate::timeline_served`], `#510 cache-serve-every-read`), so older
    /// history is reachable from it. The serve is counted as it is handed back; the caller starts
    /// the read's refresh upstream and hands its answer to the returned [`PendingServe`], so
    /// the comparison runs as for any shadow. Anything else is [`ServeOutcome::NotServed`]
    /// with the ask's shadow to carry on with, and the request goes upstream exactly as it did
    /// before serving existed.
    pub async fn try_serve(
        &self,
        channel: &ChannelId,
        ask: Ask,
        live_fed: impl FnOnce() -> bool,
    ) -> ServeOutcome {
        if !self.serve_enabled() {
            return ServeOutcome::NotServed(self.shadow(channel, ask, live_fed));
        }
        // The gates that do not need the snapshot, checked again after it is read: what may
        // change under the evaluation may not slip a serve past them.
        let gated = |sink: &Self| {
            let routes = sink.state.routes();
            routes.fills.get(channel).is_some_and(|route| {
                shadow::serve_eligible(&ask, route.shape)
                    && sink.verification(&route.key) == Verification::Verified
            })
        };
        if !gated(self) {
            return ServeOutcome::NotServed(self.shadow(channel, ask, live_fed));
        }
        let Some(mut shadowed) = self.shadow(channel, ask.clone(), live_fed) else {
            return ServeOutcome::NotServed(None);
        };
        let Some(task) = shadowed.task.as_mut() else {
            return ServeOutcome::NotServed(Some(shadowed));
        };
        if !task.evaluate_for(SERVE_WAIT).await {
            return ServeOutcome::NotServed(Some(shadowed));
        }
        let decision = match task.evaluated.as_ref().map(|evaluated| &evaluated.read) {
            Some(Read::Served(view)) if !shadow::late(view, task.asked_ms) => {
                let evaluated = task.evaluated.as_ref().expect("just matched");
                shadow::decide(
                    &ask,
                    view,
                    &shadow::Facts {
                        asked_ms: task.asked_ms,
                        live_fed: task.live_fed,
                        fed_since_ms: task.fed_since_ms,
                        dropped_ms: evaluated.dropped_ms,
                        barrier: evaluated.barrier,
                        feed: task.feed,
                    },
                )
            }
            _ => Decision::Miss(Miss::Blocked),
        };
        if decision != Decision::Serve || !gated(self) {
            return ServeOutcome::NotServed(Some(shadowed));
        }
        let mut task = shadowed.task.take().expect("a task was evaluated");
        let view = match task.evaluated.as_ref().map(|evaluated| &evaluated.read) {
            Some(Read::Served(view)) => view.clone(),
            _ => {
                return ServeOutcome::NotServed(Some(Shadowed { task: Some(task) }));
            }
        };
        task.served = true;
        task.answered_by(shadow::Answered::Cache);
        self.state.shadow.tallies.served(ask.consumer());
        ServeOutcome::Served(Box::new(ServedAnswer {
            view,
            pending: PendingServe { task: Some(task) },
        }))
    }

    /// `channel` as the cache names it, under the provider the latest routes give it: for a
    /// caller reading the cache back beside the sink (a served answer's corrections).
    #[must_use]
    pub fn chan_key(&self, channel: &ChannelId) -> Option<ChanKey> {
        let routes = self.state.routes();
        routes
            .fills
            .get(channel)
            .map(|route| ChanKey::new(route.key.clone(), channel.as_str()))
    }

    /// What the shadow has counted since this process started, per consumer, in
    /// [`shadow::Consumer::ALL`]'s order.
    #[must_use]
    pub fn shadow_counts(&self) -> Vec<shadow::ConsumerCounts> {
        self.state.shadow.tallies.report()
    }

    /// Wait, up to `timeout`, until every shadow evaluation begun so far has read the cache. For
    /// tests that hold a read upstream until the evaluation beside it is done.
    pub async fn shadow_read(&self, timeout: Duration) -> bool {
        self.shadow_wait(timeout, |progress| {
            progress.read.load(Ordering::SeqCst) >= progress.begun.load(Ordering::SeqCst)
        })
        .await
    }

    /// Wait, up to `timeout`, until every shadow evaluation begun so far has been counted: handed
    /// upstream's answer or dropped, and decided. For tests.
    pub async fn shadow_settled(&self, timeout: Duration) -> bool {
        self.shadow_wait(timeout, |progress| {
            progress.counted.load(Ordering::SeqCst) >= progress.begun.load(Ordering::SeqCst)
        })
        .await
    }

    async fn shadow_wait(&self, timeout: Duration, done: impl Fn(&ShadowProgress) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if done(&self.state.shadow) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The cache as the diagnostics report shows it, or `None` on a server whose configuration has
    /// no `[message_cache]` table and behind whose sink no cache was put: one that never had a
    /// cache says nothing of one. What the file holds is measured on the blocking pool.
    pub async fn report(&self) -> Option<CacheReport> {
        let lifecycle = self.lifecycle();
        let behind = self.cache();
        if !lifecycle.is_some_and(|lifecycle| lifecycle.configured()) && behind.is_none() {
            return None;
        }
        let open = behind.filter(|cache| cache.is_open());
        let available = open.is_some();
        let measured = match open {
            Some(cache) => tokio::task::spawn_blocking(move || {
                (
                    cache.bytes_on_disk(),
                    cache.counts().served().unwrap_or_default(),
                    cache.covered_ms().served().unwrap_or(0),
                    cache.stats(),
                    cache.blocks(),
                )
            })
            .await
            .ok(),
            None => None,
        };
        let (bytes, counts, covered_ms, stats, blocked) = match measured {
            Some((bytes, counts, covered_ms, stats, blocked)) => {
                (bytes, counts, covered_ms, Some(stats), blocked)
            }
            None => (0, Counts::default(), 0, None, 0),
        };
        let router = self.state.routes().router.clone();
        let providers = router.map_or_else(Vec::new, |router| {
            router
                .entries()
                .iter()
                .map(|entry| ProviderStanding {
                    provider: entry.key.clone(),
                    verification: self.verification(&entry.key).as_str(),
                })
                .collect()
        });
        // To the thousandth of a day, about a minute and a half: a figure to read, not to compute
        // with.
        let covered_days = (covered_ms as f64 / 86_400.0).round() / 1_000.0;
        Some(CacheReport {
            available,
            enabled: lifecycle.map_or(available, |lifecycle| lifecycle.enabled()),
            reason: lifecycle.and_then(|lifecycle| lifecycle.reason(available)),
            bytes,
            messages: counts.messages,
            channels: counts.channels,
            tombstones: counts.tombstones,
            intervals: counts.intervals,
            covered_days,
            stats,
            providers,
            blocked: u64::try_from(blocked).unwrap_or(u64::MAX),
            shadow: self.shadow_counts(),
        })
    }

    /// Write one `info` line with [`Sink::report`]'s figures: totals since the process started, and
    /// the file as it stands. Numbers only. Nothing at all when there is no report.
    pub async fn log_hour(&self) {
        let Some(report) = self.report().await else {
            return;
        };
        let stats = report.stats.clone().unwrap_or_default();
        let shadow = report
            .shadow
            .iter()
            .fold(shadow::ConsumerCounts::default(), |sum, counts| {
                sum.plus(counts)
            });
        let standing = |word: &str| {
            report
                .providers
                .iter()
                .filter(|provider| provider.verification == word)
                .count()
        };
        tracing::info!(
            available = report.available,
            enabled = report.enabled,
            bytes = report.bytes,
            messages = report.messages,
            channels = report.channels,
            covered_days = report.covered_days,
            ops_applied = stats.ops_applied,
            ops_dropped = stats.ops_dropped,
            bytes_dropped = stats.bytes_dropped,
            ops_stale = stats.ops_stale,
            ops_unrouted = stats.ops_unrouted,
            ops_failed = stats.ops_failed,
            batches = stats.batches,
            batch_p50_us = stats.batch_us.quantile_us(0.5),
            batch_p99_us = stats.batch_us.quantile_us(0.99),
            orphans_dropped = stats.orphans_dropped,
            providers_verified = standing("verified"),
            providers_suspended = standing("suspended"),
            blocked = report.blocked,
            shadow_asked = shadow.asked,
            shadow_would_serve = shadow.would_serve,
            shadow_served = shadow.served,
            shadow_needs_existence_check = shadow.needs_existence_check,
            shadow_late = shadow.late,
            shadow_compared = shadow.compared,
            shadow_agreed = shadow.agreed,
            shadow_missing_rows = shadow.missing_rows,
            shadow_stale_rows = shadow.stale_rows,
            shadow_newer_rows = shadow.newer_rows,
            shadow_extra_rows = shadow.extra_rows,
            shadow_content_differs = shadow.content_differs,
            shadow_miss_off = shadow.misses.off,
            shadow_miss_blocked = shadow.misses.blocked,
            shadow_miss_suspect = shadow.misses.suspect,
            shadow_miss_gap = shadow.misses.gap,
            shadow_miss_incomplete = shadow.misses.incomplete,
            shadow_miss_stale_head = shadow.misses.stale_head,
            shadow_miss_barrier = shadow.misses.barrier,
            read_timings = read_timings(&report.shadow).as_str(),
            gap_causes = gap_causes(&report.shadow).as_str(),
            "message cache figures since this process started"
        );
    }

    /// Whether `provider_key`'s account has been checked against the cache behind the sink.
    #[must_use]
    pub fn verification(&self, provider_key: &str) -> Verification {
        let suspended = matches!(
            self.state.identities().providers.get(provider_key),
            Some(Standing::Suspended { .. })
        );
        match self.cache() {
            _ if suspended => Verification::Suspended,
            Some(cache) if self.state.standing(provider_key, &cache) => Verification::Verified,
            _ => Verification::Unverified,
        }
    }

    /// Check every provider's account against the file again, from the start, filling nothing of
    /// any until it has: for whoever has just erased the whole file. A fill notices such an erase
    /// on its own; this does not wait for one to.
    pub fn reverify(&self) {
        State::verify_all(&self.state);
    }

    /// Wait, up to `timeout`, until every provider's account has been checked against the cache
    /// behind the sink. `false` on timeout, or with no cache or no providers known.
    pub async fn identities_verified(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.state.all_verified() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Wait, up to `timeout`, for a server that has just started to have checked every provider's
    /// account against its cache file, before a background reader's first read: a fill begun
    /// before then writes no rows and leaves its channel in doubt until later reads see each row
    /// again. At once when no cache is open or the sink is not armed, since then no check is
    /// coming. `#502 cache-serve-coverage`.
    pub async fn first_checks(&self, timeout: Duration) {
        if self.cache().is_none_or(|cache| !cache.is_open()) || !self.is_armed() {
            return;
        }
        let _ = self.identities_verified(timeout).await;
    }

    /// Wait, up to `timeout`, until the summaries task has followed every event the cache behind
    /// the sink has sent so far. `true` at once with no cache, or no task. For tests, after the
    /// cache has been waited for ([`MessageCache::quiesce_blocking`]): a summary deleted, or
    /// rightly kept, is only known to be so once the task has had its turn.
    pub async fn orphans_settled(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let Some(cache) = self.cache() else {
                return true;
            };
            if self.state.summaries.get().is_none() {
                return true;
            }
            let (handled, sent_before) = {
                let progress = self
                    .state
                    .orphans
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                (
                    progress.handled.load(Ordering::SeqCst),
                    progress.sent_before,
                )
            };
            if sent_before.saturating_add(handled) >= cache.stats().orphans_sent {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl State {
    fn routes(&self) -> std::sync::MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn identities(&self) -> std::sync::MutexGuard<'_, Identities> {
        self.identities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn cache(&self) -> Option<Arc<MessageCache>> {
        self.slot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether `cache` is still the one behind the sink.
    fn holds(&self, cache: &Arc<MessageCache>) -> bool {
        self.cache().is_some_and(|held| Arc::ptr_eq(&held, cache))
    }

    /// Whether `provider_key` was verified against `cache` as the file stands now.
    fn standing(&self, provider_key: &str, cache: &MessageCache) -> bool {
        matches!(
            self.identities().providers.get(provider_key),
            Some(Standing::Verified { generation }) if *generation == cache.generation()
        )
    }

    /// Every provider the routes name is verified against the cache behind the sink.
    fn all_verified(&self) -> bool {
        let Some(router) = self.routes().router.clone() else {
            return false;
        };
        let Some(cache) = self.cache() else {
            return false;
        };
        router
            .entries()
            .iter()
            .all(|entry| self.standing(&entry.key, &cache))
    }

    /// Whether a fill of `provider_key`'s channels may begin on `cache`. A provider verified
    /// against a file that has since been erased is not, and starts verification over.
    fn may_fill(state: &Arc<Self>, provider_key: &str, cache: &MessageCache) -> bool {
        let erased_since = match state.identities().providers.get(provider_key) {
            Some(Standing::Verified { generation }) if *generation == cache.generation() => {
                return true;
            }
            Some(Standing::Verified { .. }) => true,
            _ => false,
        };
        if erased_since {
            Self::verify_all(state);
        }
        false
    }

    /// Start verification over: every provider unverified (a suspended one stays suspended), and,
    /// with a cache behind an armed sink and a runtime to run on, one task per provider asking its
    /// own client which account it is. Tasks of an earlier round see the round has moved and stop.
    fn verify_all(state: &Arc<Self>) {
        let cache = state.cache();
        let router = {
            let routes = state.routes();
            routes.armed.then(|| routes.router.clone()).flatten()
        };
        let round = {
            let mut identities = state.identities();
            identities.round += 1;
            identities
                .providers
                .retain(|_, standing| matches!(standing, Standing::Suspended { .. }));
            identities.running.clear();
            identities.round
        };
        let (Some(cache), Some(router)) = (cache, router) else {
            return;
        };
        for entry in router.entries() {
            Self::start_verifying(state, &cache, &router, &entry.key, round);
        }
    }

    /// Start `provider_key`'s verification task for `round`, unless one is running, and when there
    /// is a runtime to run it on.
    fn start_verifying(
        state: &Arc<Self>,
        cache: &Arc<MessageCache>,
        router: &ChatRouter,
        provider_key: &str,
        round: u64,
    ) {
        let (Some(entry), Ok(runtime)) = (
            router.entry(provider_key),
            tokio::runtime::Handle::try_current(),
        ) else {
            return;
        };
        {
            let mut identities = state.identities();
            if identities.round != round || !identities.running.insert(provider_key.to_owned()) {
                return;
            }
        }
        runtime.spawn(verify(
            Arc::clone(state),
            Arc::downgrade(cache),
            Arc::clone(&entry.client),
            provider_key.to_owned(),
            round,
        ));
    }

    /// The cache a verification of `round` was started against, while that still matters: the
    /// round has not moved and the cache is still the one behind the sink.
    fn in_round(&self, round: u64, cache: &Weak<MessageCache>) -> Option<Arc<MessageCache>> {
        let cache = cache.upgrade()?;
        (self.identities().round == round && self.holds(&cache)).then_some(cache)
    }

    /// Whether `provider_key` is suspended.
    fn suspended(&self, provider_key: &str) -> bool {
        matches!(
            self.identities().providers.get(provider_key),
            Some(Standing::Suspended { .. })
        )
    }

    /// `provider_key` matched the file at `generation`, in `round`: fills may begin, and a
    /// suspension ends.
    fn verified(&self, round: u64, cache: &Arc<MessageCache>, provider_key: &str, generation: u64) {
        if !self.holds(cache) {
            return;
        }
        let mut identities = self.identities();
        if identities.round == round {
            identities
                .providers
                .insert(provider_key.to_owned(), Standing::Verified { generation });
            cache.resume_provider(provider_key);
        }
    }

    /// `provider_key`'s verification task of `round` has ended.
    fn stopped_verifying(&self, round: u64, provider_key: &str) {
        let mut identities = self.identities();
        if identities.round == round {
            identities.running.remove(provider_key);
        }
    }

    /// Listen to the router's access signals while a cache is behind an armed sink, and stop
    /// otherwise. On starting, what is in doubt or suspended is blocked in the cache afresh: a
    /// cache opened since holds no blocks.
    fn listen(state: &Arc<Self>) {
        let (router, armed) = {
            let routes = state.routes();
            (routes.router.clone(), routes.armed)
        };
        let Some(router) = router else {
            return;
        };
        let Some(cache) = state.cache().filter(|_| armed) else {
            router.access().listen(None);
            return;
        };
        let suspended: Vec<String> = state
            .identities()
            .providers
            .iter()
            .filter(|(_, standing)| matches!(standing, Standing::Suspended { .. }))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &suspended {
            cache.suspend_provider(key);
        }
        // What is in doubt now is blocked afresh as the listener is installed.
        let listener: Weak<dyn AccessListener> = Arc::downgrade(state) as Weak<dyn AccessListener>;
        router.access().listen(Some(listener));
    }

    /// The cache's name for a suspect's channel, and the scope in doubt when it is one thread.
    fn suspect_scope(suspect: &Suspect) -> (ChanKey, Option<Scope>) {
        (
            ChanKey::new(suspect.provider_key.clone(), suspect.channel.as_str()),
            suspect.thread.clone().map(Scope::Thread),
        )
    }
}

#[async_trait]
impl AccessListener for State {
    fn block(&self, suspect: &Suspect) {
        let Some(cache) = self.cache() else {
            return;
        };
        match Self::suspect_scope(suspect) {
            (chan, None) => cache.block_channel(&chan),
            (chan, Some(scope)) => cache.block_scope(&chan, &scope),
        }
    }

    fn unblock(&self, suspect: &Suspect) {
        if let Some(cache) = self.cache() {
            let (chan, scope) = Self::suspect_scope(suspect);
            cache.unblock(&chan, scope.as_ref());
        }
    }

    /// The server's own read: the channel's sample, or the newest message of the thread.
    async fn probe(&self, suspect: &Suspect) -> Option<Result<(), ChatError>> {
        self.cache()?;
        let router = self.routes().router.clone()?;
        let client = Arc::clone(&router.entry(&suspect.provider_key)?.client);
        Some(match &suspect.thread {
            None => client.probe_channel(&suspect.channel, 1).await.map(drop),
            Some(thread) => client
                .fetch_timeline(
                    &suspect.channel,
                    &TimelineRequest {
                        view: TimelineView::Thread,
                        thread_id: Some(thread.clone()),
                        limit: 1,
                        ..TimelineRequest::default()
                    },
                )
                .await
                .map(drop),
        })
    }

    /// Erase the channel and forget the store's summaries of it, which the task that follows the
    /// cache's erases would not (the channel is still listed); or erase the thread, whose messages'
    /// summaries that task does follow, one by one.
    fn confirmed(&self, suspect: &Suspect) {
        let Some(cache) = self.cache() else {
            return;
        };
        let (chan, scope) = Self::suspect_scope(suspect);
        tracing::warn!(
            provider = suspect.provider_key.as_str(),
            scope = if scope.is_some() { "thread" } else { "channel" },
            "a provider confirmed this server can no longer read a conversation; its saved copy \
             is erased"
        );
        match scope {
            None => {
                cache.erase_channels(std::slice::from_ref(&chan));
                cache.unblock(&chan, None);
                if let (Some(summaries), Ok(runtime)) =
                    (self.summaries.get(), tokio::runtime::Handle::try_current())
                {
                    let summaries = summaries.clone();
                    let channel = suspect.channel.clone();
                    runtime.spawn(async move { summaries.forget_channel(&channel).await });
                }
            }
            Some(scope) => {
                cache.erase_scope(&chan, &scope);
                cache.unblock(&chan, Some(&scope));
            }
        }
    }

    fn credential_refused(&self, provider_key: &str, revoked: bool) {
        let (Some(state), Some(cache)) = (self.me.upgrade(), self.cache()) else {
            return;
        };
        let now = tokio::time::Instant::now();
        let (erase, round) = {
            let mut identities = self.identities();
            let (since, erased) = match identities.providers.get(provider_key) {
                Some(Standing::Suspended { since, erased }) => (*since, *erased),
                _ => (now, false),
            };
            let erase = !erased && (revoked || now.duration_since(since) >= CREDENTIAL_GRACE);
            identities.providers.insert(
                provider_key.to_owned(),
                Standing::Suspended {
                    since,
                    erased: erased || erase,
                },
            );
            (erase, identities.round)
        };
        cache.suspend_provider(provider_key);
        if erase {
            tracing::warn!(
                provider = provider_key,
                revoked,
                "a provider refuses this server's credential; everything saved from it is erased"
            );
            cache.erase_provider(provider_key);
        }
        let router = self.routes().router.clone();
        if let Some(router) = router {
            Self::start_verifying(&state, &cache, &router, provider_key, round);
        }
    }
}

/// Write [`Sink::log_hour`] every [`crate::upstream_profile::LOG_INTERVAL`], first one interval
/// from now. On a server whose configuration has no `[message_cache]` table it writes nothing.
pub async fn log_every_hour(sink: Arc<Sink>) {
    let every = crate::upstream_profile::LOG_INTERVAL;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        sink.log_hour().await;
    }
}

/// What one check of a provider's account against the file found.
enum Checked {
    /// The account matches the file, which was at this generation throughout.
    Matched(u64),
    /// Nothing to check against any more: no file is open, or the cache behind the sink, or the
    /// round, has moved on.
    Off,
    /// No answer this time: asked again later.
    Unanswered,
}

/// What the file says a provider's account is.
enum Stored {
    /// What it says, once everything asked of the writer before the read was applied.
    Read(Option<Account>),
    /// No cache file is open.
    Off,
    /// It could not say just now.
    Unread,
}

/// Ask `client` which account it is until the file agrees, in `round`: the task
/// [`State::verify_all`] starts per provider. It holds the cache only while it uses it, so a cache
/// taken out from behind the sink closes when its other holders let it go.
async fn verify(
    state: Arc<State>,
    cache: Weak<MessageCache>,
    client: Arc<dyn ChatClient>,
    provider_key: String,
    round: u64,
) {
    let mut wait = VERIFY_RETRY_FIRST;
    loop {
        match check(&state, round, &cache, client.as_ref(), &provider_key).await {
            Checked::Matched(generation) => {
                if let Some(cache) = state.in_round(round, &cache) {
                    state.verified(round, &cache, &provider_key, generation);
                }
                break;
            }
            Checked::Off => break,
            Checked::Unanswered => {}
        }
        // A suspended provider is asked on its own schedule, which a refusal does not shorten.
        if state.suspended(&provider_key) {
            tokio::time::sleep(SUSPENDED_RECHECK).await;
        } else {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(VERIFY_RETRY_CAP);
        }
    }
    state.stopped_verifying(round, &provider_key);
}

/// Check `provider_key`'s account against the file once: see the module documentation.
async fn check(
    state: &State,
    round: u64,
    cache: &Weak<MessageCache>,
    client: &dyn ChatClient,
    provider_key: &str,
) -> Checked {
    let generation = match state.in_round(round, cache) {
        Some(cache) if cache.is_open() => cache.generation(),
        _ => return Checked::Off,
    };
    let identity = client.identity().await;
    let Some(cache) = state.in_round(round, cache) else {
        return Checked::Off;
    };
    let cache = &cache;
    let identity = match identity {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(
                provider = provider_key,
                outcome = crate::upstream_profile::Outcome::of(&error).as_str(),
                "the message cache could not ask a provider which account it is; nothing of it is \
                 filled until it can"
            );
            return Checked::Unanswered;
        }
    };
    let workspace = identity.workspace.as_deref();
    let wrote = match stored_account(cache, provider_key).await {
        Stored::Read(Some(account)) if account.is(&identity.id, workspace) => false,
        Stored::Read(Some(_)) => {
            tracing::warn!(
                provider = provider_key,
                "the message cache holds messages read as another account of this provider; \
                 erasing them"
            );
            cache.erase_provider(provider_key);
            cache.set_account(provider_key, &identity.id, workspace);
            true
        }
        Stored::Read(None) => {
            cache.set_account(provider_key, &identity.id, workspace);
            true
        }
        Stored::Off => return Checked::Off,
        Stored::Unread => return Checked::Unanswered,
    };
    if wrote {
        match stored_account(cache, provider_key).await {
            Stored::Read(Some(account)) if account.is(&identity.id, workspace) => {}
            Stored::Off => return Checked::Off,
            Stored::Read(_) | Stored::Unread => return Checked::Unanswered,
        }
    }
    if cache.generation() == generation {
        Checked::Matched(generation)
    } else {
        Checked::Unanswered
    }
}

/// The account the file records for `provider_key`, read once the writer has applied everything
/// asked of it so far. Off the runtime's workers: it blocks on SQLite and on the writer.
async fn stored_account(cache: &Arc<MessageCache>, provider_key: &str) -> Stored {
    let cache = Arc::clone(cache);
    let provider_key = provider_key.to_owned();
    let read = tokio::task::spawn_blocking(move || {
        if !cache.is_open() {
            return Read::Off;
        }
        if !cache.barrier_blocking(ACCOUNT_WAIT) {
            return Read::Blocked;
        }
        cache.account(&provider_key)
    })
    .await;
    match read {
        Ok(Read::Served(account)) => Stored::Read(account),
        Ok(Read::Off) => Stored::Off,
        Ok(_) | Err(_) => Stored::Unread,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::grant::ServerFill;
    use super::super::{CacheOptions, Read, Scope};
    use super::*;
    use crate::auth::Scope as Caller;

    #[test]
    fn a_sink_that_is_off_begins_no_fill_under_any_grant() {
        let sink = Sink::off();
        let channel = ChannelId("1000000000000000001".to_owned());
        let caller = FillGrant::for_caller(Caller::Write).expect("write scope may fill");
        assert!(sink.begin(&caller, &channel).is_none());
        for kind in ServerFill::ALL {
            assert!(
                sink.begin(&FillGrant::server(kind), &channel).is_none(),
                "{kind:?}"
            );
        }
    }

    fn open(dir: &crate::testing::TempDir) -> Arc<MessageCache> {
        Arc::new(
            MessageCache::open(CacheOptions::new(
                dir.path().join("private").join("cache.sqlite3"),
            ))
            .expect("a cache opens"),
        )
    }

    const WAIT: Duration = Duration::from_secs(20);

    #[tokio::test]
    async fn nothing_reaches_the_cache_until_the_sink_is_armed() {
        let dir = crate::testing::TempDir::new("sink-armed");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        let cache = open(&dir);
        sink.install(Arc::clone(&cache));
        sink.set_routes(&state.all_channels(), &state.providers, &state.config);
        let read = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let chan = ChanKey::new(crate::config::LEGACY_DISCORD_PROVIDER_KEY, read.as_str());

        // Unarmed: no snapshot reached the cache, so it serves nothing and the sink fills nothing
        // but deletions.
        assert!(!sink.is_armed());
        assert!(!sink
            .begin_for(Caller::Write, &read)
            .expect("a fill of deletions alone")
            .writes_rows());
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        assert_eq!(cache.applied_seq(), 0, "no sweep was queued");
        assert_eq!(cache.newest(&chan, &Scope::Main, 5), Read::Blocked);

        sink.arm();
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        assert_eq!(
            cache.newest(&chan, &Scope::Main, 5),
            Read::Served(Vec::new())
        );
        assert!(sink.identities_verified(WAIT).await);
        let fill = sink
            .begin_for(Caller::Write, &read)
            .expect("an armed sink fills");
        assert!(fill.writes_rows());
        assert_eq!(fill.chan(), &chan);
        assert_eq!(fill.grant().label(), "caller");
        assert!(sink.begin_for(Caller::Read, &read).is_none(), "read scope");
        assert!(
            sink.begin_for(Caller::Write, &ChannelId("999".to_owned()))
                .is_none(),
            "a channel the server does not answer for"
        );
    }

    /// 2026-08-18T12:00:00Z.
    const T: i64 = 1_787_054_400_000;

    fn big_message(channel: &ChannelId, n: i64, body: &str) -> Message {
        let at = T + n * 1_000;
        Message {
            edited_timestamp: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
            thread: None,
            id: crate::model::MessageId::at_time_ms(at),
            channel_id: channel.clone(),
            author: "Ada".to_owned(),
            author_id: crate::model::UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: crate::clock::iso_from_ms(at),
            spoken_time: String::new(),
            reply_to: None,
            content: body.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
        }
    }

    #[tokio::test]
    async fn a_publish_with_a_full_queue_never_waits_it_drops_the_offer_and_marks_the_channel_suspect(
    ) {
        let dir = crate::testing::TempDir::new("sink-full-queue");
        let (state, _fake) = crate::testing::state();
        let clock = crate::testing::ManualClock::at(T + 60 * 60 * 1_000);
        let state = crate::testing::with_message_cache(state, dir.path(), clock).await;
        let cache = state.message_cache.cache().expect("installed");
        assert!(cache.quiesce_blocking(Duration::from_secs(10)));
        let channel = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let chan = ChanKey::new(crate::config::LEGACY_DISCORD_PROVIDER_KEY, channel.as_str());

        // The writer stands still, so nothing leaves the queue, and every publish offers a few
        // megabytes: the queue's byte budget runs out after a handful.
        let resume = cache.pause_writer();
        let body = "x".repeat(3 * 1024 * 1024);
        let mut published = 0;
        while cache.last_drop_ms(&chan).is_none() {
            assert!(published < 40, "the hub never offered the cache anything");
            let started = std::time::Instant::now();
            state
                .live
                .publish(&channel, big_message(&channel, published, &body));
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "a publish waited on the cache"
            );
            published += 1;
        }
        assert!(published > 1, "some offers fitted before one was dropped");
        let stats = cache.stats();
        assert_eq!(
            stats.ops_dropped, 1,
            "the offer that did not fit was dropped"
        );
        // The publish itself went on: every message is in the channel's feed.
        assert_eq!(
            state.live.subscribe(&channel, None).replay.len(),
            usize::try_from(published).expect("small")
        );

        drop(resume);
        assert!(cache.quiesce_blocking(Duration::from_secs(30)));
        assert_eq!(
            cache.counts().served().expect("counts").messages,
            u64::try_from(published - 1).expect("small"),
            "everything queued was applied, and the dropped offer was not"
        );
    }

    /// What one kind of read offers: `message` as the only thing it saw.
    type OfferOne = fn(Fill, &Message);

    #[tokio::test]
    async fn every_offer_is_observed_when_its_fill_began_not_when_it_is_offered() {
        let dir = crate::testing::TempDir::new("sink-observed");
        let (state, _fake) = crate::testing::state();
        let clock = crate::testing::ManualClock::at(T + 60 * 60 * 1_000);
        let state = crate::testing::with_message_cache(state, dir.path(), clock.clone()).await;
        let cache = state.message_cache.cache().expect("installed");
        assert!(cache.quiesce_blocking(Duration::from_secs(10)));
        let channel = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let chan = ChanKey::new(crate::config::LEGACY_DISCORD_PROVIDER_KEY, channel.as_str());
        let live = |kind, message: &Message| LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind,
            historical: false,
            message: message.clone(),
            self_posted: false,
            client_key: None,
        };
        // Each kind of read, offering one message it saw. A page with more behind it, so no read
        // here claims the channel's start and none takes away what an earlier one wrote.
        let offers: Vec<(&str, OfferOne)> = vec![
            ("timeline", |fill, message| {
                let page = TimelinePage {
                    messages: vec![message.clone()],
                    has_more: true,
                    ..TimelinePage::default()
                };
                fill.offer_timeline(&TimelineRequest::default(), TimelineRead::Newest, &page);
            }),
            ("recent", |fill, message| {
                fill.offer_recent_dated(&RecentRead {
                    messages: vec![message.clone()],
                    ..RecentRead::default()
                });
            }),
            ("page", |fill, message| {
                fill.offer_page(PageRead::Newest, std::slice::from_ref(message), false);
            }),
            ("walk", |fill, message| {
                fill.offer_walk(std::slice::from_ref(message));
            }),
            ("single", |fill, message| fill.offer_single(message, "1")),
            ("forward", |fill, message| {
                fill.offer_forward(std::slice::from_ref(message), &[], T, false);
            }),
            ("posted", |fill, message| fill.offer_posted(message, None)),
        ];
        let mut n = 0;
        for (kind, offer) in offers {
            n += 1;
            let message = big_message(&channel, n, kind);
            let fill = state
                .message_cache
                .begin_for(crate::auth::Scope::Write, &channel)
                .expect("a fill begins");
            let began = clock.now();
            // The read takes its time.
            clock.advance(60_000);
            offer(fill, &message);
            assert!(cache.quiesce_blocking(Duration::from_secs(10)));
            let held = cache
                .message(&chan, &message.id)
                .served()
                .flatten()
                .unwrap_or_else(|| panic!("{kind}: offered"));
            assert_eq!(held.observed_ms, began, "{kind}: observed when it began");
        }
        // A change from the live feed, offered through a fill of its own.
        n += 1;
        let message = big_message(&channel, n, "pushed");
        let fill = state
            .message_cache
            .begin(&FillGrant::server(ServerFill::Ingest), &channel)
            .expect("a fill begins");
        let began = clock.now();
        clock.advance(60_000);
        fill.offer_push(&live(crate::live::LiveKind::Create, &message));
        assert!(cache.quiesce_blocking(Duration::from_secs(10)));
        assert_eq!(
            cache
                .message(&chan, &message.id)
                .served()
                .flatten()
                .expect("offered")
                .observed_ms,
            began
        );
    }

    #[test]
    fn a_cache_installed_after_arming_gets_the_latest_snapshot_at_once() {
        let dir = crate::testing::TempDir::new("sink-install");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        sink.set_routes(&state.all_channels(), &state.providers, &state.config);
        sink.arm();
        let cache = open(&dir);
        sink.install(Arc::clone(&cache));
        assert!(cache.barrier_blocking(Duration::from_secs(10)));
        let chan = ChanKey::new(
            crate::config::LEGACY_DISCORD_PROVIDER_KEY,
            crate::testing::WRITE_CHANNEL,
        );
        assert_eq!(
            cache.newest(&chan, &Scope::Main, 5),
            Read::Served(Vec::new())
        );
        assert!(sink.uninstall().is_some());
        assert!(sink.cache().is_none());
        assert!(sink
            .begin_for(
                Caller::Write,
                &ChannelId(crate::testing::WRITE_CHANNEL.to_owned())
            )
            .is_none());
    }

    #[tokio::test]
    async fn no_fill_begins_before_the_account_is_checked_and_a_file_erase_checks_it_again() {
        let dir = crate::testing::TempDir::new("sink-identity");
        let (state, _fake) = crate::testing::state();
        let sink = Sink::off();
        let cache = open(&dir);
        let read = ChannelId(crate::testing::READ_CHANNEL.to_owned());
        let key = crate::config::LEGACY_DISCORD_PROVIDER_KEY;

        // Armed and routed, but nothing behind the sink yet: nothing to check against.
        sink.set_routes(&state.all_channels(), &state.providers, &state.config);
        sink.arm();
        assert_eq!(sink.verification(key), Verification::Unverified);
        assert!(!sink.identities_verified(Duration::from_millis(50)).await);

        sink.install(Arc::clone(&cache));
        assert!(sink.identities_verified(WAIT).await);
        assert_eq!(sink.verification(key), Verification::Verified);
        let account = cache.account(key).served().flatten().expect("written");
        assert!(
            account.is(crate::discord::fake::FAKE_BOT_USER_ID, None),
            "the first start records the account the provider's own client named"
        );
        let writes = |sink: &Sink| {
            sink.begin_for(Caller::Write, &read)
                .expect("a fill begins")
                .writes_rows()
        };
        assert!(writes(&sink));

        // The whole file erased: its account went with it, so no fill writes a row until the
        // provider has been asked again, and the account recorded again.
        cache.erase_file(true).expect("reopened");
        assert_eq!(sink.verification(key), Verification::Unverified);
        assert!(!writes(&sink));
        assert!(sink.identities_verified(WAIT).await);
        assert!(cache
            .account(key)
            .served()
            .flatten()
            .expect("written again")
            .is(crate::discord::fake::FAKE_BOT_USER_ID, None));
        assert!(writes(&sink));
    }

    /// `#497 cache-shadow-missing-row`: the diagnostics write at most [`NOTES_PER_HOUR`] lines of
    /// each kind an hour, each kind on its own allowance, and the first line written after some
    /// were held back says how many.
    #[test]
    fn the_diagnostics_write_a_bounded_number_of_lines_an_hour_and_count_the_rest() {
        let mut notes = Notes::default();
        let start = Instant::now();
        for _ in 0..NOTES_PER_HOUR {
            assert_eq!(notes.take(NoteKind::Differs, start), Some(0));
        }
        assert_eq!(notes.take(NoteKind::Differs, start), None);
        assert_eq!(notes.take(NoteKind::Differs, start), None);
        assert_eq!(
            notes.take(NoteKind::Past, start),
            Some(0),
            "each kind has its own allowance"
        );
        let hour = start + Duration::from_secs(60 * 60);
        assert_eq!(
            notes.take(NoteKind::Differs, hour),
            Some(2),
            "an hour on, the next line says two were held back"
        );
        assert_eq!(notes.take(NoteKind::Differs, hour), Some(0));
    }

    /// A time is written as an offset from the ask, and a row's place in the read scope's
    /// coverage by where its instant falls: above the head, in the band, in a claim, or nowhere.
    #[test]
    fn a_diagnostic_line_places_a_message_against_the_head_and_the_claims() {
        use super::super::coverage::{Head, Interval, Source};
        assert_eq!(since_ask(Some(1_500), 1_000), "+500ms");
        assert_eq!(since_ask(Some(400), 1_000), "-600ms");
        assert_eq!(since_ask(None, 1_000), "none");
        let view = View {
            head: Some(Head {
                head_ms: 10_000,
                unsettled_from_ms: 8_000,
                head_listed_ms: 20_000,
            }),
            coverage: vec![Interval {
                t0_ms: 1_000,
                t1_ms: 8_000,
                listed_ms: 19_000,
                full_ms: Some(19_000),
                complete: false,
                source: Source::Read,
            }],
            ..View::default()
        };
        assert_eq!(claim_at(&view, 10_001, 21_000), "above-head");
        assert_eq!(claim_at(&view, 9_000, 21_000), "band");
        assert_eq!(claim_at(&view, 5_000, 21_000), "read@-2000ms");
        assert_eq!(claim_at(&view, 500, 21_000), "uncovered");
    }

    /// `#384 cache-serve`: a serve that stops waiting for its evaluation does not lose the
    /// count. The evaluation's handle survives the timeout, and counting still decides it
    /// off the path, asked and missed included.
    #[tokio::test]
    async fn an_evaluation_a_serve_stopped_waiting_for_is_still_counted() {
        let progress = Arc::new(ShadowProgress::default());
        let reading = tokio::task::spawn_blocking(|| {
            std::thread::sleep(Duration::from_millis(200));
            Evaluated::off()
        });
        let mut task = ShadowTask {
            ask: Ask::Window {
                consumer: shadow::Consumer::Messages,
                limit: 1,
            },
            chan: ChanKey::new("legacy", "c1"),
            scope: Some(Scope::Main),
            reason: crate::read_reason::ReadReason::Untagged,
            served: false,
            arrivals: Weak::new(),
            feed: shadow::Feed::AdditionsOnly,
            live_fed: false,
            fed_since_ms: None,
            asked_ms: 0,
            began: Instant::now(),
            answered: None,
            reading: Some(reading),
            evaluated: None,
            progress: Arc::clone(&progress),
            cache: Weak::new(),
        };
        assert!(
            !task.evaluate_for(Duration::from_millis(10)).await,
            "still reading when the wait ran out"
        );
        task.evaluate().await;
        assert!(task.evaluated.is_some(), "and it lands when it lands");
        task.settle(None);
        let deadline = tokio::time::Instant::now() + WAIT;
        while progress.counted.load(Ordering::SeqCst) == 0 {
            assert!(tokio::time::Instant::now() < deadline, "counted in time");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let messages = progress
            .tallies
            .report()
            .into_iter()
            .find(|counts| counts.consumer == "messages")
            .expect("the consumer is counted");
        assert_eq!(
            (messages.asked, messages.misses.off),
            (1, 1),
            "decided and missed, not dropped from the counts: {messages:?}"
        );
    }
}
