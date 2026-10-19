//! The inbound subscription backend.
//!
//! [`SlackBackend`] implements the provider-neutral [`ChatSubscriptionBackend`] so the generic
//! process server (`chat_subscription_plugin::serve_stdio`) can host it. One subscription opens one
//! transport event stream for the configured channels and maps each sequenced event to one batch:
//!
//! * a `message.created` event in a configured channel, from an allowed sender, not posted by a
//!   bot or app, not an edit or join notice, and not starting with an ignored prefix, becomes a
//!   `message_created` event. Its thread is the thread root's `ts` (the message's own `ts` for a
//!   top-level message), so replies to a thread route back into that thread;
//! * every other sequenced event becomes a `checkpoint`, so the cursor still advances.
//!
//! # Cursor and recovery
//!
//! The durable cursor is `slack1:seq:<n>`, the provider's event sequence number of the batch, or
//! `slack1:since:<unix ms>` before the first event has been seen. A restart resumes the stream at
//! `--catchup=<n>` (or `--since=<seconds>`), skips events at or before the saved sequence, and
//! carries on. Batches are built only from the event itself, so a replayed event produces an
//! identical batch. A new lineage (no cursor) first commits a `checkpoint` at
//! `slack1:since:<start - initial_backfill_seconds>`; messages created before that instant are
//! never delivered. A provider report of lost events, or a saved position the provider can no
//! longer replay, becomes a `gap`, which stops the bridge for operator review.

use std::collections::BTreeSet;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_subscription::{
    BackendCapabilities, BackendFailure, CancellationError, ChannelId, ChatSubscriptionBackend,
    ChatSubscriptionCancellation, ChatSubscriptionDriver, CommittableEvent, DeliveryBatch,
    DeliveryId, EventKind, EventSequence, Heartbeat, InboundMessage, MessageId, ProviderCursor,
    ProviderPayload, ReconciliationGap, ReplaySupport, SenderId, SubscribeRequest,
    SubscriptionItem, ThreadId,
};
use serde_json::{Map, Value};

use crate::cli::{CliOptions, CliTransport, TailOptions};
use crate::diag::{unix_millis, Diagnostics};
use crate::ids;
use crate::transport::{
    Catchup, EventStream, SlackMessage, SlackTransport, StreamCancel, StreamRecord, StreamWait,
    SubscribeSpec, WebApiTransport,
};

/// Schema of `backend_configuration` for this plugin.
pub const CONFIGURATION_SCHEMA: &str = "agentctl.slack.v1";
/// Schema of each delivered message's provider payload.
pub const PAYLOAD_SCHEMA: &str = "agentctl.slack.message.v1";
/// Backend name reported in capabilities.
pub const BACKEND_NAME: &str = "slack";
const CURSOR_PREFIX: &str = "slack1:";
const MAX_CHANNELS: usize = 8;
const MAX_IGNORED_PREFIXES: usize = 32;
// Subtypes that still carry a human message; every other subtype (edits, joins, topic changes,
// bot posts) is not a request.
const MESSAGE_SUBTYPES: &[&str] = &["thread_broadcast", "file_share", "me_message"];
const TRUNCATION_MARKER: &str = "\n[message truncated by the Slack provider]";

/// Which transport reaches Slack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    /// An installed Slack command-line client ([`CliTransport`]).
    Cli,
    /// The direct Slack Web API ([`WebApiTransport`], not implemented in this release).
    WebApi,
}

/// Validated `backend_configuration.data` for schema [`CONFIGURATION_SCHEMA`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfig {
    /// Selected transport.
    pub transport: TransportKind,
    /// Client settings, required for the CLI transport.
    pub cli: Option<CliOptions>,
    /// Seconds between idle heartbeats sent to the host.
    pub heartbeat_interval: Duration,
    /// A new lineage also delivers messages created this many seconds before start.
    pub initial_backfill_seconds: u64,
    /// Messages starting with one of these (after leading whitespace) are never delivered.
    pub ignore_text_prefixes: Vec<String>,
    /// Diagnostics file.
    pub diagnostics_log: Option<PathBuf>,
}

