//! The Discord chat-subscription backend: REST polling, addressing, cursoring.
//!
//! Each poll lists every subscribed channel's messages after the committed cursor (ascending
//! snowflakes), and turns each message into exactly one delivery batch:
//!
//! | Post | Event |
//! |---|---|
//! | Owner post addressed to this host ([`crate::addressing`]), sender allowed | `message_created` (a request, through the bridge's durable queue) |
//! | Same, sender not allowed (whatever its target) | `message_rejected` (`sender_not_allowed`) |
//! | Allowed sender, but the `agent=` target is refused (unknown agent, another host, malformed) | `message_rejected` (`target_refused`): the bridge marks the post with its rejection reaction |
//! | Configured peer bot in a relay channel, also a subscription relay sender, and a line of its text opens an `agent-teams-` fence | `message_created` with the peer's id, text unchanged (the bridge hands it to `agent-teams relay-in`, never a prompt) |
//! | Everything else: unaddressed (MSG-1), own posts, other bots, webhooks, system notices, unparsable messages, a peer bot's ordinary chat | `checkpoint` (no reaction, no reply) |
//!
//! **Owner channels and relay channels are separate.** The bridge treats a relay channel as
//! relay-only: anyone but a listed peer posting there, the owner included, is refused. So the
//! owner talks to this host in a channel listed only in `channel_ids`, and the peer bots'
//! envelopes travel in a channel that is also in `relay_channels`. The plugin still reports an
//! owner's addressed post in a relay channel, and the bridge answers it with its rejection
//! reaction, so a post in the wrong channel is visibly refused, never silently lost.
//!
//! A peer bot's posts in the relay channel are not all relay traffic (it may answer there too).
//! `agent-teams relay-in` quarantines and alarms on any post from a confirmed peer that does not
//! parse, so only a post with a line opening an `agent-teams-` fence (envelope, receipt,
//! announce), or the relay's plain-text receipt, goes to it ([`is_relay_post`]).
//!
//! The ACK line naming host and agent is posted by the outbound helper, when the bridge adds its
//! acknowledgement reaction: that is, only after the bridge has admitted the request
//! ([`crate::outbound`]).
//!
//! # Polling and the rate budget
//!
//! Each poll is one `GET /channels/{id}/messages?after=<cursor>&limit=100` per channel (the
//! fake-provider tests count this); heartbeats make no call. At the default `poll_seconds: 5`
//! that is 12 calls a minute per channel, 0.2 a second, against Discord's global limit of 50 a
//! second per bot; each host's bot has its own budget. A new post is seen within
//! `poll_seconds` (2.5 s on average) plus one REST round trip, so an owner-to-other-host relay
//! hop costs at most two poll intervals. The per-route bucket for listing messages is not
//! published; the transport reads it from every response and pauses before it empties
//! ([`crate::transport`]). A 429 delays the next poll by its `retry_after` (never less than
//! `poll_seconds`); other retryable failures back off 30 s, including during the initial
//! baseline (heartbeats continue meanwhile). A Gateway subscription would cut the latency to one
//! round trip; it is a later increment.
//!
//! # Thread roots
//!
//! A Discord reply chain maps to the bridge thread of its root message. The plugin remembers
//! the last 4,096 roots it saw; for a reply whose parent it does not know (after a restart, or
//! older than that window) it reads the parent chain back from Discord, at most
//! [`MAX_ROOT_HOPS`] messages, so a restart does not split a thread.
//!
//! A request for one named agent (`agent=NAME@HOST:`) is delivered as the bridge's existing
//! owner command `!@NAME <request>`, which opens a thread bound to that agent and delivers to
//! its pane or queue with the usual admission and receipt rules. No new delivery path.
//!
//! The cursor (`discord.v1;<channel>=<last message>,...`) advances only on the bridge's
//! acknowledgement, so a crash replays at most the one unacknowledged message; the bridge's own
//! request ledger and the relay's receiving ledger absorb that replay.
//!
//! Settings arrive in the Start frame's `backend_configuration` (schema `agentctl.discord.v1`):
//!
//! ```json
//! {"bot_user_id": "1545796614826889226", "host": "newton-fedora",
//!  "aliases": ["newton-team"], "local_agents": ["coord"],
//!  "peer_bots": {"1532416065114607829": "fleet@lead-host"},
//!  "token_file": "/root/.claude/channels/discord/.env", "token_key": "DISCORD_BOT_TOKEN",
//!  "poll_seconds": 5, "heartbeat_seconds": 30, "curl_path": "/usr/bin/curl"}
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chat_subscription::{
    BackendCapabilities, BackendConfiguration, BackendFailure, CancellationError, ChannelId,
    ChatSubscriptionBackend, ChatSubscriptionCancellation, ChatSubscriptionDriver,
    CommittableEvent, DeliveryBatch, DeliveryId, EventKind, EventSequence, Heartbeat,
    InboundMessage, MessageId, ProviderCursor, RejectedMessage, RejectionReason, ReplaySupport,
    SenderId, SubscribeRequest, SubscriptionItem, ThreadId,
};
use serde_json::Value;

