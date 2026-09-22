//! Bounded process framing for provider-neutral ordered-subscription backends.
//!
//! Rust traits are never passed through a dynamic-library ABI. A plugin is an ordinary process
//! that reads and writes length-prefixed JSON frames. The host and plugin negotiate an explicit
//! protocol version and capability report before subscribing. Each frame is bounded before
//! allocation, and the server emits at most one receipt-bearing batch before waiting for commit.
//!
//! [`PluginBackend`] adapts a connected process into the same object-safe trait used by statically
//! linked backends. [`serve`] hosts any statically linked backend over the protocol. Process
//! discovery, containment, credential setup, and lifecycle supervision belong to the caller.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Write};
use std::num::NonZeroU16;

use chat_subscription::{
    BackendCapabilities, BackendConfiguration, BackendFailure, ChannelId, ChatSubscription,
    ChatSubscriptionBackend, ChatSubscriptionDriver, CommittableEvent, DeliveryBatch, DeliveryId,
    EventKind, EventSequence, Heartbeat, InboundMessage, MessageId, ProviderCursor,
    ProviderPayload, ReconciliationGap, ReplaySupport, SenderId, SubscribeRequest,
    SubscriptionError, SubscriptionItem, ThreadId,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// The only currently supported process-protocol version.
pub const PROTOCOL_VERSION: u16 = 1;
/// Stable wire-protocol identity recorded by plugin manifests.
pub const PROTOCOL_NAME: &str = "agentctl-chat-subscription";
/// Maximum serialized payload bytes in one frame, excluding its four-byte length prefix.
pub const MAX_FRAME_BYTES: usize = 1_048_576;
// Each event's fixed field names, punctuation, tags, boolean/null value, and separating comma fit
// conservatively within 256 bytes; all variable JSON values are counted by MAX_BATCH_BYTES.
const MAX_WIRE_EVENT_FIXED_BYTES: usize = 256;
// The Item/Batch envelopes, sequence digits, array punctuation, and other fixed fields are much
// smaller than this deliberately auditable allowance.
const MAX_WIRE_BATCH_FIXED_BYTES: usize = 1_024;
const MAX_JSON_ESCAPED_TOKEN_BYTES: usize = 2 + 6 * chat_subscription::MAX_TOKEN_BYTES;
/// Conservative upper bound for any process frame made from a core-valid delivery batch.
pub const MAX_CORE_BATCH_FRAME_BYTES: usize = chat_subscription::MAX_BATCH_BYTES
    + chat_subscription::MAX_BATCH_EVENTS * MAX_WIRE_EVENT_FIXED_BYTES
    + 2 * MAX_JSON_ESCAPED_TOKEN_BYTES
    + MAX_WIRE_BATCH_FIXED_BYTES;
const _: () = assert!(MAX_CORE_BATCH_FRAME_BYTES <= MAX_FRAME_BYTES);

/// A bounded framing, negotiation, or remote-backend failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginError {
    code: &'static str,
    detail: String,
}

impl PluginError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        let mut detail = detail.into();
        if detail.len() > 2_000 {
            let mut boundary = 2_000;
            while !detail.is_char_boundary(boundary) {
                boundary -= 1;
            }
            detail.truncate(boundary);
        }
        Self { code, detail }
    }

    /// Return the stable machine-readable classification.
    #[must_use]
    pub fn code(&self) -> &str {
        self.code
    }

    /// Return the bounded human-readable detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for PluginError {}

impl From<io::Error> for PluginError {
    fn from(error: io::Error) -> Self {
        Self::new("plugin_io", error.to_string())
    }
}

fn backend_failure(error: PluginError) -> BackendFailure {
    let retryable = matches!(
        error.code(),
        "plugin_io" | "unexpected_eof" | "truncated_frame" | "commit_outcome_unknown"
    );
    BackendFailure::new(error.code(), error.detail(), retryable).unwrap_or_else(|_| {
        BackendFailure::new(
            "plugin_protocol",
            "plugin failed with an invalid or oversized diagnostic",
            false,
        )
        .expect("constant fallback backend failure is valid")
    })
}

struct FramedIo<R, W> {
    reader: R,
    writer: W,
}

struct StrictJsonSeed;

impl<'de> DeserializeSeed<'de> for StrictJsonSeed {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("strict JSON without duplicate object keys")
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        StrictJsonSeed.deserialize(deserializer)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while sequence.next_element_seed(StrictJsonSeed)?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            map.next_value_seed(StrictJsonSeed)?;
        }
        Ok(())
    }

    fn visit_newtype_struct<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        StrictJsonSeed.deserialize(deserializer)
    }

    fn visit_bytes<E: de::Error>(self, _value: &[u8]) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_byte_buf<E: de::Error>(self, _value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(())
    }
}

fn validate_strict_json(payload: &[u8]) -> Result<(), PluginError> {
    let mut deserializer = serde_json::Deserializer::from_slice(payload);
    StrictJsonSeed
        .deserialize(&mut deserializer)
        .and_then(|()| deserializer.end())
        .map_err(|error| PluginError::new("invalid_frame", error.to_string()))
}

/// Decode one JSON document after rejecting duplicate object keys at every depth.
///
/// This is also suitable for manifests that select this protocol: accepting last-key-wins JSON
/// there could make the reviewed identity differ from the identity used at runtime.
///
/// # Errors
///
/// Returns [`PluginError`] when the document is malformed, contains a duplicate key, has trailing
/// data, or does not satisfy `T`'s serde contract.
pub fn decode_strict_json<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, PluginError> {
    validate_strict_json(payload)?;
    serde_json::from_slice(payload)
        .map_err(|error| PluginError::new("invalid_frame", error.to_string()))
}

