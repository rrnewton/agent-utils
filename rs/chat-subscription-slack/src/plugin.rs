//! The inbound subscription backend.
//!
//! [`SlackBackend`] implements the provider-neutral [`ChatSubscriptionBackend`] so the generic
//! process server (`chat_subscription_plugin::serve_stdio`) can host it. One subscription opens one
//! transport event stream for the configured channels and maps each sequenced event to one batch:
//!
//! * a `message.created` event in a configured channel, from an allowed sender, not an edit or
//!   join notice, and not starting with an ignored prefix, becomes a
//!   `message_created` event. Its thread is the thread root's `ts` (the message's own `ts` for a
//!   top-level message), so replies to a thread route back into that thread;
//! * an otherwise eligible new message from another sender, or with an ignored prefix, becomes
//!   a metadata-only `message_rejected` candidate. The outbound helper verifies the author's
//!   account is human before adding a rejection reaction; message annotations are insufficient;
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
//!
//! # Keeping the cursor inside the stream's retention
//!
//! The stream retains events for a bounded time (about eight hours) and its sequence counts
//! every event the client receives, while the client passes on only the configured channel's
//! messages. A cursor that moved only with those messages fell out of the retention in a quiet
//! channel, and the next restart found a gap (#399 slack-quiet-cursor). So when the transport
//! proves a later position ([`StreamRecord::Position`]: every event at or before it has already
//! been returned; see [`crate::cli::PositionProof`] for how the client's output proves one),
//! the driver commits a `checkpoint` there. A proof comes once per stream, after its catch-up,
//! so when the cursor has not moved for `cursor_refresh_seconds` the driver closes the stream
//! and opens a new one from its committed sequence; that stream's catch-up drains to the head
//! and proves it. Reopening loses nothing: the new stream replays everything after the
//! committed sequence, and no batch is outstanding at that point. A time cursor
//! (`slack1:since:`) is never moved this way, since its resume is not a sequence catch-up.
//!
//! A reopened stream that proves nothing (the override subscription mode, a failed catch-up,
//! client output this plugin does not recognise) would restart catch-up from the same and
//! ever older sequence, until that fell out of the retention itself. So staleness is measured
//! from the last cursor move, never from a reopen; the override mode never reopens; and once a
//! reopened stream has gone `cursor_refresh_seconds` without a proof, the driver stops
//! reopening for the life of the subscription and says so in the diagnostics log. If an event
//! ever arrives at or before a proven position without having been delivered, the proof was
//! wrong and events may be lost, so the driver reports a `gap` rather than skip it.
//!
//! # Whose events the stream carries
//!
//! The event stream carries the channel events delivered to the identity the client runs as.
//! Slack delivers channel message events to a human member's own subscription, not to an app's
//! Crewmate: on 2026-10-19 a tail run as the posting app's Crewmate subscribed and received no
//! event for hours, while the same tail run as the owner caught up at once (#390
//! slack-inbound-identity). So `cli_path` must run as a member, while the outbound helper's own
//! `--cli` may run as the Crewmate. The diagnostics log warns once when the client's standard
//! error names a Crewmate, and notes once when a stream stays subscribed for
//! [`QUIET_NOTICE_MILLIS`] with no event at all, which in a busy channel points the same way.
//!
//! # Rescan: messages the stream never carries
//!
//! The stream does not carry every message. A message with an attached file never arrives at all
//! (#415 slack-file-share-delivery; #485 chat-file-messages: nine of the owner's screenshots in four
//! days), so neither its text nor its file reached the agent. So every `rescan_interval_seconds`
//! (90 by default; 0 turns it off), when the stream is idle, the driver lists each configured
//! channel's history from the last `rescan_window_seconds` (a day by default) and the threads in it
//! with replies it has not listed, through [`SlackTransport::scan_messages`] as the same member the
//! tail runs as. A message at least [`rescan::GRACE_MILLIS`] old that the stream did not carry and
//! the rescan has not delivered goes through the same admission as a stream message, and an
//! admitted one becomes its own batch. Its text ends with a note that it came by the rescan
//! and, when it has files, their IDs and the exact command to download each to a temporary path;
//! a stream message with files gets the same file note.
//!
//! A rescued batch's cursor is the cursor before it with `;rescued:<channel>/<ts>` appended, so a
//! subscription resumed from it resumes the stream where the cursor before it did. Which messages
//! the stream carried and the rescan delivered, and the one rescued batch not yet acknowledged,
//! are kept durably in [`rescan::Ledger`] beside the diagnostics log; without a diagnostics log the
//! rescan stays off. The ledger is what lets the bridge admit a message seen both ways once, never
//! offer an edited message again as new, and receive an identical rescued batch again after a
//! restart (see the [`rescan`] module). A pass is bounded in pages, threads, messages and time,
//! backs off exponentially after a failed listing, and logs what it rescued.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chat_subscription::{
    BackendCapabilities, BackendFailure, CancellationError, ChannelId, ChatSubscriptionBackend,
    ChatSubscriptionCancellation, ChatSubscriptionDriver, CommittableEvent, DeliveryBatch,
    DeliveryId, EventKind, EventSequence, Heartbeat, InboundMessage, MessageId, ProviderCursor,
    ProviderPayload, ReconciliationGap, RejectedMessage, RejectionReason, ReplaySupport, SenderId,
    SubscribeRequest, SubscriptionItem, ThreadId,
};
use serde_json::{Map, Value};

