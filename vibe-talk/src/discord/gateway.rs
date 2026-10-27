//! Discord's Gateway, for reaction changes only. `#578 discord-live-reactions`.
//!
//! A reaction added in Discord used to reach an open page only when something read the message
//! again: the page's own re-read, or, with `live_poll_seconds` set, the poller's bounded re-read
//! of a watched channel's newest window each tick. That is seconds to minutes, and nothing at all
//! for an older message or a thread reply outside that window. Discord PUSHES reaction changes
//! over its Gateway; this module listens for them.
//!
//! # What it holds, and what it does not
//!
//! One Gateway session per Discord provider with `gateway_reactions = true`, identified with two
//! intents and no others: `GUILD_MESSAGE_REACTIONS` and `DIRECT_MESSAGE_REACTIONS` ([`INTENTS`]).
//! Neither is privileged, so the application needs nothing switched on in the developer portal,
//! and no message text ever crosses this connection: Discord sends message content only under the
//! privileged `MESSAGE_CONTENT` intent, which this does not ask for. New messages, edits and
//! deletions still arrive the way they always did.
//!
//! # What an event does
//!
//! `MESSAGE_REACTION_ADD`, `MESSAGE_REACTION_REMOVE`, `MESSAGE_REACTION_REMOVE_ALL` and
//! `MESSAGE_REACTION_REMOVE_EMOJI` each name a message and say how its reactions moved. Rather
//! than keep a second copy of every message's tallies and apply deltas to it, which drifts the
//! first time an event is missed, the message is READ AGAIN over REST
//! ([`crate::chat::ChatClient::fetch_messages`], one `GET /channels/{id}/messages/{id}`) and the
//! copy Discord returns is published with [`crate::live::LiveHub::publish_update`], exactly as
//! the poller publishes a reaction change it found. So the page gets the authoritative counts,
//! through the held window, the message cache and the replay tail like any other update.
//!
//! Events for one message are gathered for [`Timing::coalesce`] first, so a burst of reactions
//! costs one read, not one each; the REST client obeys Discord's 429s
//! ([`super::ratelimit`]). A read that fails is tried again, waiting [`Timing::retry`] and
//! doubling, [`READ_ATTEMPTS`] times in all; after that the channel is handed back to the poller's
//! own reaction refresh until the next fresh session, so a failure delays a change, never loses
//! it where the poller runs. A reaction in a thread names the thread, which this server knows by
//! its parent: the thread's parent is read once (`GET /channels/{thread}`) and remembered. An
//! event in a channel that is neither configured nor a thread of one is ignored.
//!
//! What Discord's REST copy cannot say is whether the OWNER reacted: its `me` is the bot. The
//! events can, because each names the user. So the reactions the configured `owner_user_id` makes
//! and takes back are remembered here ([`mark_owner_reactions`]) and every Discord read marks
//! them, REST reads included, so a later re-read never takes the mark away. A reaction the owner
//! made before this process started is not known to be his, and reads as anybody's, which is
//! what `owner_reacted = false` already means ([`crate::model::Reaction::owner_reacted`]); so do
//! the marks a session task learned once it starts a fresh session, since a gap Discord did not
//! replay may have taken any of them back.
//!
//! # Staying connected
//!
//! Heartbeats every `heartbeat_interval`, the first after a random fraction of it. A heartbeat
//! Discord did not acknowledge before the next is due means a dead connection, which is closed
//! and resumed. A session resumes (op 6, at `resume_gateway_url`) after a dropped connection, a
//! Reconnect (op 7), a resumable Invalid Session, and every close code Discord documents as
//! reconnectable; Discord then replays what was missed, so nothing needs reconciling. It
//! identifies afresh after an Invalid Session that cannot resume, close codes 4007 and 4009, and
//! three failed attempts to reach the resume address. Events in that gap are lost, so after
//! every fresh session each watched channel's newest window is read and diffed against what the
//! hub last saw ([`crate::live::LiveHub::reactions_changed`]), as one poller tick would.
//!
//! Close codes Discord marks as not reconnectable (4004 a refused token, 4010 to 4014 shards,
//! version, intents) stop this task with an error in the log; the poller goes on as before.
//!
//! Identifying is the expensive step: Discord allows 1000 a day per token, and RESETS THE TOKEN
//! of an application that goes past that. So every identify is preceded by `GET /gateway/bot`,
//! whose `session_start_limit` says how many remain (none left waits for `reset_after`), it waits
//! at least [`Timing::identify_wait`], and a connection that ends before it was
//! [`Timing::stable`] doubles the next wait, up to [`Timing::max_wait`].
//!
//! While a session is up, its provider's channels are marked on the hub
//! ([`crate::live::LiveHub::set_reactions_pushed`]) and the poller spends no reaction refresh on
//! them. The connection marks them and clears them itself, so the mark goes the moment the
//! connection does, whatever the reaction side is busy with. A resume that fails before
//! RESUMED, however it fails, counts towards the three that give the session up.
//!
//! The connection is made directly, never through an HTTP proxy (three fresh sessions in a row
//! that cannot open their socket say so once, at ERROR), and speaks JSON without compression.
//! A Close Discord sends before its Hello is read like any other.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::chat::{ChatClient, ChatError, Lookup};
use crate::config::{DiscordConfig, Secret};
use crate::live::LiveHub;
use crate::model::{ChannelId, MessageId, Reaction};