use crate::addressing::{AddressingPolicy, Post, Target, Verdict};
use crate::admission::{Admission, DedupeSet, Origin};
use crate::ids::{self, Snowflake};
use crate::transport::{
    CurlTransport, DiscordMessage, DiscordRest, RestError, TokenSource, DEFAULT_API_BASE,
};

/// Backend configuration schema.
pub const CONFIG_SCHEMA: &str = "agentctl.discord.v1";
/// Backend name reported in capabilities.
pub const BACKEND_NAME: &str = "discord-rest-poll";
const CURSOR_PREFIX: &str = "discord.v1;";
const PAGE: u8 = 100;
const ROOT_MAP_CAPACITY: usize = 4096;

/// Parsed plugin settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordConfig {
    /// The addressing identity.
    pub policy: AddressingPolicy,
    /// Where the bot token comes from.
    pub token: TokenSource,
    /// Seconds between polls (2-60).
    pub poll_seconds: u64,
    /// Seconds of silence before a heartbeat (5-300).
    pub heartbeat_seconds: u64,
    /// Absolute curl path.
    pub curl_path: PathBuf,
    /// REST base URL.
    pub api_base: String,
}

impl DiscordConfig {
    /// Parse the Start frame's backend configuration.
    ///
    /// # Errors
    ///
    /// A wrong schema, a missing or invalid field.
    pub fn from_backend_configuration(
        configuration: &BackendConfiguration,
    ) -> Result<Self, String> {
        if configuration.schema() != CONFIG_SCHEMA {
            return Err(format!(
                "backend configuration schema must be {CONFIG_SCHEMA}, found {}",
                configuration.schema()
            ));
        }
        let data = configuration.data();
        let known = [
            "bot_user_id",
            "host",
            "aliases",
            "local_agents",
            "peer_bots",
            "token_file",
            "token_key",
            "token_env",
            "poll_seconds",
            "heartbeat_seconds",
            "curl_path",
            "api_base",
            "relay_channels",
        ];
        if let Some(unknown) = data.keys().find(|key| !known.contains(&key.as_str())) {
            return Err(format!("unknown discord setting {unknown}"));
        }
        let own_bot = data
            .get("bot_user_id")
            .and_then(Value::as_str)
            .and_then(Snowflake::parse)
            .ok_or("bot_user_id must be a Discord user id string")?;
        let host = data
            .get("host")
            .and_then(Value::as_str)
            .filter(|host| !host.is_empty())
            .ok_or("host must be a nonempty string")?
            .to_owned();
        let strings = |key: &str| -> Result<Option<BTreeSet<String>>, String> {
            match data.get(key) {
                None => Ok(None),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .filter(|text| !text.is_empty())
                            .map(str::to_owned)
                            .ok_or_else(|| format!("{key} must hold nonempty strings"))
                    })
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map(Some),
                Some(_) => Err(format!("{key} must be an array")),
            }
        };
        let aliases = strings("aliases")?.unwrap_or_default();
        let local_agents = strings("local_agents")?;
        let mut peer_bots = BTreeMap::new();
        if let Some(peers) = data.get("peer_bots") {
            let peers = peers.as_object().ok_or("peer_bots must be an object")?;
            for (id, team) in peers {
                let id = Snowflake::parse(id).ok_or("peer_bots keys must be Discord user ids")?;
                let team = team
                    .as_str()
                    .filter(|team| !team.is_empty())
                    .ok_or("peer_bots values must be team names")?;
                if id == own_bot {
                    return Err("this host's bot must not also be a peer bot".to_owned());
                }
                peer_bots.insert(id, team.to_owned());
            }
        }
        // The bridge requires its relay channel list to be repeated here (it refuses a backend
        // configuration whose list drifts from its own). The subscription's own relay authority
        // still comes from the Start request; this copy is checked against `peer_bots`.
        if let Some(relay) = data.get("relay_channels") {
            let entries = relay
                .as_array()
                .filter(|entries| entries.len() <= 32)
                .ok_or(
                    "relay_channels must be a list of at most 32 {channel, peer_senders} entries",
                )?;
            for entry in entries {
                entry
                    .get("channel")
                    .and_then(Value::as_str)
                    .and_then(Snowflake::parse)
                    .ok_or("relay_channels entries need a Discord channel id")?;
                let peers = entry
                    .get("peer_senders")
                    .and_then(Value::as_array)
                    .filter(|peers| !peers.is_empty())
                    .ok_or("relay_channels peer_senders must be a nonempty list of user ids")?;
                for peer in peers {
                    let id = peer
                        .as_str()
                        .and_then(Snowflake::parse)
                        .ok_or("relay_channels peer_senders must be Discord user ids")?;
                    if !peer_bots.contains_key(&id) {
                        return Err(format!(
                            "relay_channels names peer {id}, which is not in peer_bots"
                        ));
                    }
                }
            }
        }
        let token = match (data.get("token_file"), data.get("token_env")) {
            (Some(_), Some(_)) => return Err("set token_file or token_env, not both".to_owned()),
            (Some(path), None) => TokenSource::DotEnv {
                path: PathBuf::from(path.as_str().ok_or("token_file must be a string")?),
                key: data
                    .get("token_key")
                    .and_then(Value::as_str)
                    .unwrap_or("DISCORD_BOT_TOKEN")
                    .to_owned(),
            },
            (None, Some(name)) => TokenSource::Env(
                name.as_str()
                    .ok_or("token_env must be a string")?
                    .to_owned(),
            ),
            (None, None) => TokenSource::Env("DISCORD_BOT_TOKEN".to_owned()),
        };
        let seconds = |key: &str, default: u64, low: u64, high: u64| -> Result<u64, String> {
            match data.get(key) {
                None => Ok(default),
                Some(value) => value
                    .as_u64()
                    .filter(|value| (low..=high).contains(value))
                    .ok_or_else(|| format!("{key} must be an integer {low}-{high}")),
            }
        };
        Ok(Self {
            policy: AddressingPolicy {
                own_bot,
                host,
                peer_bots,
                aliases,
                local_agents,
            },
            token,
            poll_seconds: seconds("poll_seconds", 5, 2, 60)?,
            heartbeat_seconds: seconds("heartbeat_seconds", 30, 5, 300)?,
            curl_path: PathBuf::from(
                data.get("curl_path")
                    .and_then(Value::as_str)
                    .unwrap_or("/usr/bin/curl"),
            ),
            api_base: data
                .get("api_base")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_API_BASE)
                .to_owned(),
        })
    }
}