use crate::cli::{CliOptions, CliTransport, TailOptions};
use crate::diag::{unix_millis, Diagnostics};
use crate::ids;
use crate::rescan::{self, message_key, Ledger, PendingRescue};
use crate::transport::{
    Catchup, EventStream, ScanQuery, SlackMessage, SlackTransport, StreamCancel, StreamRecord,
    StreamWait, SubscribeSpec, TransportError, WebApiTransport,
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
/// Default `cursor_refresh_seconds`: well inside the stream's retention of about eight hours.
pub const DEFAULT_CURSOR_REFRESH: Duration = Duration::from_secs(2 * 60 * 60);
/// How long a stream may stay subscribed without any event before the diagnostics log notes it.
pub const QUIET_NOTICE_MILLIS: u64 = 30 * 60 * 1_000;
/// How many of the newest message timestamps a subscription remembers having carried, to notice a
/// thread reply whose root it never carried.
const MAX_CARRIED: usize = 4_096;
const CREWMATE_WARNING: &str = "warning: the event stream client's output names a Crewmate, \
so the tail may run as an app's Crewmate; Slack delivers channel message events to a human \
member's own subscription, not to an app's Crewmate, so such a tail receives none. Point cli_path \
at a client that runs as the human member, and keep the Crewmate for the outbound helper's --cli";

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
    /// Reopen the stream when a sequence cursor has not moved for this long; `None` never does.
    pub cursor_refresh: Option<Duration>,
    /// A new lineage also delivers messages created this many seconds before start.
    pub initial_backfill_seconds: u64,
    /// Messages starting with one of these (after leading whitespace) are never delivered.
    pub ignore_text_prefixes: Vec<String>,
    /// Diagnostics file.
    pub diagnostics_log: Option<PathBuf>,
    /// How often to rescan for messages the stream did not carry; `None` never does.
    pub rescan_interval: Option<Duration>,
    /// How far back a rescan looks.
    pub rescan_window: Duration,
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
            "cursor_refresh_seconds",
            "initial_backfill_seconds",
            "ignore_text_prefixes",
            "diagnostics_log",
            "rescan_interval_seconds",
            "rescan_window_seconds",
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
                    prefix_arguments: Vec::new(),
                    identity: crate::cli::ClientIdentity::User,
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
        let cursor_refresh = match optional_seconds(data, "cursor_refresh_seconds", 0..=21_600)? {
            None => Some(DEFAULT_CURSOR_REFRESH),
            Some(0) => None,
            Some(seconds) if seconds < 600 => {
                return Err(invalid(
                    "cursor_refresh_seconds must be 0 (never) or from 600 to 21600",
                ))
            }
            Some(seconds) => Some(Duration::from_secs(seconds)),
        };
        let rescan_interval = match optional_seconds(data, "rescan_interval_seconds", 0..=3_600)? {
            None => Some(rescan::DEFAULT_INTERVAL),
            Some(0) => None,
            Some(seconds) if seconds < 30 => {
                return Err(invalid(
                    "rescan_interval_seconds must be 0 (never) or from 30 to 3600",
                ))
            }
            Some(seconds) => Some(Duration::from_secs(seconds)),
        };
        let rescan_window = optional_seconds(data, "rescan_window_seconds", 600..=604_800)?
            .map_or(rescan::DEFAULT_WINDOW, Duration::from_secs);
        Ok(Self {
            transport,
            cli,
            heartbeat_interval: Duration::from_secs(
                optional_seconds(data, "heartbeat_interval_seconds", 1..=300)?.unwrap_or(30),
            ),
            cursor_refresh,
            initial_backfill_seconds: optional_seconds(
                data,
                "initial_backfill_seconds",
                0..=86_400,
            )?
            .unwrap_or(0),
            ignore_text_prefixes,
            diagnostics_log: optional_string(data, "diagnostics_log")?.map(PathBuf::from),
            rescan_interval,
            rescan_window,
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

    /// Decode a cursor produced by [`Self::encode`], or a rescued batch's cursor built on one
    /// ([`rescan::rescue_cursor`]), which resumes where its base does.
    #[must_use]
    pub fn decode(value: &str) -> Option<Self> {
        let rest = rescan::strip_rescue(value).strip_prefix(CURSOR_PREFIX)?;
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
                EventKind::MessageRejected,
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
        let refusals = crate::refusals::RefusalRecorder::beside(config.diagnostics_log.as_deref());
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
        let override_mode = config
            .cli
            .as_ref()
            .is_some_and(|cli| cli.tail.mode.as_deref() == Some("override"));
        let cursor_refresh = config.cursor_refresh.filter(|_| !override_mode);
        if override_mode && config.cursor_refresh.is_some() {
            diagnostics.log(
                "cursor refresh is off: a tail in the override subscription mode cannot prove \
                 a stream position",
            );
        }
        let rescan = open_rescan(&config, &diagnostics, resume.as_deref(), now);
        let download_command = config.cli.as_ref().map(|cli| {
            let mut words: Vec<String> = Vec::new();
            if !cli.environment.is_empty() {
                words.push("env".to_owned());
                words.extend(
                    cli.environment
                        .iter()
                        .map(|(name, value)| rescan::shell_word(&format!("{name}={value}"))),
                );
            }
            words.push(rescan::shell_word(&cli.program.to_string_lossy()));
            words.extend(
                cli.prefix_arguments
                    .iter()
                    .map(|word| rescan::shell_word(word)),
            );
            words.join(" ")
        });
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
            refusals,
            now_millis: self.now_millis,
            opened_at_millis: now,
            advanced_at_millis: now,
            cursor_refresh,
            reopened_at_millis: None,
            proven_floor: None,
            // The resume position was delivered by an earlier subscription.
            last_event_sequence: floor_sequence,
            channel_ids: spec.channel_ids,
            events_seen: false,
            quiet_noted: false,
            crewmate_warned: false,
            carried: BTreeSet::new(),
            carried_since_micros: None,
            uncarried_noted: BTreeSet::new(),
            rescan,
            download_command,
            transport,
        }))
    }
}