use super::http::{HttpDiscordClient, MAX_REACTION_CHARS};

/// `GUILD_MESSAGE_REACTIONS`: reaction events in server channels and their threads.
pub const GUILD_MESSAGE_REACTIONS: u64 = 1 << 10;

/// `DIRECT_MESSAGE_REACTIONS`: reaction events in direct messages.
pub const DIRECT_MESSAGE_REACTIONS: u64 = 1 << 13;

/// Every intent this session identifies with. Neither is privileged.
pub const INTENTS: u64 = GUILD_MESSAGE_REACTIONS | DIRECT_MESSAGE_REACTIONS;

/// The Gateway version asked for, the same as the REST API's (`/api/v10`).
pub const GATEWAY_VERSION: &str = "10";

/// How many messages the owner's own reactions are remembered for.
pub const OWNER_MEMORY: usize = 4096;

/// How many channel ids' thread parents (or lack of one) are remembered.
pub const PARENT_MEMORY: usize = 1024;

/// How many consecutive failures to reach a resume address give the session up.
const RESUME_ATTEMPTS: u32 = 3;

/// The close code this client closes with when it means to resume: anything but 1000 and 1001,
/// which would end the session. In the range the WebSocket standard leaves to applications.
const CLOSE_TO_RESUME: u16 = 4900;

/// The waits this module keeps. [`Timing::default`] is what the server uses; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How long events for one message are gathered before it is read again.
    pub coalesce: Duration,
    /// The least wait before reconnecting to resume.
    pub resume_wait: Duration,
    /// The least wait before identifying again. Discord admits one identify per five seconds
    /// for an application of this size (`max_concurrency` 1).
    pub identify_wait: Duration,
    /// The most a run of failing connections waits between attempts.
    pub max_wait: Duration,
    /// How long a session must last for its end not to count as a failure.
    pub stable: Duration,
    /// Deadline for connecting and receiving Hello.
    pub connect_timeout: Duration,
    /// The first wait before reading a message again after a read failed; it doubles for each
    /// of the [`READ_ATTEMPTS`] retries.
    pub retry: Duration,
}

/// How many times a message whose reactions moved is read before its channel is handed back to
/// the poller's own reaction refresh until the next fresh session.
pub const READ_ATTEMPTS: u32 = 5;

/// The longest `heartbeat_interval` believed, in milliseconds (Discord's is about 41 seconds).
const MAX_HEARTBEAT_MS: u64 = 600_000;

/// How many fresh sessions in a row that never open their socket earn one ERROR naming the
/// likeliest cause.
const UNREACHABLE_BEFORE_ERROR: u32 = 3;

impl Default for Timing {
    fn default() -> Self {
        Self {
            coalesce: Duration::from_millis(150),
            resume_wait: Duration::from_secs(1),
            identify_wait: Duration::from_secs(5),
            max_wait: Duration::from_secs(600),
            stable: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(20),
            retry: Duration::from_secs(1),
        }
    }
}

/// How one reaction event moved a message's reactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReactionChange {
    /// `MESSAGE_REACTION_ADD`: `user` reacted with `emoji`.
    Added {
        /// Who reacted.
        user: String,
        /// The emoji's key ([`emoji_key`]).
        emoji: String,
    },
    /// `MESSAGE_REACTION_REMOVE`: `user` took `emoji` back.
    Removed {
        /// Who took it back.
        user: String,
        /// The emoji's key.
        emoji: String,
    },
    /// `MESSAGE_REACTION_REMOVE_ALL`: every reaction was cleared.
    RemovedAll,
    /// `MESSAGE_REACTION_REMOVE_EMOJI`: every reaction with `emoji` was cleared.
    RemovedEmoji {
        /// The emoji's key.
        emoji: String,
    },
}

/// One reaction event: which message, in which Discord channel (a thread's own id for a reply in
/// one), and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionEvent {
    /// The channel the event names.
    pub channel: String,
    /// The message the event names.
    pub message: String,
    /// What happened.
    pub change: ReactionChange,
}