/// Builds the REST transport for a subscription.
pub type TransportFactory =
    Box<dyn FnMut(&DiscordConfig) -> Result<Box<dyn DiscordRest>, String> + Send>;

#[derive(Default)]
struct CancelState {
    cancelled: Mutex<bool>,
    changed: Condvar,
}

struct Cancel(Arc<CancelState>);

impl ChatSubscriptionCancellation for Cancel {
    fn cancel(&self) -> Result<(), CancellationError> {
        *self
            .0
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.0.changed.notify_all();
        Ok(())
    }
}

/// The Discord backend.
pub struct DiscordBackend {
    factory: TransportFactory,
    cancel: Arc<CancelState>,
}

impl DiscordBackend {
    /// A backend whose subscriptions use `factory` for their transport.
    #[must_use]
    pub fn new(factory: TransportFactory) -> Self {
        Self {
            factory,
            cancel: Arc::new(CancelState::default()),
        }
    }

    /// The production backend: `curl` with the configured token source.
    #[must_use]
    pub fn with_default_transport() -> Self {
        Self::new(Box::new(|config: &DiscordConfig| {
            let token = config.token.load()?;
            let transport =
                CurlTransport::new(config.curl_path.clone(), config.api_base.clone(), token, 20)?;
            Ok(Box::new(transport) as Box<dyn DiscordRest>)
        }))
    }