/// The rescan state for a new subscription, settling a rescued batch the previous one left
/// unacknowledged: one the bridge committed (it resumes from that batch's cursor) is recorded
/// rescued; one it may have rolled back (it resumes from the cursor before) is offered again
/// first, identical; any other is recorded rescued too, so it is never offered with a possibly
/// different text, and logged.
fn open_rescan(
    config: &PluginConfig,
    diagnostics: &Diagnostics,
    resume: Option<&str>,
    now_millis: u64,
) -> Option<Rescan> {
    let interval = config.rescan_interval?;
    let Some(path) = Ledger::path_beside(config.diagnostics_log.as_deref()) else {
        diagnostics.log(
            "rescan is off: it keeps its ledger beside diagnostics_log, which is not configured, \
             so messages the event stream does not carry (such as ones with files) are not rescued",
        );
        return None;
    };
    let (mut ledger, note) = Ledger::load(path, u128::from(now_millis) * 1_000);
    if let Some(note) = note {
        diagnostics.log(&format!("rescan: {note}"));
    }
    let mut replay = None;
    if let Some(pending) = ledger.pending.clone() {
        if resume == Some(pending.cursor.as_str()) {
            diagnostics.log(&format!(
                "rescan: the bridge committed the rescued message {} before this subscription",
                pending.key
            ));
            ledger.mark_rescued(&pending.key);
            ledger.pending = None;
        } else if resume == pending.prior_cursor.as_deref() {
            diagnostics.log(&format!(
                "rescan: offering the unacknowledged rescued message {} again first",
                pending.key
            ));
            replay = Some(pending);
        } else {
            diagnostics.log(&format!(
                "rescan: the unacknowledged rescued message {} belongs to another cursor than \
                 this subscription's; it is recorded delivered and not offered again",
                pending.key
            ));
            ledger.mark_rescued(&pending.key);
            ledger.pending = None;
        }
        if let Err(error) = ledger.save() {
            diagnostics.log(&format!("rescan: the ledger could not be saved: {error}"));
        }
    }
    diagnostics.log(&format!(
        "rescan: every {} s over the last {} s; ledger {}",
        interval.as_secs(),
        config.rescan_window.as_secs(),
        ledger.path().display()
    ));
    Some(Rescan {
        interval,
        window: config.rescan_window,
        ledger,
        threads: BTreeMap::new(),
        queue: VecDeque::new(),
        next_due_millis: now_millis,
        failures: 0,
        stopped: false,
        delivering: None,
        replay,
        noted: BTreeSet::new(),
        truncation_noted: false,
    })
}

/// The rescan's state within one subscription.
struct Rescan {
    interval: Duration,
    window: Duration,
    ledger: Ledger,
    // The newest reply each thread was listed through, by `C…/<root ts>`.
    threads: BTreeMap<String, u128>,
    // Messages found by the last pass and not yet offered, oldest first.
    queue: VecDeque<SlackMessage>,
    next_due_millis: u64,
    // Consecutive failed passes.
    failures: u32,
    // The transport cannot list; the ledger still guards the stream.
    stopped: bool,
    // The rescued batch awaiting acknowledgement, by ledger key.
    delivering: Option<String>,
    // An unacknowledged rescued batch to offer again before anything else.
    replay: Option<PendingRescue>,
    // Allowed senders' messages a pass did not admit, logged once each.
    noted: BTreeSet<String>,
    truncation_noted: bool,
}

const MAX_NOTED: usize = 1_024;
const MAX_TRACKED_THREADS: usize = 1_024;

