//! Inbound Discord ingestion, and the fan-out that lets a page be TOLD rather than have to ask.
//!
//! Everything else in this crate is pulled: a question arrives, a channel is read, an answer goes
//! back. This module is the one place that runs without being asked, so the two decisions behind
//! it are written down here rather than left to be inferred from the code.
//!
//! # Decision one: ingestion is bounded POLLING, not a Gateway connection
//!
//! Discord's real-time mechanism is the Gateway, and this server does not use it. It keeps a
//! per-channel snowflake cursor, seeds it with
//! [`crate::discord::DiscordClient::fetch_recent`], and thereafter walks forward from it with
//! [`crate::discord::DiscordClient::fetch_page`]'s `after` on an interval.
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
//! **The Gateway is the upgrade path, behind this same seam.** Everything above this module sees
//! [`LiveHub`]: a channel-keyed publish/subscribe with a replay tail. A Gateway implementation
//! would publish into exactly that and delete [`poll_forever`]; no route, no page and no test
//! above the hub would change.
//!
//! ## What polling costs, stated plainly
//!
//! It multiplies this server's Discord request volume by one request per channel per interval,
//! forever, whether or not anybody is listening. The README's Known gaps already record that
//! Discord's rate limits are NOT handled here and that a 429 surfaces to the caller as HTTP 502.
//! So ingestion is **off unless configured** (`discord.live_poll_seconds`, default 0), the
//! interval has a floor the configuration refuses to go below, and a failing channel backs off
//! rather than hammering — see [`backoff`].
//!
//! **The backoff is not decoration, and it is not left to be read.** [`poll_loop`] sleeping longer
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
//! A message arriving in Discord should be able to reach a live voice conversation as a
//! `contextual_update`. Someone has to hold the socket that carries it, and today that is the
//! browser: this server only mints a signed URL (`GET /api/v1/signed-url`) and holds no
//! per-conversation vendor state at all.
//!
//! It stays that way. Moving the socket server-side would turn gent-talk into an always-connected,
//! BILLED conversation holder — a process whose cost accrues while nobody is in the car — and it
//! would put third-party channel text on a vendor socket that no human is currently looking at.
//!
//! **The cost of that decision, said out loud: contextual updates reach the agent only while the
//! page is open.** Close the tab and the channel keeps moving, this server keeps ingesting, and
//! the agent hears nothing until somebody opens `/voice` again and asks. That is a real
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

use tokio::sync::broadcast;

use crate::discord::{DiscordClient, DiscordError};
use crate::model::{sort_oldest_first, ChannelId, Message, MessageId};

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

/// How many self-posted ids are remembered, across all channels.
///
/// Bounded because it is a de-duplication window, not a record: the only thing that ever consults
/// it is a message arriving from a poll seconds after this server posted it.
pub const SELF_POSTED_MEMORY: usize = 256;

/// The smallest polling interval the configuration will accept, in seconds.
///
/// Not a preference. One request per channel per interval goes against a rate limit this server
/// does not yet handle, so an operator who types `1` should be refused with a reason rather than
/// quietly given a request storm.
pub const MIN_POLL_SECONDS: u64 = 5;

/// One message, as it is published to live subscribers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LiveMessage {
    /// The message itself. UNTRUSTED: written by whoever is in the channel.
    pub message: Message,
    /// Whether THIS SERVER posted it, through [`crate::ops::reply`].
    ///
    /// The page must not relay these into a live conversation; see the module documentation.
    pub self_posted: bool,
}

/// What a subscriber gets when it attaches.
pub struct Subscription {
    /// Messages already published that are newer than the caller's cursor, oldest first.
    pub replay: Vec<LiveMessage>,
    /// Everything published from the instant of attachment onwards.
    pub receiver: broadcast::Receiver<LiveMessage>,
}

struct Feed {
    sender: broadcast::Sender<LiveMessage>,
    tail: VecDeque<LiveMessage>,
}

/// The fan-out: one publisher per channel, each with a bounded replay tail.
///
/// Feeds are created on first use rather than from the configured channel list, and the map is
/// still bounded, because **every path into this type is already allowlist-gated**:
/// [`crate::ops::watch`] resolves the channel against the configuration before subscribing, and
/// the poll loop only ever visits the configured channels. A hub that carried its own copy of the
/// allowlist would be a second definition of "which channels exist" — and two of those is how one
/// of them goes stale.
#[derive(Default)]
pub struct LiveHub {
    feeds: Mutex<BTreeMap<ChannelId, Feed>>,
    self_posted: Mutex<(VecDeque<MessageId>, BTreeSet<MessageId>)>,
}

impl LiveHub {
    /// An empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Publish one message to `channel`, and remember it for replay.
    ///
    /// A publish with no subscribers is NOT a failure: the tail is what a page attaching a moment
    /// later reads, so the message is kept either way.
    pub fn publish(&self, channel: &ChannelId, message: Message) {
        let self_posted = self.is_self_posted(&message.id);
        let live = LiveMessage {
            message,
            self_posted,
        };
        let mut feeds = lock(&self.feeds);
        let feed = feeds.entry(channel.clone()).or_insert_with(new_feed);
        feed.tail.push_back(live.clone());
        while feed.tail.len() > REPLAY_TAIL {
            feed.tail.pop_front();
        }
        let _ = feed.sender.send(live);
    }

