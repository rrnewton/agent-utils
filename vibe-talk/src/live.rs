//! Inbound chat ingestion, and the fan-out that lets a page be TOLD rather than have to ask.
//!
//! Everything else in this crate is pulled: a question arrives, a channel is read, an answer goes
//! back. This module is the one place that runs without being asked, so the two decisions behind
//! it are written down here rather than left to be inferred from the code.
//!
//! There are two mutually exclusive MESSAGE producers. The built-in provider path is bounded
//! polling; an external provider adapter can instead push normalized create, update, and delete
//! events through the authenticated `/api/v1/live/events` endpoint. Both publish into the same
//! [`LiveHub`], and the browser always consumes the same SSE stream. A polled provider may also
//! receive content-free `/api/v1/live/hints`: a hint merely wakes its existing cursor, while the
//! periodic interval remains the recovery path, so it is not a second producer.
//!
//! A Slack provider's own event-stream records, text included, are the one producer allowed
//! beside the poller (`/api/v1/live/tail`, `#452 slack-live-ingest`; see [`crate::live_tail`]):
//! the hub remembers the messages it created ([`CREATED_MEMORY`]), so whichever of the record and
//! the poller's read comes second publishes nothing new, and a read that says more than the
//! record goes out as an update.
//!
//! # The built-in Discord source is bounded POLLING, not a Gateway connection
//!
//! Discord's real-time mechanism is the Gateway, and this server does not use it. It keeps a
//! per-channel snowflake cursor, seeds it with
//! [`crate::chat::ChatClient::fetch_recent`], and thereafter walks forward from it with
//! [`crate::chat::ChatClient::fetch_page`]'s `after` on an interval.
//!
//! That is a deliberate choice against the more capable option, for three reasons:
//!
//! 1. **A Gateway is three new things at once.** It needs a WebSocket client for Discord (this
//!    crate has one for ElevenLabs and none for Discord), a heartbeat / resume /
//!    session-invalidate state machine, and the PRIVILEGED `MESSAGE_CONTENT` intent — which a
//!    Discord application has to be granted, and without which every message arrives with an
//!    empty body. Polling needs none of those.
//! 2. **Polling reuses what is already tested.** `fetch_page`, [`crate::model::sort_oldest_first`]
//!    and [`crate::model::MessageId::numeric`] are the whole mechanism, and all three already have
//!    tests — including the string-versus-numeric ordering trap, which is exactly the bug a
//!    hand-rolled cursor would reintroduce, and the `before`/`after` asymmetry, which is the other
//!    one.
//! 3. **The fake can genuinely fail.** [`crate::discord::fake::FakeDiscord`] has `fail_next` and an
//!    unknown-channel refusal, so the failure path of the poll loop is exercised in the suite
//!    rather than reasoned about. A Gateway fake would be a second protocol implementation whose
//!    fidelity nobody could check.
//!
//! And one reason that is about sequencing rather than design: **the Discord layer here has never
//! run against live Discord.** The README says so in Known gaps. Making first contact and
//! introducing a stateful always-connected client in the same change is two untested things at
//! once, and when it misbehaves there is no way to tell which one is wrong.
//!
//! **A Gateway remains an upgrade path behind this same seam.** Everything above this module sees
//! [`LiveHub`]: a channel-keyed publish/subscribe with a replay tail. The external adapter endpoint
//! already publishes into it; a native Gateway implementation would do the same.
//!
//! ## What polling costs, stated plainly
//!
//! It multiplies this server's Discord request volume by one request per channel per interval,
//! forever, whether or not anybody is listening. (A provider that reads channel heads natively,
//! Discord itself among them, spends one head read per tick on its quiet channels instead, and
//! pages only the channels that moved: remote-ops-audit W12, see `quiet_channels`.) A 429 is no
//! longer simply passed on: the client obeys Discord's `Retry-After` within a bounded budget, and
//! only a limit it could not clear reaches this loop — see [`crate::discord::ratelimit`]. That
//! makes a tick cheaper to get wrong, not free. So polling is still **off unless configured**
//! (`discord.live_poll_seconds`, default 0), the interval still has a floor the configuration
//! refuses to go below, and a failing channel still backs off rather than hammering — see
//! [`backoff`]. Adapter authentication is separately opt-in with `ingest.token`; full pushed
//! events remain refused for a provider this server polls, while content-free hints safely wake
//! that poller without bypassing its backoff.
//!
//! The two waits nest rather than compete: the client's is the inner, precise one Discord asked
//! for, and [`backoff`] is the outer one, which never waits LESS than Discord's outstanding
//! request. There is one notion of "slow down" here, applied at two scales.
//!
//! **The backoff is not decoration, and it is not left to be read.** [`poll_forever`] sleeping longer
//! after a failure is pinned by a test that measures the loop's own elapsed time against a healthy
//! control, because deleting the one line that applies it changes no other assertion in this
//! suite: a hammering loop still fetches, still publishes and still recovers.
//!
//! ## What a busy channel costs, also stated plainly
//!
//! A tick walks FORWARD from its cursor with Discord's `after` parameter rather than re-reading
//! the most recent window, which is the difference between "at most `limit` messages per tick get
//! through" and "nothing is ever skipped". Reading the newest `limit` messages and then moving the
//! cursor to the newest of them SILENTLY DROPS everything in between when a channel produces more
//! than `limit` in one interval — no gap, no warning, and a page and an agent that are simply
//! missing lines.
//!
//! Forward paging cannot lose them: whatever is not read this tick is still after the cursor on
//! the next one. What it can do is fall behind, so the work per tick is bounded at
//! [`MAX_PAGES_PER_TICK`] pages and [`Tick::backlog`] says when that bound is what stopped it. The
//! loop logs that, once, on the way into it.
//!
//! # Decision two: the PAGE keeps the ElevenLabs conversation socket
//!
//! A message arriving in Discord should be able to reach a live voice conversation. Someone has to
//! hold the socket that carries it, and today that is the browser: this server only mints a signed
//! URL (`GET /api/v1/signed-url`) and holds no per-conversation vendor state at all. The page
//! decides what to do with the arrival — say nothing, summarise it, or read it out — and sends the
//! turn itself; nothing about that choice is known here.
//!
//! It stays that way. Moving the socket server-side would turn vibe-talk into an always-connected,
//! BILLED conversation holder — a process whose cost accrues while nobody is in the car — and it
//! would put third-party channel text on a vendor socket that no human is currently looking at.
//!
//! **The cost of that decision, said out loud: an arriving message reaches the agent only while
//! the page is open.** Close the tab and the channel keeps moving, this server keeps ingesting,
//! and the agent hears nothing until somebody opens the app again and asks. That is a real
//! limitation, and the page says so rather than implying a relay that is not running.
//!
//! # Self-posted messages
//!
//! [`crate::ops::reply`] posts as the bot, and the poller then reads that post back like any
//! other message. Relaying it into the conversation would let the agent hear its own reply and
//! answer it, which is a feedback loop that spends money. So every id this server itself posted
//! is recorded here ([`LiveHub::note_self_posted`]) and travels with the published message as
//! [`LiveMessage::self_posted`]. It is deliberately NOT an author comparison: this server does
//! not know its own bot's user id — [`crate::discord::http::HttpDiscordClient`] never calls
//! `/users/@me` — so an author check would be a guess.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{broadcast, watch};

use crate::chat::{ChatClient, ChatError, LiveRead};
use crate::contract::{LiveDeleteEvent, LiveMessageEvent, LiveResetEvent};
use crate::model::{sort_oldest_first, ChannelId, ChannelInfo, Message, MessageId, Reaction};
use crate::state::AppState;

/// How many published messages each channel keeps for replay after a dropped connection.
///
/// A bound rather than a history: this is a RECONNECTION aid, not storage. A page that has been
/// away longer than this re-reads the channel through `/messages` instead, which is why the
/// stream ends with `event: reset` rather than quietly resuming short.
pub const REPLAY_TAIL: usize = 200;

/// How far behind a subscriber may fall before it is told to re-read.
///
/// Small on purpose. A subscriber this far behind is not going to catch up by being given more
/// buffer, and silently dropping messages into a gap is the one outcome this stream must not
/// produce.
pub const BROADCAST_CAPACITY: usize = 64;

/// How many published events the stream of every channel keeps for a page that reconnects.
///
/// `#337 voice-relay-silent`. More than one channel's [`REPLAY_TAIL`] would need, because every
/// channel's events share it: a call whose stream dropped for a few seconds should get back what
/// it missed in all of them, and a cursor that has fallen out of this is answered with a reset,
/// never a short replay.
pub const ALL_TAIL: usize = 512;

/// How far behind a subscriber to the stream of every channel may fall before it is told so.
///
/// Larger than [`BROADCAST_CAPACITY`] for the reason [`ALL_TAIL`] is: one poll tick of one busy
/// channel can publish a few hundred messages at once, and every channel's arrive here.
pub const ALL_BROADCAST_CAPACITY: usize = 256;

/// How many self-posted ids are remembered, across all channels.
///
/// Bounded because it is a de-duplication window, not a record: the only thing that ever consults
/// it is a message arriving from a poll seconds after this server posted it.
pub const SELF_POSTED_MEMORY: usize = 256;

/// How many adapter event ids are remembered per channel for retry de-duplication.
///
/// This is deliberately larger than [`REPLAY_TAIL`]: an adapter may retry an acknowledged event
/// after it has fallen out of a reconnecting browser's tail, and that retry must still be a no-op.
/// It is nevertheless a bound rather than durable storage. Adapters must give every distinct
/// create, update, and delete its own stable id and retry the same id only for the same event.
pub const INGEST_DEDUPE_MEMORY: usize = 1_024;

/// Largest accepted adapter event id, in bytes.
pub const MAX_EVENT_ID_BYTES: usize = 256;

/// Namespace that keeps an adapter's numeric event id from being mistaken for a message cursor.
const INGEST_EVENT_PREFIX: &str = "push:";

/// How many created messages per channel the hub remembers, with the copy each was published
/// with. `#452 slack-live-ingest`.
///
/// Two producers may now publish the same new message: a provider's event stream pushes it with
/// its text, and the poller's walk forward reads it again moments later. Whichever comes second
/// finds it here and publishes nothing new: the poller's copy goes out as an update only when it
/// says something the pushed one did not (a file, a reaction, a name the stream did not carry).
/// A window over the recently created messages, not a record: the poller follows the stream by
/// one poll interval, and a channel creates far fewer than this many messages in one.
pub const CREATED_MEMORY: usize = 512;

/// How old a pushed message may be, by its own id, and still be published as news before the
/// poller has seeded the channel. `#452 slack-live-ingest`.
///
/// After the seed, the poller's cursor decides first: anything at or behind it is history the
/// page already read or the poller already published. Before it, nothing says what a page has
/// read, so only a message this recent is news; an older one is a stream replaying its history
/// after a restart, and is left to reads.
pub const PUSH_NEWS_MS: i64 = 2 * 60 * 1000;

/// How old a pushed message may be, by its own id, and still be published as news at all.
/// `#452 slack-live-ingest`.
///
/// A stream resuming from a position an hour back replays messages newer than the poller's
/// cursor only if the poller has not walked that far, and older than the hub's memory of what it
/// created: without this bound they would go out again as news. A record this late is left to
/// the poller, whose walk publishes what it never saw.
pub const PUSH_STALE_MS: i64 = 15 * 60 * 1000;

/// How many threads per channel the relist remembers as recently active. `#452
/// slack-live-ingest`.
pub const ACTIVE_THREAD_MEMORY: usize = 32;

/// How many messages per channel the hub remembers reaction sets for. `#378 live-reactions`.
///
/// The diff memory behind the poller's reaction refresh: a window over the recently visible
/// messages, not a record. A message that fell out is re-learned silently by the next refresh
/// rather than announced, exactly as one first seen is.
pub const REACTION_MEMORY: usize = 512;

/// How many of a watched channel's newest messages one tick re-reads to diff their reactions
/// against the hub's memory, at least. `#378 live-reactions`.
///
/// The refresh actually asks for the poll limit or this, whichever is larger, so its window is
/// never smaller than the page a reader has open (the page reads the same limit). Spent only on
/// a channel somebody is watching ([`LiveHub::is_watched`]): an unwatched channel costs no
/// refresh read at all. A provider that reads its newest page on every live read anyway (Slack,
/// through [`crate::chat::ChatClient::fetch_live`]) reports its changes in
/// [`crate::chat::LiveRead::changed`] and pays nothing extra.
pub const REACTION_REFRESH_MESSAGES: u16 = 50;

/// The smallest polling interval the configuration will accept, in seconds.
///
/// Not a preference. One request per channel per interval goes against a rate limit this server
/// does not yet handle, so an operator who types `1` should be refused with a reason rather than
/// quietly given a request storm.
pub const MIN_POLL_SECONDS: u64 = 5;

/// How briefly a change-hint wake-up waits for the rest of the same upstream burst.
///
/// Provider event streams commonly replay several records together after connecting. A hint is
/// deliberately content-free, so polling once for the burst is both cheaper and just as current
/// as polling once per record. The first wake-up waits only one tenth of a second; sustained
/// hint-driven reads are separately spaced by [`MIN_POLL_SECONDS`] so this fast path cannot bypass
/// the configured polling safety floor.
pub const HINT_COALESCE_MILLIS: u64 = 100;

/// What happened to the message carried by a live event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveKind {
    /// A message appeared for the first time.
    Create,
    /// The provider changed an existing message.
    Update,
    /// The provider removed an existing message.
    Delete,
}

/// One provider-neutral event, as it is published to live subscribers.
///
/// Deletes carry a private tombstone in `message`: only its channel and message ids are rendered
/// on the wire. Keeping one envelope type lets polling retain its small `LiveMessage` interface
/// while typed adapter events share the same ordered broadcast and replay tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMessage {
    /// Stable id of this provider event. Retries reuse it; distinct changes must not.
    pub event_id: String,
    /// Create, update, or delete.
    pub kind: LiveKind,
    /// Whether the adapter classified this as catch-up/history rather than a new live event.
    pub historical: bool,
    /// The message itself. UNTRUSTED: written by whoever is in the channel.
    pub message: Message,
    /// Whether THIS SERVER posted it, through [`crate::ops::reply`].
    ///
    /// The page must not relay these into a live conversation; see the module documentation.
    pub self_posted: bool,
}

/// One channel's remembered reaction sets: the order they were first noted in, and the set each
/// message was last seen with (`None` for a known message no copy has reported one for).
type ReactionSets = (
    VecDeque<MessageId>,
    BTreeMap<MessageId, Option<Vec<Reaction>>>,
);

/// What a subscriber gets when it attaches.
pub struct Subscription {
    /// Messages already published that are newer than the caller's cursor, oldest first.
    pub replay: Vec<LiveMessage>,
    /// Everything published from the instant of attachment onwards.
    pub receiver: broadcast::Receiver<LiveMessage>,
    /// Whether the requested opaque cursor has fallen out of the bounded tail.
    pub reset_on_attach: bool,
}

struct Feed {
    sender: broadcast::Sender<LiveMessage>,
    tail: VecDeque<LiveMessage>,
    dedupe_order: VecDeque<String>,
    dedupe: BTreeSet<String>,
    /// The recently created messages, oldest first, with the copy each was last published with,
    /// or `None` once its deletion was ([`CREATED_MEMORY`]). `#452 slack-live-ingest`. A deleted
    /// one is remembered too, so a late copy of it is never published as news again.
    created_order: VecDeque<MessageId>,
    created: BTreeMap<MessageId, Option<Message>>,
}

/// Where the poller stands in one channel. `#452 slack-live-ingest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PollerCursor {
    /// What it seeded from: everything at or behind it, thread replies included, is history.
    seed: u64,
    /// The newest of the channel's own messages it has walked to. A thread reply is placed by
    /// its own thread, not by this: one behind it may not have been published yet.
    walked: u64,
}

/// What became of a create a producer offered the hub. `#452 slack-live-ingest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    /// It went out as a new message.
    New,
    /// The message was already out; this copy said something more, and went out as an update.
    Updated,
    /// The message was already out, and this copy said nothing new: nothing was published.
    Known,
    /// A pushed message at or behind the poller's cursor, or too old to be news: history a page
    /// reads, never announced.
    History,
}

/// How a create is checked against the messages already created. `#452 slack-live-ingest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreatedCheck {
    /// Not checked: the producer is the only one that can publish it (an adapter's typed event,
    /// a correction, an update). A create is still remembered.
    Unchecked,
    /// A provider's event stream pushed it: a message already out is not published again.
    Pushed,
    /// A read found it: a message already out is published again only as an update, and only
    /// when this copy differs.
    Read,
}

/// Whether two copies of one message say the same thing to a reader. `#452 slack-live-ingest`.
///
/// What only this server adds is left out: the spoken forms, the rendered body, a thread's
/// colour and a link, which every copy is given on its way to a reader anyway. So is a root's
/// reply count, which different reads of one root report differently and every new reply moves:
/// comparing it would publish an update per reply, and could go back and forth between reads. A
/// copy that says nothing about reactions (a pushed one) agrees with any set, since absent is
/// "not seen".
fn same_to_a_reader(held: &Message, read: &Message) -> bool {
    let thread = |message: &Message| {
        message
            .thread
            .as_ref()
            .map(|thread| (thread.id.clone(), thread.is_root))
    };
    held.content == read.content
        && held.author == read.author
        && held.author_id == read.author_id
        && held.author_is_bot == read.author_is_bot
        && held.edited_timestamp == read.edited_timestamp
        && held.mentions == read.mentions
        && thread(held) == thread(read)
        && (read.reactions.is_none() || held.reactions == read.reactions)
}