fn micros_ts(micros: u128) -> String {
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
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
    refusals: crate::refusals::RefusalRecorder,
    now_millis: fn() -> u64,
    opened_at_millis: u64,
    // When the sequence cursor last moved, or the subscription opened; never a reopen.
    advanced_at_millis: u64,
    // `config.cursor_refresh`, cleared when reopening cannot prove a position.
    cursor_refresh: Option<Duration>,
    // When the stream was last reopened, while no position has been proven since.
    reopened_at_millis: Option<u64>,
    // The highest position committed from a proof in this subscription.
    proven_floor: Option<u64>,
    // The highest event sequence known delivered: the resume position, or any event read.
    last_event_sequence: Option<u64>,
    channel_ids: Vec<String>,
    // Whether the stream has carried any sequenced event, replayed or new.
    events_seen: bool,
    quiet_noted: bool,
    crewmate_warned: bool,
    // The newest `MAX_CARRIED` message timestamps the stream carried in the configured channels,
    // and the oldest instant from which it remembers every one: see `note_carried`.
    carried: BTreeSet<u128>,
    carried_since_micros: Option<u128>,
    // Thread roots already noted as never carried, so each is noted once.
    uncarried_noted: BTreeSet<u128>,
    rescan: Option<Rescan>,
    // The shell-quoted client command a file note tells the agent to run.
    download_command: Option<String>,
    transport: Box<dyn SlackTransport>,
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

    /// Reopen the stream from the committed sequence when it has not moved for
    /// `cursor_refresh`, so the new stream's catch-up can prove a position near the head.
    /// Called only between batches, when none awaits acknowledgement.
    fn refresh_cursor(&mut self) -> Result<(), BackendFailure> {
        let (Some(refresh), Some(floor)) = (self.cursor_refresh, self.floor_sequence) else {
            return Ok(());
        };
        let now = (self.now_millis)();
        let still = now.saturating_sub(self.advanced_at_millis);
        if u128::from(still) < refresh.as_millis() {
            return Ok(());
        }
        if let Some(reopened) = self.reopened_at_millis {
            if u128::from(now.saturating_sub(reopened)) < refresh.as_millis() {
                return Ok(());
            }
            // Another reopen would restart catch-up from the same, older sequence.
            self.cursor_refresh = None;
            self.diagnostics.log(&format!(
                "cursor refresh stopped: the stream reopened from sequence {floor} proved no \
                 position in {} minutes; the cursor stays there until an event moves it",
                now.saturating_sub(reopened) / 60_000
            ));
            return Ok(());
        }
        // Whatever the old stream still holds is replayed by the new one, which starts after
        // the committed sequence.
        self.reopened_at_millis = Some(now);
        self.diagnostics.log(&format!(
            "cursor refresh: the cursor has not moved for {} minutes; reopening the event stream \
             from sequence {floor}",
            still / 60_000
        ));
        if let Err(detail) = self.stream.close() {
            return Err(failure("stream_close_failed", detail, true));
        }
        let spec = SubscribeSpec {
            channel_ids: self.channel_ids.clone(),
            catchup: Catchup::Sequence(floor),
        };
        self.stream = self.transport.open_stream(&spec).map_err(|error| {
            failure(
                &error.code,
                format!("reopening the Slack event stream failed: {}", error.detail),
                error.retryable,
            )
        })?;
        self.cancel.register(self.stream.canceller());
        Ok(())
    }

    /// Remember that the stream carried `message`, and note in the diagnostics log a thread reply
    /// whose root it has not carried although the root is newer than the lineage start and than
    /// the first message this subscription carried.
    ///
    /// The event stream does not publish every message: on 2026-10-21 the owner's thread root
    /// with an attached file never reached his subscription, while his plain reply in that thread
    /// did; his queue held no event of any type for the root (#415 slack-file-share-delivery).
    /// Such a message cannot be delivered from the stream (the rescan delivers it instead; see the
    /// module documentation), so the log also names the root for an operator to find. Only the log: a batch is
    /// built from its event alone, so a replay stays identical; a root may also merely be late,
    /// and the client documents that it emits no posts by other bots, so a bot's root looks the
    /// same and is no message from a person to alarm about.
    ///
    /// The instant from which every carried message is remembered is the first one carried, raised
    /// as the oldest are forgotten, and never lowered: a message that arrives late with an older
    /// timestamp does not make the roots before it look missed.
    fn note_carried(&mut self, sequence: u64, message: &SlackMessage) {
        if !self.channels.contains(&message.channel_id) {
            return;
        }
        let Some(created) = ids::ts_micros(&message.message_ts) else {
            return;
        };
        let since = self.carried_since_micros;
        let root = message
            .thread_ts
            .as_deref()
            .filter(|root| *root != message.message_ts && ids::valid_ts(root));
        if let (Some(root), Some(since)) = (root, since) {
            if ids::ts_micros(root).is_some_and(|at| {
                at > since
                    && at >= self.created_floor_micros
                    && !self.carried.contains(&at)
                    && self.uncarried_noted.insert(at)
            }) {
                self.diagnostics.log(&format!(
                    "event {sequence}: thread reply {} (user {}) is in thread {root}, whose root \
                     this stream has not carried although it carried an earlier message; the event \
                     stream does not publish some messages, such as one with a file, so unless it \
                     is only late, that root reaches the agent only through the rescan
                    message.message_ts,
                    message.user_id.as_deref().unwrap_or("none")
                ));
                if self.uncarried_noted.len() > MAX_CARRIED {
                    self.uncarried_noted.pop_first();
                }
            }
        }
        if since.is_none() {
            self.carried_since_micros = Some(created);
        }
        self.carried.insert(created);
        if self.carried.len() > MAX_CARRIED {
            // Up to the oldest one forgotten, nothing is known to have been carried.
            if let Some(forgotten) = self.carried.pop_first() {
                self.carried_since_micros = self.carried_since_micros.max(Some(forgotten));
            }
        }
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

    /// The inbound message for an admitted Slack message, or why it is not a request. Stream and
    /// rescan share it; `rescued` marks a message the rescan found.
    fn admit(&self, message: &SlackMessage, rescued: bool) -> Result<InboundMessage, &'static str> {
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
        // theirs (live finding 2026-10-19: three allowed human thread replies
        // with this annotation were dropped as bot-authored). The sender allowlist is the identity
        // check. The bridge's own posts are written as its app's bot user, a
        // different user ID, which is never an allowed sender, so they cannot
        // come back as requests.
        let Some(user) = message
            .user_id
            .as_deref()
            .filter(|user| self.senders.contains(*user))
        else {
            return Err("sender not allowed");
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
        let note = rescan::delivery_note(
            self.download_command.as_deref(),
            &message.channel_id,
            &message.file_ids,
            rescued,
        );
        let limit = chat_subscription::MAX_MESSAGE_TEXT_BYTES - note.len();
        if text.len() > limit {
            let mut boundary = limit - TRUNCATION_MARKER.len();
            while !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            text.truncate(boundary);
            text.push_str(TRUNCATION_MARKER);
        }
        text.push_str(&note);
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
        rescan::annotate_payload(&mut payload, &message.file_ids, rescued);
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

    /// For a stream event's message in a configured channel: a checkpoint in its place when the
    /// rescan already delivered it, and otherwise a durable record that the stream carried it, so
    /// the rescan never offers it.
    fn rescan_guard(&mut self, sequence: u64, message: &SlackMessage) -> Option<CommittableEvent> {
        let rescan = self.rescan.as_mut()?;
        if !self.channels.contains(&message.channel_id) || !ids::valid_ts(&message.message_ts) {
            return None;
        }
        let key = message_key(&message.channel_id, &message.message_ts);
        if rescan.ledger.rescued(&key) {
            self.diagnostics.log(&format!(
                "event {sequence}: checkpoint ({key} was already delivered by the rescan)"
            ));
            return Some(CommittableEvent::Checkpoint);
        }
        if rescan.ledger.mark_carried(&key) {
            rescan
                .ledger
                .prune(u128::from((self.now_millis)()) * 1_000, rescan.window);
            if let Err(error) = rescan.ledger.save() {
                self.diagnostics.log(&format!(
                    "event {sequence}: the rescan ledger could not record {key} carried: {error}"
                ));
            }
        }
        None
    }

    /// The next rescued batch: an unacknowledged one to offer again, else the oldest message the
    /// last pass found that the stream has not carried since. Its pending record is saved before
    /// it is offered; if that fails nothing is offered, and the next pass finds the message again.
    fn next_rescued(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        let Some(mut rescan) = self.rescan.take() else {
            return Ok(None);
        };
        let item = self.next_rescued_from(&mut rescan);
        self.rescan = Some(rescan);
        item
    }

    fn next_rescued_from(
        &mut self,
        rescan: &mut Rescan,
    ) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if let Some(pending) = rescan.replay.take() {
            rescan.delivering = Some(pending.key.clone());
            self.diagnostics.log(&format!(
                "rescan: request {} again at {}",
                pending.message.message_id().as_str(),
                pending.cursor
            ));
            return self
                .batch(
                    pending.cursor,
                    CommittableEvent::message_created(pending.message),
                )
                .map(Some);
        }
        while let Some(message) = rescan.queue.pop_front() {
            let key = message_key(&message.channel_id, &message.message_ts);
            if rescan.ledger.carried(&key) || rescan.ledger.rescued(&key) {
                continue;
            }
            let Ok(inbound) = self.admit(&message, true) else {
                continue;
            };
            let base = self
                .last_cursor
                .clone()
                .unwrap_or_else(|| Cursor::Since(self.created_floor_millis()).encode());
            let cursor = rescan::rescue_cursor(&base, &key);
            rescan.ledger.pending = Some(PendingRescue {
                prior_cursor: self.last_cursor.clone(),
                cursor: cursor.clone(),
                key: key.clone(),
                message: inbound.clone(),
            });
            if let Err(error) = rescan.ledger.save() {
                rescan.ledger.pending = None;
                rescan.queue.clear();
                self.diagnostics.log(&format!(
                    "rescan: {key} was not offered because the ledger could not be saved \
                     ({error}); the next pass finds it again"
                ));
                return Ok(None);
            }
            rescan.delivering = Some(key);
            self.diagnostics.log(&format!(
                "rescan: request {} ({} file{}) at {cursor}",
                inbound.message_id().as_str(),
                message.file_ids.len(),
                if message.file_ids.len() == 1 { "" } else { "s" }
            ));
            return self
                .batch(cursor, CommittableEvent::message_created(inbound))
                .map(Some);
        }
        Ok(None)
    }

    fn created_floor_millis(&self) -> u64 {
        u64::try_from(self.created_floor_micros / 1_000).unwrap_or(u64::MAX)
    }

    /// Run a rescan pass when one is due, and offer the first message it found.
    fn rescan_if_due(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        let now = (self.now_millis)();
        let Some(mut rescan) = self.rescan.take() else {
            return Ok(None);
        };
        if !rescan.stopped && now >= rescan.next_due_millis {
            self.rescan_pass(&mut rescan, now);
        }
        let item = self.next_rescued_from(&mut rescan);
        self.rescan = Some(rescan);
        item
    }

    /// One bounded pass over every configured channel's history and the threads in it with
    /// replies not yet listed; see the module documentation.
    fn rescan_pass(&mut self, rescan: &mut Rescan, now: u64) {
        let now_micros = u128::from(now) * 1_000;
        let floor = self
            .created_floor_micros
            .max(rescan.ledger.tracking_since_micros)
            .max(now_micros.saturating_sub(rescan.window.as_micros()));
        let ripe = now_micros.saturating_sub(u128::from(rescan::GRACE_MILLIS) * 1_000);
        let started = Instant::now();
        let mut listings = 0_usize;
        let mut found = Vec::new();
        let mut failure: Option<TransportError> = None;
        let channels: Vec<String> = self.channels.iter().cloned().collect();
        let stop = |driver: &Self| {
            driver.cancel.cancelled.load(Ordering::SeqCst)
                || started.elapsed() >= rescan::PASS_BUDGET
        };
        'pass: for channel in &channels {
            let mut due_threads: Vec<(u128, String)> = Vec::new();
            let mut token: Option<String> = None;
            for page in 0..rescan::MAX_HISTORY_PAGES {
                if stop(self) {
                    break 'pass;
                }
                let oldest = micros_ts(floor);
                let query = ScanQuery {
                    channel_id: channel,
                    thread_ts: None,
                    oldest: &oldest,
                    page_token: token.as_deref(),
                };
                let listed = match self.transport.scan_messages(&query) {
                    Ok(listed) => listed,
                    Err(error) => {
                        failure = Some(error);
                        break 'pass;
                    }
                };
                listings += 1;
                for row in &listed.messages {
                    self.consider(rescan, &row.message, floor, ripe, &mut found);
                    let root = &row.message.message_ts;
                    if let Some(latest) = row
                        .latest_reply
                        .as_deref()
                        .and_then(ids::ts_micros)
                        .filter(|_| row.reply_count > 0)
                    {
                        let through = rescan
                            .threads
                            .get(&message_key(channel, root))
                            .copied()
                            .unwrap_or(0);
                        if latest >= floor && latest > through {
                            due_threads.push((latest, root.clone()));
                        }
                    }
                }
                token = listed.next_page_token;
                if token.is_none() {
                    break;
                }
                if page + 1 == rescan::MAX_HISTORY_PAGES && !rescan.truncation_noted {
                    rescan.truncation_noted = true;
                    self.diagnostics.log(&format!(
                        "rescan: {channel} has more than {} top-level messages in the window; a \
                         pass reads only the newest",
                        rescan::MAX_HISTORY_PAGES * 20
                    ));
                }
            }
            due_threads.sort_by(|a, b| b.cmp(a));
            due_threads.dedup_by(|a, b| a.1 == b.1);
            due_threads.truncate(rescan::MAX_THREADS_PER_PASS);
            for (latest, root) in due_threads {
                let key = message_key(channel, &root);
                let through = rescan.threads.get(&key).copied().unwrap_or(0);
                let oldest = micros_ts(floor.max(through));
                let mut listed_through = through;
                let mut token: Option<String> = None;
                let mut complete = false;
                for _ in 0..rescan::MAX_THREAD_PAGES {
                    if stop(self) {
                        break 'pass;
                    }
                    let query = ScanQuery {
                        channel_id: channel,
                        thread_ts: Some(&root),
                        oldest: &oldest,
                        page_token: token.as_deref(),
                    };
                    let listed = match self.transport.scan_messages(&query) {
                        Ok(listed) => listed,
                        Err(error) => {
                            failure = Some(error);
                            break 'pass;
                        }
                    };
                    listings += 1;
                    for row in &listed.messages {
                        if row.message.message_ts == root {
                            continue;
                        }
                        self.consider(rescan, &row.message, floor, ripe, &mut found);
                        if let Some(at) = ids::ts_micros(&row.message.message_ts) {
                            if at <= ripe {
                                listed_through = listed_through.max(at);
                            }
                        }
                    }
                    token = listed.next_page_token;
                    if token.is_none() {
                        complete = true;
                        break;
                    }
                }
                if complete && latest <= ripe {
                    listed_through = listed_through.max(latest);
                }
                rescan.threads.insert(key, listed_through);
            }
        }
        rescan.threads.retain(|_, through| *through >= floor);
        while rescan.threads.len() > MAX_TRACKED_THREADS {
            rescan.threads.pop_first();
        }
        rescan
            .queue
            .make_contiguous()
            .sort_by_key(|message| ids::ts_micros(&message.message_ts).unwrap_or(0));
        match failure {
            Some(error)
                if matches!(
                    error.code.as_str(),
                    "scan_unsupported" | "transport_unimplemented"
                ) =>
            {
                rescan.stopped = true;
                self.diagnostics.log(&format!(
                    "rescan stopped: the transport cannot list history ({}: {})",
                    error.code, error.detail
                ));
            }
            Some(error) => {
                rescan.failures = rescan.failures.saturating_add(1);
                let delay = rescan
                    .interval
                    .saturating_mul(1_u32 << rescan.failures.min(16))
                    .min(rescan::MAX_BACKOFF);
                rescan.next_due_millis =
                    now.saturating_add(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX));
                self.diagnostics.log(&format!(
                    "rescan: a listing failed after {listings} succeeded ({}: {}); next pass in \
                     {} s (failure {} in a row)",
                    error.code,
                    error.detail,
                    delay.as_secs(),
                    rescan.failures
                ));
            }
            None => {
                rescan.failures = 0;
                rescan.next_due_millis = now
                    .saturating_add(u64::try_from(rescan.interval.as_millis()).unwrap_or(u64::MAX));
            }
        }
        if !found.is_empty() {
            self.diagnostics.log(&format!(
                "rescan: {listings} listings found {} message{} the stream did not carry: {}",
                found.len(),
                if found.len() == 1 { "" } else { "s" },
                found.join(", ")
            ));
        }
    }

    /// Queue one listed message for delivery when it is inside the pass's window, neither
    /// carried by the stream nor already rescued or queued, and admitted.
    fn consider(
        &self,
        rescan: &mut Rescan,
        message: &SlackMessage,
        floor: u128,
        ripe: u128,
        found: &mut Vec<String>,
    ) {
        let Some(created) = ids::ts_micros(&message.message_ts) else {
            return;
        };
        if created < floor || created > ripe || rescan.queue.len() >= rescan::MAX_RESCUES_PER_PASS {
            return;
        }
        let key = message_key(&message.channel_id, &message.message_ts);
        if rescan.ledger.carried(&key)
            || rescan.ledger.rescued(&key)
            || rescan.queue.iter().any(|queued| {
                queued.channel_id == message.channel_id && queued.message_ts == message.message_ts
            })
        {
            return;
        }
        match self.admit(message, true) {
            Ok(_) => {
                let files = message.file_ids.len();
                found.push(if files == 0 {
                    key
                } else {
                    format!("{key} ({files} file{})", if files == 1 { "" } else { "s" })
                });
                rescan.queue.push_back(message.clone());
            }
            Err(reason) => {
                let allowed = message
                    .user_id
                    .as_deref()
                    .is_some_and(|user| self.senders.contains(user));
                if allowed && rescan.noted.len() < MAX_NOTED && rescan.noted.insert(key.clone()) {
                    self.diagnostics.log(&format!(
                        "rescan: {key} from an allowed sender is not a request ({reason})"
                    ));
                }
            }
        }
    }

    /// Metadata for a rejected new message; never text, thread routing or an admitted request.
    ///
    /// `bot_id` and `app_id` annotate the delivery and cannot establish the author is a bot or
    /// human. The outbound helper must verify the exact account before adding a reaction.
    fn rejected(&self, message: &SlackMessage) -> Option<RejectedMessage> {
        if !self.channels.contains(&message.channel_id)
            || message
                .subtype
                .as_deref()
                .is_some_and(|subtype| !MESSAGE_SUBTYPES.contains(&subtype))
            || ids::ts_micros(&message.message_ts)? < self.created_floor_micros
        {
            return None;
        }
        let user = message
            .user_id
            .as_deref()
            .filter(|user| ids::valid_user_id(user))?;
        let reason = if !self.senders.contains(user) {
            RejectionReason::SenderNotAllowed
        } else {
            let text = ids::unescape_text(&message.text);
            if !self
                .config
                .ignore_text_prefixes
                .iter()
                .any(|prefix| text.trim_start().starts_with(prefix.as_str()))
            {
                return None;
            }
            RejectionReason::IgnoredPrefix
        };
        Some(RejectedMessage::new(
            ChannelId::new(message.channel_id.clone()).ok()?,
            MessageId::new(ids::message_resource(
                &message.channel_id,
                &message.message_ts,
            ))
            .ok()?,
            SenderId::new(user).ok()?,
            reason,
        ))
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
        if let Some(item) = self.next_rescued()? {
            return Ok(Some(item));
        }
        loop {
            if self.cancel.cancelled.load(Ordering::SeqCst) {
                return Err(failure("cancelled", "the subscription was cancelled", true));
            }
            match self.stream.next(self.config.heartbeat_interval) {
                StreamWait::Idle | StreamWait::Record(StreamRecord::Heartbeat) => {
                    if !self.events_seen
                        && !self.quiet_noted
                        && (self.now_millis)().saturating_sub(self.opened_at_millis)
                            >= QUIET_NOTICE_MILLIS
                    {
                        self.quiet_noted = true;
                        self.diagnostics.log(&format!(
                            "notice: no event has arrived in the {} minutes since this stream \
                             subscribed; if the channel had messages in that time, check that \
                             cli_path runs as a member that receives the channel's events (a \
                             tail run as an app's Crewmate receives none)",
                            QUIET_NOTICE_MILLIS / 60_000
                        ));
                    }
                    self.refresh_cursor()?;
                    if let Some(item) = self.rescan_if_due()? {
                        return Ok(Some(item));
                    }
                    let sequence = self.take_sequence()?;
                    return Ok(Some(SubscriptionItem::Heartbeat(Heartbeat::new(sequence))));
                }
                StreamWait::Record(StreamRecord::Position(position)) => {
                    self.reopened_at_millis = None;
                    // A time cursor is not converted: its lineage floor would not survive a
                    // restart, and its stream is not a sequence catch-up that proves one.
                    let Some(floor) = self.floor_sequence else {
                        continue;
                    };
                    if position <= floor {
                        continue;
                    }
                    self.floor_sequence = Some(position);
                    self.proven_floor = Some(position);
                    self.advanced_at_millis = (self.now_millis)();
                    self.diagnostics.log(&format!(
                        "position {position}: checkpoint (the stream proved every event at or \
                         before it was delivered; the cursor was at {floor})"
                    ));
                    return self
                        .batch(
                            Cursor::Sequence(position).encode(),
                            CommittableEvent::Checkpoint,
                        )
                        .map(Some);
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
                    if !self.crewmate_warned
                        && notice.starts_with("stream stderr:")
                        && notice.to_ascii_lowercase().contains("crewmate")
                    {
                        self.crewmate_warned = true;
                        self.diagnostics.log(CREWMATE_WARNING);
                    }
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
                    self.events_seen = true;
                    let previous = self.last_event_sequence;
                    self.last_event_sequence = Some(previous.map_or(sequence, |l| l.max(sequence)));
                    if self.floor_sequence.is_some_and(|floor| sequence <= floor) {
                        // A replay of a delivered event is never above every event read so far;
                        // a new one at or before a proven position means the proof was wrong.
                        if let Some(proven) = self
                            .proven_floor
                            .filter(|&proven| sequence <= proven)
                            .filter(|_| previous.is_none_or(|last| sequence > last))
                        {
                            let cursor = self
                                .last_cursor
                                .clone()
                                .unwrap_or_else(|| Cursor::Sequence(proven).encode());
                            return self
                                .gap(
                                    cursor,
                                    &format!(
                                        "event {sequence} arrived at or before the proven stream \
                                         position {proven} without having been delivered; the \
                                         position proof was wrong and events may have been skipped"
                                    ),
                                )
                                .map(Some);
                        }
                        continue;
                    }
                    self.floor_sequence = Some(sequence);
                    self.advanced_at_millis = (self.now_millis)();
                    if let Some(message) = message.as_deref() {
                        self.note_carried(sequence, message);
                    }
                    let cursor = Cursor::Sequence(sequence).encode();
                    if let Some(suppressed) = message
                        .as_deref()
                        .and_then(|message| self.rescan_guard(sequence, message))
                    {
                        return self.batch(cursor, suppressed).map(Some);
                    }
                    let event = match message.as_ref().map(|message| self.admit(message, false)) {
                        Some(Ok(inbound)) => {
                            self.diagnostics.log(&format!(
                                "event {sequence}: request {}",
                                inbound.message_id().as_str()
                            ));
                            CommittableEvent::message_created(inbound)
                        }
                        Some(Err(reason)) => {
                            let rejected = message
                                .as_ref()
                                .filter(|_| event_type == "message.created")
                                .and_then(|message| self.rejected(message));
                            // Identifiers only, never text: enough to see why an expected
                            // message was not admitted (for example an unlisted sender ID).
                            let author = message.as_ref().map_or("?", |message| {
                                message.user_id.as_deref().unwrap_or("none")
                            });
                            self.diagnostics.log(&format!(
                                "event {sequence}: {} ({reason}; user {author})",
                                if rejected.is_some() {
                                    "rejection candidate"
                                } else {
                                    "checkpoint"
                                }
                            ));
                            // A refused message from an allowed sender is counted durably, so
                            // agentctl can raise an alarm: a person's message was meant to be a
                            // request (live finding 2026-10-19: three owner replies were dropped
                            // while every health signal stayed green).
                            if let Some(message) = message.as_ref().filter(|message| {
                                message
                                    .user_id
                                    .as_deref()
                                    .is_some_and(|user| self.senders.contains(user))
                            }) {
                                let refusal = crate::refusals::Refusal {
                                    at_millis: crate::refusals::now_millis(),
                                    sequence,
                                    channel_id: message.channel_id.clone(),
                                    message_ts: message.message_ts.clone(),
                                    user_id: author.to_owned(),
                                    reason: reason.to_owned(),
                                };
                                if let Err(error) = self.refusals.record(&refusal) {
                                    self.diagnostics.log(&format!(
                                        "event {sequence}: the allowed-sender refusal could not be saved: {error}"
                                    ));
                                }
                            }
                            rejected.map_or(
                                CommittableEvent::Checkpoint,
                                CommittableEvent::message_rejected,
                            )
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
                let now_micros = u128::from((self.now_millis)()) * 1_000;
                if let Some(rescan) = self.rescan.as_mut() {
                    if let Some(key) = rescan.delivering.take() {
                        rescan.ledger.mark_rescued(&key);
                        rescan.ledger.pending = None;
                        rescan.ledger.prune(now_micros, rescan.window);
                        if let Err(error) = rescan.ledger.save() {
                            self.diagnostics.log(&format!(
                                "rescan: the ledger could not record {key} delivered: {error}"
                            ));
                        }
                    }
                }
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