fn failure(code: &str, detail: impl Into<String>, retryable: bool) -> BackendFailure {
    let detail = crate::transport::bounded(&detail.into());
    BackendFailure::new(crate::transport::slug(code), detail, retryable)
        .expect("bounded slug and detail are valid")
}

fn invalid(detail: impl Into<String>) -> BackendFailure {
    failure("slack_configuration_invalid", detail, false)
}

fn optional_string(data: &Map<String, Value>, key: &str) -> Result<Option<String>, BackendFailure> {
    match data.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(invalid(format!(
            "{key} must be a nonempty single-line string of at most 256 bytes"
        ))),
    }
}

fn optional_seconds(
    data: &Map<String, Value>,
    key: &str,
    range: std::ops::RangeInclusive<u64>,
) -> Result<Option<u64>, BackendFailure> {
    match data.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|seconds| range.contains(seconds))
            .map(Some)
            .ok_or_else(|| {
                invalid(format!(
                    "{key} must be an integer from {} to {}",
                    range.start(),
                    range.end()
                ))
            }),
    }
}

impl PluginConfig {
    /// Parse and validate the host-supplied configuration. Unknown keys are refused.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable [`BackendFailure`] describing the first invalid field.
    pub fn from_request(request: &SubscribeRequest) -> Result<Self, BackendFailure> {
        let configuration = request.backend_configuration().ok_or_else(|| {
            invalid(format!(
                "backend_configuration with schema {CONFIGURATION_SCHEMA} is required"
            ))
        })?;
        if configuration.schema() != CONFIGURATION_SCHEMA {
            return Err(invalid(format!(
                "backend_configuration schema must be {CONFIGURATION_SCHEMA}"
            )));
        }
        let data = configuration.data();
        const KNOWN: &[&str] = &[
            "transport",
            "cli_path",
            "cli_environment",
            "cli_timeout_seconds",
            "device_id",
            "caller",
            "subscriber_app_id",
            "subscription_mode",
            "tail_heartbeat_seconds",
            "tail_stall_timeout_seconds",
            "heartbeat_interval_seconds",
            "initial_backfill_seconds",
            "ignore_text_prefixes",
            "diagnostics_log",
        ];
        if let Some(unknown) = data.keys().find(|key| !KNOWN.contains(&key.as_str())) {
            return Err(invalid(format!(
                "unknown backend_configuration key {unknown:?}"
            )));
        }
        let transport = match optional_string(data, "transport")?.as_deref() {
            None | Some("cli") => TransportKind::Cli,
            Some("web_api") => TransportKind::WebApi,
            Some(_) => return Err(invalid("transport must be \"cli\" or \"web_api\"")),
        };
        let mut environment = Vec::new();
        match data.get("cli_environment") {
            None | Some(Value::Null) => {}
            Some(Value::Object(values)) if values.len() <= 32 => {
                for (name, value) in values {
                    let valid_name = !name.is_empty()
                        && name.len() <= 128
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        && !name.as_bytes()[0].is_ascii_digit();
                    let Some(value) = value
                        .as_str()
                        .filter(|value| value.len() <= 4_096 && !value.contains('\0'))
                    else {
                        return Err(invalid(
                            "cli_environment values must be strings of at most 4096 bytes",
                        ));
                    };
                    if !valid_name {
                        return Err(invalid(
                            "cli_environment names must match [A-Za-z_][A-Za-z0-9_]*",
                        ));
                    }
                    environment.push((name.clone(), value.to_owned()));
                }
            }
            Some(_) => {
                return Err(invalid(
                    "cli_environment must be an object of at most 32 names",
                ))
            }
        }
        let cli = match transport {
            TransportKind::WebApi => None,
            TransportKind::Cli => {
                let program = optional_string(data, "cli_path")?
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .ok_or_else(|| {
                        invalid("cli_path must be the absolute path of the Slack client")
                    })?;
                Some(CliOptions {
                    program,
                    environment,
                    timeout: Duration::from_secs(
                        optional_seconds(data, "cli_timeout_seconds", 1..=120)?.unwrap_or(60),
                    ),
                    tail: TailOptions {
                        device_id: optional_string(data, "device_id")?,
                        caller: optional_string(data, "caller")?,
                        subscriber_app_id: optional_string(data, "subscriber_app_id")?,
                        mode: optional_string(data, "subscription_mode")?,
                        heartbeat_seconds: optional_seconds(
                            data,
                            "tail_heartbeat_seconds",
                            1..=3_600,
                        )?,
                        stall_timeout_seconds: optional_seconds(
                            data,
                            "tail_stall_timeout_seconds",
                            10..=86_400,
                        )?,
                    },
                })
            }
        };
        let ignore_text_prefixes = match data.get("ignore_text_prefixes") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(values)) if values.len() <= MAX_IGNORED_PREFIXES => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|prefix| !prefix.is_empty() && prefix.len() <= 256)
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("ignore_text_prefixes entries must be nonempty strings of at most 256 bytes"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(invalid("ignore_text_prefixes must be a list of at most 32 strings")),
        };
        Ok(Self {
            transport,
            cli,
            heartbeat_interval: Duration::from_secs(
                optional_seconds(data, "heartbeat_interval_seconds", 1..=300)?.unwrap_or(30),
            ),
            initial_backfill_seconds: optional_seconds(
                data,
                "initial_backfill_seconds",
                0..=86_400,
            )?
            .unwrap_or(0),
            ignore_text_prefixes,
            diagnostics_log: optional_string(data, "diagnostics_log")?.map(PathBuf::from),
        })
    }
}