impl<R: Read, W: Write> FramedIo<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    fn send<T: Serialize>(&mut self, value: &T) -> Result<(), PluginError> {
        let payload = serde_json::to_vec(value)
            .map_err(|error| PluginError::new("invalid_local_frame", error.to_string()))?;
        if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
            return Err(PluginError::new(
                "frame_limit_exceeded",
                format!(
                    "outbound plugin frame contains {} bytes; maximum is {MAX_FRAME_BYTES}",
                    payload.len()
                ),
            ));
        }
        let length = u32::try_from(payload.len()).map_err(|_| {
            PluginError::new("frame_limit_exceeded", "plugin frame length exceeds u32")
        })?;
        self.writer.write_all(&length.to_be_bytes())?;
        self.writer.write_all(&payload)?;
        self.writer.flush()?;
        Ok(())
    }

    fn receive<T: for<'de> Deserialize<'de>>(&mut self) -> Result<Option<T>, PluginError> {
        let mut header = [0_u8; 4];
        loop {
            match self.reader.read(&mut header[..1]) {
                Ok(0) => return Ok(None),
                Ok(1) => break,
                Ok(_) => unreachable!("one-byte read returned more than one byte"),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        self.reader.read_exact(&mut header[1..]).map_err(|error| {
            PluginError::new(
                "truncated_frame",
                format!("plugin frame ended inside its length prefix: {error}"),
            )
        })?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(PluginError::new(
                "frame_limit_exceeded",
                format!("plugin announced a {length}-byte frame; maximum is {MAX_FRAME_BYTES}"),
            ));
        }
        let mut payload = vec![0_u8; length];
        self.reader.read_exact(&mut payload).map_err(|error| {
            PluginError::new(
                "truncated_frame",
                format!("plugin frame ended before {length} payload bytes: {error}"),
            )
        })?;
        decode_strict_json(&payload).map(Some)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientFrame {
    Hello {
        min_version: u16,
        max_version: u16,
    },
    Start {
        channel_ids: Vec<String>,
        allowed_senders: Vec<String>,
        max_uncommitted: u16,
        resume_from: Option<String>,
        backend_configuration: Option<WireBackendConfiguration>,
    },
    Commit {
        sequence: u64,
        delivery_id: String,
    },
    Close,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ServerFrame {
    Hello {
        version: u16,
        capabilities: WireCapabilities,
    },
    Subscribed,
    Item {
        item: WireItem,
    },
    Committed {
        sequence: u64,
    },
    End,
    Error {
        code: String,
        detail: String,
        retryable: bool,
        fatal: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCapabilities {
    backend_name: String,
    replay: WireReplay,
    full_message_data: bool,
    max_uncommitted: u16,
    event_kinds: Vec<WireEventKind>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireReplay {
    CurrentOnly,
    Cursor,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireEventKind {
    MessageCreated,
    Checkpoint,
    Gap,
    Heartbeat,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireItem {
    Batch {
        sequence: u64,
        provider_cursor: String,
        delivery_id: String,
        events: Vec<WireEvent>,
    },
    Heartbeat {
        sequence: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireEvent {
    MessageCreated(Box<WireMessageCreated>),
    Checkpoint,
    Gap { reason: Option<String> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMessageCreated {
    channel_id: String,
    message_id: String,
    thread_id: String,
    sender_id: String,
    text: String,
    created_at: String,
    thread_reply: bool,
    provider_payload: Option<WireProviderPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireProviderPayload {
    schema: String,
    data: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBackendConfiguration {
    schema: String,
    data: serde_json::Map<String, serde_json::Value>,
}

impl From<&BackendCapabilities> for WireCapabilities {
    fn from(value: &BackendCapabilities) -> Self {
        Self {
            backend_name: value.backend_name().to_owned(),
            replay: match value.replay() {
                ReplaySupport::CurrentOnly => WireReplay::CurrentOnly,
                ReplaySupport::Cursor => WireReplay::Cursor,
            },
            full_message_data: value.full_message_data(),
            max_uncommitted: value.max_uncommitted().get(),
            event_kinds: value
                .event_kinds()
                .iter()
                .map(|kind| match kind {
                    EventKind::MessageCreated => WireEventKind::MessageCreated,
                    EventKind::Checkpoint => WireEventKind::Checkpoint,
                    EventKind::Gap => WireEventKind::Gap,
                    EventKind::Heartbeat => WireEventKind::Heartbeat,
                })
                .collect(),
        }
    }
}

impl TryFrom<WireCapabilities> for BackendCapabilities {
    type Error = PluginError;

    fn try_from(value: WireCapabilities) -> Result<Self, Self::Error> {
        let max_uncommitted = NonZeroU16::new(value.max_uncommitted).ok_or_else(|| {
            PluginError::new("invalid_capabilities", "max_uncommitted must be nonzero")
        })?;
        BackendCapabilities::new(
            value.backend_name,
            match value.replay {
                WireReplay::CurrentOnly => ReplaySupport::CurrentOnly,
                WireReplay::Cursor => ReplaySupport::Cursor,
            },
            value.full_message_data,
            max_uncommitted,
            value
                .event_kinds
                .into_iter()
                .map(|kind| match kind {
                    WireEventKind::MessageCreated => EventKind::MessageCreated,
                    WireEventKind::Checkpoint => EventKind::Checkpoint,
                    WireEventKind::Gap => EventKind::Gap,
                    WireEventKind::Heartbeat => EventKind::Heartbeat,
                })
                .collect(),
        )
        .map_err(|error| PluginError::new("invalid_capabilities", error.to_string()))
    }
}

impl From<&SubscriptionItem> for WireItem {
    fn from(value: &SubscriptionItem) -> Self {
        match value {
            SubscriptionItem::Heartbeat(heartbeat) => Self::Heartbeat {
                sequence: heartbeat.sequence().get(),
            },
            SubscriptionItem::Batch(batch) => Self::Batch {
                sequence: batch.sequence().get(),
                provider_cursor: batch.cursor().as_str().to_owned(),
                delivery_id: batch.delivery_id().as_str().to_owned(),
                events: batch.events().iter().map(WireEvent::from).collect(),
            },
        }
    }
}

impl From<&CommittableEvent> for WireEvent {
    fn from(value: &CommittableEvent) -> Self {
        match value {
            CommittableEvent::MessageCreated(message) => {
                Self::MessageCreated(Box::new(WireMessageCreated {
                    channel_id: message.channel_id().as_str().to_owned(),
                    message_id: message.message_id().as_str().to_owned(),
                    thread_id: message.thread_id().as_str().to_owned(),
                    sender_id: message.sender_id().as_str().to_owned(),
                    text: message.text().to_owned(),
                    created_at: message.created_at().to_owned(),
                    thread_reply: message.is_thread_reply(),
                    provider_payload: message.provider_payload().map(|payload| {
                        WireProviderPayload {
                            schema: payload.schema().to_owned(),
                            data: payload.data().clone(),
                        }
                    }),
                }))
            }
            CommittableEvent::Checkpoint => Self::Checkpoint,
            CommittableEvent::Gap(gap) => Self::Gap {
                reason: gap.reason().map(str::to_owned),
            },
        }
    }
}

impl TryFrom<WireItem> for SubscriptionItem {
    type Error = PluginError;

    fn try_from(value: WireItem) -> Result<Self, Self::Error> {
        match value {
            WireItem::Heartbeat { sequence } => Ok(Self::Heartbeat(Heartbeat::new(
                sequence_from_wire(sequence)?,
            ))),
            WireItem::Batch {
                sequence,
                provider_cursor,
                delivery_id,
                events,
            } => {
                let events = events
                    .into_iter()
                    .map(CommittableEvent::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                DeliveryBatch::new(
                    sequence_from_wire(sequence)?,
                    ProviderCursor::new(provider_cursor)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    DeliveryId::new(delivery_id)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    events,
                )
                .map(Self::Batch)
                .map_err(|error| PluginError::new("invalid_item", error.to_string()))
            }
        }
    }
}

fn sequence_from_wire(value: u64) -> Result<EventSequence, PluginError> {
    EventSequence::new(value).map_err(|error| PluginError::new("invalid_item", error.to_string()))
}

impl TryFrom<WireEvent> for CommittableEvent {
    type Error = PluginError;

    fn try_from(value: WireEvent) -> Result<Self, Self::Error> {
        match value {
            WireEvent::Checkpoint => Ok(Self::Checkpoint),
            WireEvent::Gap { reason } => ReconciliationGap::new(reason)
                .map(Self::Gap)
                .map_err(|error| PluginError::new("invalid_item", error.to_string())),
            WireEvent::MessageCreated(message) => {
                let WireMessageCreated {
                    channel_id,
                    message_id,
                    thread_id,
                    sender_id,
                    text,
                    created_at,
                    thread_reply,
                    provider_payload,
                } = *message;
                let provider_payload = provider_payload
                    .map(|payload| ProviderPayload::new(payload.schema, payload.data))
                    .transpose()
                    .map_err(|error| PluginError::new("invalid_item", error.to_string()))?;
                let message = InboundMessage::new(
                    ChannelId::new(channel_id)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    MessageId::new(message_id)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    ThreadId::new(thread_id)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    SenderId::new(sender_id)
                        .map_err(|error| PluginError::new("invalid_item", error.to_string()))?,
                    text,
                    created_at,
                    thread_reply,
                )
                .map_err(|error| PluginError::new("invalid_item", error.to_string()))?;
                let message = match provider_payload {
                    Some(provider_payload) => message.with_provider_payload(provider_payload),
                    None => message,
                };
                Ok(Self::message_created(message))
            }
        }
    }
}

fn remote_failure(code: String, detail: String, retryable: bool) -> BackendFailure {
    BackendFailure::new(code, detail, retryable).unwrap_or_else(|_| {
        BackendFailure::new(
            "invalid_remote_error",
            "plugin returned an invalid or oversized error",
            false,
        )
        .expect("constant fallback backend failure is valid")
    })
}

/// A connected process plugin that implements the ordinary backend factory trait.
///
/// Construction performs the version/capability handshake. One instance opens at most one
/// subscription because its byte streams move into the resulting driver.
pub struct PluginBackend<R, W> {
    io: Option<FramedIo<R, W>>,
    capabilities: BackendCapabilities,
}

impl<R: Read + Send + 'static, W: Write + Send + 'static> PluginBackend<R, W> {
    /// Negotiate the protocol and read the plugin's capability report.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] for framing, version, or capability failures.
    pub fn connect(reader: R, writer: W) -> Result<Self, PluginError> {
        let mut io = FramedIo::new(reader, writer);
        io.send(&ClientFrame::Hello {
            min_version: PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
        })?;
        let response: ServerFrame = io.receive()?.ok_or_else(|| {
            PluginError::new(
                "unexpected_eof",
                "plugin exited before protocol negotiation",
            )
        })?;
        let capabilities = match response {
            ServerFrame::Hello {
                version,
                capabilities,
            } if version == PROTOCOL_VERSION => capabilities.try_into()?,
            ServerFrame::Hello { version, .. } => {
                return Err(PluginError::new(
                    "incompatible_protocol",
                    format!("plugin selected unsupported protocol version {version}"),
                ));
            }
            ServerFrame::Error { code, detail, .. } => {
                return Err(PluginError::new(
                    "remote_refused",
                    format!("{code}: {detail}"),
                ));
            }
            _ => {
                return Err(PluginError::new(
                    "unexpected_frame",
                    "plugin did not begin with a hello frame",
                ));
            }
        };
        Ok(Self {
            io: Some(io),
            capabilities,
        })
    }

    /// Cooperatively close a negotiated connection before a subscription starts.
    ///
    /// Once [`ChatSubscriptionBackend::subscribe`] moves the streams into a driver, call
    /// [`ChatSubscription::close`] instead. Neither path replaces bounded child-process
    /// supervision: a plugin blocked in provider code may not read the close frame.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] if the close frame cannot be written.
    pub fn close(&mut self) -> Result<(), PluginError> {
        if let Some(mut io) = self.io.take() {
            io.send(&ClientFrame::Close)?;
        }
        Ok(())
    }
}

impl<R: Read + Send + 'static, W: Write + Send + 'static> ChatSubscriptionBackend
    for PluginBackend<R, W>
{
    fn capabilities(&self) -> BackendCapabilities {
        self.capabilities.clone()
    }

    fn subscribe(
        &mut self,
        request: &SubscribeRequest,
    ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
        let mut io = self.io.take().ok_or_else(|| {
            BackendFailure::new(
                "plugin_already_subscribed",
                "one plugin connection can open only one subscription",
                false,
            )
            .expect("constant backend failure is valid")
        })?;
        io.send(&ClientFrame::Start {
            channel_ids: request
                .channel_ids()
                .iter()
                .map(|channel| channel.as_str().to_owned())
                .collect(),
            allowed_senders: request
                .allowed_senders()
                .iter()
                .map(|sender| sender.as_str().to_owned())
                .collect(),
            max_uncommitted: request.max_uncommitted().get(),
            resume_from: request
                .resume_from()
                .map(|cursor| cursor.as_str().to_owned()),
            backend_configuration: request.backend_configuration().map(|configuration| {
                WireBackendConfiguration {
                    schema: configuration.schema().to_owned(),
                    data: configuration.data().clone(),
                }
            }),
        })
        .map_err(backend_failure)?;
        match io.receive::<ServerFrame>().map_err(backend_failure)? {
            Some(ServerFrame::Subscribed) => Ok(Box::new(PluginDriver {
                io,
                pending: None,
                terminal: false,
            })),
            Some(ServerFrame::Error {
                code,
                detail,
                retryable,
                ..
            }) => Err(remote_failure(code, detail, retryable)),
            Some(_) => Err(backend_failure(PluginError::new(
                "unexpected_frame",
                "plugin did not acknowledge the subscription",
            ))),
            None => Err(backend_failure(PluginError::new(
                "unexpected_eof",
                "plugin exited before acknowledging the subscription",
            ))),
        }
    }
}

struct PluginDriver<R, W> {
    io: FramedIo<R, W>,
    pending: Option<(EventSequence, DeliveryId)>,
    terminal: bool,
}

impl<R: Read + Send, W: Write + Send> ChatSubscriptionDriver for PluginDriver<R, W> {
    fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
        if self.pending.is_some() {
            return Err(BackendFailure::new(
                "plugin_backpressure_violation",
                "plugin driver was asked for another item before commit",
                false,
            )
            .expect("constant backend failure is valid"));
        }
        if self.terminal {
            return Ok(None);
        }
        let frame = self.io.receive::<ServerFrame>().map_err(backend_failure)?;
        match frame {
            Some(ServerFrame::Item { item }) => {
                let item = SubscriptionItem::try_from(item).map_err(backend_failure)?;
                if let SubscriptionItem::Batch(batch) = &item {
                    self.pending = Some((batch.sequence(), batch.delivery_id().clone()));
                }
                Ok(Some(item))
            }
            Some(ServerFrame::End) => {
                self.terminal = true;
                Ok(None)
            }
            None => Err(backend_failure(PluginError::new(
                "unexpected_eof",
                "plugin connection closed without an explicit end frame",
            ))),
            Some(ServerFrame::Error {
                code,
                detail,
                retryable,
                ..
            }) => Err(remote_failure(code, detail, retryable)),
            Some(_) => Err(backend_failure(PluginError::new(
                "unexpected_frame",
                "plugin emitted a control frame while an event was expected",
            ))),
        }
    }

    fn acknowledge(&mut self, delivery_id: &DeliveryId) -> Result<(), BackendFailure> {
        let (sequence, expected_delivery_id) = self.pending.as_ref().ok_or_else(|| {
            BackendFailure::new(
                "plugin_no_pending_delivery",
                "plugin driver has no delivery to acknowledge",
                false,
            )
            .expect("constant backend failure is valid")
        })?;
        if expected_delivery_id != delivery_id {
            return Err(BackendFailure::new(
                "plugin_delivery_id_mismatch",
                "acknowledgement id does not match the pending delivery batch",
                false,
            )
            .expect("constant backend failure is valid"));
        }
        let sequence = *sequence;
        self.io
            .send(&ClientFrame::Commit {
                sequence: sequence.get(),
                delivery_id: delivery_id.as_str().to_owned(),
            })
            .map_err(backend_failure)?;
        match self.io.receive::<ServerFrame>().map_err(backend_failure)? {
            Some(ServerFrame::Committed { sequence: observed }) if observed == sequence.get() => {
                self.pending = None;
                Ok(())
            }
            Some(ServerFrame::Error {
                code,
                detail,
                retryable,
                ..
            }) => Err(remote_failure(code, detail, retryable)),
            Some(_) => Err(backend_failure(PluginError::new(
                "unexpected_frame",
                "plugin did not confirm the exact commit sequence",
            ))),
            None => Err(backend_failure(PluginError::new(
                "commit_outcome_unknown",
                "plugin exited before confirming the commit",
            ))),
        }
    }

    fn close(&mut self) -> Result<(), BackendFailure> {
        if self.terminal {
            return Ok(());
        }
        self.io.send(&ClientFrame::Close).map_err(backend_failure)?;
        self.pending = None;
        self.terminal = true;
        Ok(())
    }
}

fn send_backend_error<R: Read, W: Write>(
    io: &mut FramedIo<R, W>,
    error: &BackendFailure,
    fatal: bool,
) -> Result<(), PluginError> {
    io.send(&ServerFrame::Error {
        code: error.code().to_owned(),
        detail: error.detail().to_owned(),
        retryable: error.retryable(),
        fatal,
    })
}

fn subscription_failure(error: SubscriptionError) -> BackendFailure {
    match error {
        SubscriptionError::Backend(error) => error,
        other => BackendFailure::new("subscription_contract", other.to_string(), false)
            .expect("core contract diagnostics are bounded"),
    }
}

/// Host one object-safe backend on a bounded framed connection.
///
/// The function performs no filesystem operations. It reads the next provider item only after the
/// previous delivery has been committed, so pipe/socket backpressure and core backpressure agree.
/// Every exit after opening a subscription invokes its cooperative close hook; an outstanding
/// delivery is never acknowledged by cancellation or transport EOF.
///
/// # Errors
///
/// Returns [`PluginError`] for malformed frames, incompatible versions, or connection failures.
pub fn serve<R: Read, W: Write>(
    backend: &mut dyn ChatSubscriptionBackend,
    reader: R,
    writer: W,
) -> Result<(), PluginError> {
    let mut io = FramedIo::new(reader, writer);
    let hello = io.receive::<ClientFrame>()?.ok_or_else(|| {
        PluginError::new(
            "unexpected_eof",
            "host disconnected before protocol negotiation",
        )
    })?;
    match hello {
        ClientFrame::Hello {
            min_version,
            max_version,
        } if min_version <= PROTOCOL_VERSION && PROTOCOL_VERSION <= max_version => {
            io.send(&ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                capabilities: WireCapabilities::from(&backend.capabilities()),
            })?;
        }
        ClientFrame::Hello { .. } => {
            io.send(&ServerFrame::Error {
                code: "incompatible_protocol".to_owned(),
                detail: format!("plugin supports only protocol version {PROTOCOL_VERSION}"),
                retryable: false,
                fatal: true,
            })?;
            return Ok(());
        }
        _ => {
            return Err(PluginError::new(
                "unexpected_frame",
                "host did not begin with a hello frame",
            ));
        }
    }

    let request = match io.receive::<ClientFrame>()? {
        Some(ClientFrame::Start {
            channel_ids,
            allowed_senders,
            max_uncommitted,
            resume_from,
            backend_configuration,
        }) => {
            if max_uncommitted != 1 {
                return Err(PluginError::new(
                    "invalid_start",
                    "protocol v1 requires max_uncommitted=1",
                ));
            }
            let channel_ids = channel_ids
                .into_iter()
                .map(ChannelId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| PluginError::new("invalid_start", error.to_string()))?;
            let allowed_senders = allowed_senders
                .into_iter()
                .map(SenderId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| PluginError::new("invalid_start", error.to_string()))?;
            let request = SubscribeRequest::new(
                channel_ids,
                allowed_senders,
                resume_from
                    .map(ProviderCursor::new)
                    .transpose()
                    .map_err(|error| PluginError::new("invalid_start", error.to_string()))?,
            )
            .map_err(|error| PluginError::new("invalid_start", error.to_string()))?;
            match backend_configuration {
                Some(configuration) => request.with_backend_configuration(
                    BackendConfiguration::new(configuration.schema, configuration.data)
                        .map_err(|error| PluginError::new("invalid_start", error.to_string()))?,
                ),
                None => request,
            }
        }
        Some(ClientFrame::Close) | None => return Ok(()),
        Some(_) => {
            return Err(PluginError::new(
                "unexpected_frame",
                "host did not send start after negotiation",
            ));
        }
    };
    let mut subscription = match ChatSubscription::open(backend, &request) {
        Ok(subscription) => subscription,
        Err(error) => {
            let error = subscription_failure(error);
            send_backend_error(&mut io, &error, true)?;
            return Ok(());
        }
    };
    let result = io
        .send(&ServerFrame::Subscribed)
        .and_then(|()| serve_subscription(&mut subscription, &mut io));
    let close_result = subscription
        .close()
        .map_err(|error| PluginError::new("backend_close", error.to_string()));
    match (result, close_result) {
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn serve_subscription<R: Read, W: Write>(
    subscription: &mut ChatSubscription,
    io: &mut FramedIo<R, W>,
) -> Result<(), PluginError> {
    loop {
        let item = match subscription.next_item() {
            Ok(Some(item)) => item,
            Ok(None) => {
                io.send(&ServerFrame::End)?;
                return Ok(());
            }
            Err(error) => {
                let error = subscription_failure(error);
                send_backend_error(io, &error, true)?;
                return Ok(());
            }
        };
        io.send(&ServerFrame::Item {
            item: WireItem::from(&item),
        })?;
        let SubscriptionItem::Batch(delivery) = item else {
            continue;
        };
        match io.receive::<ClientFrame>()? {
            Some(ClientFrame::Commit {
                sequence,
                delivery_id,
            }) if sequence == delivery.sequence().get()
                && delivery_id == delivery.delivery_id().as_str() =>
            {
                if let Err(error) = subscription.commit_durable(&delivery) {
                    let error = subscription_failure(error);
                    send_backend_error(io, &error, true)?;
                    return Ok(());
                }
                io.send(&ServerFrame::Committed { sequence })?;
            }
            Some(ClientFrame::Close) | None => return Ok(()),
            Some(ClientFrame::Commit { .. }) => {
                let error = BackendFailure::new(
                    "commit_mismatch",
                    "host commit does not match the outstanding delivery",
                    false,
                )
                .expect("constant backend failure is valid");
                send_backend_error(io, &error, true)?;
                return Ok(());
            }
            Some(_) => {
                return Err(PluginError::new(
                    "unexpected_frame",
                    "host sent a non-commit frame while delivery was outstanding",
                ));
            }
        }
    }
}

/// Host a backend on this process's standard input and output.
///
/// # Errors
///
/// Returns [`PluginError`] as [`serve`].
pub fn serve_stdio(backend: &mut dyn ChatSubscriptionBackend) -> Result<(), PluginError> {
    serve(backend, io::stdin().lock(), io::stdout().lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct ProtocolBackend {
        input: Option<Vec<u8>>,
    }

    impl ChatSubscriptionBackend for ProtocolBackend {
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "protocol-fixture",
                ReplaySupport::Cursor,
                false,
                NonZeroU16::new(1).expect("one is nonzero"),
                vec![EventKind::Heartbeat],
            )
            .expect("fixture capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            Ok(Box::new(PluginDriver {
                io: FramedIo::new(
                    Cursor::new(self.input.take().expect("one subscription")),
                    Vec::<u8>::new(),
                ),
                pending: None,
                terminal: false,
            }))
        }
    }

    fn fixture_request() -> SubscribeRequest {
        SubscribeRequest::new(
            vec![ChannelId::new("channels/one").expect("channel")],
            vec![SenderId::new("users/owner").expect("sender")],
            Some(ProviderCursor::new("cursor-before").expect("cursor")),
        )
        .expect("fixture request")
    }

    fn frame(raw: &[u8]) -> Vec<u8> {
        let mut framed = Vec::with_capacity(raw.len() + 4);
        framed.extend_from_slice(
            &u32::try_from(raw.len())
                .expect("frame fits u32")
                .to_be_bytes(),
        );
        framed.extend_from_slice(raw);
        framed
    }

    fn append_frame<T: Serialize>(bytes: &mut Vec<u8>, value: &T) {
        let payload = serde_json::to_vec(value).expect("serialize fixture frame");
        bytes.extend(frame(&payload));
    }

    struct CloseCountingBackend {
        closes: Arc<AtomicUsize>,
        acknowledgements: Arc<AtomicUsize>,
    }

    impl ChatSubscriptionBackend for CloseCountingBackend {
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::new(
                "close-counting-fixture",
                ReplaySupport::Cursor,
                false,
                NonZeroU16::new(1).expect("one is nonzero"),
                vec![EventKind::Checkpoint, EventKind::Heartbeat],
            )
            .expect("fixture capabilities")
        }

        fn subscribe(
            &mut self,
            _request: &SubscribeRequest,
        ) -> Result<Box<dyn ChatSubscriptionDriver>, BackendFailure> {
            Ok(Box::new(CloseCountingDriver {
                closes: Arc::clone(&self.closes),
                acknowledgements: Arc::clone(&self.acknowledgements),
                emitted: false,
            }))
        }
    }

    struct CloseCountingDriver {
        closes: Arc<AtomicUsize>,
        acknowledgements: Arc<AtomicUsize>,
        emitted: bool,
    }

    struct FailAfterFirstFrame {
        first_frame_flushed: bool,
    }

    impl Write for FailAfterFirstFrame {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.first_frame_flushed {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture refuses the subscribed frame",
                ))
            } else {
                Ok(bytes.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            self.first_frame_flushed = true;
            Ok(())
        }
    }

    impl ChatSubscriptionDriver for CloseCountingDriver {
        fn next_item(&mut self) -> Result<Option<SubscriptionItem>, BackendFailure> {
            assert!(
                !self.emitted,
                "fixture must be cancelled before another read"
            );
            self.emitted = true;
            DeliveryBatch::new(
                EventSequence::new(1).expect("sequence"),
                ProviderCursor::new("cursor-one").expect("cursor"),
                DeliveryId::new("delivery-one").expect("delivery"),
                vec![CommittableEvent::Checkpoint],
            )
            .map(SubscriptionItem::Batch)
            .map(Some)
            .map_err(|error| {
                BackendFailure::new("fixture", error.to_string(), false)
                    .expect("fixture diagnostic")
            })
        }

        fn acknowledge(&mut self, _delivery_id: &DeliveryId) -> Result<(), BackendFailure> {
            self.acknowledgements.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn close(&mut self) -> Result<(), BackendFailure> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn run_server_cancellation(include_close_frame: bool) -> (usize, usize) {
        let closes = Arc::new(AtomicUsize::new(0));
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        let mut backend = CloseCountingBackend {
            closes: Arc::clone(&closes),
            acknowledgements: Arc::clone(&acknowledgements),
        };
        let mut input = Vec::new();
        append_frame(
            &mut input,
            &ClientFrame::Hello {
                min_version: PROTOCOL_VERSION,
                max_version: PROTOCOL_VERSION,
            },
        );
        append_frame(
            &mut input,
            &ClientFrame::Start {
                channel_ids: vec!["channels/one".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                max_uncommitted: 1,
                resume_from: Some("cursor-zero".to_owned()),
                backend_configuration: None,
            },
        );
        if include_close_frame {
            append_frame(&mut input, &ClientFrame::Close);
        }

        serve(&mut backend, Cursor::new(input), Vec::<u8>::new())
            .expect("host cancellation is clean");
        (
            closes.load(Ordering::SeqCst),
            acknowledgements.load(Ordering::SeqCst),
        )
    }

    #[test]
    fn server_close_frame_reaches_provider_without_acknowledging() {
        assert_eq!(run_server_cancellation(true), (1, 0));
    }

    #[test]
    fn server_eof_reaches_provider_without_acknowledging() {
        assert_eq!(run_server_cancellation(false), (1, 0));
    }

    #[test]
    fn subscribed_write_failure_closes_open_provider_without_acknowledging() {
        let closes = Arc::new(AtomicUsize::new(0));
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        let mut backend = CloseCountingBackend {
            closes: Arc::clone(&closes),
            acknowledgements: Arc::clone(&acknowledgements),
        };
        let mut input = Vec::new();
        append_frame(
            &mut input,
            &ClientFrame::Hello {
                min_version: PROTOCOL_VERSION,
                max_version: PROTOCOL_VERSION,
            },
        );
        append_frame(
            &mut input,
            &ClientFrame::Start {
                channel_ids: vec!["channels/one".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                max_uncommitted: 1,
                resume_from: Some("cursor-zero".to_owned()),
                backend_configuration: None,
            },
        );

        let error = serve(
            &mut backend,
            Cursor::new(input),
            FailAfterFirstFrame {
                first_frame_flushed: false,
            },
        )
        .expect_err("subscribed write failure is reported");
        assert_eq!(error.code(), "plugin_io");
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn inbound_length_is_refused_before_payload_allocation() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            &(u32::try_from(MAX_FRAME_BYTES).expect("fits u32") + 1).to_be_bytes(),
        );
        let mut io = FramedIo::new(Cursor::new(bytes), Vec::new());
        let error = io
            .receive::<ClientFrame>()
            .expect_err("oversized frame must be refused");
        assert_eq!(error.code(), "frame_limit_exceeded");
    }

    #[test]
    fn truncated_payload_is_not_clean_eof() {
        let bytes = [0_u8, 0, 0, 4, b'{'];
        let mut io = FramedIo::new(Cursor::new(bytes), Vec::new());
        let error = io
            .receive::<ClientFrame>()
            .expect_err("truncated payload must fail");
        assert_eq!(error.code(), "truncated_frame");
    }

    #[test]
    fn empty_stream_is_clean_eof() {
        let mut io = FramedIo::new(Cursor::new(Vec::<u8>::new()), Vec::new());
        assert!(io
            .receive::<ClientFrame>()
            .expect("empty stream is readable")
            .is_none());
    }

    #[test]
    fn explicit_end_is_the_only_stable_clean_termination() {
        let payload = serde_json::to_vec(&ServerFrame::End).expect("serialize end");
        let mut backend = ProtocolBackend {
            input: Some(frame(&payload)),
        };
        let mut subscription =
            ChatSubscription::open(&mut backend, &fixture_request()).expect("subscription opens");
        assert_eq!(subscription.next_item().expect("explicit end"), None);
        assert_eq!(subscription.next_item().expect("stable end"), None);
        assert!(!subscription.is_poisoned());
    }

    #[test]
    fn raw_eof_is_retryable_failure_and_poisons_core_generation() {
        let mut backend = ProtocolBackend {
            input: Some(Vec::new()),
        };
        let mut subscription =
            ChatSubscription::open(&mut backend, &fixture_request()).expect("subscription opens");
        let error = subscription
            .next_item()
            .expect_err("EOF without end is not graceful");
        let SubscriptionError::Backend(failure) = error else {
            panic!("expected backend failure, found {error:?}");
        };
        assert_eq!(failure.code(), "unexpected_eof");
        assert!(failure.retryable());
        assert!(subscription.is_poisoned());
    }

    #[test]
    fn deterministic_protocol_violations_are_not_retryable() {
        let cases = [
            (
                frame(br#"{"type":"item","item":{"kind":"heartbeat","sequence":0}}"#),
                "invalid_item",
            ),
            (
                frame(br#"{"type":"item","item":not-json}"#),
                "invalid_frame",
            ),
            (
                (u32::try_from(MAX_FRAME_BYTES).expect("frame bound fits u32") + 1)
                    .to_be_bytes()
                    .to_vec(),
                "frame_limit_exceeded",
            ),
        ];
        for (input, expected_code) in cases {
            let mut backend = ProtocolBackend { input: Some(input) };
            let mut subscription = ChatSubscription::open(&mut backend, &fixture_request())
                .expect("subscription opens");
            let error = subscription
                .next_item()
                .expect_err("invalid protocol input must poison the generation");
            let SubscriptionError::Backend(failure) = error else {
                panic!("expected backend failure, found {error:?}");
            };
            assert_eq!(failure.code(), expected_code);
            assert!(!failure.retryable());
            assert!(subscription.is_poisoned());
        }
    }

    #[test]
    fn close_is_a_public_single_frame_cancellation_path() {
        let mut driver = PluginDriver {
            io: FramedIo::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new()),
            pending: Some((
                EventSequence::new(7).expect("sequence"),
                DeliveryId::new("delivery-seven").expect("delivery"),
            )),
            terminal: false,
        };
        ChatSubscriptionDriver::close(&mut driver).expect("close frame is written");
        let written_after_first = driver.io.writer.len();
        ChatSubscriptionDriver::close(&mut driver).expect("repeated close is idempotent");
        assert_eq!(driver.io.writer.len(), written_after_first);
        assert!(driver.pending.is_none());
        assert!(driver.terminal);

        let mut reader = FramedIo::new(Cursor::new(&driver.io.writer), Vec::<u8>::new());
        assert!(matches!(
            reader.receive::<ClientFrame>().expect("decode close"),
            Some(ClientFrame::Close)
        ));
        assert!(reader
            .receive::<ClientFrame>()
            .expect("no duplicate close")
            .is_none());
    }

    #[test]
    fn plugin_diagnostic_truncation_preserves_utf8_boundaries() {
        let error = PluginError::new("fixture", format!("{}😀", "x".repeat(1_999)));
        assert_eq!(error.detail().len(), 1_999);
        assert!(error.detail().chars().all(|character| character == 'x'));
    }

    #[test]
    fn duplicate_keys_are_refused_at_every_object_depth() {
        let top_level = br#"{"type":"hello","type":"close","min_version":1,"max_version":1}"#;
        let payload_object = br#"{"type":"item","item":{"kind":"batch","sequence":1,"provider_cursor":"cursor","delivery_id":"delivery","events":[{"kind":"message_created","channel_id":"channels/one","message_id":"messages/one","thread_id":"threads/one","sender_id":"users/one","text":"","created_at":"2026-01-01T00:00:00Z","thread_reply":false,"provider_payload":{"schema":"fixture.message.v1","data":{"resource":1,"resource":2}}}]}}"#;
        let nested_payload_object = br#"{"type":"item","item":{"kind":"batch","sequence":1,"provider_cursor":"cursor","delivery_id":"delivery","events":[{"kind":"message_created","channel_id":"channels/one","message_id":"messages/one","thread_id":"threads/one","sender_id":"users/one","text":"","created_at":"2026-01-01T00:00:00Z","thread_reply":false,"provider_payload":{"schema":"fixture.message.v1","data":{"resource":{"name":"one","name":"two"}}}}]}}"#;
        let configuration_object = br#"{"type":"start","channel_ids":["channels/one"],"allowed_senders":["users/one"],"max_uncommitted":1,"resume_from":null,"backend_configuration":{"schema":"fixture.config.v1","data":{"subscription":"one","subscription":"two"}}}"#;
        for raw in [
            top_level.as_slice(),
            payload_object,
            nested_payload_object,
            configuration_object,
        ] {
            let mut io = FramedIo::new(Cursor::new(frame(raw)), Vec::new());
            let error = io
                .receive::<ServerFrame>()
                .expect_err("duplicate object key must fail");
            assert_eq!(error.code(), "invalid_frame");
            assert!(error.detail().contains("duplicate JSON object key"));
        }
    }

    #[test]
    fn canonical_start_and_message_batch_field_names_are_stable() {
        assert_eq!(PROTOCOL_NAME, "agentctl-chat-subscription");
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(
            serde_json::to_value(ClientFrame::Hello {
                min_version: 1,
                max_version: 1,
            })
            .expect("serialize client hello"),
            serde_json::json!({"type": "hello", "min_version": 1, "max_version": 1})
        );
        assert_eq!(
            serde_json::to_value(ServerFrame::Hello {
                version: 1,
                capabilities: WireCapabilities {
                    backend_name: "fixture".to_owned(),
                    replay: WireReplay::Cursor,
                    full_message_data: true,
                    max_uncommitted: 1,
                    event_kinds: vec![WireEventKind::MessageCreated, WireEventKind::Heartbeat],
                },
            })
            .expect("serialize server hello"),
            serde_json::json!({
                "type": "hello",
                "version": 1,
                "capabilities": {
                    "backend_name": "fixture",
                    "replay": "cursor",
                    "full_message_data": true,
                    "max_uncommitted": 1,
                    "event_kinds": ["message_created", "heartbeat"]
                }
            })
        );
        assert_eq!(
            serde_json::to_value(ClientFrame::Start {
                channel_ids: vec!["channels/one".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                max_uncommitted: 1,
                resume_from: Some("cursor-before".to_owned()),
                backend_configuration: Some(WireBackendConfiguration {
                    schema: "fixture.config.v1".to_owned(),
                    data: serde_json::Map::from_iter([(
                        "subscription".to_owned(),
                        serde_json::Value::String("subscriptions/one".to_owned()),
                    )]),
                }),
            })
            .expect("serialize start"),
            serde_json::json!({
                "type": "start",
                "channel_ids": ["channels/one"],
                "allowed_senders": ["users/owner"],
                "max_uncommitted": 1,
                "resume_from": "cursor-before",
                "backend_configuration": {
                    "schema": "fixture.config.v1",
                    "data": {"subscription": "subscriptions/one"}
                }
            })
        );
        assert_eq!(
            serde_json::to_value(ServerFrame::Item {
                item: WireItem::Batch {
                    sequence: 7,
                    provider_cursor: "provider-cursor".to_owned(),
                    delivery_id: "delivery-id".to_owned(),
                    events: vec![WireEvent::MessageCreated(Box::new(WireMessageCreated {
                        channel_id: "channels/one".to_owned(),
                        message_id: "messages/one".to_owned(),
                        thread_id: "threads/one".to_owned(),
                        sender_id: "users/owner".to_owned(),
                        text: String::new(),
                        created_at: "2026-01-01T00:00:00Z".to_owned(),
                        thread_reply: false,
                        provider_payload: Some(WireProviderPayload {
                            schema: "fixture.message.v1".to_owned(),
                            data: serde_json::Map::from_iter([(
                                "resource".to_owned(),
                                serde_json::json!({"name": "messages/one"}),
                            )]),
                        }),
                    }))],
                }
            })
            .expect("serialize item"),
            serde_json::json!({
                "type": "item",
                "item": {
                    "kind": "batch",
                    "sequence": 7,
                    "provider_cursor": "provider-cursor",
                    "delivery_id": "delivery-id",
                    "events": [{
                        "kind": "message_created",
                        "channel_id": "channels/one",
                        "message_id": "messages/one",
                        "thread_id": "threads/one",
                        "sender_id": "users/owner",
                        "text": "",
                        "created_at": "2026-01-01T00:00:00Z",
                        "thread_reply": false,
                        "provider_payload": {
                            "schema": "fixture.message.v1",
                            "data": {"resource": {"name": "messages/one"}}
                        }
                    }]
                }
            })
        );
        for (frame, expected) in [
            (
                serde_json::to_value(WireEvent::Checkpoint).expect("serialize checkpoint"),
                serde_json::json!({"kind": "checkpoint"}),
            ),
            (
                serde_json::to_value(WireEvent::Gap {
                    reason: Some("retention boundary".to_owned()),
                })
                .expect("serialize gap"),
                serde_json::json!({"kind": "gap", "reason": "retention boundary"}),
            ),
            (
                serde_json::to_value(ClientFrame::Commit {
                    sequence: 7,
                    delivery_id: "delivery-id".to_owned(),
                })
                .expect("serialize commit"),
                serde_json::json!({"type": "commit", "sequence": 7, "delivery_id": "delivery-id"}),
            ),
            (
                serde_json::to_value(ClientFrame::Close).expect("serialize close"),
                serde_json::json!({"type": "close"}),
            ),
            (
                serde_json::to_value(ServerFrame::Subscribed).expect("serialize subscribed"),
                serde_json::json!({"type": "subscribed"}),
            ),
            (
                serde_json::to_value(ServerFrame::Item {
                    item: WireItem::Heartbeat { sequence: 8 },
                })
                .expect("serialize heartbeat"),
                serde_json::json!({"type": "item", "item": {"kind": "heartbeat", "sequence": 8}}),
            ),
            (
                serde_json::to_value(ServerFrame::Committed { sequence: 7 })
                    .expect("serialize committed"),
                serde_json::json!({"type": "committed", "sequence": 7}),
            ),
            (
                serde_json::to_value(ServerFrame::End).expect("serialize end"),
                serde_json::json!({"type": "end"}),
            ),
            (
                serde_json::to_value(ServerFrame::Error {
                    code: "provider_unavailable".to_owned(),
                    detail: "fixture".to_owned(),
                    retryable: true,
                    fatal: true,
                })
                .expect("serialize error"),
                serde_json::json!({
                    "type": "error",
                    "code": "provider_unavailable",
                    "detail": "fixture",
                    "retryable": true,
                    "fatal": true
                }),
            ),
        ] {
            assert_eq!(frame, expected);
        }
    }

    #[test]
    fn every_core_valid_worst_case_batch_fits_one_protocol_frame() {
        let message = || {
            InboundMessage::new(
                ChannelId::new("\0".repeat(chat_subscription::MAX_RESOURCE_ID_BYTES))
                    .expect("maximum escaped channel"),
                MessageId::new("\0".repeat(chat_subscription::MAX_RESOURCE_ID_BYTES))
                    .expect("maximum escaped message"),
                ThreadId::new("\0".repeat(chat_subscription::MAX_RESOURCE_ID_BYTES))
                    .expect("maximum escaped thread"),
                SenderId::new("\0".repeat(chat_subscription::MAX_SENDER_ID_BYTES))
                    .expect("maximum escaped sender"),
                "\0".repeat(chat_subscription::MAX_MESSAGE_TEXT_BYTES),
                "2026-01-01T00:00:00Z",
                false,
            )
            .expect("maximum escaped normalized message")
        };
        let mut events = vec![
            CommittableEvent::message_created(message()),
            CommittableEvent::message_created(message()),
        ];
        for _ in 0..5 {
            events.push(CommittableEvent::Gap(
                ReconciliationGap::new(Some("\0".repeat(chat_subscription::MAX_GAP_REASON_BYTES)))
                    .expect("maximum escaped gap"),
            ));
        }
        events.resize(
            chat_subscription::MAX_BATCH_EVENTS,
            CommittableEvent::Checkpoint,
        );
        let batch = DeliveryBatch::new(
            EventSequence::new(u64::MAX).expect("maximum sequence"),
            ProviderCursor::new("\0".repeat(chat_subscription::MAX_TOKEN_BYTES))
                .expect("maximum escaped cursor"),
            DeliveryId::new("\0".repeat(chat_subscription::MAX_TOKEN_BYTES))
                .expect("maximum escaped delivery"),
            events,
        )
        .expect("encoded variable payload remains inside the core batch bound");
        let payload = serde_json::to_vec(&ServerFrame::Item {
            item: WireItem::from(&SubscriptionItem::Batch(batch)),
        })
        .expect("serialize worst-case valid item");
        assert!(
            payload.len() <= MAX_CORE_BATCH_FRAME_BYTES,
            "{}",
            payload.len()
        );
    }

    #[test]
    fn start_accepts_absent_backend_configuration_as_none() {
        let frame: ClientFrame = decode_strict_json(
            br#"{"type":"start","channel_ids":["channels/one"],"allowed_senders":["users/owner"],"max_uncommitted":1,"resume_from":null}"#,
        )
        .expect("v1 start without backend configuration");
        assert!(matches!(
            frame,
            ClientFrame::Start {
                backend_configuration: None,
                ..
            }
        ));
    }
}