    /// Attach to a channel, replaying what was published after `after`.
    ///
    /// **The receiver is created BEFORE the tail is read, under the same lock.** That ordering is
    /// the whole correctness of this function: attaching afterwards leaves a window in which a
    /// message is published after the tail was copied and before the receiver exists, and it is
    /// gone — the one outcome the recovery story is supposed to rule out. Holding the lock across
    /// both is what makes it atomic with respect to [`LiveHub::publish`].
    ///
    /// `after` is a message id the caller has already seen. Only strictly-newer held messages come
    /// back; an `after` that does not parse as a snowflake replays the whole tail, because the
    /// alternative — silently replaying nothing — looks identical to "you are up to date".
    #[must_use]
    pub fn subscribe(&self, channel: &ChannelId, after: Option<&MessageId>) -> Subscription {
        let mut feeds = lock(&self.feeds);
        let feed = feeds.entry(channel.clone()).or_insert_with(new_feed);
        let receiver = feed.sender.subscribe();
        let cursor = after.and_then(MessageId::numeric);
        let replay = feed
            .tail
            .iter()
            .filter(|held| match (cursor, held.message.id.numeric()) {
                (Some(seen), Some(id)) => id > seen,
                _ => true,
            })
            .cloned()
            .collect();
        Subscription { replay, receiver }
    }
}

fn new_feed() -> Feed {
    let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
    Feed {
        sender,
        tail: VecDeque::new(),
    }
}

/// The SSE event name every arriving message carries.
pub const EVENT_MESSAGE: &str = "message";

/// The SSE event name that ends a stream a subscriber fell too far behind on.
///
/// A page that receives this must RE-READ the channel through `/messages`. It is deliberately not
/// a resumption: the messages that fell out of the broadcast buffer are gone from this stream, and
/// resuming short would be the silent drop this whole design exists to rule out.
pub const EVENT_RESET: &str = "reset";