/// Build the transport a configuration selects.
///
/// # Errors
///
/// Returns a non-retryable [`BackendFailure`] for an unusable CLI configuration.
pub fn default_transport(config: &PluginConfig) -> Result<Box<dyn SlackTransport>, BackendFailure> {
    match config.transport {
        TransportKind::WebApi => Ok(Box::new(WebApiTransport)),
        TransportKind::Cli => {
            let options = config
                .cli
                .clone()
                .ok_or_else(|| invalid("the cli transport requires cli_path"))?;
            CliTransport::new(options)
                .map(|transport| Box::new(transport) as Box<dyn SlackTransport>)
                .map_err(invalid)
        }
    }
}

/// A decoded durable cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cursor {
    /// After the event with this sequence number.
    Sequence(u64),
    /// Nothing created before this Unix time in milliseconds is delivered.
    Since(u64),
}

impl Cursor {
    /// Encode as the opaque provider cursor string.
    #[must_use]
    pub fn encode(&self) -> String {
        match self {
            Self::Sequence(sequence) => format!("{CURSOR_PREFIX}seq:{sequence}"),
            Self::Since(millis) => format!("{CURSOR_PREFIX}since:{millis}"),
        }
    }

    /// Decode a cursor produced by [`Self::encode`].
    #[must_use]
    pub fn decode(value: &str) -> Option<Self> {
        let rest = value.strip_prefix(CURSOR_PREFIX)?;
        if let Some(sequence) = rest.strip_prefix("seq:") {
            return sequence.parse().ok().map(Self::Sequence);
        }
        rest.strip_prefix("since:")?.parse().ok().map(Self::Since)
    }
}

type TransportFactory =
    Box<dyn FnMut(&PluginConfig) -> Result<Box<dyn SlackTransport>, BackendFailure> + Send>;

#[derive(Default)]
struct SharedCancel {
    cancelled: AtomicBool,
    stream: Mutex<Option<Arc<dyn StreamCancel>>>,
}

impl SharedCancel {
    fn register(&self, stream: Arc<dyn StreamCancel>) {
        let mut slot = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.cancelled.load(Ordering::SeqCst) {
            stream.cancel();
        }
        *slot = Some(stream);
    }
}