    /// The capabilities this backend advertises.
    ///
    /// # Panics
    ///
    /// Only if the constant capability set violates the protocol's bounds.
    #[must_use]
    pub fn advertised() -> BackendCapabilities {
        BackendCapabilities::new(
            BACKEND_NAME,
            ReplaySupport::Cursor,
            true,
            NonZeroU16::new(1).expect("one is nonzero"),
            vec![
                EventKind::MessageCreated,
                EventKind::MessageRejected,
                EventKind::Checkpoint,
                EventKind::Heartbeat,
            ],
        )
        .expect("constant capabilities are valid")
    }
}

fn failure(code: &str, detail: impl Into<String>, retryable: bool) -> BackendFailure {
    let detail = detail.into();
    BackendFailure::new(
        code,
        if detail.is_empty() {
            code.to_owned()
        } else {
            detail
        },
        retryable,
    )
    .unwrap_or_else(|_| {
        BackendFailure::new("discord_failure", "discord backend failure", retryable)
            .expect("constant failure is valid")
    })
}

impl ChatSubscriptionBackend for DiscordBackend {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(Cancel(Arc::clone(&self.cancel)))
    }

    fn capabilities(&self) -> BackendCapabilities {
        Self::advertised()
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        let configuration = request.backend_configuration().ok_or_else(|| {
            failure(
                "config_missing",
                "discord needs backend_configuration",
                false,
            )
        })?;
        let config = DiscordConfig::from_backend_configuration(configuration)
            .map_err(|problem| failure("config_invalid", problem, false))?;
        let channels = request
            .channel_ids()
            .iter()
            .map(|channel| {
                Snowflake::parse(channel.as_str()).ok_or_else(|| {
                    failure(
                        "config_invalid",
                        "channel ids must be Discord snowflakes",
                        false,
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cursor = match request.resume_from() {
            Some(cursor) => parse_cursor(cursor.as_str())
                .ok_or_else(|| failure("cursor_invalid", "unreadable discord cursor", false))?,
            None => BTreeMap::new(),
        };
        let rest = (self.factory)(&config)
            .map_err(|problem| failure("transport_unavailable", problem, false))?;
        let strings = |items: &[SenderId]| -> HashSet<String> {
            items.iter().map(|id| id.as_str().to_owned()).collect()
        };
        Ok(Box::new(DiscordDriver {
            rest,
            baselined: false,
            poll: Duration::from_secs(config.poll_seconds),
            heartbeat: Duration::from_secs(config.heartbeat_seconds),
            policy: config.policy,
            channels,
            allowed: strings(request.allowed_senders()),
            relay_senders: strings(request.relay_senders()),
            relay_channels: request
                .relay_channels()
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
            needs_boundary: request.resume_from().is_none(),
            cursor,
            pending: VecDeque::new(),
            in_flight: None,
            sequence: 0,
            next_poll: None,
            last_item: Instant::now(),
            dedupe: DedupeSet::new(ROOT_MAP_CAPACITY),
            roots: HashMap::new(),
            root_order: VecDeque::new(),
            cancel: Arc::clone(&self.cancel),
        }))
    }
}

/// Encode a cursor: `discord.v1;<channel>=<message>,...` in channel order.
#[must_use]
pub fn encode_cursor(cursor: &BTreeMap<Snowflake, Snowflake>) -> String {
    let parts: Vec<String> = cursor
        .iter()
        .map(|(channel, message)| format!("{channel}={message}"))
        .collect();
    format!("{CURSOR_PREFIX}{}", parts.join(","))
}

/// Decode [`encode_cursor`]'s form.
#[must_use]
pub fn parse_cursor(text: &str) -> Option<BTreeMap<Snowflake, Snowflake>> {
    let body = text.strip_prefix(CURSOR_PREFIX)?;
    let mut cursor = BTreeMap::new();
    if body.is_empty() {
        return Some(cursor);
    }
    for part in body.split(',') {
        let (channel, message) = part.split_once('=')?;
        cursor.insert(Snowflake::parse(channel)?, Snowflake::parse(message)?);
    }
    Some(cursor)
}

struct InFlight {
    delivery_id: String,
    cursor: BTreeMap<Snowflake, Snowflake>,
    dedupe_key: Option<String>,
}

/// What [`DiscordDriver::classify`] decided for one message.
struct Classified {
    event: CommittableEvent,
    dedupe_key: Option<String>,
}

impl Classified {
    fn checkpoint() -> Self {
        Self {
            event: CommittableEvent::Checkpoint,
            dedupe_key: None,
        }
    }
}

/// The most parent messages read back from Discord to find one reply's thread root.
pub const MAX_ROOT_HOPS: usize = 8;

/// The plain-text receipt `agent-teams relay-in` posts for a post it cannot parse.
const RELAY_PLAIN_RECEIPT: &str = "agent-teams receipt:";

/// The fence opener every relay block starts with (`agent-teams-envelope`, `-receipt`,
/// `-announce`).
const RELAY_FENCE_OPENER: &str = "```agent-teams-";

/// Whether a peer bot's post is relay traffic for `agent-teams relay-in`: a line of it opens an
/// `agent-teams-` fence, or it is the relay's plain-text receipt. Discord stores text
/// byte-for-byte, so the fence info string survives and is the authority. A peer's ordinary
/// chat is not relay traffic, even when it mentions agent-teams or the channel's name, and
/// handing it over would quarantine each such post as unparseable.
#[must_use]
pub fn is_relay_post(content: &str) -> bool {
    content.trim_start().starts_with(RELAY_PLAIN_RECEIPT)
        || content
            .lines()
            .any(|line| line.trim_start().starts_with(RELAY_FENCE_OPENER))
}

/// When to poll again after a failed poll, or `None` when the failure is not retryable.
#[must_use]
pub fn poll_backoff(error: &RestError, poll: Duration) -> Option<Duration> {
    if !error.retryable {
        return None;
    }
    Some(match error.retry_after {
        Some(wait) if error.code == "rate_limited" => wait.max(poll),
        _ => poll.max(Duration::from_secs(30)),
    })
}

struct DiscordDriver {
    rest: Box<dyn DiscordRest>,
    baselined: bool,
    policy: AddressingPolicy,
    channels: Vec<Snowflake>,
    allowed: HashSet<String>,
    relay_senders: HashSet<String>,
    relay_channels: HashSet<String>,
    needs_boundary: bool,
    cursor: BTreeMap<Snowflake, Snowflake>,
    pending: VecDeque<DiscordMessage>,
    in_flight: Option<InFlight>,
    sequence: u64,
    poll: Duration,
    heartbeat: Duration,
    next_poll: Option<Instant>,
    last_item: Instant,
    dedupe: DedupeSet,
    roots: HashMap<Snowflake, Snowflake>,
    root_order: VecDeque<Snowflake>,
    cancel: Arc<CancelState>,
}

impl DiscordDriver {
    fn next_sequence(&mut self) -> Result<EventSequence, BackendFailure> {
        self.sequence += 1;
        EventSequence::new(self.sequence)
            .map_err(|error| failure("sequence_invalid", error.to_string(), false))
    }

    fn batch(
        &mut self,
        cursor: BTreeMap<Snowflake, Snowflake>,
        classified: Classified,
    ) -> Result<SubscriptionItem, BackendFailure> {
        let Classified { event, dedupe_key } = classified;
        let sequence = self.next_sequence()?;
        let delivery_id = format!("discord-delivery-{}", sequence.get());
        let invalid = |error: chat_subscription::ValidationError| {
            failure("batch_invalid", error.to_string(), false)
        };
        let batch = DeliveryBatch::new(
            sequence,
            ProviderCursor::new(encode_cursor(&cursor)).map_err(invalid)?,
            DeliveryId::new(delivery_id.clone()).map_err(invalid)?,
            vec![event],
        )
        .map_err(invalid)?;
        self.in_flight = Some(InFlight {
            delivery_id,
            cursor,
            dedupe_key,
        });
        self.last_item = Instant::now();
        Ok(SubscriptionItem::Batch(batch))
    }

    fn cancelled(&self) -> bool {
        *self
            .cancel
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait_until(&self, deadline: Instant) {
        let guard = self
            .cancel
            .cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let timeout = deadline.saturating_duration_since(Instant::now());
        if !*guard && !timeout.is_zero() {
            drop(
                self.cancel
                    .changed
                    .wait_timeout(guard, timeout)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    }

    /// Record a newly listed channel's current newest message as its starting point.
    fn baseline(&mut self) -> Result<bool, RestError> {
        let mut changed = false;
        for channel in self.channels.clone() {
            if self.cursor.contains_key(&channel) {
                continue;
            }
            if let Some(newest) = self.rest.list_messages(channel, None, 1)?.last() {
                self.cursor.insert(channel, newest.id);
                changed = true;
            }
        }
        Ok(changed)
    }

    fn poll_channels(&mut self) -> Result<(), RestError> {
        for channel in self.channels.clone() {
            let after = self.cursor.get(&channel).copied();
            let mut listed = self.rest.list_messages(channel, after, PAGE)?;
            listed.sort_by_key(|message| message.id);
            for message in listed {
                if message.channel_id != channel || after.is_some_and(|after| message.id <= after) {
                    continue;
                }
                let key = crate::admission::dedupe_key(channel, message.id, &self.policy.host);
                if !self.dedupe.contains(&key)
                    && !self.pending.iter().any(|queued| queued.id == message.id)
                {
                    self.pending.push_back(message);
                }
            }
        }
        Ok(())
    }

    /// The root of the reply chain `parent` belongs to: from memory when known, else read back
    /// from Discord (at most [`MAX_ROOT_HOPS`] messages). A read failure ends the walk at the
    /// last message reached, which is logged.
    fn resolve_root(&mut self, channel: Snowflake, parent: Snowflake) -> Snowflake {
        let mut current = parent;
        for _ in 0..MAX_ROOT_HOPS {
            if let Some(root) = self.roots.get(&current) {
                return *root;
            }
            match self.rest.get_message(channel, current) {
                Ok(found) => match found.reply_to {
                    Some(next) => current = next,
                    None => return current,
                },
                Err(error) => {
                    eprintln!(
                        "agentctl-discord-plugin: thread root of {} not resolved past {current}: {error}",
                        ids::message_resource(channel, parent)
                    );
                    return current;
                }
            }
        }
        current
    }

    fn thread_root(&mut self, message: &DiscordMessage) -> Snowflake {
        let root = match message.reply_to {
            None => message.id,
            Some(parent) => self.resolve_root(message.channel_id, parent),
        };
        if self.root_order.len() == ROOT_MAP_CAPACITY {
            if let Some(old) = self.root_order.pop_front() {
                self.roots.remove(&old);
            }
        }
        self.roots.insert(message.id, root);
        self.root_order.push_back(message.id);
        root
    }

    fn rejection(message: &DiscordMessage, reason: RejectionReason) -> Classified {
        match (
            ChannelId::new(message.channel_id.to_string()),
            MessageId::new(ids::message_resource(message.channel_id, message.id)),
            SenderId::new(message.author_id.to_string()),
        ) {
            (Ok(channel), Ok(id), Ok(sender)) => Classified {
                event: CommittableEvent::message_rejected(RejectedMessage::new(
                    channel, id, sender, reason,
                )),
                dedupe_key: None,
            },
            _ => Classified::checkpoint(),
        }
    }

    /// The single event for one listed message, and its dedupe key when it is admitted.
    fn classify(&mut self, message: &DiscordMessage) -> Classified {
        if !message.is_user_message() {
            // Joins, pins, thread notices, unparsable messages and other system messages are
            // never requests.
            return Classified::checkpoint();
        }
        let verdict = self.policy.classify(&Post {
            author: message.author_id,
            // A webhook or system post is never a person, whatever its author fields say.
            author_is_bot: message.author_is_bot || message.webhook || message.author_is_system,
            content: &message.content,
        });
        let author = message.author_id.to_string();
        let channel = message.channel_id.to_string();
        let (origin, target, text) = match verdict {
            Verdict::Ignore(_) => return Classified::checkpoint(),
            Verdict::Reject(reason) => {
                let typed = if self.allowed.contains(&author) {
                    RejectionReason::TargetRefused
                } else {
                    // Authorization comes first: a stranger learns nothing about targets.
                    RejectionReason::SenderNotAllowed
                };
                eprintln!(
                    "agentctl-discord-plugin: refused {} as {}: {reason:?}",
                    ids::message_resource(message.channel_id, message.id),
                    typed.as_str()
                );
                return Self::rejection(message, typed);
            }
            Verdict::Relay { peer_team } => {
                if !self.relay_senders.contains(&author)
                    || !self.relay_channels.contains(&channel)
                    || !is_relay_post(&message.content)
                {
                    return Classified::checkpoint();
                }
                (
                    Origin::PeerBot {
                        bot: message.author_id,
                        team: peer_team,
                    },
                    None,
                    message.content.clone(),
                )
            }
            Verdict::Deliver(addressed) => {
                if !self.allowed.contains(&author) {
                    return Self::rejection(message, RejectionReason::SenderNotAllowed);
                }
                let text = match &addressed.target {
                    Target::Bridge => addressed.request.clone(),
                    Target::Agent { name } => format!("!@{name} {}", addressed.request),
                };
                (
                    Origin::Owner {
                        user: message.author_id,
                    },
                    Some(addressed.target),
                    text,
                )
            }
        };
        let admission = Admission {
            origin,
            host: self.policy.host.clone(),
            target,
            channel: message.channel_id,
            message: message.id,
        };
        let key = admission.dedupe_key();
        let root = self.thread_root(message);
        let built = (|| -> Result<InboundMessage, String> {
            let inbound = InboundMessage::new(
                ChannelId::new(channel).map_err(|e| e.to_string())?,
                MessageId::new(ids::message_resource(message.channel_id, message.id))
                    .map_err(|e| e.to_string())?,
                ThreadId::new(ids::thread_resource(message.channel_id, root))
                    .map_err(|e| e.to_string())?,
                SenderId::new(author).map_err(|e| e.to_string())?,
                text,
                message.timestamp.clone(),
                message.reply_to.is_some(),
            )
            .map_err(|e| e.to_string())?;
            Ok(inbound.with_provider_payload(admission.to_payload()?))
        })();
        match built {
            Ok(inbound) => Classified {
                event: CommittableEvent::message_created(inbound),
                dedupe_key: Some(key),
            },
            Err(problem) => {
                eprintln!(
                    "agentctl-discord-plugin: skipped {}: {problem}",
                    ids::message_resource(message.channel_id, message.id)
                );
                Classified::checkpoint()
            }
        }
    }
}

impl ChatSubscriptionDriver for DiscordDriver {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(Cancel(Arc::clone(&self.cancel)))
    }

    fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if self.in_flight.is_some() {
            return Err(failure(
                "outstanding_delivery",
                "next item requested before the previous batch was acknowledged",
                false,
            ));
        }
        loop {
            if self.cancelled() {
                return Err(failure("cancelled", "discord subscription cancelled", true));
            }
            if !self.baselined && self.next_poll.is_none_or(|at| Instant::now() >= at) {
                match self.baseline() {
                    Ok(changed) => {
                        self.baselined = true;
                        self.next_poll = Some(Instant::now());
                        if changed || self.needs_boundary {
                            self.needs_boundary = false;
                            let cursor = self.cursor.clone();
                            return self.batch(cursor, Classified::checkpoint()).map(Some);
                        }
                    }
                    Err(error) => match poll_backoff(&error, self.poll) {
                        // Heartbeats continue while the baseline waits.
                        Some(wait) => {
                            eprintln!(
                                "agentctl-discord-plugin: baseline failed, retrying in {} ms: {error}",
                                wait.as_millis()
                            );
                            self.next_poll = Some(Instant::now() + wait);
                        }
                        None => return Err(rest_failure(error)),
                    },
                }
            }
            if !self.baselined {
                let now = Instant::now();
                let heartbeat_due = self.last_item + self.heartbeat;
                if now >= heartbeat_due {
                    let sequence = self.next_sequence()?;
                    self.last_item = now;
                    return Ok(Some(SubscriptionItem::Heartbeat(Heartbeat::new(sequence))));
                }
                self.wait_until(self.next_poll.unwrap_or(now).min(heartbeat_due));
                continue;
            }
            if let Some(message) = self.pending.pop_front() {
                let mut cursor = self.cursor.clone();
                cursor.insert(message.channel_id, message.id);
                let classified = self.classify(&message);
                return self.batch(cursor, classified).map(Some);
            }
            let now = Instant::now();
            let next_poll = self.next_poll.unwrap_or(now);
            if now >= next_poll {
                match self.poll_channels() {
                    Ok(()) => self.next_poll = Some(now + self.poll),
                    Err(error) => match poll_backoff(&error, self.poll) {
                        Some(wait) => {
                            eprintln!(
                                "agentctl-discord-plugin: poll failed, next poll in {} ms: {error}",
                                wait.as_millis()
                            );
                            self.next_poll = Some(now + wait);
                        }
                        None => return Err(rest_failure(error)),
                    },
                }
                continue;
            }
            let heartbeat_due = self.last_item + self.heartbeat;
            if now >= heartbeat_due {
                let sequence = self.next_sequence()?;
                self.last_item = now;
                return Ok(Some(SubscriptionItem::Heartbeat(Heartbeat::new(sequence))));
            }
            self.wait_until(next_poll.min(heartbeat_due));
        }
    }

    fn acknowledge(&mut self, delivery_id: &DeliveryId) -> Result<(), BackendFailure> {
        match self.in_flight.take() {
            Some(in_flight) if in_flight.delivery_id == delivery_id.as_str() => {
                self.cursor = in_flight.cursor;
                if let Some(key) = in_flight.dedupe_key {
                    self.dedupe.insert(key);
                }
                Ok(())
            }
            other => {
                self.in_flight = other;
                Err(failure(
                    "unknown_delivery",
                    "acknowledgement names no outstanding batch",
                    false,
                ))
            }
        }
    }
}

fn rest_failure(error: RestError) -> BackendFailure {
    failure(&error.code, error.detail, error.retryable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeRest;

    #[test]
    fn cursor_round_trips() {
        let cursor = BTreeMap::from([
            (Snowflake::from_u64(1), Snowflake::from_u64(10)),
            (Snowflake::from_u64(2), Snowflake::from_u64(20)),
        ]);
        let text = encode_cursor(&cursor);
        assert_eq!(text, "discord.v1;1=10,2=20");
        assert_eq!(parse_cursor(&text), Some(cursor));
        assert_eq!(parse_cursor("discord.v1;"), Some(BTreeMap::new()));
        assert_eq!(parse_cursor("slack;1=2"), None);
    }

    #[test]
    fn config_requires_schema_and_rejects_unknown_keys() {
        let data = serde_json::json!({"bot_user_id": "5", "host": "h", "extra": 1});
        let configuration =
            BackendConfiguration::new(CONFIG_SCHEMA, data.as_object().cloned().expect("object"))
                .expect("configuration");
        assert!(DiscordConfig::from_backend_configuration(&configuration)
            .expect_err("unknown key")
            .contains("extra"));
        let data = serde_json::json!({"bot_user_id": "5", "host": "h",
            "token_file": "/x/.env", "peer_bots": {"6": "fleet@b"}});
        let configuration =
            BackendConfiguration::new(CONFIG_SCHEMA, data.as_object().cloned().expect("object"))
                .expect("configuration");
        let config = DiscordConfig::from_backend_configuration(&configuration).expect("valid");
        assert_eq!(
            config.token,
            TokenSource::DotEnv {
                path: PathBuf::from("/x/.env"),
                key: "DISCORD_BOT_TOKEN".to_owned()
            }
        );
        assert_eq!(config.poll_seconds, 5);
        let _unused: Option<FakeRest> = None;
    }

    #[test]
    fn rate_limits_set_the_next_poll() {
        let poll = Duration::from_secs(5);
        let mut limited = RestError::from_status(429, r#"{"retry_after": 12.5}"#);
        assert_eq!(
            poll_backoff(&limited, poll),
            Some(Duration::from_millis(12_500)),
            "a 429 waits exactly its retry_after"
        );
        limited.retry_after = Some(Duration::from_millis(100));
        assert_eq!(
            poll_backoff(&limited, poll),
            Some(poll),
            "never sooner than the poll interval"
        );
        assert_eq!(
            poll_backoff(&RestError::from_status(502, ""), poll),
            Some(Duration::from_secs(30))
        );
        assert_eq!(poll_backoff(&RestError::from_status(403, ""), poll), None);
    }

    #[test]
    fn only_relay_shaped_peer_posts_go_to_relay_in() {
        assert!(is_relay_post("hi\n```agent-teams-envelope\n{}\n```"));
        assert!(is_relay_post("```agent-teams-receipt\n{}\n```"));
        assert!(
            !is_relay_post("see #agent-teams-relay for the agent-teams-envelope format"),
            "naming the channel or a fence in prose is not relay traffic"
        );
        assert!(is_relay_post(
            "agent-teams receipt: not-recorded (unparseable relay post)"
        ));
        assert!(!is_relay_post(
            "Bound to lane-a. Messages in this thread go to it."
        ));
        assert!(!is_relay_post("ACK lead-host/coordinator: received"));
    }
}