/// Read a dispatch as a reaction event, when it is one. Anything else, or a reaction event
/// without the fields it needs, is `None`.
#[must_use]
pub fn parse_reaction_event(name: &str, data: &Value) -> Option<ReactionEvent> {
    let text = |field: &str| {
        data.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let emoji = || data.get("emoji").and_then(emoji_key);
    let change = match name {
        "MESSAGE_REACTION_ADD" => ReactionChange::Added {
            user: text("user_id")?,
            emoji: emoji()?,
        },
        "MESSAGE_REACTION_REMOVE" => ReactionChange::Removed {
            user: text("user_id")?,
            emoji: emoji()?,
        },
        "MESSAGE_REACTION_REMOVE_ALL" => ReactionChange::RemovedAll,
        "MESSAGE_REACTION_REMOVE_EMOJI" => ReactionChange::RemovedEmoji { emoji: emoji()? },
        _ => return None,
    };
    Some(ReactionEvent {
        channel: text("channel_id")?,
        message: text("message_id")?,
        change,
    })
}

/// The key one emoji is known by: a custom emoji's id, or a standard emoji itself, bounded as a
/// read bounds it ([`super::http::parse_reactions`]) so the two always agree.
#[must_use]
pub fn emoji_key(emoji: &Value) -> Option<String> {
    let text = |field: &str| {
        emoji
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(MAX_REACTION_CHARS).collect::<String>())
    };
    text("id").or_else(|| text("name"))
}

/// The key a read's reaction is known by, as [`emoji_key`] names an event's.
fn reaction_key(reaction: &Reaction) -> &str {
    reaction.custom_id.as_deref().unwrap_or(&reaction.emoji)
}

/// The owner's reactions the Gateway has seen, per message id: process-wide, because every
/// Discord read marks them and a read has only the message to go on. Message ids are snowflakes,
/// unique across Discord, and a bridge's ids never match one. Each entry remembers which session
/// task learned it ([`Source`]), so a task that loses track (a gap it cannot resume) forgets its
/// own entries and nobody else's.
///
/// What it cannot tell apart, said plainly: two Discord providers in one deployment that read the
/// same message share its marks, which is right while they name the same owner (one deployment
/// has one); and a super-reaction is not told from a normal one, so taking back one of the two
/// takes the mark while the other may remain.
struct OwnerMemory {
    order: VecDeque<String>,
    reacted: BTreeMap<String, (Source, BTreeSet<String>)>,
}

/// Which session task learned an owner mark: one number per [`run`].
pub type Source = u64;

static NEXT_SOURCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

static OWNER_REACTIONS: Mutex<OwnerMemory> = Mutex::new(OwnerMemory {
    order: VecDeque::new(),
    reacted: BTreeMap::new(),
});

fn owner_memory() -> MutexGuard<'static, OwnerMemory> {
    OWNER_REACTIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Mark the reactions on `message_id` that the owner is known to have made. Called for every
/// Discord message read; a message the Gateway never saw him react to is left as read.
pub(crate) fn mark_owner_reactions(message_id: &str, reactions: &mut [Reaction]) {
    let memory = owner_memory();
    let Some((_, keys)) = memory.reacted.get(message_id) else {
        return;
    };
    for reaction in reactions {
        if keys.contains(reaction_key(reaction)) {
            reaction.owner_reacted = true;
        }
    }
}

/// Remember what `event`, seen by the session task `source`, says about the owner's own
/// reactions. `owner` is the provider's `owner_user_id`; without one, only clears are
/// remembered, and there is nothing to clear.
pub fn note_owner_reaction(event: &ReactionEvent, owner: Option<&str>, source: Source) {
    let mut memory = owner_memory();
    let OwnerMemory { order, reacted } = &mut *memory;
    let message = &event.message;
    match &event.change {
        ReactionChange::Added { user, emoji } if owner == Some(user.as_str()) => {
            let (learned, keys) = reacted.entry(message.clone()).or_insert_with(|| {
                order.push_back(message.clone());
                (source, BTreeSet::new())
            });
            *learned = source;
            keys.insert(emoji.clone());
            while order.len() > OWNER_MEMORY {
                if let Some(evicted) = order.pop_front() {
                    reacted.remove(&evicted);
                }
            }
        }
        ReactionChange::Removed { user, emoji } if owner == Some(user.as_str()) => {
            forget_owner_emoji(order, reacted, message, emoji);
        }
        ReactionChange::RemovedEmoji { emoji } => {
            forget_owner_emoji(order, reacted, message, emoji);
        }
        ReactionChange::RemovedAll => {
            if reacted.remove(message).is_some() {
                order.retain(|held| held != message);
            }
        }
        ReactionChange::Added { .. } | ReactionChange::Removed { .. } => {}
    }
}

/// Forget every owner mark `source` learned: after a gap it could not resume, any of them may
/// have been taken back unseen, and "not known to be his" is what a forgotten mark reads as.
pub fn forget_owner_reactions(source: Source) {
    let mut memory = owner_memory();
    let OwnerMemory { order, reacted } = &mut *memory;
    reacted.retain(|_, (learned, _)| *learned != source);
    order.retain(|message| reacted.contains_key(message));
}

fn forget_owner_emoji(
    order: &mut VecDeque<String>,
    reacted: &mut BTreeMap<String, (Source, BTreeSet<String>)>,
    message: &str,
    emoji: &str,
) {
    let Some((_, keys)) = reacted.get_mut(message) else {
        return;
    };
    keys.remove(emoji);
    if keys.is_empty() {
        reacted.remove(message);
        order.retain(|held| held != message);
    }
}