struct CancelHandle(Arc<SharedCancel>);

impl ChatSubscriptionCancellation for CancelHandle {
    fn cancel(&self) -> Result<(), CancellationError> {
        let slot = self
            .0
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.cancelled.store(true, Ordering::SeqCst);
        if let Some(stream) = slot.as_ref() {
            stream.cancel();
        }
        Ok(())
    }
}

/// The Slack subscription backend.
pub struct SlackBackend {
    factory: TransportFactory,
    cancel: Arc<SharedCancel>,
    now_millis: fn() -> u64,
}

impl SlackBackend {
    /// A backend whose transport comes from `factory`, given the validated configuration.
    #[must_use]
    pub fn new(factory: TransportFactory) -> Self {
        Self {
            factory,
            cancel: Arc::new(SharedCancel::default()),
            now_millis: unix_millis,
        }
    }

    /// A backend using [`default_transport`].
    #[must_use]
    pub fn with_default_transport() -> Self {
        Self::new(Box::new(default_transport))
    }

    /// Replace the wall clock, for tests.
    #[must_use]
    pub fn with_clock(mut self, now_millis: fn() -> u64) -> Self {
        self.now_millis = now_millis;
        self
    }

    /// The capabilities this backend reports.
    ///
    /// # Panics
    ///
    /// Panics only if the constant capability set violates the public domain bounds.
    #[must_use]
    pub fn slack_capabilities() -> BackendCapabilities {
        BackendCapabilities::new(
            BACKEND_NAME,
            ReplaySupport::Cursor,
            true,
            NonZeroU16::new(1).expect("one is nonzero"),
            vec![
                EventKind::MessageCreated,
                EventKind::Checkpoint,
                EventKind::Gap,
                EventKind::Heartbeat,
            ],
        )
        .expect("constant capabilities are valid")
    }
}

impl ChatSubscriptionBackend for SlackBackend {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(CancelHandle(Arc::clone(&self.cancel)))
    }

    fn capabilities(&self) -> BackendCapabilities {
        Self::slack_capabilities()
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        if self.cancel.cancelled.load(Ordering::SeqCst) {
            return Err(failure(
                "cancelled",
                "the subscription was cancelled before it opened",
                true,
            ));
        }
        let config = PluginConfig::from_request(request)?;
        let diagnostics = Diagnostics::new(config.diagnostics_log.clone(), "slack-plugin");
        let channels: BTreeSet<String> = request
            .channel_ids()
            .iter()
            .map(|channel| channel.as_str().to_owned())
            .collect();
        if channels.is_empty() || channels.len() > MAX_CHANNELS {
            return Err(invalid(format!(
                "between 1 and {MAX_CHANNELS} Slack channels are required"
            )));
        }
        if let Some(bad) = channels
            .iter()
            .find(|channel| !ids::valid_channel_id(channel))
        {
            return Err(invalid(format!("{bad:?} is not a Slack conversation ID")));
        }
        let senders: BTreeSet<String> = request
            .allowed_senders()
            .iter()
            .map(|sender| sender.as_str().to_owned())
            .collect();
        if let Some(bad) = senders.iter().find(|sender| !ids::valid_user_id(sender)) {
            return Err(invalid(format!("{bad:?} is not a Slack user ID")));
        }
        let now = (self.now_millis)();
        let resume = request
            .resume_from()
            .map(|cursor| cursor.as_str().to_owned());
        let decoded = resume.as_deref().map(Cursor::decode);
        let (catchup, floor_sequence, created_floor_millis, initial) = match &decoded {
            None => {
                let since = now.saturating_sub(config.initial_backfill_seconds * 1_000);
                let catchup = if config.initial_backfill_seconds == 0 {
                    Catchup::None
                } else {
                    Catchup::SinceSeconds(config.initial_backfill_seconds + 5)
                };
                (
                    catchup,
                    None,
                    since,
                    Some(Initial::Checkpoint(Cursor::Since(since).encode())),
                )
            }
            Some(Some(Cursor::Sequence(sequence))) => {
                (Catchup::Sequence(*sequence), Some(*sequence), 0, None)
            }
            Some(Some(Cursor::Since(since))) => {
                let seconds = now.saturating_sub(*since).div_ceil(1_000) + 5;
                (Catchup::SinceSeconds(seconds), None, *since, None)
            }
            Some(None) => (
                Catchup::None,
                None,
                now,
                Some(Initial::Gap(
                    resume.clone().unwrap_or_default(),
                    "the saved cursor is not a Slack provider cursor".to_owned(),
                )),
            ),
        };
        let mut transport = (self.factory)(&config)?;
        let spec = SubscribeSpec {
            channel_ids: channels.iter().cloned().collect(),
            catchup,
        };
        let stream = transport.open_stream(&spec).map_err(|error| {
            failure(
                &error.code,
                format!("opening the Slack event stream failed: {}", error.detail),
                error.retryable,
            )
        })?;
        self.cancel.register(stream.canceller());
        diagnostics.log(&format!(
            "subscribed version={} channels={} catchup={:?}",
            crate::VERSION,
            spec.channel_ids.join(","),
            spec.catchup
        ));
        Ok(Box::new(SlackDriver {
            stream,
            config,
            channels,
            senders,
            next_sequence: 1,
            pending: None,
            floor_sequence,
            created_floor_micros: u128::from(created_floor_millis) * 1_000,
            last_cursor: resume,
            initial,
            cancel: Arc::clone(&self.cancel),
            diagnostics,
            _transport: transport,
        }))
    }
}