/// Turn one [`Subscription`] into the body of a Server-Sent Events response.
///
/// The replay tail goes out first, oldest first, then everything live. Each event carries
/// `id: <message id>` so a reconnecting browser's `Last-Event-ID` has something to send back, and
/// `event: message` so the page branches on the type rather than on the payload's shape.
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
pub fn events(
    subscription: Subscription,
) -> impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
{
    struct State {
        replay: VecDeque<LiveMessage>,
        receiver: broadcast::Receiver<LiveMessage>,
        done: bool,
    }
    let start = State {
        replay: subscription.replay.into_iter().collect(),
        receiver: subscription.receiver,
        done: false,
    };
    futures_util::stream::unfold(start, |mut state| async move {
        if let Some(held) = state.replay.pop_front() {
            return Some((Ok(message_event(&held, true)), state));
        }
        if state.done {
            return None;
        }
        match state.receiver.recv().await {
            Ok(live) => Some((Ok(message_event(&live, false)), state)),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                state.done = true;
                Some((Ok(reset_event(missed)), state))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    })
}

fn message_event(live: &LiveMessage, replayed: bool) -> axum::response::sse::Event {
    let event = axum::response::sse::Event::default()
        .id(live.message.id.as_str())
        .event(EVENT_MESSAGE);
    event
        .json_data(serde_json::json!({
            "message": live.message,
            "self_posted": live.self_posted,
            // Whether this came out of the replay tail rather than off the wire just now. See
            // [`events`]: the page renders both and announces only the second.
            "replayed": replayed,
            // The SAME standing reminder `MessagesResponse` carries. A pushed message is
            // third-party text exactly as a fetched one is, and a stream is not a reason for the
            // boundary to go quiet.
            "untrusted_content_notice": crate::untrusted::NOTICE,
        }))
        // A `Message` is plain data and cannot fail to serialize. If that ever stops being true,
        // ending the stream with a reset is the honest answer: the page re-reads and sees the
        // message, rather than being handed a frame it cannot parse.
        .unwrap_or_else(|_| reset_event(0))
}

fn reset_event(missed: u64) -> axum::response::sse::Event {
    axum::response::sse::Event::default()
        .event(EVENT_RESET)
        .data(format!(
            "{{\"missed\":{missed},\"detail\":\"this subscriber fell behind; re-read the \
             channel rather than resuming\"}}"
        ))
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
/// # Errors
///
/// [`DiscordError`] from the fetch. **The cursor is not advanced past anything that was not
/// published**, so the next successful tick publishes everything that arrived in the meantime
/// rather than skipping it. It IS advanced past what this tick already published before the
/// failure, because republishing that would relay the same message into a paid conversation twice.
pub async fn poll_once(
    discord: &dyn DiscordClient,
    hub: &LiveHub,
    channel: &ChannelId,
    limit: u16,
    cursor: &mut Option<u64>,
) -> Result<Tick, DiscordError> {
    let Some(seen) = *cursor else {
        let messages = discord.fetch_recent(channel, limit).await?;
        // Seed. `unwrap_or(0)` rather than leaving the cursor unset: an EMPTY channel is a
        // perfectly good starting point, and leaving it `None` would make the next tick seed
        // again and publish nothing for as long as the channel stayed quiet.
        *cursor = Some(
            messages
                .iter()
                .filter_map(|m| m.id.numeric())
                .max()
                .unwrap_or(0),
        );
        return Ok(Tick::default());
    };

    let page_size = usize::from(limit.max(1));
    let mut published = 0;
    let mut at = seen;
    let mut backlog = false;
    for page in 0..MAX_PAGES_PER_TICK {
        let after = MessageId(at.to_string());
        let mut messages = match discord.fetch_page(channel, limit, None, Some(&after)).await {
            Ok(messages) => messages,
            Err(error) => {
                // Whatever went out in an earlier page of this tick really went out. Rewinding to
                // where the tick started would republish it, and the page cannot tell a
                // republished message from a new one once it has scrolled away.
                *cursor = Some(at);
                return Err(error);
            }
        };
        sort_oldest_first(&mut messages);
        let full = messages.len() >= page_size;
        let before = at;
        for message in messages {
            let Some(id) = message.id.numeric() else {
                continue;
            };
            if id <= at {
                continue;
            }
            at = id;
            hub.publish(channel, message);
            published += 1;
        }
        // `at == before` means the page held nothing this cursor can move past — an empty answer,
        // or one made entirely of ids that do not parse. Walking again would fetch the same page
        // forever.
        if !full || at == before {
            break;
        }
        if page + 1 == MAX_PAGES_PER_TICK {
            backlog = true;
        }
    }
    *cursor = Some(at);
    Ok(Tick { published, backlog })
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

/// Poll every channel forever, publishing into `hub`.
///
/// Never returns and never gives up. A channel that fails backs off and is tried again; it does
/// not take the loop down with it, and it does not advance its own cursor, so nothing is skipped
/// once it recovers. Spawned by `main` and cancelled only by the process exiting.
pub async fn poll_forever(
    discord: std::sync::Arc<dyn DiscordClient>,
    hub: std::sync::Arc<LiveHub>,
    channels: Vec<ChannelId>,
    limit: u16,
    interval: Duration,
) {
    let mut cursors = BTreeMap::new();
    poll_loop(
        discord.as_ref(),
        hub.as_ref(),
        &channels,
        limit,
        interval,
        &mut cursors,
        None,
    )
    .await;
}

/// The body of [`poll_forever`], with the cursors handed in and a tick budget.
///
/// `ticks: None` is the production case and never returns. The cursors are the caller's so that a
/// test can stop the loop, change the channel underneath it, and start it again WHERE IT LEFT OFF
/// — which is the only way to state "an error did not lose the cursor" as an assertion rather than
/// as a comment.
async fn poll_loop(
    discord: &dyn DiscordClient,
    hub: &LiveHub,
    channels: &[ChannelId],
    limit: u16,
    interval: Duration,
    cursors: &mut BTreeMap<ChannelId, Option<u64>>,
    ticks: Option<u32>,
) {
    let max_backoff = interval.saturating_mul(MAX_BACKOFF_INTERVALS);
    let mut failures: BTreeMap<ChannelId, u32> = BTreeMap::new();
    let mut behind: BTreeMap<ChannelId, bool> = BTreeMap::new();
    let mut remaining = ticks;
    loop {
        let mut wait = interval;
        for channel in channels {
            let cursor = cursors.entry(channel.clone()).or_default();
            let failed = failures.entry(channel.clone()).or_insert(0);
            match poll_once(discord, hub, channel, limit, cursor).await {
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
                    wait = wait.max(backoff(interval, *failed, max_backoff));
                }
            }
        }
        if let Some(left) = remaining.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                return;
            }
        }
        tokio::time::sleep(wait).await;
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

        assert_eq!(
            fake.fetch_count() - fetches_after_seed,
            2,
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
    fn a_cursor_that_is_not_a_snowflake_replays_everything_rather_than_nothing() {
        let channel = ChannelId("1111".to_owned());
        let hub = hub_for(&channel);
        hub.publish(&channel, message(&channel, 10, "m"));
        let subscription = hub.subscribe(&channel, Some(&MessageId("garbage".to_owned())));
        assert_eq!(
            subscription.replay.len(),
            1,
            "replaying nothing would be indistinguishable from being up to date"
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

    fn message(channel: &ChannelId, id: u64, content: &str) -> Message {
        Message {
            id: MessageId(id.to_string()),
            channel_id: channel.clone(),
            author: "codex".to_owned(),
            author_id: crate::model::UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "2026-08-20T10:00:00+00:00".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: content.to_owned(),
        }
    }
}