/// The channels of the provider a session serves, asked afresh each time: channels can be added
/// and hidden while it runs.
pub type Channels = Arc<dyn Fn() -> Vec<ChannelId> + Send + Sync>;

/// Hold a Gateway session for `config`'s provider and publish its reaction changes into `hub`
/// until a close code Discord marks as final, or the token is refused. `chat` is the provider's
/// client as the rest of the server uses it, for the re-reads; `channels` lists its channels.
pub async fn run(
    config: &DiscordConfig,
    hub: Arc<LiveHub>,
    chat: Arc<dyn ChatClient>,
    channels: Channels,
    timing: Timing,
) {
    let rest = match HttpDiscordClient::new(config) {
        Ok(rest) => Arc::new(rest),
        Err(error) => {
            tracing::error!(%error, "the Discord Gateway cannot start: its client could not be built");
            return;
        }
    };
    let official = crate::config::is_official_discord_api_base(&config.api_base);
    let (signals, received) = mpsc::unbounded_channel();
    let pushed = Arc::new(Pushed {
        hub: Arc::clone(&hub),
        channels: Arc::clone(&channels),
        held: Mutex::new(Vec::new()),
    });
    let sync = ReactionSync {
        hub,
        chat,
        rest: Arc::clone(&rest),
        channels,
        pushed: Arc::clone(&pushed),
        owner: config.owner_user_id.clone(),
        source: NEXT_SOURCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        timing,
        parents: BTreeMap::new(),
    };
    let syncing = tokio::spawn(sync.run(received));
    drive(
        &rest,
        &config.bot_token,
        official,
        &signals,
        &pushed,
        &timing,
    )
    .await;
    drop(signals);
    let _ = syncing.await;
}

/// The channels marked on the hub as pushed by this session task. Marked by the connection the
/// moment a session is ready and cleared the moment it ends, never behind the reaction side's
/// queue; cleared too when the last holder goes, an aborted task included.
struct Pushed {
    hub: Arc<LiveHub>,
    channels: Channels,
    held: Mutex<Vec<ChannelId>>,
}

impl Pushed {
    fn held(&self) -> MutexGuard<'_, Vec<ChannelId>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn mark(&self) {
        let now = (self.channels)();
        let mut held = self.held();
        self.hub.set_reactions_pushed(&held, false);
        self.hub.set_reactions_pushed(&now, true);
        *held = now;
    }

    fn clear(&self) {
        let mut held = self.held();
        self.hub.set_reactions_pushed(&held, false);
        held.clear();
    }

    /// Hand `channel` back to the poller's reaction refresh until the next session is ready.
    fn release(&self, channel: &ChannelId) {
        let mut held = self.held();
        held.retain(|known| known != channel);
        self.hub
            .set_reactions_pushed(std::slice::from_ref(channel), false);
    }
}

impl Drop for Pushed {
    fn drop(&mut self) {
        self.clear();
    }
}

/// What a session was, for resuming it.
struct Session {
    id: String,
    resume_url: String,
}

/// What the session task tells the reaction side.
#[derive(Debug)]
enum Signal {
    /// A reaction event.
    Event(ReactionEvent),
    /// A session is up: a fresh one (`resumed` false), whose gap needs reconciling, or a resumed
    /// one, whose gap Discord replayed.
    Up { resumed: bool },
}

/// How a connection ended, and so what the next one does.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    /// Resume the session, when there is one.
    Resume,
    /// Start a new session.
    Identify,
    /// Never reached Hello.
    Unreachable,
    /// Stop: Discord said not to reconnect.
    Fatal(String),
}

/// What Discord's close code says to do next. The table is Discord's own (Gateway Close Event
/// Codes): 4004 and 4010 to 4014 are not reconnectable, 4007 and 4009 need a new session, and
/// every other code, an unknown one included, resumes.
fn after_close(code: u16) -> Ending {
    match code {
        4004 => Ending::Fatal("Discord refused the bot token (close code 4004)".to_owned()),
        4010..=4012 => Ending::Fatal(format!(
            "Discord refused the session's shard or version (close code {code})"
        )),
        4013 | 4014 => Ending::Fatal(format!(
            "Discord refused the reaction intents (close code {code}); see the README's \
             Discord live reactions section"
        )),
        4007 | 4009 => Ending::Identify,
        _ => Ending::Resume,
    }
}

/// The address to open for `raw`, a Gateway or resume URL Discord gave: its scheme checked, and
/// the version and encoding this client speaks asked for. Plain `ws://` only when the API base is
/// not Discord's own, which is a test's fake.
fn socket_url(raw: &str, official: bool) -> Option<String> {
    let mut url = reqwest::Url::parse(raw).ok()?;
    match url.scheme() {
        "wss" => {}
        "ws" if !official => {}
        _ => return None,
    }
    url.set_query(Some(&format!("v={GATEWAY_VERSION}&encoding=json")));
    Some(url.into())
}