enum Initial {
    Checkpoint(String),
    Gap(String, String),
}

struct SlackDriver {
    stream: Box<dyn EventStream>,
    config: PluginConfig,
    channels: BTreeSet<String>,
    senders: BTreeSet<String>,
    next_sequence: u64,
    pending: Option<DeliveryId>,
    floor_sequence: Option<u64>,
    created_floor_micros: u128,
    last_cursor: Option<String>,
    initial: Option<Initial>,
    cancel: Arc<SharedCancel>,
    diagnostics: Diagnostics,
    _transport: Box<dyn SlackTransport>,
}

impl SlackDriver {
    fn take_sequence(&mut self) -> Result<EventSequence, BackendFailure> {
        let sequence = EventSequence::new(self.next_sequence)
            .map_err(|error| failure("sequence_invalid", error.detail().to_owned(), false))?;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| failure("sequence_exhausted", "protocol sequence exhausted", false))?;
        Ok(sequence)
    }

    fn batch(
        &mut self,
        cursor: String,
        event: CommittableEvent,
    ) -> Result<SubscriptionItem, BackendFailure> {
        let sequence = self.take_sequence()?;
        let delivery = DeliveryId::new(format!("slack-delivery-{}", sequence.get()))
            .map_err(|error| failure("delivery_invalid", error.detail().to_owned(), false))?;
        let provider_cursor = ProviderCursor::new(cursor.clone())
            .map_err(|error| failure("cursor_invalid", error.detail().to_owned(), false))?;
        let batch = DeliveryBatch::new(sequence, provider_cursor, delivery.clone(), vec![event])
            .map_err(|error| failure("batch_invalid", error.detail().to_owned(), false))?;
        self.pending = Some(delivery);
        self.last_cursor = Some(cursor);
        Ok(SubscriptionItem::Batch(batch))
    }

    fn gap(&mut self, cursor: String, reason: &str) -> Result<SubscriptionItem, BackendFailure> {
        self.diagnostics.log(&format!("reporting gap: {reason}"));
        let gap = ReconciliationGap::new(Some(crate::transport::bounded(reason)))
            .map_err(|error| failure("gap_invalid", error.detail().to_owned(), false))?;
        self.batch(cursor, CommittableEvent::Gap(gap))
    }

    /// The inbound message for an admitted Slack message, or why it is not a request.
    fn admit(&self, message: &SlackMessage) -> Result<InboundMessage, &'static str> {
        if !self.channels.contains(&message.channel_id) {
            return Err("other channel");
        }
        if message
            .subtype
            .as_deref()
            .is_some_and(|subtype| !MESSAGE_SUBTYPES.contains(&subtype))
        {
            return Err("non-message subtype");
        }
        // The event stream's slack_message extension fills app_id with the
        // app the delivery came through, not the author (live finding
        // 2026-10-14: the owner's own message arrived with app_id set and was
        // dropped), so app_id never decides authorship.
        //
        // A bot_id does not decide it either when the user is an allowed
        // sender. A person who posts through an app with their own user token
        // gets that app's bot_id on the message while slack_user_id stays
        // theirs (live finding 2026-10-19: three of the owner's thread replies
        // arrived with slack_user_id U0C23P5291P and bot_id B0C6X0ASTSL and
        // were dropped as bot-authored). The sender allowlist is the identity
        // check. The bridge's own posts are written as its app's bot user, a
        // different user ID, which is never an allowed sender, so they cannot
        // come back as requests.
        let Some(user) = message
            .user_id
            .as_deref()
            .filter(|user| self.senders.contains(*user))
        else {
            return Err(if message.bot_id.is_some() {
                "bot author"
            } else {
                "sender not allowed"
            });
        };
        let created = ids::ts_micros(&message.message_ts).ok_or("invalid message ts")?;
        if created < self.created_floor_micros {
            return Err("created before the lineage start");
        }
        let mut text = ids::unescape_text(&message.text);
        if self
            .config
            .ignore_text_prefixes
            .iter()
            .any(|prefix| text.trim_start().starts_with(prefix.as_str()))
        {
            return Err("ignored prefix");
        }
        if text.len() > chat_subscription::MAX_MESSAGE_TEXT_BYTES {
            let mut boundary = chat_subscription::MAX_MESSAGE_TEXT_BYTES - TRUNCATION_MARKER.len();
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            text.truncate(boundary);
            text.push_str(TRUNCATION_MARKER);
        }
        let root = message
            .thread_ts
            .as_deref()
            .filter(|thread| ids::valid_ts(thread))
            .unwrap_or(&message.message_ts);
        let thread_reply = root != message.message_ts;
        let created_at = ids::ts_rfc3339(&message.message_ts).ok_or("invalid message ts")?;
        let mut payload = Map::new();
        payload.insert(
            "channel_id".to_owned(),
            Value::String(message.channel_id.clone()),
        );
        payload.insert(
            "message_ts".to_owned(),
            Value::String(message.message_ts.clone()),
        );
        payload.insert("thread_ts".to_owned(), Value::String(root.to_owned()));
        payload.insert("user_id".to_owned(), Value::String(user.to_owned()));
        if let Some(team) = &message.team_id {
            payload.insert("team_id".to_owned(), Value::String(team.clone()));
        }
        let provider_payload =
            ProviderPayload::new(PAYLOAD_SCHEMA, payload).map_err(|_| "payload too large")?;
        InboundMessage::new(
            ChannelId::new(message.channel_id.clone()).map_err(|_| "invalid channel")?,
            MessageId::new(ids::message_resource(
                &message.channel_id,
                &message.message_ts,
            ))
            .map_err(|_| "invalid message id")?,
            ThreadId::new(ids::thread_resource(&message.channel_id, root))
                .map_err(|_| "invalid thread id")?,
            SenderId::new(user.to_owned()).map_err(|_| "invalid sender")?,
            text,
            created_at,
            thread_reply,
        )
        .map(|inbound| inbound.with_provider_payload(provider_payload))
        .map_err(|_| "invalid message")
    }
}