/// One event as the stream of every channel carries it: its place in the hub's single order of
/// everything published, and the channel it was published to. `#337 voice-relay-silent`.
#[derive(Clone, Debug)]
pub struct Arrival {
    /// Its place in the order: 1 for the first event this hub published, counting every channel.
    pub seq: u64,
    /// The channel it was published to.
    pub channel: ChannelId,
    /// The event.
    pub live: LiveMessage,
}

/// What a subscriber to the stream of every channel gets when it attaches.
pub struct AllSubscription {
    /// On a fresh attach, the newest [`Arrival::seq`] published when it attached (`0` before any):
    /// where it starts. `None` on an attach that resumed from a cursor, or that is to be reset.
    pub start: Option<u64>,
    /// What was published after the caller's cursor, oldest first. Empty on a fresh attach: the
    /// stream of every channel carries what arrives from now on, never the history before it.
    pub replay: Vec<Arrival>,
    /// Everything published from the instant of attachment onwards.
    pub receiver: broadcast::Receiver<Arrival>,
    /// Whether the caller's cursor cannot be resumed from: another process's, or fallen out of
    /// [`ALL_TAIL`].
    pub reset_on_attach: bool,
}

struct AllFeed {
    sender: broadcast::Sender<Arrival>,
    tail: VecDeque<Arrival>,
    published: u64,
}

/// The fan-out: one publisher per channel, each with a bounded replay tail.
///
/// Feeds are created on first use rather than from the configured channel list, and the map is
/// still bounded, because **every path into this type is already allowlist-gated**:
/// [`crate::ops::watch`] resolves the channel against the configuration before subscribing, and
/// the poll loop only ever visits the configured channels. A hub that carried its own copy of the
/// allowlist would be a second definition of "which channels exist" — and two of those is how one
/// of them goes stale.
pub struct LiveHub {
    feeds: Mutex<BTreeMap<ChannelId, Feed>>,
    /// The poller's cursor in each channel, from its seed on: every message at or behind it is
    /// history a page has read or the poller has published. `#452 slack-live-ingest`; see
    /// [`LiveHub::publish_pushed`].
    seeded: Mutex<BTreeMap<ChannelId, PollerCursor>>,
    /// Channels whose next relist was asked for early: a stream that lost records. `#452
    /// slack-live-ingest`; see [`crate::relist`].
    relist_wanted: Mutex<BTreeSet<ChannelId>>,
    /// The threads each channel's stream recently pushed a reply in, newest last, with when.
    /// `#452 slack-live-ingest`: what the relist reads besides each channel's newest page.
    active_threads: Mutex<BTreeMap<ChannelId, VecDeque<(String, i64)>>>,
    /// Every channel's events in the one order they were published, for a page in a call. `#337
    /// voice-relay-silent`. Taken only while `feeds` is held, so that order is the feeds' order.
    all: Mutex<AllFeed>,
    /// This process's name in the cursors of [`LiveHub::subscribe_all`], so a cursor handed out
    /// before a restart is told apart from one of this process's and answered with a reset.
    epoch: String,
    self_posted: Mutex<(VecDeque<MessageId>, BTreeSet<MessageId>)>,
    hinted: Mutex<BTreeMap<Option<String>, BTreeSet<ChannelId>>>,
    hint_generation: watch::Sender<u64>,
    /// The newest created message per channel the owner set to clear unread at its provider.
    /// `#232 upstream-mark-read`. Fed on every publish, never waited on: see
    /// [`crate::upstream_clear`].
    upstream_clear: std::sync::Arc<crate::upstream_clear::Candidates>,
    /// Each channel's newest window, kept current by every publish. `#210 voice-catchup-speed`.
    /// Here for the reason `upstream_clear` is: both producers reach the hub, and only the hub,
    /// so this is the one place a change cannot bypass. See [`crate::recent`].
    recent: std::sync::Arc<crate::recent::RecentWindows>,
    /// The server's message-cache handle, offered every change under the feed lock. `#208
    /// persistent-cache`; see [`crate::cache::sink`]. Attached once, by whoever builds the server
    /// ([`LiveHub::attach_cache`]); a hub with none attached offers nothing.
    cache: std::sync::OnceLock<std::sync::Arc<crate::cache::Sink>>,
    /// The durable store, for the colours of threads the poller is the first to meet. `#427
    /// server-thread-hues`. Attached once like the cache ([`LiveHub::attach_store`]); a hub with
    /// none attached colours nothing itself, and the stream's way out and the next write-scope
    /// read colour its threads instead.
    store: std::sync::OnceLock<std::sync::Arc<dyn crate::store::StateStore>>,
    /// The reaction set last seen for each recent message, per channel, oldest-noted first: an
    /// entry of `None` is a KNOWN message no copy has reported reactions for yet, which is what
    /// tells its first reaction apart from a message never seen. `#378 live-reactions`; see
    /// [`LiveHub::reactions_changed`]. Taken only under the feed lock or on its own, never the
    /// other way round.
    reaction_memory: Mutex<BTreeMap<ChannelId, ReactionSets>>,
    /// How many reaction updates and corrections this hub has published: the uniqueness of
    /// their event ids. A subscriber resumes after the LAST event of the id it names, so an
    /// update or correction reusing its message's create id would silently swallow everything
    /// between the two (`#384 cache-serve`).
    updates: std::sync::atomic::AtomicU64,
}

impl Default for LiveHub {
    fn default() -> Self {
        Self::with_recent(std::sync::Arc::new(crate::recent::RecentWindows::new()))
    }
}

impl LiveHub {
    /// An empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty hub keeping its channel windows in `recent`. For a test that shortens the
    /// catch-up's durations; see [`crate::recent::Timing`].
    #[must_use]
    pub fn with_recent(recent: std::sync::Arc<crate::recent::RecentWindows>) -> Self {
        let (hint_generation, _receiver) = watch::channel(0);
        let (all_sender, _) = broadcast::channel(ALL_BROADCAST_CAPACITY);
        let mut epoch = [0_u8; 8];
        getrandom::fill(&mut epoch)
            .expect("the system CSPRNG must be available to name a live hub");
        Self {
            feeds: Mutex::new(BTreeMap::new()),
            seeded: Mutex::new(BTreeMap::new()),
            relist_wanted: Mutex::new(BTreeSet::new()),
            active_threads: Mutex::new(BTreeMap::new()),
            all: Mutex::new(AllFeed {
                sender: all_sender,
                tail: VecDeque::new(),
                published: 0,
            }),
            epoch: epoch.iter().map(|byte| format!("{byte:02x}")).collect(),
            self_posted: Mutex::new((VecDeque::new(), BTreeSet::new())),
            hinted: Mutex::new(BTreeMap::new()),
            hint_generation,
            upstream_clear: std::sync::Arc::new(crate::upstream_clear::Candidates::new()),
            recent,
            cache: std::sync::OnceLock::new(),
            store: std::sync::OnceLock::new(),
            reaction_memory: Mutex::new(BTreeMap::new()),
            updates: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Offer every change this hub publishes to `sink` from now on. `#208 persistent-cache`.
    ///
    /// Once per hub: a second sink is ignored, so nothing can swap the cache a running hub feeds.
    /// The server attaches its one sink before anything is published.
    pub fn attach_cache(&self, sink: std::sync::Arc<crate::cache::Sink>) {
        let _ = self.cache.set(sink);
    }

    /// The message-cache handle this hub offers its changes to, if one is attached.
    #[must_use]
    pub fn message_cache(&self) -> Option<&std::sync::Arc<crate::cache::Sink>> {
        self.cache.get()
    }

    /// File this hub's thread colours in `store` from now on. `#427 server-thread-hues`.
    ///
    /// Once per hub, like the cache: a second store is ignored. The server attaches its one
    /// store before anything is published, so the poller colours a thread in the tick it first
    /// reads a message of it — a server act, under the poller's own standing, as its message
    /// cache fills are.
    pub fn attach_store(&self, store: std::sync::Arc<dyn crate::store::StateStore>) {
        let _ = self.store.set(store);
    }

    /// The store this hub colours threads in, if one is attached.
    #[must_use]
    pub fn store(&self) -> Option<&std::sync::Arc<dyn crate::store::StateStore>> {
        self.store.get()
    }

    /// Each channel's held window. `#210 voice-catchup-speed`.
    #[must_use]
    pub fn recent(&self) -> &std::sync::Arc<crate::recent::RecentWindows> {
        &self.recent
    }

    /// The messages waiting to be cleared at their provider. `#232 upstream-mark-read`.
    #[must_use]
    pub fn upstream_clear(&self) -> &std::sync::Arc<crate::upstream_clear::Candidates> {
        &self.upstream_clear
    }

    /// Queue one content-free request to poll `channel` now.
    ///
    /// The provider key is `None` for a single-provider deployment, matching the poll task's own
    /// key, and `Some` in a multi-provider deployment. Repeated hints coalesce until that task
    /// takes the channel. The periodic poll remains the recovery path, so a full notification
    /// buffer, process restart, or event-stream gap can delay a read but cannot make the hint the
    /// only record that a message existed.
    #[must_use]
    pub fn queue_hint(&self, provider_key: Option<&str>, channel: &ChannelId) -> bool {
        let inserted = lock(&self.hinted)
            .entry(provider_key.map(str::to_owned))
            .or_default()
            .insert(channel.clone());
        if inserted {
            let next = (*self.hint_generation.borrow()).wrapping_add(1);
            self.hint_generation.send_replace(next);
        }
        inserted
    }

    fn subscribe_hints(&self) -> watch::Receiver<u64> {
        self.hint_generation.subscribe()
    }

    fn take_hints(&self, provider_key: Option<&str>) -> BTreeSet<ChannelId> {
        lock(&self.hinted)
            .remove(&provider_key.map(str::to_owned))
            .unwrap_or_default()
    }

    /// Remember that this server posted `id`, so the poll that reads it back can say so.
    ///
    /// Bounded to [`SELF_POSTED_MEMORY`] ids, oldest evicted first.
    pub fn note_self_posted(&self, id: &MessageId) {
        let mut guard = lock(&self.self_posted);
        let (order, set) = &mut *guard;
        if !set.insert(id.clone()) {
            return;
        }
        order.push_back(id.clone());
        while order.len() > SELF_POSTED_MEMORY {
            if let Some(evicted) = order.pop_front() {
                set.remove(&evicted);
            }
        }
    }

    /// Whether `id` is one this server posted.
    #[must_use]
    pub fn is_self_posted(&self, id: &MessageId) -> bool {
        lock(&self.self_posted).1.contains(id)
    }

    /// Whether anybody is listening for `channel` right now: a subscriber to its own stream, or
    /// to the stream of every channel. `#378 live-reactions`: what the reaction refresh is spent
    /// on — a channel nobody is watching shows its reactions at its next read, as it always did.
    #[must_use]
    pub fn is_watched(&self, channel: &ChannelId) -> bool {
        if lock(&self.all).sender.receiver_count() > 0 {
            return true;
        }
        lock(&self.feeds)
            .get(channel)
            .is_some_and(|feed| feed.sender.receiver_count() > 0)
    }

    /// Remember `message`'s reaction set where this copy reports one, and remember the message
    /// itself either way. `#378 live-reactions`.
    ///
    /// A copy that says nothing about reactions never downgrades a known set: absent is "not
    /// seen", never "none" ([`crate::model::Message::reactions`]). It still marks the message
    /// known, so the first set a later read reports for it reads as a change, not a first sight.
    pub fn note_reactions(&self, channel: &ChannelId, message: &Message) {
        let mut memory = lock(&self.reaction_memory);
        let (order, sets) = memory.entry(channel.clone()).or_default();
        match &message.reactions {
            Some(reactions) => {
                if sets
                    .insert(message.id.clone(), Some(reactions.clone()))
                    .is_none()
                {
                    order.push_back(message.id.clone());
                }
            }
            None => {
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    sets.entry(message.id.clone())
                {
                    slot.insert(None);
                    order.push_back(message.id.clone());
                }
            }
        }
        while order.len() > REACTION_MEMORY {
            if let Some(evicted) = order.pop_front() {
                sets.remove(&evicted);
            }
        }
    }

    /// Whether a READ copy of `message` carries a different reaction set than the remembered
    /// one, remembering the copy's either way — decided and recorded under one lock, so a push
    /// landing meanwhile is never overwritten by this older copy. `#378 live-reactions`.
    ///
    /// A message never seen before is learned silently (`false`): its set reaches the page with
    /// the read that brought it, and announcing it would burst an update per message after every
    /// restart. A KNOWN message is the live case: its first reported set, or a set that moved.
    ///
    /// The last case is gated on `absent_means_none`. Both wire formats drop the field when the
    /// last reaction goes, but absent equally means "this endpoint cannot say"
    /// ([`crate::model::Message::reactions`]) — a bridge may carry reactions on one read path
    /// and not another. So a copy reporting nothing reads as the removal ONLY where the caller
    /// knows this read reports reactions where they exist: some message in the same read carried
    /// a set. Without that, the remembered set stands. For the removal the copy is stamped with
    /// the empty set it means, so what is published says "none" outright.
    #[must_use]
    pub fn reactions_changed(
        &self,
        channel: &ChannelId,
        message: &mut Message,
        absent_means_none: bool,
    ) -> bool {
        let mut memory = lock(&self.reaction_memory);
        let (order, sets) = memory.entry(channel.clone()).or_default();
        let held = sets.get(&message.id).cloned();
        let fresh_slot = held.is_none();
        let changed = match (&held, &message.reactions) {
            (Some(Some(set)), None) if absent_means_none && !set.is_empty() => {
                message.reactions = Some(Vec::new());
                true
            }
            (Some(before), Some(_)) => before != &message.reactions,
            _ => false,
        };
        match &message.reactions {
            Some(reactions) => {
                sets.insert(message.id.clone(), Some(reactions.clone()));
            }
            None => {
                if fresh_slot {
                    sets.insert(message.id.clone(), None);
                }
            }
        }
        if fresh_slot {
            order.push_back(message.id.clone());
            while order.len() > REACTION_MEMORY {
                if let Some(evicted) = order.pop_front() {
                    sets.remove(&evicted);
                }
            }
        }
        changed
    }

    /// Forget `id`'s reaction set: the message is gone, and a reused id must be learned afresh.
    fn forget_reactions(&self, channel: &ChannelId, id: &MessageId) {
        let mut memory = lock(&self.reaction_memory);
        if let Some((order, sets)) = memory.get_mut(channel) {
            sets.remove(id);
            order.retain(|held| held != id);
        }
    }

    /// Publish one already-known message whose reactions changed, as a live update carrying its
    /// full current copy. `#378 live-reactions`.
    ///
    /// Through the feed exactly as an ingested update goes — held window, message cache, replay
    /// tail and all — except that nothing is offered for clearing unread: the message is not new.
    pub fn publish_update(&self, channel: &ChannelId, message: Message) {
        self.publish_update_read(channel, message, None);
    }

    /// [`LiveHub::publish_update`] for a copy a read that began at `read` found, as
    /// [`LiveHub::publish_read`] publishes a new message.
    fn publish_update_read(
        &self,
        channel: &ChannelId,
        message: Message,
        read: Option<crate::cache::ReadStart>,
    ) {
        let at = self
            .updates
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        // The hub's epoch is in the id, as it is in the all-stream's cursors: a page reconnecting
        // across a restart with a stale update cursor must get a reset, never a coincidental
        // match against this process's same-numbered update for the same message.
        let live = LiveMessage {
            event_id: format!("{}~u{}-{at}", message.id.as_str(), self.epoch),
            kind: LiveKind::Update,
            historical: false,
            message,
            self_posted: false,
        };
        self.publish_to_feed(channel, live, false, read);
    }

    /// Publish one message to `channel`, and remember it for replay.
    ///
    /// A publish with no subscribers is NOT a failure: the tail is what a page attaching a moment
    /// later reads, so the message is kept either way.
    pub fn publish(&self, channel: &ChannelId, message: Message) {
        self.publish_read(channel, message, None);
    }

    /// [`LiveHub::publish`] a message a read found, the read having begun at `read` (its time by
    /// the message cache's clock, and its stamp): the cache is offered it as of then, not as of
    /// now, so an edit or a deletion the cache learned of while the read was on its way back is not
    /// undone by it, and an erase made meanwhile drops it as it drops the read's own offers.
    /// `#208 persistent-cache`.
    ///
    /// A message a provider's event stream already pushed is not published again (`#452
    /// slack-live-ingest`): this copy goes out as an update only when it says more.
    fn publish_read(
        &self,
        channel: &ChannelId,
        message: Message,
        read: Option<crate::cache::ReadStart>,
    ) -> Published {
        let self_posted = self.is_self_posted(&message.id);
        let live = LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind: LiveKind::Create,
            historical: false,
            message,
            self_posted,
        };
        self.publish_inner(channel, live, false, read, CreatedCheck::Read)
    }

    /// Publish one thread reply a provider read apart from the channel's own timeline, and
    /// remember it for replay. `#307 slack-reply-live`; see
    /// [`crate::chat::ChatClient::fetch_live`].
    ///
    /// Exactly [`LiveHub::publish`], held windows included, except that the reply is not offered
    /// for clearing unread at the provider ([`crate::upstream_clear`]). It was read out of step
    /// with the channel's own messages, so the poller may not yet have every one of those up to
    /// its instant, and a clear through it could mark read a message vibe-talk never received. The
    /// next message of the channel's own is offered as ever.
    pub fn publish_reply(&self, channel: &ChannelId, message: Message) {
        self.publish_reply_read(channel, message, None);
    }

    /// [`LiveHub::publish_reply`] a reply a read found, the read having begun at `read`, as
    /// [`LiveHub::publish_read`] publishes a message of the channel's own: the cache is offered it
    /// as of then. `#208 persistent-cache`.
    fn publish_reply_read(
        &self,
        channel: &ChannelId,
        message: Message,
        read: Option<crate::cache::ReadStart>,
    ) -> Published {
        let self_posted = self.is_self_posted(&message.id);
        let live = LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind: LiveKind::Create,
            historical: false,
            message,
            self_posted,
        };
        self.publish_checked(channel, live, false, read, CreatedCheck::Read)
    }