/// Why no Gateway address could be had.
enum AddressRefusal {
    /// Try again after the usual wait.
    Retry(String),
    /// No session starts are left until `Duration` has passed.
    Exhausted(Duration),
    /// Stop.
    Fatal(String),
}

/// `GET /gateway/bot`, read for the address to identify at and the session starts left.
async fn gateway_address(
    rest: &HttpDiscordClient,
    official: bool,
) -> Result<String, AddressRefusal> {
    let value = match rest.gateway_bot().await {
        Ok(value) => value,
        Err(error) => {
            return Err(match error.cause() {
                ChatError::Status { status: 401, .. } => AddressRefusal::Fatal(
                    "Discord refused the bot token reading GET /gateway/bot (401)".to_owned(),
                ),
                _ => AddressRefusal::Retry(error.to_string()),
            });
        }
    };
    if let Some(limit) = value.get("session_start_limit") {
        if limit.get("remaining").and_then(Value::as_u64) == Some(0) {
            let reset = limit
                .get("reset_after")
                .and_then(Value::as_u64)
                .unwrap_or(60_000);
            return Err(AddressRefusal::Exhausted(Duration::from_millis(
                reset.clamp(1_000, 86_400_000),
            )));
        }
    }
    value
        .get("url")
        .and_then(Value::as_str)
        .and_then(|raw| socket_url(raw, official))
        .ok_or_else(|| {
            AddressRefusal::Retry("GET /gateway/bot named no usable Gateway address".to_owned())
        })
}

/// How long to wait before the next connection: `floor` after a healthy one, doubling with each
/// failure in a row, never more than `max`.
fn wait(failures: u32, floor: Duration, max: Duration) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    if failures == 0 {
        floor
    } else {
        floor.saturating_mul(1 << doublings).min(max.max(floor))
    }
}

/// Connect, and keep connecting, until told to stop.
async fn drive(
    rest: &HttpDiscordClient,
    token: &Secret,
    official: bool,
    signals: &mpsc::UnboundedSender<Signal>,
    pushed: &Pushed,
    timing: &Timing,
) {
    let mut session: Option<Session> = None;
    let mut seq: Option<u64> = None;
    let mut failures = 0_u32;
    let mut unreachable_resumes = 0_u32;
    let mut unreachable_fresh = 0_u32;
    loop {
        let url = match &session {
            Some(session) => session.resume_url.clone(),
            None => match gateway_address(rest, official).await {
                Ok(url) => url,
                Err(AddressRefusal::Fatal(reason)) => {
                    tracing::error!(%reason, "the Discord Gateway stopped; reactions arrive by polling only");
                    return;
                }
                Err(AddressRefusal::Exhausted(until)) => {
                    tracing::warn!(
                        wait_seconds = until.as_secs(),
                        "Discord says this bot token has no Gateway session starts left today; \
                         waiting for them to reset rather than risk the token"
                    );
                    tokio::time::sleep(until).await;
                    continue;
                }
                Err(AddressRefusal::Retry(reason)) => {
                    failures = failures.saturating_add(1);
                    let pause = wait(failures, timing.identify_wait, timing.max_wait);
                    tracing::warn!(%reason, wait_ms = pause.as_millis(), "could not find the Discord Gateway");
                    tokio::time::sleep(pause).await;
                    continue;
                }
            },
        };
        let started = Instant::now();
        let resuming = session.is_some();
        let (ending, ready) = connection(
            &url,
            token,
            official,
            &mut session,
            &mut seq,
            signals,
            pushed,
            timing,
        )
        .await;
        if ready {
            pushed.clear();
        }
        if !resuming {
            if ending == Ending::Unreachable {
                unreachable_fresh += 1;
                if unreachable_fresh == UNREACHABLE_BEFORE_ERROR {
                    tracing::error!(
                        "Discord answered GET /gateway/bot but its Gateway WebSocket could not be \
                         opened {UNREACHABLE_BEFORE_ERROR} times in a row. The WebSocket is \
                         opened directly, never through HTTP_PROXY or HTTPS_PROXY; a host that \
                         reaches the internet only through a proxy cannot hold the session, and \
                         reactions arrive by polling only"
                    );
                }
            } else {
                unreachable_fresh = 0;
            }
        }
        match &ending {
            Ending::Fatal(reason) => {
                tracing::error!(%reason, "the Discord Gateway stopped; reactions arrive by polling only");
                return;
            }
            Ending::Identify => {
                session = None;
                seq = None;
                unreachable_resumes = 0;
            }
            // A resume that never got as far as RESUMED counts as a failed one, however it ended.
            Ending::Resume | Ending::Unreachable if resuming && !ready => {
                unreachable_resumes += 1;
                if unreachable_resumes >= RESUME_ATTEMPTS {
                    session = None;
                    seq = None;
                    unreachable_resumes = 0;
                }
            }
            Ending::Resume | Ending::Unreachable => unreachable_resumes = 0,
        }
        if ready && started.elapsed() >= timing.stable {
            failures = 0;
        } else {
            failures = failures.saturating_add(1);
        }
        let floor = if session.is_some() {
            timing.resume_wait
        } else {
            timing.identify_wait
        };
        let pause = wait(failures, floor, timing.max_wait);
        tracing::info!(
            ?ending,
            resuming = session.is_some(),
            wait_ms = pause.as_millis(),
            "the Discord Gateway connection ended; reconnecting"
        );
        tokio::time::sleep(pause).await;
    }
}