impl ChatSubscriptionDriver for SlackDriver {
    fn cancellation(&self) -> Arc<dyn ChatSubscriptionCancellation> {
        Arc::new(CancelHandle(Arc::clone(&self.cancel)))
    }

    fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if self.pending.is_some() {
            return Err(failure(
                "backpressure_violation",
                "next_item was called before the outstanding batch was acknowledged",
                false,
            ));
        }
        match self.initial.take() {
            Some(Initial::Checkpoint(cursor)) => {
                return self.batch(cursor, CommittableEvent::Checkpoint).map(Some);
            }
            Some(Initial::Gap(cursor, reason)) => return self.gap(cursor, &reason).map(Some),
            None => {}
        }
        loop {
            if self.cancel.cancelled.load(Ordering::SeqCst) {
                return Err(failure("cancelled", "the subscription was cancelled", true));
            }
            match self.stream.next(self.config.heartbeat_interval) {
                StreamWait::Idle | StreamWait::Record(StreamRecord::Heartbeat) => {
                    let sequence = self.take_sequence()?;
                    return Ok(Some(SubscriptionItem::Heartbeat(Heartbeat::new(sequence))));
                }
                StreamWait::Cancelled => {
                    return Err(failure("cancelled", "the subscription was cancelled", true));
                }
                StreamWait::Closed(reason) => {
                    self.diagnostics.log(&format!("stream closed: {reason}"));
                    return Err(failure("stream_closed", reason, true));
                }
                StreamWait::Record(StreamRecord::Notice(notice)) => {
                    self.diagnostics.log(&notice);
                }
                StreamWait::Record(StreamRecord::Gap(reason)) => {
                    let cursor = self
                        .last_cursor
                        .clone()
                        .unwrap_or_else(|| Cursor::Since(0).encode());
                    return self.gap(cursor, &reason).map(Some);
                }
                StreamWait::Record(StreamRecord::Event {
                    sequence,
                    event_type,
                    message,
                }) => {
                    if self.floor_sequence.is_some_and(|floor| sequence <= floor) {
                        continue;
                    }
                    self.floor_sequence = Some(sequence);
                    let cursor = Cursor::Sequence(sequence).encode();
                    let event = match message.as_ref().map(|message| self.admit(message)) {
                        Some(Ok(inbound)) => {
                            self.diagnostics.log(&format!(
                                "event {sequence}: request {}",
                                inbound.message_id().as_str()
                            ));
                            CommittableEvent::message_created(inbound)
                        }
                        Some(Err(reason)) => {
                            // Identifiers only, never text: enough to see why an expected
                            // message was not admitted (for example an unlisted sender ID).
                            let author = message.as_ref().map_or("?", |message| {
                                message.user_id.as_deref().unwrap_or("none")
                            });
                            self.diagnostics.log(&format!(
                                "event {sequence}: checkpoint ({reason}; user {author})"
                            ));
                            CommittableEvent::Checkpoint
                        }
                        None => {
                            self.diagnostics.log(&format!(
                                "event {sequence}: checkpoint ({event_type} without a message)"
                            ));
                            CommittableEvent::Checkpoint
                        }
                    };
                    return self.batch(cursor, event).map(Some);
                }
            }
        }
    }

    fn acknowledge(&mut self, delivery_id: &DeliveryId) -> Result<(), BackendFailure> {
        match &self.pending {
            Some(pending) if pending == delivery_id => {
                self.pending = None;
                Ok(())
            }
            Some(_) => Err(failure(
                "delivery_mismatch",
                "acknowledgement names another delivery",
                false,
            )),
            None => Err(failure(
                "no_pending_delivery",
                "no delivery awaits acknowledgement",
                false,
            )),
        }
    }

    fn close(&mut self) -> Result<(), BackendFailure> {
        self.diagnostics.log("closing the event stream");
        self.stream
            .close()
            .map_err(|detail| failure("stream_close_failed", detail, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip() {
        for cursor in [Cursor::Sequence(18_230), Cursor::Since(1_700_000_000_000)] {
            assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        }
        assert_eq!(Cursor::decode("18230"), None);
        assert_eq!(Cursor::decode("slack1:seq:x"), None);
    }
}