    /// Publish one new message a provider's own event stream pushed with its text, without
    /// reading it again. `#452 slack-live-ingest`.
    ///
    /// Published as the poller would publish the same message — its id as its event id, a thread
    /// reply (`reply`) not offered for clearing unread, as [`LiveHub::publish_reply`] — so that a
    /// page cannot tell the two producers apart, and the poller's later read of it publishes
    /// nothing new ([`CREATED_MEMORY`]). Offered to the message cache as an adapter's push, whose
    /// text a later read proves whole (merge rule 5), under [`crate::cache::ServerFill::Stream`],
    /// which never lets the provider's feed vouch for what the stream did not carry.
    ///
    /// Not published at all when the message is already out ([`Published::Known`]), or is
    /// history ([`Published::History`], [`LiveHub::is_history`]): at or behind the poller's
    /// cursor, older than [`PUSH_STALE_MS`], or, before any seed, older than [`PUSH_NEWS_MS`] at
    /// `now_ms`. A stream replays its
    /// recent history when it resumes, and a page has already read that history; announcing it
    /// again would relay old words into a live conversation as news.
    pub fn publish_pushed(
        &self,
        channel: &ChannelId,
        message: Message,
        reply: bool,
        now_ms: i64,
    ) -> Published {
        if self.is_history(channel, &message.id, reply, now_ms) {
            return Published::History;
        }
        if reply {
            if let Some(thread) = message.thread.as_ref() {
                self.note_thread_activity(channel, &thread.id, now_ms);
            }
        }
        let self_posted = self.is_self_posted(&message.id);
        let live = LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind: LiveKind::Create,
            historical: false,
            message,
            self_posted,
        };
        if reply {
            self.publish_checked(channel, live, true, None, CreatedCheck::Pushed)
        } else {
            self.publish_inner(channel, live, true, None, CreatedCheck::Pushed)
        }
    }

    /// Publish what a relist of one scope of `channel` found changed: each message readers were
    /// given that the listing holds differently, as an update, and each that lies inside the
    /// listing's span and is gone from it, as a deletion. `#452 slack-live-ingest`; see
    /// [`crate::relist`].
    ///
    /// `thread` is the thread the listing read, or `None` for the channel's own timeline, which
    /// lists thread roots and the messages nobody answered, never a reply: only those can be
    /// missing from it. What readers were given is the hub's own: the copies it published
    /// ([`CREATED_MEMORY`]) and the channel's held window. The span runs from the listing's
    /// oldest message up to `proven_until_ms` (provider time), or to its newest message when
    /// that is later: a newest page lists everything from its oldest message to the instant its
    /// read began, and the caller holds that instant back by a settling margin, so a message
    /// created just before the read, which the provider may not list yet, is never taken for a
    /// deleted one. A message the provider leaves out of every read (one with no author) is never
    /// one readers were given. An empty listing proves nothing, and deletes nothing.
    ///
    /// An update is not offered to the message cache, which the relist's own read already
    /// offered whole; a deletion is, as a push, since a listing that may leave messages out is
    /// no proof of a deletion to the cache. Each is checked under the feed lock against the copy
    /// it replaces, as a correction is, so a newer copy is never put back.
    pub fn publish_relisted(
        &self,
        channel: &ChannelId,
        listed: &[Message],
        thread: Option<&str>,
        proven_until_ms: i64,
    ) -> (usize, usize) {
        let in_scope = |message: &Message| match thread {
            None => message.thread.as_ref().is_none_or(|held| held.is_root),
            Some(thread) => message
                .thread
                .as_ref()
                .is_some_and(|held| held.id == thread),
        };
        let span = listed
            .iter()
            .filter(|message| in_scope(message))
            .filter_map(|message| message.id.numeric())
            .fold(None, |span: Option<(u64, u64)>, id| match span {
                None => Some((id, id)),
                Some((low, high)) => Some((low.min(id), high.max(id))),
            })
            .map(|(low, high)| {
                let proven = MessageId::at_time_ms(proven_until_ms)
                    .numeric()
                    .unwrap_or(0);
                (low, high.max(proven))
            });
        // The held window's copy wins where both have one: it is the copy a correction is
        // checked against below.
        let (mut given, deleted): (BTreeMap<MessageId, Message>, BTreeSet<MessageId>) =
            lock(&self.feeds)
                .get(channel)
                .map(|feed| {
                    let given = feed
                        .created
                        .iter()
                        .filter_map(|(id, held)| Some((id.clone(), held.clone()?)))
                        .collect();
                    let deleted = feed
                        .created
                        .iter()
                        .filter(|(_, held)| held.is_none())
                        .map(|(id, _)| id.clone())
                        .collect();
                    (given, deleted)
                })
                .unwrap_or_default();
        for message in self
            .recent
            .snapshot(channel)
            .held
            .map(|held| held.messages)
            .unwrap_or_default()
        {
            if !deleted.contains(&message.id) {
                given.insert(message.id.clone(), message);
            }
        }
        let listed_ids: BTreeSet<&MessageId> = listed.iter().map(|message| &message.id).collect();
        let mut changes: Vec<(LiveKind, Message, Message)> = Vec::new();
        for message in listed {
            if let Some(held) = given.get(&message.id) {
                if !same_to_a_reader(held, message) {
                    changes.push((LiveKind::Update, message.clone(), held.clone()));
                }
            }
        }
        if let Some((low, high)) = span {
            for (id, held) in &given {
                let inside = id.numeric().is_some_and(|id| id >= low && id <= high);
                if inside && in_scope(held) && !listed_ids.contains(id) {
                    changes.push((LiveKind::Delete, held.clone(), held.clone()));
                }
            }
        }
        let (mut updates, mut deletes) = (0, 0);
        for (kind, message, held) in changes {
            let self_posted = kind != LiveKind::Delete && self.is_self_posted(&message.id);
            let at = self
                .updates
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            let live = LiveMessage {
                event_id: format!("{}~r{}-{at}", message.id.as_str(), self.epoch),
                kind,
                historical: false,
                message,
                self_posted,
            };
            let offer = kind == LiveKind::Delete;
            let published = self.publish_to_feed_offering(
                channel,
                live,
                false,
                None,
                offer,
                Some(&held),
                CreatedCheck::Unchecked,
            );
            if published == Published::New {
                match kind {
                    LiveKind::Delete => deletes += 1,
                    _ => updates += 1,
                }
            }
        }
        (updates, deletes)
    }

    /// Whether a pushed message `id` of `channel` is history rather than news at `now_ms`: at or
    /// behind the poller's cursor, older than [`PUSH_STALE_MS`], or, before the poller has seeded
    /// the channel, older than [`PUSH_NEWS_MS`]. `#452 slack-live-ingest`; see
    /// [`LiveHub::publish_pushed`]. An id that does not order is never news.
    ///
    /// For a thread reply (`reply`) only the seed counts, not how far the poller has walked the
    /// channel's own messages since: a reply posted just before a message the poller has
    /// already walked past may still be unpublished, and in a thread rooted beyond the newest page
    /// the poller would never publish it.
    #[must_use]
    pub fn is_history(
        &self,
        channel: &ChannelId,
        id: &MessageId,
        reply: bool,
        now_ms: i64,
    ) -> bool {
        let Some(age) = id.created_at_ms().map(|at| now_ms.saturating_sub(at)) else {
            return true;
        };
        match (lock(&self.seeded).get(channel).copied(), id.numeric()) {
            (_, None) => true,
            (Some(cursor), Some(numeric)) => {
                let behind = if reply { cursor.seed } else { cursor.walked };
                numeric <= behind || age > PUSH_STALE_MS
            }
            (None, Some(_)) => age > PUSH_NEWS_MS,
        }
    }

    /// Record the cursor the poller seeded `channel` from, or has walked forward to since: it
    /// only ever moves forward. `#452 slack-live-ingest`.
    pub fn note_poller_cursor(&self, channel: &ChannelId, cursor: u64) {
        let mut cursors = lock(&self.seeded);
        let held = cursors.entry(channel.clone()).or_insert(PollerCursor {
            seed: cursor,
            walked: cursor,
        });
        held.walked = held.walked.max(cursor);
    }

    /// The newest of `channel`'s own messages the poller has walked to, once it has seeded the
    /// channel.
    #[must_use]
    pub fn poller_cursor(&self, channel: &ChannelId) -> Option<u64> {
        lock(&self.seeded).get(channel).map(|cursor| cursor.walked)
    }

    /// Forget the poller's cursor in `channel`: it is off the list, and seeds again if it comes
    /// back.
    pub fn forget_seeded(&self, channel: &ChannelId) {
        lock(&self.seeded).remove(channel);
    }

    /// Ask for `channel`'s next relist now rather than at its interval. `#452 slack-live-ingest`.
    pub fn request_relist(&self, channel: &ChannelId) {
        lock(&self.relist_wanted).insert(channel.clone());
    }

    /// Take every channel whose relist was asked for early.
    #[must_use]
    pub fn take_relist_requests(&self) -> BTreeSet<ChannelId> {
        std::mem::take(&mut *lock(&self.relist_wanted))
    }

    /// Remember that a reply was just pushed in `thread` of `channel`, at `now_ms`.
    pub fn note_thread_activity(&self, channel: &ChannelId, thread: &str, now_ms: i64) {
        let mut active = lock(&self.active_threads);
        let threads = active.entry(channel.clone()).or_default();
        threads.retain(|(held, _)| held != thread);
        threads.push_back((thread.to_owned(), now_ms));
        while threads.len() > ACTIVE_THREAD_MEMORY {
            threads.pop_front();
        }
    }

    /// The threads of `channel` a reply was pushed in at or after `since_ms`, most recent first.
    #[must_use]
    pub fn active_threads(&self, channel: &ChannelId, since_ms: i64) -> Vec<String> {
        lock(&self.active_threads)
            .get(channel)
            .map(|threads| {
                threads
                    .iter()
                    .rev()
                    .filter(|(_, at)| *at >= since_ms)
                    .map(|(thread, _)| thread.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Publish one event delivered by an external adapter.
    ///
    /// Returns `false` when this channel has already accepted `event_id` inside the bounded
    /// de-duplication window. The duplicate is not broadcast or added to the replay tail.
    pub fn publish_ingested(
        &self,
        event_id: String,
        kind: LiveKind,
        historical: bool,
        channel: &ChannelId,
        message: Message,
    ) -> bool {
        let self_posted = kind != LiveKind::Delete && self.is_self_posted(&message.id);
        let live = LiveMessage {
            event_id: format!("{INGEST_EVENT_PREFIX}{event_id}"),
            kind,
            historical,
            message,
            self_posted,
        };
        self.publish_inner(channel, live, true, None, CreatedCheck::Unchecked) == Published::New
    }

    fn publish_inner(
        &self,
        channel: &ChannelId,
        live: LiveMessage,
        deduplicate: bool,
        read: Option<crate::cache::ReadStart>,
        check: CreatedCheck,
    ) -> Published {
        // Both producers reach this, so it is where a created message is noted for clearing at
        // its provider: one map update and a wake-up, after the feed lock is released.
        let created = (live.kind == LiveKind::Create)
            .then(|| (live.message.id.clone(), live.message.timestamp.clone()));
        let published = self.publish_checked(channel, live, deduplicate, read, check);
        if published == Published::New {
            if let Some((id, timestamp)) = created {
                self.upstream_clear.offer_parts(channel, &id, &timestamp);
            }
        }
        published
    }

    /// Publish a correction of a message a reader may already have been shown: an edit or a
    /// deletion a background refresh found after the message cache answered a newest page
    /// from what it held (`#384 cache-serve`). Broadcast, replayed and merged into the held
    /// windows like any event, so a page showing the served copy is put right where it stands.
    ///
    /// Unlike every other publish, nothing is offered to the message cache: the refresh's own
    /// read has already offered what it saw, whole, under its fill, and offering the same
    /// message again as a push would hold its text as possibly a snippet. Nor is a correction
    /// offered for clearing unread at the provider: it is not a message that just arrived.
    ///
    /// `message` replaces the copy `served_as` — what the reader was shown — and the publish
    /// checks, under the feed lock, that the channel's held window still holds that copy (or
    /// the correction itself already): a newer copy that arrived since the refresh checked
    /// the cache is never overwritten backwards by its correction. A held window that never
    /// held the message proves nothing, and the correction goes out.
    pub fn publish_correction(
        &self,
        channel: &ChannelId,
        kind: LiveKind,
        message: Message,
        served_as: &Message,
    ) {
        let self_posted = kind != LiveKind::Delete && self.is_self_posted(&message.id);
        let at = self
            .updates
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let live = LiveMessage {
            // Never the message's own id: a create already carries it, and a subscriber
            // resuming after the last event of a named id would skip all between the two.
            // The hub's epoch and counter make it this publish's alone, as a reaction
            // update's id is (`#378 live-reactions`).
            event_id: format!("{}~c{}-{at}", message.id.as_str(), self.epoch),
            kind,
            historical: false,
            message,
            self_posted,
        };
        self.publish_to_feed_offering(
            channel,
            live,
            false,
            None,
            false,
            Some(served_as),
            CreatedCheck::Unchecked,
        );
    }

    fn publish_to_feed(
        &self,
        channel: &ChannelId,
        live: LiveMessage,
        deduplicate: bool,
        read: Option<crate::cache::ReadStart>,
    ) -> bool {
        self.publish_to_feed_offering(
            channel,
            live,
            deduplicate,
            read,
            true,
            None,
            CreatedCheck::Unchecked,
        ) == Published::New
    }

    fn publish_checked(
        &self,
        channel: &ChannelId,
        live: LiveMessage,
        deduplicate: bool,
        read: Option<crate::cache::ReadStart>,
        check: CreatedCheck,
    ) -> Published {
        self.publish_to_feed_offering(channel, live, deduplicate, read, true, None, check)
    }

    /// `replaces`, for a correction, is the copy readers were shown: when the held window has
    /// since taken a different copy of the message, the correction publishes nothing, rather
    /// than putting an older text back over it.
    ///
    /// `check` says how a create is held against the messages already created (`#452
    /// slack-live-ingest`), under the feed lock, so a push and a read of the same message racing
    /// each other publish it once.
    #[allow(clippy::too_many_arguments)]
    fn publish_to_feed_offering(
        &self,
        channel: &ChannelId,
        mut live: LiveMessage,
        deduplicate: bool,
        read: Option<crate::cache::ReadStart>,
        offer_to_cache: bool,
        replaces: Option<&Message>,
        check: CreatedCheck,
    ) -> Published {
        let mut feeds = lock(&self.feeds);
        // A correction overtaken under this lock publishes nothing: the held window — raw
        // copies, as a serve hands back — now holds a copy of the message that is neither
        // what was served nor the correction, so a newer sighting of it has landed since.
        if let Some(replaces) = replaces {
            let held = self.recent.snapshot(channel).held;
            let overtaken = held
                .as_ref()
                .and_then(|held| {
                    held.messages
                        .iter()
                        .find(|message| message.id == live.message.id)
                })
                .is_some_and(|now| now != replaces && *now != live.message);
            if overtaken {
                return Published::Known;
            }
        }
        let feed = feeds.entry(channel.clone()).or_insert_with(new_feed);
        if deduplicate {
            if !feed.dedupe.insert(live.event_id.clone()) {
                return Published::Known;
            }
            feed.dedupe_order.push_back(live.event_id.clone());
            while feed.dedupe_order.len() > INGEST_DEDUPE_MEMORY {
                if let Some(evicted) = feed.dedupe_order.pop_front() {
                    feed.dedupe.remove(&evicted);
                }
            }
        }
        let mut outcome = Published::New;
        if live.kind == LiveKind::Create && check != CreatedCheck::Unchecked {
            if let Some(held) = feed.created.get(&live.message.id) {
                let Some(held) = held else {
                    // Deleted since: a late copy brings nothing back.
                    return Published::Known;
                };
                if check == CreatedCheck::Pushed || same_to_a_reader(held, &live.message) {
                    return Published::Known;
                }
                // A read's copy of a message a push already put out, saying more than the push
                // did: an update, under an id of its own, as a reaction update is.
                let at = self
                    .updates
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                live.event_id = format!("{}~u{}-{at}", live.message.id.as_str(), self.epoch);
                live.kind = LiveKind::Update;
                outcome = Published::Updated;
            }
        }
        let remembered = match live.kind {
            LiveKind::Delete => Some(None),
            LiveKind::Create => Some(Some(live.message.clone())),
            LiveKind::Update => None,
        };
        if let Some(held) = feed.created.get_mut(&live.message.id) {
            if live.kind == LiveKind::Delete {
                *held = None;
            } else if held.is_some() {
                *held = Some(live.message.clone());
            }
        } else if let Some(remembered) = remembered {
            feed.created.insert(live.message.id.clone(), remembered);
            feed.created_order.push_back(live.message.id.clone());
            while feed.created_order.len() > CREATED_MEMORY {
                if let Some(evicted) = feed.created_order.pop_front() {
                    feed.created.remove(&evicted);
                }
            }
        }
        // `#378 live-reactions`. Every producer teaches the reaction memory here, so a set a
        // push or a read already brought is never re-announced by the poller's refresh.
        if live.kind == LiveKind::Delete {
            self.forget_reactions(channel, &live.message.id);
        } else {
            self.note_reactions(channel, &live.message);
        }
        self.recent.merge(channel, &live);
        // `#208 persistent-cache`. Queued for the message cache here, under the feed lock and
        // before the send below, so every change a subscriber can have seen is already in the
        // cache's queue by the time it could read the cache. Never waits; see `Sink::offer_push`.
        // A correction (`#384 cache-serve`) is the one publish offered nothing: the read that
        // found the change offered it already, whole. Its subscribers see no less for that:
        // what a correction says was in the cache's queue before the correction was checked.
        if offer_to_cache {
            if let Some(sink) = self.cache.get() {
                // `#452 slack-live-ingest`: a stream's push vouches for no feed, since the stream
                // carries no edits or deletions.
                let producer = if check == CreatedCheck::Pushed {
                    crate::cache::ServerFill::Stream
                } else if deduplicate {
                    crate::cache::ServerFill::Ingest
                } else {
                    crate::cache::ServerFill::Poller
                };
                sink.offer_push(channel, &live, producer, read);
            }
        }
        feed.tail.push_back(live.clone());
        while feed.tail.len() > REPLAY_TAIL {
            feed.tail.pop_front();
        }
        // `#337 voice-relay-silent`. And into the stream of every channel, still under the feed
        // lock, so its order is the order every channel's own stream saw.
        {
            let mut all = lock(&self.all);
            all.published += 1;
            let arrival = Arrival {
                seq: all.published,
                channel: channel.clone(),
                live: live.clone(),
            };
            all.tail.push_back(arrival.clone());
            while all.tail.len() > ALL_TAIL {
                all.tail.pop_front();
            }
            let _ = all.sender.send(arrival);
        }
        let _ = feed.sender.send(live);
        outcome
    }

    /// Attach to a channel, replaying what was published after `after`.
    ///
    /// **The receiver is created BEFORE the tail is read, under the same lock.** That ordering is
    /// the whole correctness of this function: attaching afterwards leaves a window in which a
    /// message is published after the tail was copied and before the receiver exists, and it is
    /// gone — the one outcome the recovery story is supposed to rule out. Holding the lock across
    /// both is what makes it atomic with respect to [`LiveHub::publish`].
    ///
    /// `after` is an event id the caller has already seen. When it is still in the tail, exactly
    /// what follows it is replayed. Every missing id forces a reset, including a numeric id from the
    /// polling producer: once the bounded tail has evicted the cursor, numeric ordering can identify
    /// newer retained creates but cannot prove that nothing was lost before them. It also cannot
    /// place push updates and deletes. Replaying either a partial tail or nothing would therefore
    /// hide a gap.
    #[must_use]
    pub fn subscribe(&self, channel: &ChannelId, after: Option<&MessageId>) -> Subscription {
        let mut feeds = lock(&self.feeds);
        let feed = feeds.entry(channel.clone()).or_insert_with(new_feed);
        let receiver = feed.sender.subscribe();
        let (replay, reset_on_attach) = match after {
            None => (feed.tail.iter().cloned().collect(), false),
            Some(after) => {
                if let Some(position) = feed
                    .tail
                    .iter()
                    .rposition(|held| held.event_id == after.as_str())
                {
                    (
                        feed.tail.iter().skip(position + 1).cloned().collect(),
                        false,
                    )
                } else {
                    (Vec::new(), true)
                }
            }
        };
        Subscription {
            replay,
            receiver,
            reset_on_attach,
        }
    }

    /// Attach to every channel at once, from `after` — a cursor this hub handed out — or, with
    /// none, from now. `#337 voice-relay-silent`.
    ///
    /// The receiver is created before the tail is read, under one lock, for the reason
    /// [`LiveHub::subscribe`] gives. A fresh attach replays NOTHING: a page in a call wants what
    /// arrives during it, and every channel's history is not that. A cursor of this process's
    /// still covered by [`ALL_TAIL`] replays exactly what followed it; any other cursor — another
    /// process's, one past what was published, one whose successors fell out of the tail — is a
    /// gap this stream cannot fill, and is answered with a reset.
    #[must_use]
    pub fn subscribe_all(&self, after: Option<&str>) -> AllSubscription {
        let all = lock(&self.all);
        let receiver = all.sender.subscribe();
        let position = all.published;
        let resumable = |seq: u64| {
            let oldest = all.tail.front().map_or(position + 1, |held| held.seq);
            seq <= position && seq + 1 >= oldest
        };
        let (replay, reset_on_attach) = match after.map(|cursor| self.all_seq(cursor)) {
            None => (Vec::new(), false),
            Some(Some(seq)) if resumable(seq) => (
                all.tail
                    .iter()
                    .filter(|held| held.seq > seq)
                    .cloned()
                    .collect(),
                false,
            ),
            Some(_) => (Vec::new(), true),
        };
        AllSubscription {
            start: after.is_none().then_some(position),
            replay,
            receiver,
            reset_on_attach,
        }
    }

    /// The cursor naming [`Arrival::seq`] `seq` in this process: what the stream of every
    /// channel sends as each event's `id`, and a page sends back as `Last-Event-ID`.
    #[must_use]
    pub fn all_cursor(&self, seq: u64) -> String {
        format!("{}.{seq}", self.epoch)
    }

    /// The `seq` a cursor names, or `None` for one this process did not hand out.
    fn all_seq(&self, cursor: &str) -> Option<u64> {
        let (epoch, seq) = cursor.trim().split_once('.')?;
        if epoch != self.epoch {
            return None;
        }
        seq.parse().ok()
    }
}

fn new_feed() -> Feed {
    let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
    Feed {
        sender,
        tail: VecDeque::new(),
        dedupe_order: VecDeque::new(),
        dedupe: BTreeSet::new(),
        created_order: VecDeque::new(),
        created: BTreeMap::new(),
    }
}

/// The SSE event name every arriving message carries.
pub const EVENT_MESSAGE: &str = "message";
/// The SSE event name for a changed message.
pub const EVENT_MESSAGE_UPDATE: &str = "message_update";
/// The SSE event name for a removed message.
pub const EVENT_MESSAGE_DELETE: &str = "message_delete";

/// The SSE event name that ends a stream a subscriber fell too far behind on.
///
/// A page that receives this must RE-READ the channel through `/messages`. It is deliberately not
/// a resumption: the messages that fell out of the broadcast buffer are gone from this stream, and
/// resuming short would be the silent drop this whole design exists to rule out.
pub const EVENT_RESET: &str = "reset";

/// Turn one [`Subscription`] into the body of a Server-Sent Events response.
///
/// The replay tail goes out first, oldest first, then everything live. Each event carries an
/// `id:` cursor so a reconnecting browser's `Last-Event-ID` has something to send back, and a
/// typed event name so the page does not have to guess create/update/delete from payload shape.
///
/// **Every event says whether it came out of the tail (`replayed: true`) or arrived live
/// (`replayed: false`).** Without that the two are indistinguishable on the wire, and a page that
/// attaches with no `Last-Event-ID` — a fresh sign-in, a channel change, the reconnect after an
/// `event: reset` — is handed up to [`REPLAY_TAIL`] messages it must render but must NOT announce.
/// Announcing them means relaying stale channel text into a live, BILLED conversation as "a
/// message was just posted", which is the same "existing history labelled as new" failure that
/// [`poll_once`]'s seeding tick exists to prevent, arriving through the other door.
///
/// A subscriber that falls further behind than [`BROADCAST_CAPACITY`] gets one
/// [`EVENT_RESET`] and the stream ENDS. See its documentation for why that is better than
/// resuming.
///
/// **Every message is judged against the owner's noise rules on its way OUT**, not when it was
/// published. `#196 auto-read-noise`: a rule is evaluated on the message's current text each time
/// it is served, and the tail can be minutes old — a rule added or a message rescued since then
/// has to be reflected in what a page attaching now is told. `app` and `channel` are what that
/// needs; the replay burst shares one reading of the rules, and each live event takes a fresh one.
pub fn events(
    subscription: Subscription,
    app: AppState,
    channel: ChannelInfo,
    caller: crate::auth::Scope,
) -> impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
{
    struct State {
        replay: VecDeque<LiveMessage>,
        receiver: broadcast::Receiver<LiveMessage>,
        reset_on_attach: bool,
        done: bool,
        app: AppState,
        channel: ChannelId,
        /// How the channel's provider writes, for the rendered body each message event carries.
        /// `#217 markdown-blocks`.
        markup: crate::render::Markup,
        /// The rules as they stood when the replay burst began. Up to [`REPLAY_TAIL`] events go
        /// out back to back on attach, and two store reads for each would be the attach's whole
        /// cost for no change in the answer.
        replay_noise: Option<crate::noise::NoiseFilter>,
        /// Who is listening, for the colours of threads met on the way out (`#427
        /// server-thread-hues`): a write-scope subscriber's stream colours a thread nobody has
        /// coloured yet — one that arrived by a path that does not colour, or before a store was
        /// attached — and a read-scope one only carries colours already assigned, as every read
        /// does. Polled and ingested arrivals are coloured before they are published, so this
        /// is the backstop, not the usual path.
        caller: crate::auth::Scope,
        /// The burst's colours, decided once for the whole tail, as `replay_noise` is.
        replay_hues: Option<std::collections::BTreeMap<String, u16>>,
    }
    let markup = crate::render::markup_for(&app.config, &channel);
    let start = State {
        replay: subscription.replay.into_iter().collect(),
        receiver: subscription.receiver,
        reset_on_attach: subscription.reset_on_attach,
        done: false,
        app,
        channel: channel.id,
        markup,
        replay_noise: None,
        caller,
        replay_hues: None,
    };
    futures_util::stream::unfold(start, |mut state| async move {
        if state.reset_on_attach {
            state.reset_on_attach = false;
            state.done = true;
            return Some((Ok(reset_event(0)), state));
        }
        if let Some(mut held) = state.replay.pop_front() {
            if state.replay_noise.is_none() {
                state.replay_noise =
                    Some(crate::noise::filter_for(&state.app, &state.channel).await);
                // The whole burst's colours in one deciding, this first event included.
                let refs = crate::ops::hue_refs_of(
                    std::iter::once(&held.message)
                        .chain(state.replay.iter().map(|live| &live.message)),
                    &[],
                    None,
                );
                state.replay_hues = Some(
                    crate::ops::hues_for(&state.app, state.caller, &state.channel, &refs).await,
                );
            }
            if let Some(hues) = &state.replay_hues {
                if held.kind != LiveKind::Delete {
                    crate::ops::stamp_hues(std::slice::from_mut(&mut held.message), hues);
                }
            }
            if let Some(noise) = &state.replay_noise {
                judge(noise, &state.app, &state.channel, state.markup, &mut held);
            }
            return Some((Ok(message_event(&held, true)), state));
        }
        if state.done {
            return None;
        }
        match state.receiver.recv().await {
            Ok(mut live) => {
                if live.kind != LiveKind::Delete {
                    crate::ops::attach_message_hues(
                        &state.app,
                        state.caller,
                        &state.channel,
                        std::slice::from_mut(&mut live.message),
                    )
                    .await;
                }
                let noise = crate::noise::filter_for(&state.app, &state.channel).await;
                judge(&noise, &state.app, &state.channel, state.markup, &mut live);
                Some((Ok(message_event(&live, false)), state))
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                state.done = true;
                Some((Ok(reset_event(missed)), state))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
}

/// Turn one [`AllSubscription`] into the body of the stream of every channel: `GET
/// /api/v1/live/stream`. `#337 voice-relay-silent`.
///
/// A page in a call reads it so that a message in ANY channel on the owner's list reaches the
/// call, not only one in the channel on screen, whose stream ([`events`]) was the only one it had.
/// Each event is the one that channel's own stream sends — the same payload, judged against the
/// noise rules and rendered the same way on its way out — under this stream's own cursor as its
/// `id`, so one `Last-Event-ID` resumes every channel at once.
///
/// * A fresh attach first sends one event with an `id` and nothing else: where it starts. A page
///   that drops before any message arrives still has a cursor to resume from.
/// * Resumed, it replays what followed the cursor, as `replayed` and `from_tail`, as a
///   channel's stream replays its tail; the page tells those apart from a fresh attach's history
///   by having sent the cursor.
/// * A channel off the list — the owner took it off, or it was never on it — is not watched: its
///   event goes out as a bare `id`, which moves the cursor past it and carries nothing.
/// * A cursor it cannot resume from, or a subscriber further behind than
///   [`ALL_BROADCAST_CAPACITY`], gets one [`EVENT_RESET`] and the stream ends, as [`events`] does.
pub fn all_events(
    subscription: AllSubscription,
    app: AppState,
    caller: crate::auth::Scope,
) -> impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
{
    struct State {
        start: Option<u64>,
        replay: VecDeque<Arrival>,
        receiver: broadcast::Receiver<Arrival>,
        reset_on_attach: bool,
        done: bool,
        app: AppState,
        caller: crate::auth::Scope,
    }
    let start = State {
        start: subscription.start,
        replay: subscription.replay.into_iter().collect(),
        receiver: subscription.receiver,
        reset_on_attach: subscription.reset_on_attach,
        done: false,
        app,
        caller,
    };
    futures_util::stream::unfold(start, |mut state| async move {
        if state.reset_on_attach {
            state.reset_on_attach = false;
            state.done = true;
            return Some((Ok(reset_event(0)), state));
        }
        if let Some(position) = state.start.take() {
            let cursor = state.app.live.all_cursor(position);
            return Some((Ok(axum::response::sse::Event::default().id(cursor)), state));
        }
        let (arrival, from_tail) = if let Some(held) = state.replay.pop_front() {
            (held, true)
        } else if state.done {
            return None;
        } else {
            match state.receiver.recv().await {
                Ok(arrival) => (arrival, false),
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    state.done = true;
                    return Some((Ok(reset_event(missed)), state));
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        };
        let cursor = state.app.live.all_cursor(arrival.seq);
        let Some(channel) = state.app.channel(arrival.channel.as_str()) else {
            return Some((Ok(axum::response::sse::Event::default().id(cursor)), state));
        };
        let mut live = arrival.live;
        if live.kind != LiveKind::Delete {
            crate::ops::attach_message_hues(
                &state.app,
                state.caller,
                &channel.id,
                std::slice::from_mut(&mut live.message),
            )
            .await;
        }
        let noise = crate::noise::filter_for(&state.app, &channel.id).await;
        let markup = crate::render::markup_for(&state.app.config, &channel);
        judge(&noise, &state.app, &channel.id, markup, &mut live);
        Some((Ok(message_event_as(&live, from_tail, &cursor)), state))
    })
}

/// Decide whether one outgoing event's message is noise, and render its body and find its heading
/// for the page. A delete carries no text to judge.
///
/// Rendered on the way OUT, like the noise verdict, rather than when it was published: the tail is
/// shared by every subscriber, and only this stream's reader is the page. Its paren tag is recorded
/// first, as a read records a window's (`#242 rich-headers`): the stream is how a new message
/// reaches the page, and the next one from the same agent should not wait for a re-read.
fn judge(
    noise: &crate::noise::NoiseFilter,
    app: &AppState,
    channel: &ChannelId,
    markup: crate::render::Markup,
    live: &mut LiveMessage,
) {
    if live.kind != LiveKind::Delete {
        noise.mark(std::slice::from_mut(&mut live.message));
        let message = &mut live.message;
        if let Some(tag) = crate::heading::paren_candidate(&message.content, message.author_is_bot)
        {
            app.headings
                .record(channel, &message.author_id, &message.id, tag);
        }
        let confirmed = |tag: &str| app.headings.confirmed(channel, &message.author_id, tag);
        (message.content_html, message.heading) = crate::render::headed_body(
            &message.content,
            markup,
            message.author_is_bot,
            &confirmed,
            &message.mentions,
        );
        // `#408 copy-upstream-link`: as `ops::stamp` forms it on a read, so a message that
        // arrived live offers its link like one a read brought — and never the one an adapter
        // put there, which is overwritten here as the rendered body is. Routed by the channel
        // the event is for, which a thread message's own channel need not be.
        message.upstream_url = app.providers.client_for(channel).and_then(|client| {
            client.upstream_url(&message.channel_id, &message.id, message.thread.as_ref())
        });
    }
}

fn message_event(live: &LiveMessage, from_tail: bool) -> axum::response::sse::Event {
    message_event_as(live, from_tail, &live.event_id)
}

/// [`message_event`] under another `id`: the stream of every channel's cursor, rather than the
/// channel's own event id. `#337 voice-relay-silent`.
fn message_event_as(live: &LiveMessage, from_tail: bool, id: &str) -> axum::response::sse::Event {
    let event = axum::response::sse::Event::default()
        .id(id)
        .event(match live.kind {
            LiveKind::Create => EVENT_MESSAGE,
            LiveKind::Update => EVENT_MESSAGE_UPDATE,
            LiveKind::Delete => EVENT_MESSAGE_DELETE,
        });
    // A catch-up event is history even on its first trip through this process. Folding the
    // adapter's classification into the wire's existing `replayed` flag ensures the browser may
    // render it but never announce it to a live voice conversation as something that just
    // happened.
    // `from_tail` keeps the other half apart, because the page's reads depend on it: only an event
    // from the tail is one every read begun after the attach has already seen.
    let replayed = from_tail || live.historical;
    let encoded = match live.kind {
        LiveKind::Create | LiveKind::Update => event.json_data(LiveMessageEvent {
            message: &live.message,
            self_posted: live.self_posted,
            replayed,
            from_tail,
            untrusted_content_notice: crate::untrusted::NOTICE,
        }),
        LiveKind::Delete => event.json_data(LiveDeleteEvent {
            channel_id: live.message.channel_id.clone(),
            message_id: live.message.id.clone(),
            replayed,
            from_tail,
        }),
    };
    // A `Message` is plain data and cannot fail to serialize. If that ever stops being true,
    // ending the stream with a reset is the honest answer: the page re-reads and sees the
    // message, rather than being handed a frame it cannot parse.
    encoded.unwrap_or_else(|_| reset_event(0))
}

fn reset_event(missed: u64) -> axum::response::sse::Event {
    let data = serde_json::to_string(&LiveResetEvent {
        missed,
        detail: "this subscriber fell behind; re-read the channel rather than resuming",
    })
    .unwrap_or_else(|_| format!("{{\"missed\":{missed}}}"));
    axum::response::sse::Event::default()
        .event(EVENT_RESET)
        .data(data)
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How many pages one tick will walk forward before leaving the rest for the next tick.
///
/// The bound exists because "catch up completely, whatever that costs" is a request storm against
/// a rate limit this server does not handle: a channel that has been busy for an hour would be
/// fetched a hundred times in one tick. Four pages of `discord.default_fetch_limit` is 200
/// messages per channel per interval at the default, which is far more than a human channel
/// produces and still a fixed ceiling on the damage.
///
/// Being stopped by it loses NOTHING — the cursor only ever moves past what was actually published
/// — it just means the next tick continues. [`Tick::backlog`] says when that happened.
pub const MAX_PAGES_PER_TICK: usize = 4;

/// What one call to [`poll_once`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// How many messages were published to the hub.
    pub published: usize,
    /// Whether [`MAX_PAGES_PER_TICK`] is what stopped this tick rather than running out of
    /// messages.
    ///
    /// **Not a gap.** Everything unread is still after the cursor and is published by a later
    /// tick; this says the channel is arriving faster than one tick reads it, which is a latency
    /// fact worth a log line and not a loss.
    pub backlog: bool,
    /// Whether the provider left thread replies for the next tick ([`crate::chat::LiveRead`]).
    /// `#307 slack-reply-live`. Not a gap either: a later tick publishes them.
    pub replies_behind: bool,
    /// How many reaction updates were published: already-known messages whose reaction set
    /// moved. `#378 live-reactions`. Counted apart from `published`, which stays "new messages
    /// a page may relay": an update must never be relayed as news.
    pub changed: usize,
}

/// Colour the threads `messages` belong to, in the hub's store, and stamp the colours on.
/// `#427 server-thread-hues`.
///
/// The poller is one of the ways this server first learns of a thread, so the assignment is made
/// here, as a message arrives — the poller's own server act, like its message-cache fills — and
/// what is published then already carries the colour. Soft, like every other colouring: a hub
/// with no store attached, or a store that refuses, publishes the messages uncoloured, and the
/// stream's way out or the next write-scope read colours their threads then.
async fn colour_threads(hub: &LiveHub, channel: &ChannelId, messages: &mut [Message]) {
    let Some(store) = hub.store() else { return };
    let refs = crate::ops::hue_refs_of(&*messages, &[], None);
    if refs.is_empty() {
        return;
    }
    // The common tick colours nothing new, and it stays a read — as `ops::hues_for` keeps a
    // settled read one — rather than a write transaction per poll page to say "unchanged".
    if let Ok(held) = store.thread_hues(channel).await {
        let settled = refs.iter().all(|wanted| {
            held.iter().any(|row| {
                row.thread_id == wanted.thread_id
                    && (wanted.root_at_ms.is_none() || row.root_at_ms.is_some())
            })
        });
        if settled {
            let hues: std::collections::BTreeMap<String, u16> = held
                .into_iter()
                .map(|row| (row.thread_id, row.hue))
                .collect();
            crate::ops::stamp_hues(messages, &hues);
            return;
        }
    }
    match store.ensure_thread_hues(channel, &refs).await {
        Ok(rows) => {
            let hues: std::collections::BTreeMap<String, u16> = rows
                .into_iter()
                .map(|row| (row.thread_id, row.hue))
                .collect();
            crate::ops::stamp_hues(messages, &hues);
        }
        Err(crate::store::StoreError::Unavailable(_)) => {}
        Err(error) => {
            tracing::warn!(%error, "could not colour the threads a live read met");
        }
    }
}

/// Read one channel once, publishing whatever is newer than `cursor`.
///
/// **The first tick seeds and publishes nothing.** `cursor` starts as `None`, and on that tick
/// this function records the newest id it saw and returns 0. Without that, the first poll after a
/// restart would republish the whole recent window, and a page that attached a second later would
/// be shown existing history labelled as newly arrived — messages the reader had already read,
/// announced as though they had just been said, and relayed into a paid conversation as news.
///
/// **Every tick after that walks FORWARD from the cursor**, with Discord's `after`, and keeps
/// walking while pages come back full — up to [`MAX_PAGES_PER_TICK`]. Reading the most recent
/// `limit` messages instead would silently drop everything between the cursor and that window the
/// moment a channel produced more than `limit` in one interval: the cursor would jump to the
/// newest id and the messages in between would never be published, never be relayed, and never be
/// reported. `after` walks the OLDEST messages newer than the cursor, so the worst a burst can do
/// is take several ticks.
///
/// Messages whose id does not parse as a snowflake are skipped rather than guessed at: there is no
/// honest way to place them relative to the cursor, and publishing them on every tick would be a
/// permanent duplicate.
///
/// **Thread replies a provider reads apart are published as well.** `#307 slack-reply-live`. A
/// channel whose provider [`ChatClient::polls_thread_replies`] is seeded and stepped with
/// [`ChatClient::fetch_live`] instead, and the first page of each tick then also carries the thread
/// replies posted since the provider's previous read ([`crate::chat::LiveRead`]). They are
/// published in time order among that page's messages, through [`LiveHub::publish_reply`], and
/// they never move the cursor: a reply read a tick late may be older than a message already
/// published, and a cursor moved past a message that page did not hold would skip it. The provider
/// keeps its own place in each thread and reports each reply once; one that is also among the
/// page's messages (a reply sent to the channel too, say) is published once, as a message.
///
/// **What it reads is offered to the message cache** (`#208 persistent-cache`), under the poller's
/// own grant, when the hub has a cache attached: the seeding read as a recent window, which never
/// says where the channel starts however short it came back, read then through
/// [`ChatClient::fetch_recent_dated`] so that a copy the provider keeps is offered as of when it
/// was taken; and the walk as a listing of what lay
/// between the cursor and the newest message it saw, but only when the walk ran out of messages
/// rather than into [`MAX_PAGES_PER_TICK`]. See [`crate::cache::observe::forward`]. What it
/// publishes reaches the cache through the hub as well, as every published change does, as of when
/// the tick's read began rather than when it is published.
///
/// Through [`ChatClient::fetch_live`] (`#307 slack-reply-live`), the seed is the conversation's
/// own newest messages, offered as a recent window as well, though from no copy, so as of the
/// read's start; the walk's listing holds the conversation's own messages and nothing else; and
/// the thread replies read apart are offered as rows only, never as part of that listing nor as
/// a claim on their thread: the provider read each thread from where it last stood in it.
///
/// # Errors
///
/// [`ChatError`] from the fetch. **The cursor is not advanced past anything that was not
/// published**, so the next successful tick publishes everything that arrived in the meantime
/// rather than skipping it. It IS advanced past what this tick already published before the
/// failure, because republishing that would relay the same message into a paid conversation twice.
pub async fn poll_once(
    chat: &dyn ChatClient,
    hub: &LiveHub,
    channel: &ChannelId,
    limit: u16,
    cursor: &mut Option<u64>,
) -> Result<Tick, ChatError> {
    let apart = chat.polls_thread_replies(channel);
    // Begun before the read, as every fill is: see `crate::cache::sink`. What the walk publishes
    // is offered to the cache as of then too: see `LiveHub::publish_read`.
    let fill = hub.message_cache().and_then(|sink| {
        sink.begin(
            &crate::cache::FillGrant::server(crate::cache::ServerFill::Poller),
            channel,
        )
    });
    let read_start = fill.as_ref().map(crate::cache::Fill::read_start);
    let Some(seen) = *cursor else {
        let messages = match fill {
            // `#307 slack-reply-live`: the provider's own seed, which also notes where each thread
            // stands. It reads the conversation's own newest messages, no reply among them, as a
            // recent read did before replies were read for it, and from no copy, so nothing dates
            // it but the read's start.
            Some(fill) if apart => {
                let messages = chat.fetch_live(channel, limit, None).await?.messages;
                fill.offer_recent_dated(&crate::chat::RecentRead {
                    messages: messages.clone(),
                    ..crate::chat::RecentRead::default()
                });
                messages
            }
            None if apart => chat.fetch_live(channel, limit, None).await?.messages,
            // Dated when the cache is offered it, as every recent read the cache is offered is:
            // a copy the provider keeps is offered as of when it was taken.
            Some(fill) => {
                let read = chat.fetch_recent_dated(channel, limit).await?;
                fill.offer_recent_dated(&read);
                read.messages
            }
            None => chat.fetch_recent(channel, limit).await?,
        };
        // `#427 server-thread-hues`. The seed deliberately colours NOTHING: it is existing
        // history, exactly what it must not present as newly arrived, and a browser may still
        // hold the colours those threads were drawn in before the server decided them — its
        // one seeding offer, made before its first read, has to be able to win for them. They
        // are coloured by that first write-scope read, or by the walk when they next arrive.
        // `#378 live-reactions`. The seed learns where reactions stand, so the first refresh
        // after it can tell a set that moved from one never seen.
        for message in &messages {
            hub.note_reactions(channel, message);
        }
        // Seed. `unwrap_or(0)` rather than leaving the cursor unset: an EMPTY channel is a
        // perfectly good starting point, and leaving it `None` would make the next tick seed
        // again and publish nothing for as long as the channel stayed quiet.
        let seed = messages
            .iter()
            .filter_map(|m| m.id.numeric())
            .max()
            .unwrap_or(0);
        *cursor = Some(seed);
        // `#452 slack-live-ingest`: what a provider's event stream pushes at or behind this is
        // history this read has just seen, and never news.
        hub.note_poller_cursor(channel, seed);
        return Ok(Tick::default());
    };

    let page_size = usize::from(limit.max(1));
    let mut published = 0;
    let mut updated = 0_usize;
    let mut at = seen;
    let mut backlog = false;
    let mut replies_behind = false;
    // Every message the walk's pages returned, for the cache, and whether the walk ran out of
    // messages: a page shorter than asked for, rather than the page bound or a page it could not
    // move past.
    let mut walked: Vec<Message> = Vec::new();
    // The thread replies the provider read apart, for the cache too: rows only.
    let mut walked_replies: Vec<Message> = Vec::new();
    // `#378 live-reactions`. What the provider says changed, and what this tick published new,
    // so a changed copy of a message already published here is never announced twice.
    let mut reported_changed: Vec<Message> = Vec::new();
    let mut published_ids: BTreeSet<MessageId> = BTreeSet::new();
    let mut ran_out = false;
    for page in 0..MAX_PAGES_PER_TICK {
        let after = MessageId(at.to_string());
        let read = if apart && page == 0 {
            chat.fetch_live(channel, limit, Some(&after)).await
        } else {
            chat.fetch_page(channel, limit, None, Some(&after))
                .await
                .map(|messages| LiveRead {
                    messages,
                    ..LiveRead::default()
                })
        };
        let LiveRead {
            mut messages,
            replies,
            replies_behind: behind,
            changed,
        } = match read {
            Ok(read) => read,
            Err(error) => {
                // Whatever went out in an earlier page of this tick really went out. Rewinding to
                // where the tick started would republish it, and the page cannot tell a
                // republished message from a new one once it has scrolled away.
                *cursor = Some(at);
                hub.note_poller_cursor(channel, at);
                // `#378 live-reactions`. So did the reaction changes an earlier page reported:
                // the provider already advanced its own memory past them, so leaving them
                // unpublished here would lose them until the set moved again.
                for message in reported_changed.drain(..) {
                    if !published_ids.contains(&message.id) {
                        hub.publish_update_read(channel, message, read_start.clone());
                    }
                }
                return Err(error);
            }
        };
        replies_behind |= behind;
        reported_changed.extend(changed);
        sort_oldest_first(&mut messages);
        // `#427 server-thread-hues`: colour their threads before anything is published, so what
        // goes out already carries its colour and the hub's tail keeps it.
        colour_threads(hub, channel, &mut messages).await;
        let mut replies = replies;
        colour_threads(hub, channel, &mut replies).await;
        let full = messages.len() >= page_size;
        let before = at;
        if fill.is_some() {
            walked.extend(messages.iter().cloned());
            walked_replies.extend(replies.iter().cloned());
        }
        // Each arrival with its id, and whether it is a reply read apart.
        let mut arrivals: Vec<(u64, Message, bool)> = Vec::new();
        for message in messages {
            let Some(id) = message.id.numeric() else {
                continue;
            };
            if id <= at {
                continue;
            }
            at = id;
            arrivals.push((id, message, false));
        }
        for reply in replies {
            let Some(id) = reply.id.numeric() else {
                continue;
            };
            if arrivals.iter().any(|(held, _, _)| *held == id) {
                continue;
            }
            arrivals.push((id, reply, true));
        }
        arrivals.sort_by_key(|(id, _, _)| *id);
        for (_, message, reply) in arrivals {
            published_ids.insert(message.id.clone());
            let outcome = if reply {
                hub.publish_reply_read(channel, message, read_start.clone())
            } else {
                hub.publish_read(channel, message, read_start.clone())
            };
            // `#452 slack-live-ingest`. A message the provider's event stream already pushed is
            // not news a second time; a copy of it that says more went out as an update.
            match outcome {
                Published::New => published += 1,
                Published::Updated => updated += 1,
                Published::Known | Published::History => {}
            }
        }
        // `at == before` means the page held nothing this cursor can move past — an empty answer,
        // or one made entirely of ids that do not parse. Walking again would fetch the same page
        // forever.
        if !full || at == before {
            ran_out = !full;
            break;
        }
        if page + 1 == MAX_PAGES_PER_TICK {
            backlog = true;
        }
    }
    *cursor = Some(at);
    hub.note_poller_cursor(channel, at);
    if let Some(fill) = fill {
        let after_ms = MessageId(seen.to_string()).created_at_ms();
        fill.offer_forward(
            &walked,
            &walked_replies,
            after_ms.unwrap_or(i64::MAX),
            ran_out && after_ms.is_some(),
        );
    }
    // `#378 live-reactions`. Reaction changes to messages at or behind the cursor, which the
    // forward walk by construction never republishes. A provider whose live read already holds
    // its newest page reports them ([`LiveRead::changed`]); any other provider is diffed here
    // against the hub's memory over one bounded re-read of its newest messages — and only while
    // somebody is watching the channel, since an unwatched channel's next ordinary read brings
    // the same sets. A refresh that fails loses nothing and fails nothing: the cursor already
    // stands where the walk put it, and the next tick diffs again.
    let mut changed = updated;
    // A provider's own report is trusted outright: it diffed against ITS memory, which may hold
    // messages (a full newest page, pre-seed thread replies) the hub's smaller window never
    // learned. Gating it on the hub's memory would drop exactly those updates, permanently:
    // the provider already stands past them.
    for message in reported_changed {
        if published_ids.contains(&message.id) {
            continue;
        }
        hub.publish_update_read(channel, message, read_start.clone());
        changed += 1;
    }
    if !apart && hub.is_watched(channel) {
        match chat
            .fetch_recent(channel, limit.max(REACTION_REFRESH_MESSAGES))
            .await
        {
            Ok(recent) => {
                // Whether THIS read reports reactions at all: the gate under which an absent
                // field may read as "the last one was removed" (`LiveHub::reactions_changed`).
                let reports = recent.iter().any(|message| message.reactions.is_some());
                for mut message in recent {
                    if published_ids.contains(&message.id) {
                        continue;
                    }
                    // Only what the cursor already stood past when this tick began: anything
                    // newer is news the walk (or the next tick) publishes as a create.
                    if message.id.numeric().is_none_or(|id| id > seen) {
                        hub.note_reactions(channel, &message);
                        continue;
                    }
                    if hub.reactions_changed(channel, &mut message, reports) {
                        // Dated now, not at the tick's start: this copy was read after the
                        // whole walk, and an older stamp would lose the cache's dating race
                        // to a read that began while the tick ran and still holds the old set.
                        hub.publish_update_read(channel, message, None);
                        changed += 1;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    channel = %channel,
                    %error,
                    "the reaction refresh is left for the next tick"
                );
            }
        }
    }
    Ok(Tick {
        published,
        backlog,
        replies_behind,
        changed,
    })
}

/// How long to wait after `failures` consecutive failures on a channel.
///
/// Doubling, capped. The cap matters more than the growth: an unbounded backoff means a channel
/// that failed once at three in the morning is still not being read at nine, which reads on screen
/// as a dead bridge rather than as a recovered one.
#[must_use]
pub fn backoff(interval: Duration, failures: u32, max: Duration) -> Duration {
    if failures == 0 {
        return interval;
    }
    let shift = failures.min(6);
    interval
        .saturating_mul(1_u32 << shift)
        .min(max)
        .max(interval)
}

/// The ceiling [`backoff`] doubles up to, as a multiple of the configured interval.
///
/// Sixteen intervals: long enough that a channel that has been unreachable all night is not being
/// hammered, short enough that a five-minute outage costs at most a couple of extra minutes of
/// staleness once it clears.
pub const MAX_BACKOFF_INTERVALS: u32 = 16;

/// Poll every currently allowed channel of one provider forever, publishing into `state.live`.
///
/// Never returns and never gives up. A channel that fails backs off and is tried again; it does
/// not take the loop down with it, and it does not advance its own cursor, so nothing is skipped
/// once it recovers. The allowlist is read from [`AppState::all_channels`] on every tick so a
/// channel added in the app starts being polled without restarting the server. Cursor and backoff
/// state for a removed channel is discarded before the next fetch. Spawned by `main`, once per
/// polled provider, and cancelled only by the process exiting.
///
/// `provider_key` limits the loop to that provider's channels and `chat` is that provider's own
/// client; `None` polls every channel, which is what a single-provider deployment does.
pub async fn poll_forever(
    state: AppState,
    provider_key: Option<String>,
    chat: std::sync::Arc<dyn ChatClient>,
    limit: u16,
    interval: Duration,
) {
    let mut cursors = BTreeMap::new();
    poll_state_loop(
        &state,
        provider_key.as_deref(),
        chat.as_ref(),
        limit,
        interval,
        &mut cursors,
        None,
    )
    .await;
}

async fn poll_state_loop(
    state: &AppState,
    provider_key: Option<&str>,
    chat: &dyn ChatClient,
    limit: u16,
    interval: Duration,
    cursors: &mut BTreeMap<ChannelId, Option<u64>>,
    ticks: Option<u32>,
) {
    poll_loop_with_channels(
        chat,
        state.live.as_ref(),
        || {
            state
                .all_channels()
                .into_iter()
                .filter(|channel| {
                    provider_key.is_none_or(|key| channel.provider.as_deref() == Some(key))
                })
                .map(|channel| channel.id)
                .collect()
        },
        cursors,
        LoopConfig {
            limit,
            interval,
            ticks,
            hints: Some((state.live.as_ref(), provider_key)),
        },
    )
    .await;
}

/// The body of the static-channel polling tests, with the cursors handed in and a tick budget.
///
/// `ticks: None` is the production case and never returns. The cursors are the caller's so that a
/// test can stop the loop, change the channel underneath it, and start it again WHERE IT LEFT OFF
/// — which is the only way to state "an error did not lose the cursor" as an assertion rather than
/// as a comment.
#[cfg(test)]
async fn poll_loop(
    chat: &dyn ChatClient,
    hub: &LiveHub,
    channels: &[ChannelId],
    limit: u16,
    interval: Duration,
    cursors: &mut BTreeMap<ChannelId, Option<u64>>,
    ticks: Option<u32>,
) {
    let channels = channels.to_vec();
    poll_loop_with_channels(
        chat,
        hub,
        || channels.clone(),
        cursors,
        LoopConfig {
            limit,
            interval,
            ticks,
            hints: None,
        },
    )
    .await;
}

struct LoopConfig<'a> {
    limit: u16,
    interval: Duration,
    ticks: Option<u32>,
    hints: Option<(&'a LiveHub, Option<&'a str>)>,
}

async fn poll_loop_with_channels(
    chat: &dyn ChatClient,
    hub: &LiveHub,
    mut channels: impl FnMut() -> Vec<ChannelId>,
    cursors: &mut BTreeMap<ChannelId, Option<u64>>,
    config: LoopConfig<'_>,
) {
    let LoopConfig {
        limit,
        interval,
        ticks,
        hints,
    } = config;
    let (hint_hub, hint_provider_key) =
        hints.map_or((None, None), |(hub, provider)| (Some(hub), provider));
    let max_backoff = interval.saturating_mul(MAX_BACKOFF_INTERVALS);
    let mut failures: BTreeMap<ChannelId, u32> = BTreeMap::new();
    let mut behind: BTreeMap<ChannelId, bool> = BTreeMap::new();
    let mut replies_behind: BTreeMap<ChannelId, bool> = BTreeMap::new();
    let mut heads_failing = false;
    let mut remaining = ticks;
    let mut hinted_only: Option<BTreeSet<ChannelId>> = None;
    let mut hint_generation = hint_hub.map(LiveHub::subscribe_hints);
    // Hinted reads must not move this deadline. Otherwise a busy channel can keep resetting an
    // interval-shaped sleep and starve reconciliation for every quieter channel on its provider.
    let mut next_reconciliation = hint_hub.map(|_| tokio::time::Instant::now());
    // The first hint is latency-sensitive and may run immediately. Once it does, subsequent
    // hint-driven reads respect the same five-second floor as configured polling.
    let mut next_hint_read = hint_hub.map(|_| tokio::time::Instant::now());
    loop {
        let all_channels = channels();
        let current: BTreeSet<_> = all_channels.iter().cloned().collect();
        // Removal is durable state, not a pause. If the same channel is added later it must seed
        // from Discord again instead of resuming an old cursor and publishing the intervening
        // history as if it had just arrived.
        cursors.retain(|channel, _| {
            let kept = current.contains(channel);
            if !kept {
                hub.forget_seeded(channel);
            }
            kept
        });
        failures.retain(|channel, _| current.contains(channel));
        behind.retain(|channel, _| current.contains(channel));
        replies_behind.retain(|channel, _| current.contains(channel));
        let requested = hinted_only.take();
        let full_reconciliation = requested.is_none();
        // A full reconciliation already reads every channel for this provider. Discard hints
        // queued before it began; anything queued after this take remains pending and wakes the
        // next read, closing the fetch race without paying for every replayed event individually.
        if requested.is_none() {
            if let Some(hint_hub) = hint_hub {
                hint_hub.take_hints(hint_provider_key);
            }
        }
        let channels: Vec<_> = match requested {
            Some(requested) => all_channels
                .into_iter()
                .filter(|channel| requested.contains(channel))
                .collect(),
            None => all_channels,
        };
        let mut wait = interval;
        // remote-ops-audit W12 `#287 bulk-reads`. Only on a full pass: a hinted read was asked for
        // because something changed, so it reads.
        let quiet = if full_reconciliation {
            quiet_channels(
                chat,
                hub,
                &channels,
                cursors,
                &failures,
                &behind,
                &mut heads_failing,
            )
            .await
        } else {
            BTreeSet::new()
        };
        for channel in &channels {
            if quiet.contains(channel) {
                continue;
            }
            let cursor = cursors.entry(channel.clone()).or_default();
            let failed = failures.entry(channel.clone()).or_insert(0);
            match poll_once(chat, hub, channel, limit, cursor).await {
                Ok(tick) => {
                    if *failed > 0 {
                        tracing::info!(
                            channel = %channel,
                            after_failures = *failed,
                            "live ingestion recovered for this channel"
                        );
                        *failed = 0;
                    }
                    if tick.published > 0 {
                        tracing::debug!(
                            channel = %channel,
                            published = tick.published,
                            "published live messages"
                        );
                    }
                    // Said out loud, and said once — on the transition, exactly as a failure is.
                    // A channel arriving faster than one tick reads it is not losing anything, but
                    // it IS delivering late, and a reader who cannot see that will read the delay
                    // as the bridge being broken.
                    let was_behind = behind.entry(channel.clone()).or_insert(false);
                    if tick.backlog && !*was_behind {
                        tracing::warn!(
                            channel = %channel,
                            pages = MAX_PAGES_PER_TICK,
                            limit,
                            "live ingestion is BEHIND this channel: one tick read its {} page \
                             ceiling and more was still waiting. Nothing is skipped — the cursor \
                             only moved past what was published — but delivery is running late \
                             until it catches up.",
                            MAX_PAGES_PER_TICK
                        );
                    } else if !tick.backlog && *was_behind {
                        tracing::info!(
                            channel = %channel,
                            "live ingestion caught up with this channel"
                        );
                    }
                    *was_behind = tick.backlog;
                    // `#307 slack-reply-live`. Thread replies left for the next tick, said once on
                    // the way in and once on the way out, as the page ceiling is.
                    let replies_were_behind =
                        replies_behind.entry(channel.clone()).or_insert(false);
                    if tick.replies_behind && !*replies_were_behind {
                        tracing::info!(
                            channel = %channel,
                            "live ingestion left thread replies in this channel for the next tick: \
                             more threads moved than one tick reads, or a read of one failed. They \
                             are published by a later tick; a thread whose reads keep failing is \
                             given up with a warning of its own."
                        );
                    } else if !tick.replies_behind && *replies_were_behind {
                        tracing::info!(
                            channel = %channel,
                            "live ingestion caught up with the thread replies in this channel"
                        );
                    }
                    *replies_were_behind = tick.replies_behind;
                }
                Err(error) => {
                    // ONCE at WARN, on the transition into failure. A channel that has been
                    // failing for an hour would otherwise write a line every interval, and a log
                    // that repeats itself is a log nobody reads the rest of.
                    if *failed == 0 {
                        tracing::warn!(
                            channel = %channel,
                            %error,
                            "live ingestion failed for this channel; backing off and retrying. \
                             The cursor is NOT advanced, so nothing is skipped once it recovers."
                        );
                    } else {
                        tracing::debug!(
                            channel = %channel,
                            %error,
                            failures = *failed,
                            "live ingestion still failing"
                        );
                    }
                    *failed = failed.saturating_add(1);
                    // Two slow-downs would be one too many, so they NEST: the client has already
                    // waited out whatever `Retry-After` it could afford (see
                    // `crate::discord::ratelimit`), and what reaches here is a limit that did not
                    // clear. Taking the larger of Discord's own outstanding request and this
                    // loop's doubling means the outer wait can be longer than Discord asked for
                    // but never shorter — which is the one direction that would keep hammering a
                    // limiter that has already said no.
                    let asked_for = error.retry_after().unwrap_or_default();
                    wait = wait
                        .max(backoff(interval, *failed, max_backoff))
                        .max(asked_for);
                }
            }
        }
        if let Some(left) = remaining.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                return;
            }
        }
        if full_reconciliation {
            next_reconciliation = Some(tokio::time::Instant::now() + interval);
        } else {
            next_hint_read =
                Some(tokio::time::Instant::now() + Duration::from_secs(MIN_POLL_SECONDS));
        }
        // A failing provider already asked this loop to back off. A hint must not turn that
        // protection into a hammering path, so wake-ups shorten only the ordinary healthy wait.
        if wait > interval {
            tokio::time::sleep(wait).await;
            continue;
        }
        let (Some(hint_hub), Some(hint_generation)) = (hint_hub, hint_generation.as_mut()) else {
            tokio::time::sleep(wait).await;
            continue;
        };
        let reconciliation_deadline =
            next_reconciliation.expect("a hint receiver always has a reconciliation deadline");
        hinted_only = wait_for_hints(
            hint_hub,
            hint_provider_key,
            hint_generation,
            reconciliation_deadline,
        )
        .await;
        if hinted_only.is_some() {
            tokio::time::sleep(Duration::from_millis(HINT_COALESCE_MILLIS)).await;
            let next_wake = next_hint_read
                .expect("a hint receiver always has a hint-read deadline")
                .min(reconciliation_deadline);
            tokio::time::sleep_until(next_wake).await;
            if tokio::time::Instant::now() >= reconciliation_deadline {
                // The full pass consumes every pending hint for this provider and reads every
                // channel, so prefer it once due instead of paying for a hinted read first.
                hinted_only = None;
            } else if let Some(hinted) = hinted_only.as_mut() {
                hinted.extend(hint_hub.take_hints(hint_provider_key));
            }
        }
    }
}

/// The channels of this pass whose page can be skipped, because their provider read their heads
/// natively and the newest message of each is not newer than its cursor. remote-ops-audit W12
/// `#287 bulk-reads`.
///
/// **Asked only of a provider that reads heads natively** ([`crate::chat::ReadCaps::heads`]): an
/// emulated head is a read of the channel, no cheaper than the page it would replace, so such a
/// provider is never asked and its channels are polled exactly as before. Of those channels, only
/// ones with a cursor (seeded), not failing (a failing channel keeps being read, so its recovery
/// and its backoff stay what they were), not behind its page ceiling and not one whose thread
/// replies its provider reads apart ([`ChatClient::polls_thread_replies`]) are asked about, all in
/// one call.
///
/// **What a skip can hide: nothing this loop would have seen.** A tick reads only what is NEWER
/// than the cursor, never an edit or a delete, and a head whose newest message is not newer than
/// the cursor proves nothing newer was added: a message created after it would have taken its
/// place. A head that cannot prove that is read as before: one that failed, one from a copy older
/// than now (`as_of`), and one whose id this loop cannot compare (not a snowflake). A head that
/// does not say the channel's newest message at all never gets here: the provider refuses that
/// channel's head instead (fix round 3 of remote-ops-audit W12 `#287 bulk-reads`), so `None` is
/// only ever the provider's word that the channel has no messages.
async fn quiet_channels(
    chat: &dyn ChatClient,
    hub: &LiveHub,
    channels: &[ChannelId],
    cursors: &BTreeMap<ChannelId, Option<u64>>,
    failures: &BTreeMap<ChannelId, u32>,
    behind: &BTreeMap<ChannelId, bool>,
    heads_failing: &mut bool,
) -> BTreeSet<ChannelId> {
    let candidates: Vec<ChannelId> = channels
        .iter()
        .filter(|channel| {
            chat.read_caps(channel).heads == crate::chat::Support::Native
                // `#307 slack-reply-live`: a head names the newest of the channel's own messages,
                // and says nothing of a reply this channel's provider reads apart.
                && !chat.polls_thread_replies(channel)
                // `#378 live-reactions`: a head says nothing of a reaction either, and a watched
                // channel is exactly where one must appear within seconds, so it is read.
                && !hub.is_watched(channel)
                && cursors.get(*channel).copied().flatten().is_some()
                && failures.get(*channel).copied().unwrap_or(0) == 0
                && !behind.get(*channel).copied().unwrap_or(false)
        })
        .cloned()
        .collect();
    if candidates.is_empty() {
        return BTreeSet::new();
    }
    let heads = chat.channel_heads(&candidates).await;
    let answered = heads.iter().filter(|(_, head)| head.is_ok()).count();
    if answered == 0 && !*heads_failing {
        if let Some((_, Err(error))) = heads.first() {
            tracing::warn!(
                %error,
                "channel heads failed for this provider; every channel is read as it was before \
                 heads until they answer again"
            );
        }
        *heads_failing = true;
    } else if answered > 0 && *heads_failing {
        tracing::info!("channel heads answer again for this provider");
        *heads_failing = false;
    }
    heads
        .into_iter()
        .filter_map(|(channel, head)| {
            let head = head.ok()?;
            let cursor = cursors.get(&channel).copied().flatten()?;
            let nothing_newer = match &head.newest_id {
                None => true,
                Some(id) => id.numeric().is_some_and(|newest| newest <= cursor),
            };
            (head.is_current() && nothing_newer).then_some(channel)
        })
        .collect()
}

/// Wait for either the ordinary reconciliation tick or at least one queued change hint.
///
/// The synchronous `take_hints` before `changed` closes both startup races: a hint queued before
/// this receiver existed is still found, and one queued between the take and the await changes
/// the generation and wakes it immediately. A stale generation with an already-drained set is a
/// harmless loop, not an extra provider read.
async fn wait_for_hints(
    hub: &LiveHub,
    provider_key: Option<&str>,
    generation: &mut watch::Receiver<u64>,
    reconciliation_deadline: tokio::time::Instant,
) -> Option<BTreeSet<ChannelId>> {
    let hinted = async {
        loop {
            let pending = hub.take_hints(provider_key);
            if !pending.is_empty() {
                return pending;
            }
            if generation.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        // When both become ready together, reconciliation wins. Random branch choice here would
        // put the non-starvation guarantee at the mercy of a hot hint stream.
        biased;
        () = tokio::time::sleep_until(reconciliation_deadline) => None,
        pending = hinted => Some(pending),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fake::FakeDiscord;
    use std::sync::Arc;

    fn hub_for(_channel: &ChannelId) -> LiveHub {
        LiveHub::new()
    }

    fn fake_with(channel: &ChannelId) -> Arc<FakeDiscord> {
        let fake = Arc::new(FakeDiscord::new());
        fake.register_channel(channel);
        fake
    }

    #[test]
    fn change_hints_coalesce_only_within_their_provider_partition() {
        let hub = LiveHub::new();
        let channel = ChannelId("1111".to_owned());
        let other = ChannelId("2222".to_owned());

        assert!(hub.queue_hint(Some("slack"), &channel));
        assert!(!hub.queue_hint(Some("slack"), &channel));
        assert!(hub.queue_hint(Some("other"), &channel));
        assert!(hub.queue_hint(None, &other));

        assert_eq!(
            hub.take_hints(Some("slack")),
            BTreeSet::from([channel.clone()])
        );
        assert!(hub.take_hints(Some("slack")).is_empty());
        assert_eq!(hub.take_hints(Some("other")), BTreeSet::from([channel]));
        assert_eq!(hub.take_hints(None), BTreeSet::from([other]));
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_reconciliation_deadline_wins_over_a_pending_hint() {
        let hub = LiveHub::new();
        let channel = ChannelId("1111".to_owned());
        let mut generation = hub.subscribe_hints();
        assert!(hub.queue_hint(None, &channel));

        assert!(
            wait_for_hints(&hub, None, &mut generation, tokio::time::Instant::now())
                .await
                .is_none(),
            "a ready hint won after the full reconciliation deadline was already due"
        );
        assert_eq!(
            hub.take_hints(None),
            BTreeSet::from([channel]),
            "the deadline path should leave the hint for the full pass to consume"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_hints_cannot_bypass_the_polling_rate_floor() {
        let interval = Duration::from_secs(3_600);
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        let hub = Arc::new(LiveHub::new());
        let task_fake = Arc::clone(&fake);
        let task_hub = Arc::clone(&hub);
        let task_channel = channel.clone();
        let poller = tokio::spawn(async move {
            let mut cursors = BTreeMap::new();
            poll_loop_with_channels(
                task_fake.as_ref(),
                task_hub.as_ref(),
                || vec![task_channel.clone()],
                &mut cursors,
                LoopConfig {
                    limit: 50,
                    interval,
                    ticks: Some(3),
                    hints: Some((task_hub.as_ref(), None)),
                },
            )
            .await;
        });

        while fake.fetch_count() < 1 {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
        assert!(hub.queue_hint(None, &channel));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(HINT_COALESCE_MILLIS)).await;
        while fake.fetch_count() < 2 {
            tokio::task::yield_now().await;
        }
        let first_hint_read = tokio::time::Instant::now();
        tokio::task::yield_now().await;

        assert!(hub.queue_hint(None, &channel));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(HINT_COALESCE_MILLIS)).await;
        let just_before_floor =
            first_hint_read + Duration::from_secs(MIN_POLL_SECONDS) - Duration::from_millis(1);
        tokio::time::advance(
            just_before_floor.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await;
        tokio::task::yield_now().await;
        assert_eq!(
            fake.fetch_count(),
            2,
            "a sustained hint stream bypassed the configured polling rate floor"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        poller.await.expect("poll task");
        assert_eq!(fake.fetch_count(), 3, "the deferred hint was never read");
    }

    #[tokio::test(start_paused = true)]
    async fn a_hint_cannot_shorten_failure_backoff() {
        let interval = Duration::from_secs(10);
        let channel = ChannelId("1111".to_owned());
        // An unregistered channel fails every authoritative read.
        let fake = Arc::new(FakeDiscord::new());
        let hub = Arc::new(LiveHub::new());
        let task_fake = Arc::clone(&fake);
        let task_hub = Arc::clone(&hub);
        let task_channel = channel.clone();
        let poller = tokio::spawn(async move {
            let mut cursors = BTreeMap::new();
            poll_loop_with_channels(
                task_fake.as_ref(),
                task_hub.as_ref(),
                || vec![task_channel.clone()],
                &mut cursors,
                LoopConfig {
                    limit: 50,
                    interval,
                    ticks: Some(2),
                    hints: Some((task_hub.as_ref(), None)),
                },
            )
            .await;
        });

        while fake.fetch_count() < 1 {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
        assert!(hub.queue_hint(None, &channel));
        tokio::time::advance(Duration::from_secs(19)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            fake.fetch_count(),
            1,
            "a hint retried a failing provider before its doubled backoff elapsed"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        poller.await.expect("poll task");
        assert_eq!(fake.fetch_count(), 2, "the provider was not retried");
    }

    #[tokio::test(start_paused = true)]
    async fn steady_hints_cannot_starve_periodic_reconciliation() {
        let interval = Duration::from_secs(10);
        let noisy = ChannelId("1111".to_owned());
        let quiet = ChannelId("2222".to_owned());
        let fake = Arc::new(FakeDiscord::new());
        fake.register_channel(&noisy);
        fake.register_channel(&quiet);
        let hub = Arc::new(LiveHub::new());
        let task_fake = Arc::clone(&fake);
        let task_hub = Arc::clone(&hub);
        let task_noisy = noisy.clone();
        let task_quiet = quiet.clone();
        let poller = tokio::spawn(async move {
            let mut cursors = BTreeMap::new();
            poll_loop_with_channels(
                task_fake.as_ref(),
                task_hub.as_ref(),
                || vec![task_noisy.clone(), task_quiet.clone()],
                &mut cursors,
                LoopConfig {
                    limit: 50,
                    interval,
                    ticks: Some(3),
                    hints: Some((task_hub.as_ref(), None)),
                },
            )
            .await;
        });

        while fake.fetch_count() < 2 {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(hub.queue_hint(None, &noisy));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(HINT_COALESCE_MILLIS)).await;
        while fake.fetch_count() < 3 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            fake.fetch_count(),
            3,
            "the hint should read only its noisy channel"
        );
        tokio::task::yield_now().await;

        // The original reconciliation deadline is still ten seconds after the startup pass,
        // not ten seconds after the hinted pass at 9.1 seconds. It must read BOTH channels now.
        tokio::time::advance(Duration::from_millis(900)).await;
        for _ in 0..100 {
            if poller.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            poller.is_finished(),
            "the hint moved the reconciliation deadline and starved the quiet channel"
        );
        poller.await.expect("poll task");
        assert_eq!(
            fake.fetch_count(),
            5,
            "the full pass did not read both channels"
        );
    }

    #[tokio::test]
    async fn the_first_tick_seeds_the_cursor_and_publishes_nothing() {
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        fake.seed(&channel, "codex", "said before anyone was listening");
        fake.seed(&channel, "codex", "and this too");
        let hub = hub_for(&channel);
        let mut cursor = None;

        let tick = poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
            .await
            .expect("poll");

        assert_eq!(
            tick.published, 0,
            "the first tick must publish nothing: existing history is not news"
        );
        assert!(cursor.is_some(), "the first tick must SEED the cursor");
        let subscription = hub.subscribe(&channel, None);
        assert!(
            subscription.replay.is_empty(),
            "a page attaching after the first tick must not be shown existing history as new"
        );
    }

    #[tokio::test]
    async fn a_message_seeded_between_ticks_is_published_exactly_once() {
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        fake.seed(&channel, "codex", "old");
        let hub = hub_for(&channel);
        let mut cursor = None;
        poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
            .await
            .expect("seed tick");

        fake.seed(&channel, "codex", "arrived");
        assert_eq!(
            poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
                .await
                .expect("poll")
                .published,
            1
        );
        assert_eq!(
            poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
                .await
                .expect("poll")
                .published,
            0,
            "a second tick with nothing new must publish nothing"
        );

        let subscription = hub.subscribe(&channel, None);
        assert_eq!(subscription.replay.len(), 1);
        assert_eq!(subscription.replay[0].message.content, "arrived");
    }

    #[tokio::test]
    async fn a_lower_snowflake_is_never_republished() {
        // The trap the cursor exists for: ordering must be NUMERIC. These two ids differ in
        // LENGTH, so the newer one is lexically SMALLER — a cursor comparison written on strings
        // publishes the old message forever and never publishes the new one.
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        let hub = hub_for(&channel);
        let older = fake.seed_at(&channel, "codex", "older", 1_420_070_400_001);
        let newer = fake.seed_at(&channel, "codex", "newer", 1_420_070_400_003);
        assert!(
            newer.as_str() < older.as_str() && newer.numeric() > older.numeric(),
            "this fixture is only a trap if the newer id is lexically smaller: {older} vs {newer}"
        );
        let mut cursor = older.numeric();

        let tick = poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
            .await
            .expect("poll");

        assert_eq!(
            tick.published, 1,
            "only the numerically newer message is new"
        );
        let subscription = hub.subscribe(&channel, None);
        assert_eq!(subscription.replay.len(), 1);
        assert_eq!(subscription.replay[0].message.content, "newer");
        assert_eq!(cursor, newer.numeric());
    }

    #[tokio::test]
    async fn a_failed_fetch_leaves_the_cursor_where_it_was() {
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        fake.seed(&channel, "codex", "old");
        let hub = hub_for(&channel);
        let mut cursor = None;
        poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
            .await
            .expect("seed tick");
        let seeded = cursor;

        fake.seed(&channel, "codex", "arrived while Discord was down");
        fake.fail_next("network down");
        assert!(poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
            .await
            .is_err());
        assert_eq!(cursor, seeded, "a failure must not advance the cursor");

        assert_eq!(
            poll_once(fake.as_ref(), &hub, &channel, 50, &mut cursor)
                .await
                .expect("recovered")
                .published,
            1,
            "what arrived during the failure must be published once it recovers, not skipped"
        );
    }

    #[tokio::test]
    async fn the_loop_survives_a_failure_and_keeps_polling() {
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        let hub = hub_for(&channel);
        fake.seed(&channel, "codex", "old");
        let mut receiver = hub.subscribe(&channel, None).receiver;
        let mut cursors = BTreeMap::new();
        let interval = Duration::from_millis(0);
        let channels = std::slice::from_ref(&channel);

        // One tick to seed the cursor.
        poll_loop(
            fake.as_ref(),
            &hub,
            channels,
            50,
            interval,
            &mut cursors,
            Some(1),
        )
        .await;
        let fetches_after_seed = fake.fetch_count();

        // Now something arrives WHILE Discord is failing, and the loop takes two more ticks: the
        // first fails, the second must still happen and must still publish. A loop that returned
        // on the error would leave the message undelivered forever.
        fake.seed(&channel, "codex", "after recovery");
        fake.fail_next("network down");
        poll_loop(
            fake.as_ref(),
            &hub,
            channels,
            50,
            interval,
            &mut cursors,
            Some(2),
        )
        .await;

        // Three fetches: the failed read, the recovered read, and that tick's reaction refresh
        // (`#378 live-reactions`) — this channel is watched (the receiver above), so it pays one.
        assert_eq!(
            fake.fetch_count() - fetches_after_seed,
            3,
            "the loop stopped fetching after the failure"
        );
        let got = receiver
            .try_recv()
            .expect("the message that arrived during the failure was never published");
        assert_eq!(got.message.content, "after recovery");
        assert!(
            receiver.try_recv().is_err(),
            "and it must be published ONCE, not once per tick"
        );
    }

    #[tokio::test]
    async fn removing_a_channel_discards_its_cursor_before_it_is_added_again() {
        let (state, fake) = crate::testing::state();
        let channel = ChannelId("3333333333333333333".to_owned());
        fake.seed(&channel, "codex", "present before the first poll");
        state
            .added_channels
            .write()
            .expect("added-channel allowlist")
            .push(crate::store::AddedChannel {
                channel: channel.clone(),
                label: "temporary".to_owned(),
                writable: false,
                registration_provider: None,
                added_at_ms: 1,
            });
        let mut cursors = BTreeMap::new();
        poll_state_loop(
            &state,
            None,
            state.chat.as_ref(),
            50,
            Duration::ZERO,
            &mut cursors,
            Some(1),
        )
        .await;
        assert!(
            cursors.contains_key(&channel),
            "the channel was never polled"
        );

        state
            .added_channels
            .write()
            .expect("added-channel allowlist")
            .clear();
        poll_state_loop(
            &state,
            None,
            state.chat.as_ref(),
            50,
            Duration::ZERO,
            &mut cursors,
            Some(1),
        )
        .await;
        assert!(
            !cursors.contains_key(&channel),
            "a removed channel kept its cursor"
        );

        fake.seed(&channel, "codex", "arrived while removed");
        state
            .added_channels
            .write()
            .expect("added-channel allowlist")
            .push(crate::store::AddedChannel {
                channel: channel.clone(),
                label: "temporary again".to_owned(),
                writable: false,
                registration_provider: None,
                added_at_ms: 2,
            });
        poll_state_loop(
            &state,
            None,
            state.chat.as_ref(),
            50,
            Duration::ZERO,
            &mut cursors,
            Some(1),
        )
        .await;
        assert!(
            state.live.subscribe(&channel, None).replay.is_empty(),
            "re-adding a channel published messages from while it was removed"
        );
    }

    #[tokio::test]
    async fn a_configured_channel_the_owner_hid_is_not_polled() {
        // `#199 removable-config-channels`. Hidden means off the list everywhere, and a poller
        // that kept reading the channel would still be spending the shared rate limit on it and
        // publishing to anyone who had a stream open.
        let (state, fake) = crate::testing::state();
        let hidden = ChannelId(crate::testing::WRITE_CHANNEL.to_owned());
        fake.seed(&hidden, "codex", "present before the first poll");
        state
            .hidden_channels
            .write()
            .expect("hidden-channel list")
            .insert(hidden.clone());
        let mut cursors = BTreeMap::new();
        poll_state_loop(
            &state,
            None,
            state.chat.as_ref(),
            50,
            Duration::ZERO,
            &mut cursors,
            Some(1),
        )
        .await;
        assert!(
            !cursors.contains_key(&hidden),
            "a hidden configured channel was polled"
        );
        assert!(
            cursors.contains_key(&ChannelId(crate::testing::READ_CHANNEL.to_owned())),
            "the channel still on the list was not polled"
        );
    }

    /// Run `ticks` ticks of the loop against `channel` and answer how long the loop WAITED.
    ///
    /// The clock is paused by the caller, so this is exact rather than approximate: tokio advances
    /// virtual time to the next timer deadline whenever everything is idle, and nothing here is
    /// idle for any other reason.
    async fn elapsed_over(
        fake: &FakeDiscord,
        hub: &LiveHub,
        channel: &ChannelId,
        interval: Duration,
        ticks: u32,
    ) -> Duration {
        let mut cursors = BTreeMap::new();
        let started = tokio::time::Instant::now();
        poll_loop(
            fake,
            hub,
            std::slice::from_ref(channel),
            50,
            interval,
            &mut cursors,
            Some(ticks),
        )
        .await;
        started.elapsed()
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_channel_really_backs_off_and_a_healthy_one_is_not_slowed_down() {
        // THE HEADLINE SAFETY CLAIM OF `#44 live-push`, and the one that had no test: the module
        // doc, the README and the commit body all say a failing channel backs off rather than
        // hammering a rate limit this server does not handle. Deleting the single line in
        // `poll_loop` that applies `backoff` changed NOTHING else in this suite — the loop still
        // fetched, still published, still recovered, still logged. So the assertion has to be
        // about the one observable that changes: how long the loop actually waited.
        let interval = Duration::from_secs(10);

        let healthy = ChannelId("1111".to_owned());
        let good = fake_with(&healthy);
        let hub = LiveHub::new();
        let quiet = elapsed_over(good.as_ref(), &hub, &healthy, interval, 3).await;
        assert_eq!(
            quiet,
            interval * 2,
            "three ticks of a healthy channel are two ordinary sleeps and nothing else"
        );

        // Never registered, so every fetch answers Discord's own 404 — a channel that keeps
        // failing rather than one that fails once.
        let broken = ChannelId("2222".to_owned());
        let bad = Arc::new(FakeDiscord::new());
        let noisy = elapsed_over(bad.as_ref(), &hub, &broken, interval, 3).await;

        // 20s after the first failure and 40s after the second: doubling, as `backoff` says.
        assert_eq!(
            noisy,
            Duration::from_secs(60),
            "a channel that keeps failing must be retried FURTHER apart each time; polling on the \
             ordinary interval through an outage is the request storm the interval floor and this \
             backoff exist together to prevent"
        );
        assert!(
            noisy > quiet,
            "a failing channel waited no longer than a healthy one: the backoff is not applied"
        );
        assert!(
            bad.fetch_count() >= 3,
            "and it must still be RETRIED — backing off is not giving up"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_the_client_could_not_clear_slows_this_loop_by_what_discord_asked_for() {
        // The seam that keeps this loop's backoff and the client's `Retry-After` from being two
        // rival notions of "slow down". The client waits out what it can afford; what arrives
        // here is a limit it could NOT clear, still carrying Discord's outstanding request. This
        // loop must not come back sooner than that — its own doubling says 20s after one failure,
        // and 20s is less than Discord asked for.
        //
        // Coming back early is not merely impolite. The client refuses to SEND before the window
        // opens, so an early tick does not become a request; it becomes a tick that accomplishes
        // nothing and fails again, and the channel stays dark until some later tick happens to
        // land after the window. Waiting the time Discord actually named is what makes the very
        // next tick the one that recovers.
        let interval = Duration::from_secs(10);
        assert_eq!(
            backoff(interval, 1, interval * MAX_BACKOFF_INTERVALS),
            Duration::from_secs(20),
            "the doubling backoff alone would come back well before Discord is ready"
        );

        let channel = ChannelId("3333".to_owned());
        // One attempt and almost no waiting budget, so the 429 surfaces here with its wait intact
        // rather than being slept out inside the client. This is the shape of a real long limit:
        // longer than any one caller should be held for.
        let fake = Arc::new(FakeDiscord::with_retry_policy(
            crate::discord::ratelimit::RetryPolicy {
                max_attempts: 1,
                max_total_wait: Duration::from_secs(1),
            },
        ));
        fake.register_channel(&channel);
        fake.seed(&channel, "codex", "said before anyone was listening");
        fake.rate_limit_next(1, Duration::from_secs(45), false);

        let hub = LiveHub::new();
        let elapsed = elapsed_over(fake.as_ref(), &hub, &channel, interval, 2).await;
        assert_eq!(
            elapsed,
            Duration::from_secs(45),
            "the loop came back before Discord said it could: the two waits are fighting rather \
             than nesting"
        );
        assert_eq!(
            fake.fetch_count(),
            2,
            "and the tick after the wait must actually READ — a loop that wakes early is held by \
             the client and recovers nothing"
        );
    }

    #[tokio::test]
    async fn a_burst_larger_than_one_page_is_published_in_full_rather_than_silently_dropped() {
        // The silent-loss bug this walk exists to rule out. Reading the most recent `limit`
        // messages and then moving the cursor to the newest of them drops everything in between
        // whenever a channel produces more than `limit` between two ticks: no gap event, no WARN,
        // and a page and an agent quietly missing lines while the README says nothing is skipped.
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        let hub = hub_for(&channel);
        let mut cursor = None;
        fake.seed(&channel, "codex", "before anyone was listening");
        poll_once(fake.as_ref(), &hub, &channel, 5, &mut cursor)
            .await
            .expect("seed tick");

        // Twelve messages, a page of five: two and a bit pages, well inside the tick ceiling.
        for n in 0..12 {
            fake.seed(&channel, "codex", &format!("burst {n}"));
        }
        let tick = poll_once(fake.as_ref(), &hub, &channel, 5, &mut cursor)
            .await
            .expect("poll");

        assert_eq!(
            tick.published, 12,
            "everything that arrived between two ticks must be published, not just the last page"
        );
        assert!(
            !tick.backlog,
            "twelve messages is not a backlog at four pages"
        );
        let replay = hub.subscribe(&channel, None).replay;
        assert_eq!(
            replay
                .iter()
                .map(|m| m.message.content.clone())
                .collect::<Vec<_>>(),
            (0..12).map(|n| format!("burst {n}")).collect::<Vec<_>>(),
            "and in order, oldest first, with none missing from the middle"
        );
        assert_eq!(
            poll_once(fake.as_ref(), &hub, &channel, 5, &mut cursor)
                .await
                .expect("poll")
                .published,
            0,
            "walking forward must not leave the cursor behind and republish the burst"
        );
    }

    #[tokio::test]
    async fn a_tick_stopped_by_its_page_ceiling_says_so_and_the_next_tick_continues() {
        // The honest bound. The walk is capped so that catching up cannot become a request storm,
        // and being capped has to be VISIBLE — otherwise "nothing is skipped" and "delivery is an
        // interval late" are the same silence.
        let channel = ChannelId("1111".to_owned());
        let fake = fake_with(&channel);
        let hub = hub_for(&channel);
        let mut cursor = None;
        poll_once(fake.as_ref(), &hub, &channel, 2, &mut cursor)
            .await
            .expect("seed tick");

        let ceiling = MAX_PAGES_PER_TICK * 2;
        for n in 0..(ceiling + 3) {
            fake.seed(&channel, "codex", &format!("m{n}"));
        }
        let first = poll_once(fake.as_ref(), &hub, &channel, 2, &mut cursor)
            .await
            .expect("poll");
        assert_eq!(first.published, ceiling, "the tick is bounded at {ceiling}");
        assert!(
            first.backlog,
            "a tick that stopped at its ceiling with more waiting must SAY it is behind"
        );

        let second = poll_once(fake.as_ref(), &hub, &channel, 2, &mut cursor)
            .await
            .expect("poll");
        assert_eq!(
            second.published, 3,
            "and the next tick continues from the cursor rather than skipping the remainder"
        );
        assert!(!second.backlog, "which is then no longer behind");
    }

    #[test]
    fn the_backoff_grows_and_then_stops_growing() {
        let interval = Duration::from_secs(10);
        let max = Duration::from_secs(160);
        assert_eq!(backoff(interval, 0, max), interval, "no failures, no delay");
        assert_eq!(backoff(interval, 1, max), Duration::from_secs(20));
        assert_eq!(backoff(interval, 2, max), Duration::from_secs(40));
        assert_eq!(
            backoff(interval, 30, max),
            max,
            "a channel that has been down all night must still be retried every {max:?}"
        );
    }

    #[test]
    fn the_replay_tail_is_bounded() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        for n in 0..(REPLAY_TAIL + 20) {
            hub.publish(&channel, message(&channel, 1_000 + n as u64, "spam"));
        }
        let subscription = hub.subscribe(&channel, None);
        assert_eq!(subscription.replay.len(), REPLAY_TAIL);
        assert_eq!(
            subscription.replay[0].message.id.as_str(),
            (1_000 + 20).to_string(),
            "the OLDEST entries are the ones dropped"
        );
    }

    #[test]
    fn adapter_retry_deduplication_is_bounded_and_evicts_oldest_first() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        for n in 0..=INGEST_DEDUPE_MEMORY {
            assert!(hub.publish_ingested(
                format!("event-{n}"),
                LiveKind::Create,
                false,
                &channel,
                message(&channel, n as u64 + 1, "m"),
            ));
        }
        assert!(
            hub.publish_ingested(
                "event-0".to_owned(),
                LiveKind::Create,
                false,
                &channel,
                message(&channel, 1, "retried after the window"),
            ),
            "the oldest id must eventually leave the bounded de-duplication window"
        );
        assert!(
            !hub.publish_ingested(
                format!("event-{INGEST_DEDUPE_MEMORY}"),
                LiveKind::Create,
                false,
                &channel,
                message(&channel, INGEST_DEDUPE_MEMORY as u64 + 1, "recent retry"),
            ),
            "a recent retry must still be suppressed"
        );
    }

    #[test]
    fn subscribing_after_an_id_replays_only_what_is_strictly_newer() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        for id in [10_u64, 20, 30] {
            hub.publish(&channel, message(&channel, id, "m"));
        }
        let subscription = hub.subscribe(&channel, Some(&MessageId("20".to_owned())));
        assert_eq!(
            subscription
                .replay
                .iter()
                .map(|m| m.message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["30"],
            "the message the caller already has must not come back"
        );
    }

    #[test]
    fn any_cursor_missing_from_the_tail_forces_a_reset_instead_of_resuming_over_a_gap() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        hub.publish(&channel, message(&channel, 10, "m"));
        for missing in ["garbage", "9", "11"] {
            let subscription = hub.subscribe(&channel, Some(&MessageId(missing.to_owned())));
            assert!(
                subscription.replay.is_empty(),
                "missing cursor {missing:?} cannot identify a safe replay boundary"
            );
            assert!(
                subscription.reset_on_attach,
                "missing cursor {missing:?} must force an authoritative re-read"
            );
        }
    }

    #[test]
    fn an_evicted_numeric_cursor_resets_instead_of_replaying_a_partial_tail() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        for id in 1..=(REPLAY_TAIL as u64 + 1) {
            hub.publish(&channel, message(&channel, id, "m"));
        }

        let subscription = hub.subscribe(&channel, Some(&MessageId("1".to_owned())));
        assert!(subscription.replay.is_empty());
        assert!(
            subscription.reset_on_attach,
            "the numeric comparison cannot reveal the event evicted between the cursor and tail"
        );
    }

    #[test]
    fn one_channels_messages_never_reach_another_channels_subscriber() {
        // The hub is keyed by channel, and it has to STAY keyed: a single shared feed would
        // deliver a private channel's text to a page watching a different one, and every other
        // test here would still pass.
        let hub = LiveHub::new();
        let watched = ChannelId("1111".to_owned());
        let other = ChannelId("2222".to_owned());
        let mut receiver = hub.subscribe(&watched, None).receiver;
        hub.publish(&other, message(&other, 10, "somewhere else"));
        assert!(
            receiver.try_recv().is_err(),
            "a message published to another channel reached this subscriber"
        );
        assert!(
            hub.subscribe(&watched, None).replay.is_empty(),
            "and it must not be in this channel's replay tail either"
        );
    }

    #[test]
    fn a_message_this_server_posted_is_published_marked_as_such() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        hub.note_self_posted(&MessageId("10".to_owned()));
        hub.publish(&channel, message(&channel, 10, "our own reply"));
        hub.publish(&channel, message(&channel, 11, "somebody else"));
        let subscription = hub.subscribe(&channel, None);
        assert_eq!(
            subscription
                .replay
                .iter()
                .map(|m| m.self_posted)
                .collect::<Vec<_>>(),
            vec![true, false],
            "without this flag the agent hears its own reply and answers it"
        );
    }

    #[test]
    fn the_self_posted_memory_is_bounded_and_forgets_the_oldest_first() {
        let hub = LiveHub::new();
        for n in 0..(SELF_POSTED_MEMORY + 5) {
            hub.note_self_posted(&MessageId(n.to_string()));
        }
        assert!(!hub.is_self_posted(&MessageId("0".to_owned())));
        assert!(hub.is_self_posted(&MessageId((SELF_POSTED_MEMORY + 4).to_string())));
    }

    #[test]
    fn the_stream_of_every_channel_resumes_only_from_a_cursor_the_tail_still_covers() {
        // `#337 voice-relay-silent`. The boundaries of `subscribe_all`, every channel in one order.
        let hub = LiveHub::new();
        let one = ChannelId("1111".to_owned());
        let two = ChannelId("2222".to_owned());
        let fresh = hub.subscribe_all(None);
        assert_eq!(fresh.start, Some(0));
        assert!(fresh.replay.is_empty() && !fresh.reset_on_attach);
        let total = ALL_TAIL as u64 + 3;
        for n in 1..=total {
            let channel = if n % 2 == 0 { &one } else { &two };
            hub.publish(channel, message(channel, n, &format!("m{n}")));
        }
        let fresh = hub.subscribe_all(None);
        assert_eq!(
            fresh.start,
            Some(total),
            "a fresh attach starts at the newest event"
        );
        assert!(fresh.replay.is_empty(), "and replays none of the past");

        // The oldest event still held is `total - ALL_TAIL + 1`: a cursor just before it resumes.
        let oldest = total - ALL_TAIL as u64 + 1;
        let resumed = hub.subscribe_all(Some(&hub.all_cursor(oldest - 1)));
        assert!(!resumed.reset_on_attach);
        assert_eq!(resumed.start, None);
        assert_eq!(resumed.replay.len(), ALL_TAIL);
        assert_eq!(resumed.replay.first().map(|a| a.seq), Some(oldest));
        let channels: BTreeSet<_> = resumed.replay.iter().map(|a| a.channel.clone()).collect();
        assert_eq!(channels, BTreeSet::from([one.clone(), two.clone()]));
        // One before that, and its successor is gone: a gap, said as a reset.
        assert!(
            hub.subscribe_all(Some(&hub.all_cursor(oldest - 2)))
                .reset_on_attach
        );
        // The newest cursor resumes with nothing to replay; one past it is not this hub's to resume.
        let newest = hub.subscribe_all(Some(&hub.all_cursor(total)));
        assert!(!newest.reset_on_attach && newest.replay.is_empty());
        assert!(
            hub.subscribe_all(Some(&hub.all_cursor(total + 1)))
                .reset_on_attach
        );
        // Another process's cursor, or one that is not a cursor at all, is reset too.
        for foreign in ["0.1", "not a cursor", "", "."] {
            assert!(
                hub.subscribe_all(Some(foreign)).reset_on_attach,
                "{foreign:?}"
            );
        }
        assert_ne!(
            LiveHub::new().all_cursor(1),
            hub.all_cursor(1),
            "two hubs name their cursors apart"
        );
    }

    #[test]
    fn the_reaction_memory_tells_a_first_reaction_from_a_first_sight() {
        // `#378 live-reactions`. The owner's case is the FIRST reaction on a message already
        // read, and the removal of the last one: both must read as changes, while a message
        // never seen before is learned silently.
        let channel = ChannelId("1111".to_owned());
        let hub = LiveHub::new();
        let thumbs = |count: u32| {
            Some(vec![Reaction {
                emoji: "👍".to_owned(),
                custom: false,
                custom_id: None,
                count,
                owner_reacted: false,
            }])
        };
        // Never seen: learned, not announced, whatever it carries.
        let mut fresh = message(&channel, 9, "new");
        fresh.reactions = thumbs(1);
        assert!(!hub.reactions_changed(&channel, &mut fresh, true));
        // Seen with no answer, as every seeded message is: its first set is a change.
        let mut known = message(&channel, 5, "ship it");
        hub.note_reactions(&channel, &known);
        known.reactions = thumbs(1);
        assert!(hub.reactions_changed(&channel, &mut known.clone(), true));
        // The same set again is not.
        assert!(!hub.reactions_changed(&channel, &mut known.clone(), true));
        // A copy that cannot see reactions neither announces nor downgrades…
        let mut echo = message(&channel, 5, "ship it");
        hub.note_reactions(&channel, &echo);
        assert!(!hub.reactions_changed(&channel, &mut known.clone(), true));
        // …but a READ copy reporting nothing, from a read that reports reactions where they
        // exist, is the removal, stamped empty. From a read that cannot say, it is nothing.
        assert!(!hub.reactions_changed(&channel, &mut echo.clone(), false));
        assert!(hub.reactions_changed(&channel, &mut echo, true));
        assert_eq!(echo.reactions, Some(Vec::new()));
        assert!(!hub.reactions_changed(&channel, &mut echo.clone(), true));
    }

    /// A provider that reads replies apart, reports one reaction change on its live read, and
    /// fails every plain page: the burst-then-failure shape of `#378 live-reactions`.
    struct ApartStub {
        channel: ChannelId,
    }

    #[async_trait::async_trait]
    impl ChatClient for ApartStub {
        async fn identity(&self) -> Result<crate::chat::ChatIdentity, ChatError> {
            Err(ChatError::Refused("stub".to_owned()))
        }
        async fn fetch_page(
            &self,
            _channel: &ChannelId,
            _limit: u16,
            _before: Option<&MessageId>,
            _after: Option<&MessageId>,
        ) -> Result<Vec<Message>, ChatError> {
            Err(ChatError::Refused("page down".to_owned()))
        }
        async fn fetch_recent(
            &self,
            _channel: &ChannelId,
            _limit: u16,
        ) -> Result<Vec<Message>, ChatError> {
            Ok(vec![message(&self.channel, 5, "ship it")])
        }
        async fn post_message(
            &self,
            _channel: &ChannelId,
            _content: &str,
            _reply_to: Option<&MessageId>,
        ) -> Result<Message, ChatError> {
            Err(ChatError::Refused("stub".to_owned()))
        }
        fn polls_thread_replies(&self, _channel: &ChannelId) -> bool {
            true
        }
        async fn fetch_live(
            &self,
            _channel: &ChannelId,
            limit: u16,
            after: Option<&MessageId>,
        ) -> Result<LiveRead, ChatError> {
            let first = message(&self.channel, 5, "ship it");
            match after {
                None => Ok(LiveRead {
                    messages: vec![first],
                    ..LiveRead::default()
                }),
                Some(_) => {
                    let mut reacted = first.clone();
                    reacted.reactions = Some(vec![Reaction {
                        emoji: "👍".to_owned(),
                        custom: false,
                        custom_id: None,
                        count: 1,
                        owner_reacted: false,
                    }]);
                    Ok(LiveRead {
                        // A full page, so the tick walks on to the page that fails.
                        messages: (0..u64::from(limit))
                            .map(|i| message(&self.channel, 6 + i, "burst"))
                            .collect(),
                        changed: vec![reacted],
                        ..LiveRead::default()
                    })
                }
            }
        }
    }

    #[tokio::test]
    async fn a_page_failure_after_a_live_read_still_publishes_its_reaction_changes() {
        // `#378 live-reactions`. The provider advanced its own memory when it reported the
        // change, so a tick that then fails must not take the update down with it.
        let channel = ChannelId("1111".to_owned());
        let stub = ApartStub {
            channel: channel.clone(),
        };
        let hub = LiveHub::new();
        let mut receiver = hub.subscribe(&channel, None).receiver;
        let mut cursor = None;
        poll_once(&stub, &hub, &channel, 2, &mut cursor)
            .await
            .expect("the seed reads");
        let failed = poll_once(&stub, &hub, &channel, 2, &mut cursor).await;
        assert!(failed.is_err(), "the second page fails the tick");
        let mut kinds = Vec::new();
        while let Ok(live) = receiver.try_recv() {
            kinds.push((live.kind, live.message.id.as_str().to_owned()));
        }
        assert!(
            kinds.contains(&(LiveKind::Update, "5".to_owned())),
            "the reaction update went out before the error: {kinds:?}"
        );
    }

    fn message(channel: &ChannelId, id: u64, content: &str) -> Message {
        Message {
            edited_timestamp: None,
            thread: None,
            id: MessageId(id.to_string()),
            channel_id: channel.clone(),
            author: "codex".to_owned(),
            author_id: crate::model::UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "2026-08-20T10:00:00+00:00".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: content.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
        }
    }
}