/// A fraction of one in `[0, 1)`, for the first heartbeat's offset.
fn jitter() -> f64 {
    let mut bytes = [0_u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        return 0.5;
    }
    f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0)
}

fn identify(token: &Secret) -> Value {
    json!({
        "op": 2,
        "d": {
            "token": token.expose(),
            "intents": INTENTS,
            "properties": {
                "os": std::env::consts::OS,
                "browser": "vibe-talk",
                "device": "vibe-talk",
            },
        },
    })
}

fn heartbeat(seq: Option<u64>) -> Frame {
    Frame::text(json!({ "op": 1, "d": seq }).to_string())
}

fn close_to_resume() -> Frame {
    Frame::Close(Some(CloseFrame {
        code: CloseCode::from(CLOSE_TO_RESUME),
        reason: "".into(),
    }))
}

/// One connection, from connecting to its end. The `bool` says whether it reached a ready
/// session (READY or RESUMED), so that its end is worth announcing.
#[allow(clippy::too_many_arguments)]
async fn connection(
    url: &str,
    token: &Secret,
    official: bool,
    session: &mut Option<Session>,
    seq: &mut Option<u64>,
    signals: &mpsc::UnboundedSender<Signal>,
    pushed: &Pushed,
    timing: &Timing,
) -> (Ending, bool) {
    let socket = match tokio::time::timeout(
        timing.connect_timeout,
        tokio_tungstenite::connect_async(url),
    )
    .await
    {
        Ok(Ok((socket, _response))) => socket,
        Ok(Err(error)) => {
            tracing::warn!(%error, "could not connect to the Discord Gateway");
            return (Ending::Unreachable, false);
        }
        Err(_) => {
            tracing::warn!("connecting to the Discord Gateway timed out");
            return (Ending::Unreachable, false);
        }
    };
    let (mut sink, mut stream) = socket.split();
    // `Err` carries a close code Discord sent before its Hello, when it sent one.
    let hello = async {
        while let Some(Ok(frame)) = stream.next().await {
            let text = match frame {
                Frame::Text(text) => text,
                Frame::Close(frame) => return Err(frame.map(|frame| u16::from(frame.code))),
                _ => continue,
            };
            let Ok(payload) = serde_json::from_str::<Value>(text.as_str()) else {
                continue;
            };
            if payload.get("op").and_then(Value::as_u64) == Some(10) {
                return payload
                    .get("d")
                    .and_then(|data| data.get("heartbeat_interval"))
                    .and_then(Value::as_u64)
                    .ok_or(None);
            }
        }
        Err(None)
    };
    let interval = match tokio::time::timeout(timing.connect_timeout, hello).await {
        Ok(Ok(milliseconds)) => Duration::from_millis(milliseconds.clamp(1, MAX_HEARTBEAT_MS)),
        Ok(Err(Some(code))) => {
            let ending = after_close(code);
            if matches!(ending, Ending::Fatal(_)) {
                return (ending, false);
            }
            tracing::warn!(code, "the Discord Gateway closed before its Hello");
            return (Ending::Unreachable, false);
        }
        _ => {
            tracing::warn!("the Discord Gateway sent no Hello");
            return (Ending::Unreachable, false);
        }
    };
    let opening = match session.as_ref() {
        Some(session) => json!({
            "op": 6,
            "d": { "token": token.expose(), "session_id": session.id, "seq": *seq },
        }),
        None => identify(token),
    };
    if sink.send(Frame::text(opening.to_string())).await.is_err() {
        return (Ending::Unreachable, false);
    }
    let mut ready = false;
    let mut acknowledged = true;
    let mut beat = Instant::now() + interval.mul_f64(jitter());
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(beat) => {
                if !acknowledged {
                    tracing::warn!("the Discord Gateway did not acknowledge a heartbeat; reconnecting");
                    let _ = sink.send(close_to_resume()).await;
                    return (Ending::Resume, ready);
                }
                if sink.send(heartbeat(*seq)).await.is_err() {
                    return (Ending::Resume, ready);
                }
                acknowledged = false;
                beat += interval;
            }
            frame = stream.next() => {
                let text = match frame {
                    Some(Ok(Frame::Text(text))) => text,
                    Some(Ok(Frame::Close(frame))) => {
                        let ending = frame.map_or(Ending::Resume, |frame| after_close(frame.code.into()));
                        return (ending, ready);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return (Ending::Resume, ready),
                };
                let Ok(payload) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                match payload.get("op").and_then(Value::as_u64) {
                    Some(0) => {
                        if let Some(at) = payload.get("s").and_then(Value::as_u64) {
                            *seq = Some(at);
                        }
                        let name = payload.get("t").and_then(Value::as_str).unwrap_or_default();
                        let data = payload.get("d").unwrap_or(&Value::Null);
                        match name {
                            "READY" => {
                                *session = ready_session(data, official);
                                ready = true;
                                pushed.mark();
                                tracing::info!(
                                    resumable = session.is_some(),
                                    "Discord Gateway session ready: reaction changes are pushed"
                                );
                                let _ = signals.send(Signal::Up { resumed: false });
                            }
                            "RESUMED" => {
                                ready = true;
                                pushed.mark();
                                tracing::info!("Discord Gateway session resumed");
                                let _ = signals.send(Signal::Up { resumed: true });
                            }
                            _ => {
                                if let Some(event) = parse_reaction_event(name, data) {
                                    let _ = signals.send(Signal::Event(event));
                                }
                            }
                        }
                    }
                    // Discord asking for a heartbeat now.
                    Some(1) => {
                        if sink.send(heartbeat(*seq)).await.is_err() {
                            return (Ending::Resume, ready);
                        }
                    }
                    // Reconnect.
                    Some(7) => {
                        let _ = sink.send(close_to_resume()).await;
                        return (Ending::Resume, ready);
                    }
                    // Invalid Session: `d` says whether it may resume.
                    Some(9) => {
                        let resumable = payload.get("d").and_then(Value::as_bool) == Some(true);
                        let _ = sink.send(close_to_resume()).await;
                        let ending = if resumable && session.is_some() {
                            Ending::Resume
                        } else {
                            Ending::Identify
                        };
                        return (ending, ready);
                    }
                    Some(11) => acknowledged = true,
                    _ => {}
                }
            }
        }
    }
}

/// The session READY describes, when it says enough to resume one.
fn ready_session(data: &Value, official: bool) -> Option<Session> {
    let id = data.get("session_id").and_then(Value::as_str)?;
    let resume_url = data
        .get("resume_gateway_url")
        .and_then(Value::as_str)
        .and_then(|raw| socket_url(raw, official))?;
    Some(Session {
        id: id.to_owned(),
        resume_url,
    })
}

/// The reaction side: gathers events, reads the messages they name, publishes what moved.
struct ReactionSync {
    hub: Arc<LiveHub>,
    chat: Arc<dyn ChatClient>,
    rest: Arc<HttpDiscordClient>,
    channels: Channels,
    pushed: Arc<Pushed>,
    owner: Option<String>,
    /// This task's mark on the owner reactions it learns.
    source: Source,
    timing: Timing,
    /// Each non-configured channel id an event named: the configured channel it is a thread of,
    /// or `None` when it is not a thread.
    parents: BTreeMap<String, Option<ChannelId>>,
}

/// A message waiting to be read again.
#[derive(Clone, Copy, Debug)]
struct Pending {
    /// When its first event arrived.
    first: Instant,
    /// When it is read.
    due: Instant,
    /// How many reads of it have failed.
    failed: u32,
}

/// What reading a message again came to.
enum Refreshed {
    /// Published, or nothing to publish: gone, or not one of this server's channels.
    Done,
    /// The read failed; `channel` is where it would have been published, when that is known.
    Failed(Option<ChannelId>),
}

impl ReactionSync {
    async fn run(mut self, mut signals: mpsc::UnboundedReceiver<Signal>) {
        let mut pending: BTreeMap<(String, String), Pending> = BTreeMap::new();
        loop {
            let due = pending.values().map(|waiting| waiting.due).min();
            let signal = match due {
                Some(due) => tokio::select! {
                    signal = signals.recv() => signal.map(Some),
                    () = tokio::time::sleep_until(due) => Some(None),
                },
                None => signals.recv().await.map(Some),
            };
            let Some(signal) = signal else {
                return;
            };
            match signal {
                Some(Signal::Event(event)) => {
                    tracing::debug!(
                        channel = event.channel,
                        message = event.message,
                        change = ?event.change,
                        "a Discord reaction event"
                    );
                    note_owner_reaction(&event, self.owner.as_deref(), self.source);
                    let now = Instant::now();
                    pending
                        .entry((event.channel, event.message))
                        .or_insert(Pending {
                            first: now,
                            due: now + self.timing.coalesce,
                            failed: 0,
                        });
                }
                Some(Signal::Up { resumed }) => {
                    if !resumed {
                        // A gap Discord did not replay: any owner mark may be stale.
                        forget_owner_reactions(self.source);
                        self.reconcile().await;
                    }
                }
                None => {
                    let now = Instant::now();
                    let ready: Vec<(String, String)> = pending
                        .iter()
                        .filter(|(_, waiting)| waiting.due <= now)
                        .map(|(key, _)| key.clone())
                        .collect();
                    for key in ready {
                        let Some(waiting) = pending.remove(&key) else {
                            continue;
                        };
                        let (channel, message) = &key;
                        let Refreshed::Failed(place) =
                            self.refresh(channel, message, waiting.first).await
                        else {
                            continue;
                        };
                        let failed = waiting.failed + 1;
                        if failed < READ_ATTEMPTS {
                            let pause = self.timing.retry.saturating_mul(1 << (failed - 1));
                            pending.entry(key).or_insert(Pending {
                                first: waiting.first,
                                due: Instant::now() + pause,
                                failed,
                            });
                        } else if let Some(place) = place {
                            // The poller's own refresh finds what this could not.
                            tracing::warn!(
                                channel = %place,
                                message = message.as_str(),
                                "a message whose reactions changed could not be read after \
                                 {READ_ATTEMPTS} tries; the poller looks after this channel's \
                                 reactions until the next new Gateway session"
                            );
                            self.pushed.release(&place);
                        }
                    }
                }
            }
        }
    }

    /// After a fresh session: each watched channel's newest window, diffed, as a poller tick's
    /// reaction refresh diffs it. A channel nobody watches shows its reactions at its next read.
    async fn reconcile(&self) {
        for channel in (self.channels)() {
            if !self.hub.is_watched(&channel) {
                continue;
            }
            match self
                .chat
                .fetch_recent(&channel, crate::live::REACTION_REFRESH_MESSAGES)
                .await
            {
                Ok(recent) => {
                    for mut message in recent {
                        // Discord itself leaves the field out only when there are none.
                        if self.hub.reactions_changed(&channel, &mut message, true) {
                            self.hub.publish_update(&channel, message);
                        }
                    }
                }
                Err(error) => tracing::warn!(
                    channel = %channel,
                    %error,
                    "could not reconcile reactions after a new Discord Gateway session"
                ),
            }
        }
    }

    /// Where `raw`, a channel id an event named, lives here: a configured channel, or a thread
    /// of one with the thread's id. `Ok(None)` for anything else; `Err` when the thread's
    /// parent could not be read this time.
    async fn resolve(&mut self, raw: &str) -> Result<Option<(ChannelId, Option<String>)>, ()> {
        let configured = (self.channels)();
        let id = ChannelId(raw.to_owned());
        if configured.contains(&id) {
            return Ok(Some((id, None)));
        }
        let parent = if let Some(parent) = self.parents.get(raw) {
            parent.clone()
        } else {
            let parent = match self.rest.thread_parent(raw).await {
                Ok(parent) => parent,
                // A channel the bot may not read, or that is gone, or not a snowflake: not one
                // of this server's, now or later.
                Err(error)
                    if matches!(
                        error.cause(),
                        ChatError::Status {
                            status: 403 | 404,
                            ..
                        } | ChatError::Refused(_)
                            | ChatError::Shape(_)
                    ) =>
                {
                    None
                }
                Err(error) => {
                    tracing::debug!(channel = raw, %error, "could not read a reaction's channel");
                    return Err(());
                }
            };
            if self.parents.len() >= PARENT_MEMORY {
                self.parents.clear();
            }
            self.parents.insert(raw.to_owned(), parent.clone());
            parent
        };
        Ok(parent
            .filter(|parent| configured.contains(parent))
            .map(|parent| (parent, Some(raw.to_owned()))))
    }

    /// Read `message` again and publish it. `first` is when the first event for it arrived.
    async fn refresh(&mut self, raw: &str, message: &str, first: Instant) -> Refreshed {
        let (channel, thread) = match self.resolve(raw).await {
            Ok(Some(place)) => place,
            Ok(None) => return Refreshed::Done,
            Err(()) => return Refreshed::Failed(None),
        };
        let id = MessageId(message.to_owned());
        match self
            .chat
            .fetch_messages(&channel, thread.as_deref(), std::slice::from_ref(&id))
            .await
        {
            Ok(answers) => match answers.into_iter().next() {
                Some(Lookup::Found(found)) => {
                    let mut copy = *found;
                    // Discord leaves the field out when the last reaction goes.
                    copy.reactions.get_or_insert_with(Vec::new);
                    // Published whatever the hub remembers: the event says the set moved, a
                    // page may hold any older copy, and a read made after the change but
                    // before this one (the reconcile after a new session, a poller tick) may
                    // have taught the hub the new set already. An update that repeats a set
                    // costs a page nothing; publishing teaches the hub this one.
                    self.hub.publish_update(&channel, copy);
                    tracing::debug!(
                        channel = %channel,
                        message = message,
                        since_event_ms = first.elapsed().as_millis(),
                        "published a Discord reaction change"
                    );
                    Refreshed::Done
                }
                // Deleted since: nothing to show reactions on.
                Some(Lookup::Absent) => Refreshed::Done,
                Some(Lookup::Unseen) | None => Refreshed::Failed(Some(channel)),
            },
            Err(error) => {
                tracing::warn!(
                    channel = %channel,
                    message = message,
                    %error,
                    "could not read a message whose reactions changed; trying again"
                );
                Refreshed::Failed(Some(channel))
            }
        }
    }
}

#[cfg(test)]
mod tests;
